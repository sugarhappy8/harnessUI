#!/usr/bin/env python3
"""Tests for scripts/setup_updater.py that run on any computer.

The real setup runs the Tauri CLI's `tauri signer generate` with node, and `gh`, on the
owner's computer. These tests check what does not need them: how a public key file is read
(and a private key refused), how the key goes into tauri.updater.conf.json without changing
its layout, the check release.yml runs, and the setup's choices with stand-ins for node and
gh. No real key
is made: the public key is the catalog's test fixture, and the "private key" files are
placeholder text.

    python3 -I scripts/setup_updater_test.py

Standard library only.
"""

import base64
import contextlib
import io
import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import setup_updater as su  # noqa: E402

FIXTURE_PUB = su.ROOT / "crates" / "models" / "tests" / "fixtures" / "catalog" / "test.pub"
PLACEHOLDER_PRIVATE = "placeholder private key file, not a key"


def minisign_text() -> str:
    """Minisign's two lines of the test-only public key."""
    return FIXTURE_PUB.read_text(encoding="utf-8")


def tauri_form() -> str:
    """The same key as `tauri signer generate` writes a .pub file: base64 of the two lines."""
    return base64.b64encode(minisign_text().encode()).decode()


def key_line() -> str:
    return minisign_text().splitlines()[1]


def committed_config() -> str:
    return su.CONFIG.read_text(encoding="utf-8")


def empty_config() -> str:
    return su.with_pubkey(committed_config(), "")


class ReadingThePublicKey(unittest.TestCase):
    def test_what_tauri_writes_is_used_unchanged(self):
        value, kid = su.parse_pubkey(tauri_form() + "\n")
        self.assertEqual(value, tauri_form())
        self.assertEqual(kid, "8B4D4A1D7B380A8E")

    def test_minisigns_two_lines_become_what_the_updater_reads(self):
        value, kid = su.parse_pubkey(minisign_text())
        self.assertEqual(base64.b64decode(value).decode(), minisign_text())
        self.assertEqual(kid, "8B4D4A1D7B380A8E")

    def test_the_key_line_alone_gets_a_comment(self):
        value, kid = su.parse_pubkey(key_line())
        lines = base64.b64decode(value).decode().splitlines()
        self.assertEqual(lines, ["untrusted comment: minisign public key: 8B4D4A1D7B380A8E", key_line()])
        self.assertEqual(su.parse_pubkey(value), (value, kid))

    def test_a_private_key_is_refused_without_repeating_it(self):
        # Shaped like the key file `tauri signer generate` writes (base64 of a comment and a
        # key line), with zeros in place of a key.
        body = base64.b64encode(b"Ed" + bytes(156)).decode()
        private = base64.b64encode(f"untrusted comment: rsign encrypted secret key\n{body}\n".encode()).decode()
        with self.assertRaises(ValueError) as caught:
            su.parse_pubkey(private)
        self.assertIn("private key", str(caught.exception))
        self.assertNotIn(body[:20], str(caught.exception))
        self.assertNotIn(private[:20], str(caught.exception))

    def test_what_is_not_a_public_key_is_refused(self):
        for text in ("", "   \n", "not base64 at all!", base64.b64encode(b"hello").decode(),
                     base64.b64encode(b"untrusted comment: minisign public key\n").decode(),
                     "untrusted comment: minisign public key\nAAAA\n",
                     base64.b64encode(b"Xx" + bytes(40)).decode()):
            with self.subTest(text=text), self.assertRaises(ValueError):
                su.parse_pubkey(text)


class TheConfigFile(unittest.TestCase):
    def test_the_committed_file_keeps_its_layout(self):
        text = committed_config()
        self.assertEqual(su.with_pubkey(text, su.current_pubkey(text)), text)

    def test_the_key_goes_into_plugins_updater_pubkey_only(self):
        before = empty_config()
        after = su.with_pubkey(before, tauri_form())
        self.assertEqual(su.current_pubkey(after), tauri_form())
        self.assertIn(f'\n      "pubkey": "{tauri_form()}",\n', after)
        self.assertTrue(after.endswith("}\n"))
        changed = [(a, b) for a, b in zip(before.splitlines(), after.splitlines()) if a != b]
        self.assertEqual(len(changed), 1, changed)
        self.assertEqual(len(before.splitlines()), len(after.splitlines()))

    def test_windows_line_endings_stay(self):
        crlf = empty_config().replace("\n", "\r\n")
        after = su.with_pubkey(crlf, tauri_form())
        self.assertEqual(after.count("\r\n"), crlf.count("\r\n"))
        self.assertNotIn("\n", after.replace("\r\n", ""))
        self.assertEqual(su.current_pubkey(after), tauri_form())

    def test_no_final_newline_stays_without_one(self):
        self.assertFalse(su.with_pubkey(empty_config().rstrip("\n"), "x").endswith("\n"))

    def test_an_empty_or_blank_key_reads_as_none(self):
        self.assertEqual(su.current_pubkey(empty_config()), "")
        self.assertEqual(su.current_pubkey(su.with_pubkey(empty_config(), "  ")), "")

    def test_the_repository_comes_from_the_updater_address(self):
        self.assertEqual(su.repo_of(committed_config()), "sugarhappy8/harnessUI")
        broken = committed_config().replace("https://github.com/", "https://example.com/")
        with self.assertRaises(ValueError):
            su.repo_of(broken)


class TheReleaseCheck(unittest.TestCase):
    """`--check`, which release.yml runs before anything is built."""

    def test_no_key_fails_and_says_what_to_run(self):
        ok, message = su.check_config(empty_config())
        self.assertFalse(ok)
        self.assertIn("python3 -I scripts/setup_updater.py", message)
        self.assertIn("py scripts\\setup_updater.py", message, "a stock Windows Python has no python3")

    def test_the_key_as_tauri_writes_it_passes(self):
        ok, message = su.check_config(su.with_pubkey(empty_config(), tauri_form()))
        self.assertTrue(ok, message)
        self.assertIn("8B4D4A1D7B380A8E", message)

    def test_any_other_form_fails(self):
        for value in (minisign_text(), key_line(), "nonsense"):
            with self.subTest(value=value):
                ok, _ = su.check_config(su.with_pubkey(empty_config(), value))
                self.assertFalse(ok)

    def test_release_yml_runs_it_before_anything_is_built(self):
        workflow = (su.ROOT / ".github" / "workflows" / "release.yml").read_text(encoding="utf-8")
        keys = workflow.split("\n  keys:\n", 1)[1].split("\n  reverse-pins:\n", 1)[0]
        self.assertTrue("python3 -I scripts/setup_updater.py --check" in keys, "the keys job runs --check")
        self.assertTrue("secrets.TAURI_SIGNING_PRIVATE_KEY }}" in keys, "the keys job looks at the secret")
        self.assertFalse("contents: write" in keys, "the keys job only reads")
        publish = workflow.split("\n  publish:\n", 1)[1].split("\n  catalog:\n", 1)[0]
        self.assertRegex(publish, r"\n    needs: \[[^\]]*\bkeys\b")
        self.assertTrue("--config src-tauri/tauri.updater.conf.json" in publish, "every build has the updater key")

    def test_ci_runs_these_tests(self):
        ci = (su.ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
        self.assertTrue("python3 -I scripts/setup_updater_test.py" in ci, "ci.yml runs these tests")


class FakeTools:
    """Stand-ins for node and gh: records every call, and `node <tauri.js> signer generate -w
    PATH` writes PATH (placeholder text) and PATH.pub (the test public key in Tauri's form)."""

    def __init__(self, have=("node", "gh"), fail=()):
        self.have = have
        self.fail = fail
        self.calls = []

    def which(self, name):
        return f"/fake/bin/{name}" if name in self.have else None

    def run(self, cmd, **kwargs):
        self.calls.append((cmd, kwargs))
        tool = Path(cmd[0]).name
        if tool in self.fail:
            return SimpleNamespace(returncode=1)
        if tool == "node":
            key = Path(cmd[cmd.index("-w") + 1])
            key.write_text(PLACEHOLDER_PRIVATE, encoding="utf-8")
            key.with_name(key.name + ".pub").write_text(tauri_form(), encoding="utf-8")
        return SimpleNamespace(returncode=0)


class Setup(unittest.TestCase):
    def setUp(self):
        self.dir = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.dir)
        self.config = self.dir / "tauri.updater.conf.json"
        self.config.write_text(empty_config(), encoding="utf-8")
        self.app_config = self.dir / "tauri.conf.json"
        self.app_config.write_text('{\n  "version": "9.8.7"\n}\n', encoding="utf-8")
        self.key = self.dir / "keys" / "harnessui.key"
        self.pub = self.dir / "keys" / "harnessui.key.pub"
        patcher = mock.patch.object(su, "TAURI_CLI", FIXTURE_PUB)  # any file that exists
        patcher.start()
        self.addCleanup(patcher.stop)

    def setup(self, tools, answers=()):
        answers = list(answers)
        asked = []

        def ask(question):
            asked.append(question)
            return answers.pop(0) if answers else False

        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = su.setup(self.key, self.config, self.app_config, tools.which, tools.run, ask)
        return code, out.getvalue(), asked

    def test_an_existing_key_is_never_replaced(self):
        self.config.write_text(su.with_pubkey(empty_config(), tauri_form()), encoding="utf-8")
        before = self.config.read_text(encoding="utf-8")
        tools = FakeTools()
        code, out, asked = self.setup(tools)
        self.assertEqual(code, 0)
        self.assertIn("already set up", out)
        self.assertIn("unable to", out)
        self.assertEqual(tools.calls, [])
        self.assertEqual(asked, [])
        self.assertEqual(self.config.read_text(encoding="utf-8"), before)

    def test_a_new_key_pair_without_gh(self):
        tools = FakeTools(have=("node",))
        code, out, asked = self.setup(tools)
        self.assertEqual(code, 0, out)
        self.assertEqual(self.config.read_bytes(), su.with_pubkey(empty_config(), tauri_form()).encode())
        (cmd, kwargs), = tools.calls
        # node itself, never npx: on Windows npx is a batch file and cmd.exe would read the path.
        self.assertEqual(cmd, ["/fake/bin/node", str(su.TAURI_CLI), "signer", "generate", "-w", str(self.key)])
        self.assertEqual(kwargs["cwd"], str(su.ROOT))
        self.assertNotIn("CI", kwargs["env"])
        self.assertNotIn("input", kwargs, "the CLI asks for the password itself")
        self.assertEqual(su.current_pubkey(self.config.read_text(encoding="utf-8")), tauri_form())
        self.assertEqual(asked, [])
        self.assertIn("https://github.com/sugarhappy8/harnessUI/settings/secrets/actions", out)
        self.assertIn("TAURI_SIGNING_PRIVATE_KEY_PASSWORD", out)
        self.assertIn("password manager", out)
        self.assertIn("Commit src-tauri/tauri.updater.conf.json", out)
        self.assertIn("v9.8.7", out)
        self.assertNotIn(PLACEHOLDER_PRIVATE, out)

    def test_a_file_with_windows_line_endings_keeps_them(self):
        self.config.write_bytes(empty_config().replace("\n", "\r\n").encode())
        code, out, _ = self.setup(FakeTools(have=("node",)))
        self.assertEqual(code, 0, out)
        written = self.config.read_bytes()
        self.assertEqual(written, su.with_pubkey(empty_config(), tauri_form()).replace("\n", "\r\n").encode())

    def test_ci_in_the_environment_does_not_skip_the_password(self):
        tools = FakeTools(have=("node",))
        with mock.patch.dict(su.os.environ, {"CI": "true"}):
            self.setup(tools)
        self.assertNotIn("CI", tools.calls[0][1]["env"])

    def test_gh_saves_both_secrets_when_asked(self):
        tools = FakeTools()
        code, out, asked = self.setup(tools, answers=[True, True])
        self.assertEqual(code, 0, out)
        self.assertEqual(len(asked), 2)
        gh_calls = [c for c in tools.calls if c[0][0] == "/fake/bin/gh"]
        self.assertEqual(gh_calls[0][0], ["/fake/bin/gh", "secret", "set", "TAURI_SIGNING_PRIVATE_KEY", "--repo", "sugarhappy8/harnessUI"])
        self.assertEqual(gh_calls[0][1]["input"], PLACEHOLDER_PRIVATE.encode())
        self.assertEqual(gh_calls[1][0], ["/fake/bin/gh", "secret", "set", "TAURI_SIGNING_PRIVATE_KEY_PASSWORD", "--repo", "sugarhappy8/harnessUI"])
        self.assertNotIn("input", gh_calls[1][1], "gh asks for the password itself")
        self.assertNotIn("New repository secret", out)
        self.assertNotIn(PLACEHOLDER_PRIVATE, out)

    def test_gh_declined_or_failing_means_the_secrets_are_added_by_hand(self):
        for answers, fail in (([False], ()), ([True], ("gh",))):
            with self.subTest(answers=answers, fail=fail):
                self.config.write_text(empty_config(), encoding="utf-8")
                for f in (self.key, self.pub):
                    f.unlink(missing_ok=True)
                tools = FakeTools(fail=fail)
                code, out, _ = self.setup(tools, answers=answers)
                self.assertEqual(code, 0, out)
                self.assertIn("New repository secret", out)

    def test_an_existing_key_pair_can_be_used(self):
        self.key.parent.mkdir(parents=True)
        self.key.write_text(PLACEHOLDER_PRIVATE, encoding="utf-8")
        self.pub.write_text(tauri_form(), encoding="utf-8")
        tools = FakeTools(have=())
        code, out, asked = self.setup(tools, answers=[True])
        self.assertEqual(code, 0, out)
        self.assertIn("already exist", asked[0])
        self.assertEqual(tools.calls, [])
        self.assertEqual(su.current_pubkey(self.config.read_text(encoding="utf-8")), tauri_form())
        self.assertEqual(self.key.read_text(encoding="utf-8"), PLACEHOLDER_PRIVATE)

    def test_an_existing_key_pair_is_kept_when_declined(self):
        self.key.parent.mkdir(parents=True)
        self.key.write_text(PLACEHOLDER_PRIVATE, encoding="utf-8")
        self.pub.write_text(tauri_form(), encoding="utf-8")
        tools = FakeTools()
        code, out, _ = self.setup(tools, answers=[False])
        self.assertEqual(code, 1)
        self.assertIn("--key PATH", out)
        self.assertEqual(tools.calls, [])
        self.assertEqual(self.config.read_text(encoding="utf-8"), empty_config())
        self.assertEqual(self.key.read_text(encoding="utf-8"), PLACEHOLDER_PRIVATE)

    def test_half_a_key_pair_stops(self):
        for present in ("key", "pub"):
            with self.subTest(present=present):
                for f in (self.key, self.pub):
                    f.unlink(missing_ok=True)
                self.key.parent.mkdir(parents=True, exist_ok=True)
                (self.key if present == "key" else self.pub).write_text("x", encoding="utf-8")
                tools = FakeTools()
                code, out, _ = self.setup(tools)
                self.assertEqual(code, 1)
                self.assertEqual(tools.calls, [])
                self.assertEqual(self.config.read_text(encoding="utf-8"), empty_config())

    def test_without_node_or_the_tauri_cli_it_says_to_run_npm_ci(self):
        code, out, _ = self.setup(FakeTools(have=()))
        self.assertEqual(code, 1)
        self.assertIn("npm ci", out)
        with mock.patch.object(su, "TAURI_CLI", self.dir / "missing" / "tauri.js"):
            tools = FakeTools()
            code, out, _ = self.setup(tools)
        self.assertEqual(code, 1)
        self.assertIn("npm ci", out)
        self.assertEqual(tools.calls, [])
        self.assertEqual(self.config.read_text(encoding="utf-8"), empty_config())

    def test_a_failed_generate_writes_nothing(self):
        code, out, _ = self.setup(FakeTools(fail=("node",)))
        self.assertEqual(code, 1)
        self.assertEqual(self.config.read_text(encoding="utf-8"), empty_config())

    def test_a_bad_public_key_file_writes_nothing(self):
        self.key.parent.mkdir(parents=True)
        self.key.write_text(PLACEHOLDER_PRIVATE, encoding="utf-8")
        self.pub.write_text("nonsense", encoding="utf-8")
        code, out, _ = self.setup(FakeTools(), answers=[True])
        self.assertEqual(code, 1)
        self.assertIn("cannot be used", out)
        self.assertEqual(self.config.read_text(encoding="utf-8"), empty_config())


class CommandLine(unittest.TestCase):
    def test_check_reads_the_committed_file(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = su.main(["--check"])
        ok, message = su.check_config(committed_config())
        self.assertEqual(code, 0 if ok else 1)
        self.assertEqual(out.getvalue().strip(), message)


if __name__ == "__main__":
    unittest.main()

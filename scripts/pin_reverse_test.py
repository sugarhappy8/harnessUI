#!/usr/bin/env python3
"""Tests for scripts/pin_reverse.py that run offline on any computer.

Pinning needs the network and the publishers' verifiers; these tests check what does not:
the committed `reverse.lock.json` passes `--check`, and `--check` catches each kind of bad
lock (a host outside the part's set, a missing check, Kuna offered on Windows or Ghidra on
a Mac, a minimum above the pin, an unsafe folder or path, a missing fixture); plus the
version, engine-range and path-name helpers, and the archive survey on small fake archives.

    python3 -I scripts/pin_reverse_test.py

Standard library only.
"""

import argparse
import contextlib
import copy
import io
import json
import sys
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pin_reverse as pr  # noqa: E402


def committed() -> dict:
    return json.loads(pr.LOCK.read_text())


def part(lock: dict, name: str) -> dict:
    return next(p for p in lock["parts"] if p["name"] == name)


class CommittedLock(unittest.TestCase):
    def test_the_committed_lock_passes(self):
        self.assertEqual(pr.check(committed()), [])

    def test_the_pins_follow_rea(self):
        lock = committed()
        fixture = json.loads((pr.FIXTURES / "rea-6.9.0-catalog.json").read_text())
        self.assertEqual(fixture["tool_count"], 255)
        self.assertEqual(sorted(fixture["allow_listed"]), sorted(pr.ALLOW_LISTED))
        self.assertEqual(part(lock, "kuna")["version"], "1.990")
        self.assertEqual(part(lock, "ghidra")["version"], "12.1.4")
        self.assertEqual(part(lock, "ghidra")["minimum_version"], "12.1")
        self.assertEqual(part(lock, "java")["version"].split(".")[0], "21")
        self.assertTrue(pr.engines_allow("^22.23.4 || ^24.22.1 || >=26.12.1", part(lock, "node")["version"]))


class CheckCatches(unittest.TestCase):
    def broken(self, edit) -> list:
        lock = copy.deepcopy(committed())
        edit(lock)
        return pr.check(lock)

    def assert_caught(self, edit, words: str):
        problems = self.broken(edit)
        self.assertTrue(any(words in p for p in problems), problems)

    def test_a_url_on_another_host(self):
        def edit(lock):
            part(lock, "node")["builds"][0]["file"]["url"] = "https://evil.example/node.zip"
        self.assert_caught(edit, "not https on the part's hosts")

    def test_a_plain_http_url(self):
        def edit(lock):
            b = part(lock, "rea")["builds"][0]
            b["file"]["url"] = b["file"]["url"].replace("https://", "http://")
        self.assert_caught(edit, "not https on the part's hosts")

    def test_an_extra_host(self):
        def edit(lock):
            part(lock, "kuna")["hosts"] = pr.GITHUB_HOSTS + ["evil.example"]
        self.assert_caught(edit, "hosts")

    def test_a_missing_or_unknown_check(self):
        def none(lock):
            part(lock, "java")["builds"][0]["verified_by"] = []
        self.assert_caught(none, "verified_by")

        def unknown(lock):
            part(lock, "java")["builds"][0]["verified_by"] = ["trust-me"]
        self.assert_caught(unknown, "verified_by")

    def test_kuna_on_windows_or_ghidra_on_a_mac(self):
        def kuna(lock):
            b = copy.deepcopy(part(lock, "kuna")["builds"][0])
            b["platform"] = "windows-x86_64"
            part(lock, "kuna")["builds"].append(b)
        self.assert_caught(kuna, "platforms")

        def ghidra(lock):
            b = copy.deepcopy(part(lock, "ghidra")["builds"][0])
            b["platform"] = "macos-aarch64"
            part(lock, "ghidra")["builds"].append(b)
        self.assert_caught(ghidra, "platforms")

    def test_a_minimum_above_the_pin(self):
        def edit(lock):
            part(lock, "kuna")["minimum_version"] = "9.0"
        self.assert_caught(edit, "minimum_version")

    def test_an_unsafe_folder_or_program(self):
        def folder(lock):
            part(lock, "rea")["dir"] = "../rea-6.9.0"
        self.assert_caught(folder, "dir")

        def program(lock):
            part(lock, "node")["builds"][0]["programs"][0]["path"] = "../node"
        self.assert_caught(program, "program")

    def test_a_program_that_is_not_kept(self):
        def edit(lock):
            part(lock, "node")["builds"][0]["programs"][0]["path"] = "bin/npm"
        self.assert_caught(edit, "is not kept")

    def test_a_bad_digest(self):
        def edit(lock):
            part(lock, "ghidra")["builds"][0]["file"]["sha256"] = "XYZ"
        self.assert_caught(edit, "sha256")

    def test_a_missing_fixture(self):
        def edit(lock):
            part(lock, "rea")["accepted_rea_versions"].append(
                {"version": "6.8.0", "schema_fixture": "rea-6.8.0-catalog.json"}
            )
        self.assert_caught(edit, "fixture for 6.8.0 missing")

    def test_three_accepted_versions(self):
        def edit(lock):
            part(lock, "rea")["accepted_rea_versions"] *= 3
        self.assert_caught(edit, "at most two")

    def test_a_bad_analyzer_name(self):
        def edit(lock):
            lock["disabled_ghidra_analyzers"]["linux-x86_64"] = "-bad,name"
        self.assert_caught(edit, "disabled_ghidra_analyzers")

    def test_a_file_name_that_leaves_the_downloads_folder(self):
        for name in ["../../../escape.zip", "sub/x.zip", "..", "con.zip"]:
            def edit(lock, name=name):
                b = part(lock, "java")["builds"][0]
                b["file"]["name"] = name
                b["upstream_asset"] = name
                b["file"]["url"] = "https://github.com/adoptium/x/" + name
            self.assert_caught(edit, "file name")

    def test_each_part_needs_its_own_publishers_checks(self):
        # Another part's check, or a first download, is not enough for rea, Node.js or Java.
        for name, vb in [
            ("java", ["first-download-only"]),
            ("java", ["gpg:adoptium"]),
            ("node", ["gpg:adoptium"]),
            ("rea", ["npm-integrity", "npm-audit-signatures"]),
            ("rea", ["npm-integrity", "npm-provenance:", "npm-audit-signatures"]),
            ("ghidra", ["gpg:adoptium"]),
        ]:
            def edit(lock, name=name, vb=vb):
                part(lock, name)["builds"][0]["verified_by"] = vb
            self.assert_caught(edit, "lacks")

    def test_an_accepted_rea_below_the_minimum_without_a_reason(self):
        # What the pin script used to write at a rea bump: minimum = the new pin.
        def edit(lock):
            rea = part(lock, "rea")
            rea["accepted_rea_versions"].append({"version": "6.8.0", "schema_fixture": "rea-6.9.0-catalog.json"})
        self.assert_caught(edit, "below minimum_version")


class ReleaseGate(unittest.TestCase):
    """Ghidra pinned on first download only passes --check but never --check --release."""

    def test_only_ghidra_waits_for_its_check(self):
        lock = committed()
        for b in pr.release_blockers(lock):
            self.assertTrue(b.startswith("ghidra "), b)
        problems = pr.check(lock, release=True)
        ghidra = part(lock, "ghidra")["builds"]
        if any(b["verified_by"] == ["first-download-only"] for b in ghidra):
            self.assertTrue(problems and all("ghidra" in p and "github-digest" in p for p in problems), problems)
        else:
            self.assertEqual(problems, [])

    def test_a_ghidra_with_githubs_digest_is_ready(self):
        lock = copy.deepcopy(committed())
        for b in part(lock, "ghidra")["builds"]:
            b["verified_by"] = ["github-digest"]
        self.assertEqual(pr.check(lock, release=True), [])
        self.assertEqual(pr.release_blockers(lock), [])

    def test_kuna_needs_nothing_more_than_its_first_download(self):
        self.assertTrue(pr.release_ready("kuna", ["first-download-only"]))
        self.assertFalse(pr.release_ready("kuna", []))
        self.assertFalse(pr.release_ready("ghidra", ["first-download-only"]))
        self.assertFalse(pr.release_ready("someday", ["github-digest"]))


def raw_parts(lock: dict, rea_version: str = "6.9.0") -> dict:
    """What the pin_* functions return, rebuilt from a lock."""
    raw = {}
    for p in lock["parts"]:
        r = {k: copy.deepcopy(v) for k, v in p.items() if k in ("name", "version", "minimum_version", "vendor", "entry", "accepted_rea_versions", "builds")}
        raw[p["name"]] = r
    return raw


def fixture_for(version: str) -> dict:
    data = json.loads((pr.FIXTURES / "rea-6.9.0-catalog.json").read_text())
    data["rea_version"] = version
    return data


class Minimums(unittest.TestCase):
    """R15: a bump keeps the previous version usable; only --minimum with --reason raises it."""

    def bumped(self, minimums=(), reason=None) -> list:
        old = committed()
        raw = raw_parts(old)
        raw["rea"]["accepted_rea_versions"] = [
            {"version": "6.10.0", "schema_fixture": "rea-6.10.0-catalog.json"},
            {"version": "6.9.0", "schema_fixture": "rea-6.9.0-catalog.json"},
        ]
        raw["kuna"]["version"] = "1.991"
        raw["kuna"]["minimum_version"] = "1.991"
        return pr.assemble_parts(raw, "6.10.0", old, list(minimums), reason)

    def lock_of(self, parts) -> dict:
        lock = copy.deepcopy(committed())
        lock["parts"] = parts
        return lock

    def test_a_rea_bump_keeps_the_previous_rea_usable(self):
        parts = self.bumped()
        rea = next(p for p in parts if p["name"] == "rea")
        self.assertEqual(rea["version"], "6.10.0")
        self.assertEqual(rea["minimum_version"], "6.9.0")
        for a in rea["accepted_rea_versions"]:
            self.assertGreaterEqual(pr.version_key(a["version"]), pr.version_key(rea["minimum_version"]))
        kuna = next(p for p in parts if p["name"] == "kuna")
        self.assertEqual((kuna["version"], kuna["minimum_version"]), ("1.991", "1.990"))
        lock = self.lock_of(parts)
        # The bumped Kuna's builds still carry 1.990's assets; only the versions matter here.
        problems = [p for p in pr.check(lock, fixtures={"rea-6.10.0-catalog.json": fixture_for("6.10.0")}) if "dir" not in p]
        self.assertEqual(problems, [])

    def test_a_security_fix_raises_the_minimum_with_its_reason(self):
        parts = self.bumped(["rea=6.10.0"], "rea 6.9.0 reads files outside the folder it was given")
        rea = next(p for p in parts if p["name"] == "rea")
        self.assertEqual(rea["minimum_version"], "6.10.0")
        self.assertIn("outside the folder", rea["minimum_reason"])
        lock = self.lock_of(parts)
        problems = [p for p in pr.check(lock, fixtures={"rea-6.10.0-catalog.json": fixture_for("6.10.0")}) if "dir" not in p]
        self.assertEqual(problems, [])
        # The next bump keeps the raised minimum and its reason.
        old = lock
        raw = raw_parts(old)
        raw["rea"]["accepted_rea_versions"] = [
            {"version": "6.11.0", "schema_fixture": "rea-6.11.0-catalog.json"},
            {"version": "6.10.0", "schema_fixture": "rea-6.10.0-catalog.json"},
        ]
        nxt = next(p for p in pr.assemble_parts(raw, "6.11.0", old, [], None) if p["name"] == "rea")
        self.assertEqual(nxt["minimum_version"], "6.10.0")
        self.assertEqual(nxt.get("minimum_reason"), rea["minimum_reason"])

    def test_a_minimum_needs_a_reason(self):
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            self.bumped(["rea=6.10.0"], None)


class Temurin(unittest.TestCase):
    """Adoptium's answer is checked before anything is fetched or written."""

    GOOD = {
        "name": "OpenJDK21U-jdk_x64_linux_hotspot_21.0.13_9.tar.gz",
        "link": "https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.13%2B9/OpenJDK21U-jdk_x64_linux_hotspot_21.0.13_9.tar.gz",
        "signature_link": "https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.13%2B9/OpenJDK21U-jdk_x64_linux_hotspot_21.0.13_9.tar.gz.sig",
        "size": 207858352,
        "checksum": "d" * 64,
    }

    def test_temurins_own_names(self):
        self.assertEqual(pr.java_asset_name("21", "21.0.13+9", "linux", "x64"), self.GOOD["name"])
        self.assertEqual(
            pr.java_asset_name("21", "21.0.13+9", "windows", "x64"),
            "OpenJDK21U-jdk_x64_windows_hotspot_21.0.13_9.zip",
        )
        self.assertEqual(pr.java_package_problems(self.GOOD, self.GOOD["name"]), [])

    def test_a_hostile_answer_is_refused(self):
        for key, value in [
            ("name", "../../escaped-by-api.zip"),
            ("name", "OpenJDK21U-jdk_x64_linux_hotspot_21.0.13_9.tar.gz/../x"),
            ("link", "https://attacker.example/jdk.zip"),
            ("link", "http://github.com/adoptium/x/OpenJDK21U-jdk_x64_linux_hotspot_21.0.13_9.tar.gz"),
            ("link", "https://user@github.com/OpenJDK21U-jdk_x64_linux_hotspot_21.0.13_9.tar.gz"),
            ("link", "https://github.com:8443/OpenJDK21U-jdk_x64_linux_hotspot_21.0.13_9.tar.gz"),
            ("signature_link", "https://attacker.example/x.sig"),
            ("size", "1"),
            ("checksum", "../"),
        ]:
            pkg = dict(self.GOOD, **{key: value})
            self.assertNotEqual(pr.java_package_problems(pkg, self.GOOD["name"]), [], (key, value))

    def test_pin_java_stops_before_fetching_a_hostile_package(self):
        api = {"binaries": [
            {"os": "windows", "architecture": "x64", "image_type": "jdk", "package": dict(
                self.GOOD, name="../../escaped-by-api.zip", link="https://attacker.example/jdk.zip")},
        ]}
        fetched = []
        with tempfile.TemporaryDirectory() as d, \
                mock.patch.object(pr, "http_json", return_value=api), \
                mock.patch.object(pr, "download", side_effect=lambda *a, **k: fetched.append(a)), \
                mock.patch.object(pr, "http_get", side_effect=lambda *a, **k: fetched.append(a)), \
                contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                pr.pin_java("jdk-21.0.13+9", "21", Path(d) / "scratch")
            self.assertEqual(fetched, [])
            self.assertEqual(list(Path(d).iterdir()), [])


class Npm(unittest.TestCase):
    """npm ignores the developer's npm settings when it checks rea's signatures."""

    def test_npm_settings_are_dropped_from_the_environment(self):
        env = pr.npm_env({
            "PATH": "/usr/bin", "HOME": "/home/x",
            "npm_config_registry": "https://mirror.example/",
            "NPM_CONFIG_REGISTRY": "https://mirror.example/",
            "Npm_Config_Userconfig": "/tmp/rc", "npm_token": "x",
            "NODE_OPTIONS": "--require /tmp/evil.js",
        })
        self.assertEqual(env, {"PATH": "/usr/bin", "HOME": "/home/x"})

    def test_npm_reads_no_npmrc_and_asks_npms_registry(self):
        args = pr.npm_isolation_args(Path("/s/user-npmrc"), Path("/s/global-npmrc"))
        self.assertIn("--registry=https://registry.npmjs.org/", args)
        self.assertIn("--userconfig=/s/user-npmrc", args)
        self.assertIn("--globalconfig=/s/global-npmrc", args)


class Pin(unittest.TestCase):
    """`pin` writes the lock and fixture only when they pass --check."""

    def run_pin(self, edit) -> tuple[Path, Path, str]:
        base = committed()
        raw = raw_parts(base)
        edit(raw)
        facts = {"kuna": "1.990", "ghidra": "12.1.4", "java_major": "21",
                 "engines": "^22.23.4 || ^24.22.1 || >=26.12.1", "ghidra_minimum": "12.1"}
        rea = raw["rea"]
        fx = fixture_for("6.9.0")
        d = tempfile.mkdtemp()
        repo = Path(d) / "repo"
        lock_path = repo / "reverse.lock.json"
        fixtures = repo / "fixtures"
        args = argparse.Namespace(scratch=str(Path(d) / "scratch"), rea="6.9.0", node=None, java=None,
                                  ghidra_asset=None, first_download_only=set(), minimum=None, reason=None)
        out = io.StringIO()
        with mock.patch.object(pr, "ROOT", repo), mock.patch.object(pr, "LOCK", lock_path), \
                mock.patch.object(pr, "FIXTURES", fixtures), \
                mock.patch.object(pr, "pin_rea", return_value=(rea, {"fixture": fx, "facts": facts})), \
                mock.patch.object(pr, "pin_node", return_value=raw["node"]), \
                mock.patch.object(pr, "pin_kuna", return_value=raw["kuna"]), \
                mock.patch.object(pr, "pin_ghidra", return_value=raw["ghidra"]), \
                mock.patch.object(pr, "pin_java", return_value=raw["java"]), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            try:
                pr.cmd_pin(args)
            except SystemExit:
                pass
        return lock_path, fixtures / "rea-6.9.0-catalog.json", out.getvalue()

    def test_a_lock_that_fails_the_check_is_not_written(self):
        def edit(raw):
            raw["java"]["builds"][0]["file"]["url"] = "https://attacker.example/jdk.zip"
        lock, fixture, _ = self.run_pin(edit)
        self.assertFalse(lock.exists())
        self.assertFalse(fixture.exists())

    def test_a_good_lock_is_written_and_names_what_is_not_ready(self):
        lock, fixture, out = self.run_pin(lambda raw: None)
        self.assertTrue(lock.is_file() and fixture.is_file())
        self.assertEqual(pr.check(json.loads(lock.read_text()), fixtures={fixture.name: json.loads(fixture.read_text())}), [])
        if any(b["verified_by"] == ["first-download-only"] for b in part(committed(), "ghidra")["builds"]):
            self.assertIn("NOT READY FOR A RELEASE: ghidra", out)


class GithubDigest(unittest.TestCase):
    """`github-digest` (WP9e): CI compares GitHub's published digest of the pinned Ghidra
    asset with the lock; `--record` flips the lock to `github-digest`, online or offline."""

    def release(self, lock=None, **asset_changes):
        a = pr.pinned_assets(lock or committed(), "ghidra")[0]
        asset = {"name": a["asset"], "digest": "sha256:" + a["sha256"], "size": a["size"],
                 "state": "uploaded", "browser_download_url": a["url"]}
        asset.update(asset_changes)
        return {"tag_name": a["tag"], "assets": [{"name": "other.zip", "digest": None}, asset]}

    def run_cmd(self, argv_part="ghidra", record=False, confirmed=None, size=None, release=None, lock=None):
        """Runs the command against a copy of the lock; returns (exit code, stdout+stderr,
        the lock afterwards, the headers sent to GitHub)."""
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            path = root / "reverse.lock.json"
            path.write_text(json.dumps(lock or committed(), indent=2) + "\n")
            sent = []

            def fake_get(url, dest=None, accept=None, extra=None):
                sent.append((url, dict(extra or {})))
                return json.dumps(release if release is not None else self.release()).encode()

            args = argparse.Namespace(part=argv_part, record=record, confirmed=confirmed, size=size)
            out = io.StringIO()
            code = 0
            env = {"GITHUB_TOKEN": "ghs_secret-token-value"}
            with mock.patch.object(pr, "ROOT", root), mock.patch.object(pr, "LOCK", path), \
                    mock.patch.object(pr, "http_get", side_effect=fake_get), \
                    mock.patch.dict(pr.os.environ, env, clear=False), \
                    mock.patch.dict(pr.os.environ, {"GITHUB_STEP_SUMMARY": str(root / "summary.md")}), \
                    contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
                try:
                    pr.cmd_github_digest(args)
                except SystemExit as e:
                    code = e.code
            summary = (root / "summary.md").read_text() if (root / "summary.md").exists() else ""
            return code, out.getvalue() + summary, json.loads(path.read_text()), sent

    def test_the_pinned_ghidra_asset_is_one_file_for_both_systems(self):
        assets = pr.pinned_assets(committed(), "ghidra")
        self.assertEqual(len(assets), 1)
        a = assets[0]
        self.assertEqual((a["repo"], a["tag"]), ("NationalSecurityAgency/ghidra", "Ghidra_12.1.4_build"))
        self.assertEqual(sorted(a["platforms"]), ["linux-x86_64", "windows-x86_64"])
        self.assertEqual(a["sha256"], "ddac49f903da9d5bac833e5cc79395098b9c33cfd3279be5f31bd00387d2d4db")
        self.assertEqual(len(pr.pinned_assets(committed(), "kuna")), 3)

    def test_an_asset_of_another_repository_is_refused(self):
        lock = copy.deepcopy(committed())
        f = part(lock, "ghidra")["builds"][0]["file"]
        f["url"] = f["url"].replace("NationalSecurityAgency/ghidra", "someone/ghidra")
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            pr.pinned_assets(lock, "ghidra")

    def test_a_matching_digest_passes_and_says_what_it_read(self):
        code, out, lock, sent = self.run_cmd()
        self.assertEqual(code, 0, out)
        self.assertIn("GitHub publishes sha256:ddac49f903da9d5bac833e5cc79395098b9c33cfd3279be5f31bd00387d2d4db "
                      "and 569649598 bytes for ghidra_12.1.4_PUBLIC_20260921.zip", out)
        self.assertIn("--confirmed ddac49f903da9d5bac833e5cc79395098b9c33cfd3279be5f31bd00387d2d4db --size 569649598 --record", out)
        self.assertEqual(lock, committed(), "nothing is written without --record")
        # One request, to the release by tag, with the token as a bearer header; the token is
        # never printed.
        self.assertEqual([u for u, _ in sent],
                         ["https://api.github.com/repos/NationalSecurityAgency/ghidra/releases/tags/Ghidra_12.1.4_build"])
        self.assertEqual(sent[0][1]["Authorization"], "Bearer ghs_secret-token-value")
        self.assertNotIn("ghs_secret-token-value", out)

    def test_each_difference_fails(self):
        a = pr.pinned_assets(committed(), "ghidra")[0]
        cases = [
            ({"digest": "sha256:" + "0" * 64}, "differs from the lock's"),
            ({"digest": None}, "publishes no SHA-256 digest"),
            ({"digest": "sha512:" + "0" * 128}, "publishes no SHA-256 digest"),
            ({"size": a["size"] + 1}, "GitHub's size"),
            ({"state": "starter"}, "asset state"),
            ({"browser_download_url": a["url"] + "x"}, "download link"),
            ({"name": "renamed.zip"}, "is not an asset of"),
        ]
        for change, words in cases:
            code, out, lock, _ = self.run_cmd(record=True, release=self.release(**change))
            self.assertEqual(code, 1, (change, out))
            self.assertIn(words, out, change)
            self.assertEqual(lock, committed(), f"{change}: the lock was written")

    def test_record_online_flips_ghidra_to_githubs_digest(self):
        code, out, lock, _ = self.run_cmd(record=True)
        self.assertEqual(code, 0, out)
        for b in part(lock, "ghidra")["builds"]:
            self.assertEqual(b["verified_by"], ["github-digest"])
        self.assertEqual(pr.check(lock, release=True), [])
        self.assertEqual(pr.release_blockers(lock), [])
        # Nothing else changed.
        for p in lock["parts"]:
            if p["name"] != "ghidra":
                self.assertEqual(p, part(committed(), p["name"]))

    def test_record_offline_from_the_values_ci_printed(self):
        code, out, lock, sent = self.run_cmd(
            record=True, confirmed="DDAC49F903DA9D5BAC833E5CC79395098B9C33CFD3279BE5F31BD00387D2D4DB", size=569649598)
        self.assertEqual(code, 0, out)
        self.assertEqual(sent, [], "the offline form asks nothing of the network")
        self.assertTrue(all(b["verified_by"] == ["github-digest"] for b in part(lock, "ghidra")["builds"]))
        for confirmed, size in (("0" * 64, 569649598), ("ddac49f903da9d5bac833e5cc79395098b9c33cfd3279be5f31bd00387d2d4db", 1),
                                ("not-hex", 569649598), ("ddac49f903da9d5bac833e5cc79395098b9c33cfd3279be5f31bd00387d2d4db", None)):
            code, out, lock, _ = self.run_cmd(record=True, confirmed=confirmed, size=size)
            self.assertEqual(code, 1, (confirmed, size, out))
            self.assertEqual(lock, committed())

    def test_kuna_keeps_its_first_download_mark(self):
        lock = pr.with_github_digest(committed(), "kuna")
        for b in part(lock, "kuna")["builds"]:
            self.assertEqual(b["verified_by"], ["github-digest", "first-download-only"])
        self.assertEqual(pr.check(lock), [])
        # The offline form takes one asset; Kuna has three.
        code, out, after, _ = self.run_cmd(argv_part="kuna", record=True, confirmed="0" * 64, size=1)
        self.assertEqual(code, 1)
        self.assertIn("takes one", out)


class Helpers(unittest.TestCase):
    def test_versions(self):
        self.assertEqual(pr.version_key("21.0.13+9"), (21, 0, 13, 9))
        self.assertLess(pr.version_key("1.99"), pr.version_key("1.990"))
        with self.assertRaises(ValueError):
            pr.version_key("v1")

    def test_engines(self):
        r = "^22.23.4 || ^24.22.1 || >=26.12.1"
        for v in ["22.23.4", "24.22.3", "26.12.1", "27.0.0"]:
            self.assertTrue(pr.engines_allow(r, v), v)
        for v in ["22.23.3", "23.0.0", "24.22.0", "26.12.0", "20.0.0"]:
            self.assertFalse(pr.engines_allow(r, v), v)

    def test_safe_names(self):
        for ok in ["bin/node", "Ghidra/Features/x.jar", "legal/java.base/LICENSE"]:
            self.assertTrue(pr.safe_name(ok), ok)
        for bad in ["", "/abs", "a/../b", "a//b", "a\\b", "C:x", "con", "x/NUL.txt", "dot.", "sp "]:
            self.assertFalse(pr.safe_name(bad), bad)


def tar_gz(path: Path, entries):
    with tarfile.open(path, "w:gz") as tf:
        for name, data, mode in entries:
            info = tarfile.TarInfo(name)
            info.size = len(data)
            info.mode = mode
            tf.addfile(info, io.BytesIO(data))


class Survey(unittest.TestCase):
    def test_a_tar_gz_survey(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "node-v1-linux-x64.tar.gz"
            tar_gz(p, [
                ("node-v1-linux-x64/bin/node", b"elf", 0o755),
                ("node-v1-linux-x64/bin/npm", b"js", 0o755),
                ("node-v1-linux-x64/LICENSE", b"mit!", 0o644),
            ])
            s = pr.survey(p, "node-v1-linux-x64/", ["bin/node", "LICENSE"], [{"path": "bin/node", "exec": True}])
            self.assertEqual(s["entries"], 3)
            self.assertEqual(s["unpacked_bytes"], 7)
            self.assertEqual(s["max_path_chars"], 8)
            self.assertEqual(s["programs"], [{"path": "bin/node", "exec": True, "size": 3}])

    def test_a_zip_survey(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "g.zip"
            with zipfile.ZipFile(p, "w") as zf:
                zf.writestr("top/support/analyzeHeadless.bat", "rem")
                zf.writestr("top/Ghidra/application.properties", "application.version=1")
            s = pr.survey(p, "top/", "all", [{"path": "support/analyzeHeadless.bat", "exec": False}])
            self.assertEqual(s["entries"], 2)
            self.assertEqual(s["max_path_chars"], len("Ghidra/application.properties"))


if __name__ == "__main__":
    unittest.main()

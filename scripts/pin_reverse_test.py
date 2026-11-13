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
import email.message
import io
import json
import sys
import tarfile
import tempfile
import unittest
import urllib.request
import urllib.response
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

    def test_the_committed_lock_is_ready_for_a_release(self):
        # Ghidra's digest was recorded from the release record CI run 43380365172 saved.
        lock = committed()
        self.assertEqual(pr.check(lock, release=True), [])
        self.assertEqual(pr.release_blockers(lock), [])
        for b in part(lock, "ghidra")["builds"]:
            self.assertEqual(b["verified_by"], ["github-digest"])
            self.assertEqual(b["github_digest_run"], "https://github.com/sugarhappy8/harnessUI/actions/runs/43380365172")


# A CI run of this repository, as GitHub Actions names it, and its address.
CI_RUN_ENV = {"GITHUB_SERVER_URL": "https://github.com", "GITHUB_REPOSITORY": "sugarhappy8/harnessUI",
              "GITHUB_RUN_ID": "43380365172"}
CI_RUN = "https://github.com/sugarhappy8/harnessUI/actions/runs/43380365172"
# No CI run at all (the tests may themselves run in CI).
NO_CI_RUN_ENV = {"GITHUB_SERVER_URL": "", "GITHUB_REPOSITORY": "", "GITHUB_RUN_ID": ""}
# Addresses that are not a CI run of sugarhappy8/harnessUI.
OTHER_RUNS = [
    "https://github.com/o/harnessUI/actions/runs/123456",
    "https://github.com/sugarhappy8/harnessUI-fork/actions/runs/1",
    "https://github.com/sugarhappy8/other/actions/runs/1",
    "http://github.com/sugarhappy8/harnessUI/actions/runs/1",
    "https://github.com.example/sugarhappy8/harnessUI/actions/runs/1",
    "https://github.com/sugarhappy8/harnessUI/actions/runs/",
    "https://github.com/sugarhappy8/harnessUI/actions/runs/1/job/2",
    "https://github.com/sugarhappy8/harnessUI/actions/runs/1\n",
    "https://github.com/sugarhappy8/harnessUI/actions/runs/١٢",
]


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

    def test_githubs_digest_needs_the_ci_run_that_read_it(self):
        # `github-digest` without the run, or with a run of another repository, fails --check
        # (for Kuna too, whose release needs nothing more).
        for name in ("ghidra", "kuna"):
            for run in [None, "", 7, *OTHER_RUNS]:
                def edit(lock, name=name, run=run):
                    for b in part(lock, name)["builds"]:
                        b["verified_by"] = ["github-digest"] + (["first-download-only"] if name == "kuna" else [])
                        b.pop("github_digest_run", None)
                        if run is not None:
                            b["github_digest_run"] = run
                self.assert_caught(edit, "github_digest_run")

        def good(lock):
            for b in part(lock, "ghidra")["builds"]:
                b["verified_by"] = ["github-digest"]
                b["github_digest_run"] = CI_RUN
        self.assertEqual(self.broken(good), [])


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
            b["github_digest_run"] = CI_RUN
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

    def run_pin(self, edit, env=None, old=None) -> tuple[Path, Path, str]:
        """Runs `pin` with the pin_* functions answering from the committed lock (after
        `edit`), outside CI unless `env` names a run, over `old` as the previous lock."""
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
        if old is not None:
            repo.mkdir(parents=True)
            lock_path.write_text(json.dumps(old, indent=2) + "\n")
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
                mock.patch.dict(pr.os.environ, {**NO_CI_RUN_ENV, **(env or {})}), \
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

    def test_a_pin_keeps_githubs_digest_only_with_the_ci_run_that_read_it(self):
        def live_digest(raw):
            # What pin_ghidra and pin_kuna return when GitHub's digest matched: no run.
            for name, vb in (("ghidra", ["github-digest"]), ("kuna", ["github-digest", "first-download-only"])):
                for b in raw[name]["builds"]:
                    b["verified_by"] = list(vb)
                    b.pop("github_digest_run", None)

        def written(lock_path):
            return json.loads(lock_path.read_text())

        # Outside CI, with no earlier record: GitHub's digest matched here, but nothing links
        # it to a CI run, so it is not recorded and the pin says how to record it.
        lock, _, out = self.run_pin(live_digest)
        for name, vb in (("ghidra", ["first-download-only"]), ("kuna", ["first-download-only"])):
            for b in part(written(lock), name)["builds"]:
                self.assertEqual(b["verified_by"], vb, name)
                self.assertNotIn("github_digest_run", b)
        self.assertIn("NOT READY FOR A RELEASE: ghidra", out)
        self.assertIn("--save-release", out)
        # In a CI run of this repository: recorded with that run.
        lock, _, out = self.run_pin(live_digest, env=CI_RUN_ENV)
        for b in part(written(lock), "ghidra")["builds"]:
            self.assertEqual((b["verified_by"], b["github_digest_run"]), (["github-digest"], CI_RUN))
        self.assertEqual(pr.check(written(lock), release=True), [])
        # In another repository's CI run: not recorded.
        lock, _, _ = self.run_pin(live_digest, env={**CI_RUN_ENV, "GITHUB_REPOSITORY": "o/harnessUI"})
        self.assertTrue(all(b["verified_by"] == ["first-download-only"] for b in part(written(lock), "ghidra")["builds"]))
        # Outside CI over an earlier lock that recorded the very same file with its run: kept.
        old = copy.deepcopy(committed())
        for b in part(old, "ghidra")["builds"]:
            b["verified_by"] = ["github-digest"]
            b["github_digest_run"] = CI_RUN
        lock, _, _ = self.run_pin(live_digest, old=old)
        for b in part(written(lock), "ghidra")["builds"]:
            self.assertEqual((b["verified_by"], b["github_digest_run"]), (["github-digest"], CI_RUN))
        # ... but not for another file.
        def other_file(raw):
            live_digest(raw)
            for b in raw["ghidra"]["builds"]:
                b["file"]["size"] += 1
        lock, _, _ = self.run_pin(other_file, old=old)
        self.assertTrue(all(b["verified_by"] == ["first-download-only"] for b in part(written(lock), "ghidra")["builds"]))


class GithubDigest(unittest.TestCase):
    """`github-digest` (WP9e): CI compares GitHub's published digest of the pinned Ghidra
    asset with the lock and saves the release record it read; `--record` flips the lock to
    `github-digest`, online, or offline from that saved record (never from the lock's own
    values)."""

    def release(self, lock=None, **asset_changes):
        a = pr.pinned_assets(lock or committed(), "ghidra")[0]
        asset = {"name": a["asset"], "digest": "sha256:" + a["sha256"], "size": a["size"],
                 "state": "uploaded", "browser_download_url": a["url"]}
        asset.update(asset_changes)
        return {"tag_name": a["tag"], "assets": [{"name": "other.zip", "digest": None}, asset]}

    def run_cmd(self, argv_part="ghidra", record=False, release=None, lock=None, save_release=None,
                from_release=None, env=None):
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

            args = argparse.Namespace(part=argv_part, record=record, save_release=save_release,
                                      from_release=from_release)
            out = io.StringIO()
            code = 0
            env = {"GITHUB_TOKEN": "ghs_secret-token-value", **NO_CI_RUN_ENV, **(env or {})}
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
        code, out, lock, sent = self.run_cmd(env=CI_RUN_ENV)
        self.assertEqual(code, 0, out)
        self.assertIn("GitHub publishes sha256:ddac49f903da9d5bac833e5cc79395098b9c33cfd3279be5f31bd00387d2d4db "
                      "and 569649598 bytes for ghidra_12.1.4_PUBLIC_20260921.zip", out)
        self.assertIn("run this command again with --record", out)
        self.assertNotIn("--confirmed", out)
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
        code, out, lock, _ = self.run_cmd(record=True, env=CI_RUN_ENV)
        self.assertEqual(code, 0, out)
        for b in part(lock, "ghidra")["builds"]:
            self.assertEqual(b["verified_by"], ["github-digest"])
            self.assertEqual(b["github_digest_run"], CI_RUN)
        self.assertEqual(pr.check(lock, release=True), [])
        self.assertEqual(pr.release_blockers(lock), [])
        # Nothing else changed.
        for p in lock["parts"]:
            if p["name"] != "ghidra":
                self.assertEqual(p, part(committed(), p["name"]))

    def test_record_offline_from_the_release_record_ci_saved(self):
        run = CI_RUN_ENV
        with tempfile.TemporaryDirectory() as d:
            saved = str(Path(d) / "ghidra-release.json")
            # CI: online, saves what GitHub answered, and says how to record it from there.
            code, out, lock, sent = self.run_cmd(save_release=saved, env=run)
            self.assertEqual(code, 0, out)
            self.assertEqual(len(sent), 1)
            self.assertIn(f"download the record this run saved ({CI_RUN})", out)
            self.assertIn("--from-release <the saved file> --record", out)
            record = json.loads(Path(saved).read_text())
            self.assertEqual(record["run"], CI_RUN)
            self.assertNotIn("ghs_secret-token-value", Path(saved).read_text())
            # The lead, offline: the saved record goes through GitHub's checks; nothing is asked.
            code, out, lock, sent = self.run_cmd(record=True, from_release=saved)
            self.assertEqual(code, 0, out)
            self.assertEqual(sent, [], "the offline form asks nothing of the network")
            self.assertIn(f"saved by {CI_RUN} gives sha256:ddac49f9", out)
            self.assertIn("Name this run in the commit", out)
            self.assertTrue(all(b["verified_by"] == ["github-digest"] for b in part(lock, "ghidra")["builds"]))
            self.assertTrue(all(b["github_digest_run"] == CI_RUN for b in part(lock, "ghidra")["builds"]))
            # An offline run without --record prints no ready command.
            code, out, lock, _ = self.run_cmd(from_release=saved)
            self.assertEqual(code, 0, out)
            self.assertNotIn("--record", out)

    def test_a_saved_record_that_does_not_match_records_nothing(self):
        a = pr.pinned_assets(committed(), "ghidra")[0]
        bad = [
            ("not json", None),
            (json.dumps({"releases": "x"}), None),
            (json.dumps({"releases": []}), "holds no release record"),
            (json.dumps({"releases": [pr.saved_release({**self.release(), "tag_name": "Ghidra_12.1.3_build"},
                                                        a["repo"], "Ghidra_12.1.3_build")]}),
             "holds no release record"),
            (json.dumps({"releases": [{**pr.saved_release(self.release(), a["repo"], a["tag"]), "tag_name": "x"}]}),
             "is GitHub's release 'x'"),
            (json.dumps({"releases": [pr.saved_release(self.release(digest="sha256:" + "0" * 64), a["repo"], a["tag"])]}),
             "differs from the lock's"),
            (json.dumps({"releases": [pr.saved_release(self.release(digest=None), a["repo"], a["tag"])]}),
             "publishes no SHA-256 digest"),
        ]
        with tempfile.TemporaryDirectory() as d:
            for i, (text, words) in enumerate(bad):
                saved = Path(d) / f"r{i}.json"
                saved.write_text(text)
                code, out, lock, sent = self.run_cmd(record=True, from_release=str(saved))
                self.assertEqual(code, 1, (text, out))
                if words:
                    self.assertIn(words, out, text)
                self.assertEqual(lock, committed(), f"{text}: the lock was written")
                self.assertEqual(sent, [])

    def test_the_lock_values_alone_are_no_longer_accepted(self):
        a = pr.pinned_assets(committed(), "ghidra")[0]
        argv = ["pin_reverse.py", "github-digest", "--part", "ghidra", "--confirmed", a["sha256"],
                "--size", str(a["size"]), "--record"]
        with mock.patch.object(sys, "argv", argv), contextlib.redirect_stderr(io.StringIO()), \
                self.assertRaises(SystemExit) as e:
            pr.main()
        self.assertEqual(e.exception.code, 2)

    def saved_record(self, d: str, name: str, **fields) -> str:
        """A release record like the one CI saves (its fields replaced by `fields`; a field
        given as None is left out)."""
        a = pr.pinned_assets(committed(), "ghidra")[0]
        record = {"what": "GitHub's release records read by scripts/pin_reverse.py github-digest",
                  "part": "ghidra", "run": CI_RUN,
                  "releases": [pr.saved_release(self.release(), a["repo"], a["tag"])]}
        record.update(fields)
        path = Path(d) / name
        path.write_text(json.dumps({k: v for k, v in record.items() if v is not None}))
        return str(path)

    def test_a_saved_record_without_its_run_records_nothing(self):
        with tempfile.TemporaryDirectory() as d:
            for i, run in enumerate([None, "", 43380365172, ["x"]]):
                saved = self.saved_record(d, f"r{i}.json", run=run)
                code, out, lock, sent = self.run_cmd(record=True, from_release=saved, env=CI_RUN_ENV)
                self.assertEqual(code, 1, (run, out))
                self.assertIn("names no CI run of sugarhappy8/harnessUI", out, run)
                self.assertEqual(lock, committed(), f"{run!r}: the lock was written")
                self.assertEqual(sent, [])

    def test_a_run_of_another_repository_records_nothing(self):
        with tempfile.TemporaryDirectory() as d:
            for i, run in enumerate(OTHER_RUNS):
                saved = self.saved_record(d, f"r{i}.json", run=run)
                code, out, lock, sent = self.run_cmd(record=True, from_release=saved, env=CI_RUN_ENV)
                self.assertEqual(code, 1, (run, out))
                self.assertIn("is not a CI run of sugarhappy8/harnessUI", out, run)
                self.assertEqual(lock, committed(), f"{run!r}: the lock was written")

    def test_a_good_record_is_recorded_with_its_run(self):
        with tempfile.TemporaryDirectory() as d:
            saved = self.saved_record(d, "ghidra-release-record.json")
            code, out, lock, sent = self.run_cmd(record=True, from_release=saved)
            self.assertEqual(code, 0, out)
            self.assertEqual(sent, [])
            self.assertIn(f"Name this run in the commit that records it: {CI_RUN}", out)
            for b in part(lock, "ghidra")["builds"]:
                self.assertEqual(b["verified_by"], ["github-digest"])
                self.assertEqual(b["github_digest_run"], CI_RUN)
            self.assertEqual(pr.check(lock, release=True), [])
            for p in lock["parts"]:
                if p["name"] != "ghidra":
                    self.assertEqual(p, part(committed(), p["name"]))

    def test_record_online_needs_a_ci_run_of_this_repository(self):
        for env in [{}, {**CI_RUN_ENV, "GITHUB_REPOSITORY": "o/harnessUI"}, {**CI_RUN_ENV, "GITHUB_RUN_ID": "1x"}]:
            code, out, lock, _ = self.run_cmd(record=True, env=env)
            self.assertEqual(code, 1, (env, out))
            self.assertIn("CI run of sugarhappy8/harnessUI", out, env)
            self.assertEqual(lock, committed(), f"{env}: the lock was written")
        # Outside CI the hint is the CI path, not a --record that would be refused.
        code, out, _, _ = self.run_cmd()
        self.assertEqual(code, 0, out)
        self.assertNotIn("run this command again with --record", out)
        self.assertIn("--save-release <file>", out)
        self.assertIn("--from-release <that file> --record", out)

    def test_kuna_keeps_its_first_download_mark(self):
        lock = pr.with_github_digest(committed(), "kuna", CI_RUN)
        for b in part(lock, "kuna")["builds"]:
            self.assertEqual(b["verified_by"], ["github-digest", "first-download-only"])
            self.assertEqual(b["github_digest_run"], CI_RUN)
        self.assertEqual(pr.check(lock), [])


class FakeWeb:
    """Fake https and http handlers for `http_get` (through `OPENER_HANDLERS`): `routes` maps
    a URL to (status, Location or None, body); every request is recorded with its headers
    (plain and unredirected)."""

    def __init__(self, routes):
        self.routes = routes
        self.seen = []
        web = self

        def answer(req, scheme):
            web.seen.append((scheme, req.full_url, {k.lower(): v for k, v in req.header_items()}))
            status, location, body = web.routes.get(req.full_url, (404, None, b""))
            headers = email.message.Message()
            if location:
                headers["Location"] = location
            resp = urllib.response.addinfourl(io.BytesIO(body), headers, req.full_url, status)
            resp.msg = {200: "OK", 302: "Found"}.get(status, "Not Found")
            return resp

        class Https(urllib.request.HTTPSHandler):
            def https_open(self, req):
                return answer(req, "https")

        class Http(urllib.request.HTTPHandler):
            def http_open(self, req):
                return answer(req, "http")

        self.handlers = [urllib.request.ProxyHandler({}), Https(), Http()]


class HttpGetRedirects(unittest.TestCase):
    """`http_get` follows redirects only to https, and the headers it is given in `extra`
    (the GitHub token) never go on to a redirect's target."""

    API = "https://api.github.com/repos/o/r/releases/tags/t"
    EXTRA = {"Authorization": "Bearer ghs_REVIEW_CANARY", "X-GitHub-Api-Version": "2022-11-28"}

    def get(self, routes):
        web = FakeWeb(routes)
        out = io.StringIO()
        with mock.patch.object(pr, "OPENER_HANDLERS", web.handlers), contextlib.redirect_stderr(out):
            try:
                body = pr.http_get(self.API, accept="application/json", extra=self.EXTRA)
            except SystemExit as e:
                body = e
        return body, web.seen, out.getvalue()

    def test_the_token_goes_to_the_api_only(self):
        body, seen, _ = self.get({self.API: (200, None, b"{}")})
        self.assertEqual(body, b"{}")
        self.assertEqual(seen[0][2]["authorization"], "Bearer ghs_REVIEW_CANARY")

    def test_a_redirect_to_plain_http_is_refused(self):
        body, seen, err = self.get({self.API: (302, "http://127.0.0.1:9/steal", b"")})
        self.assertIsInstance(body, SystemExit)
        self.assertEqual([s for s, _, _ in seen], ["https"], "nothing was sent over plain http")
        self.assertIn("non-https", err)
        self.assertNotIn("ghs_REVIEW_CANARY", err)

    def test_a_redirect_to_another_host_carries_no_token(self):
        other = "https://objects.example/asset"
        body, seen, _ = self.get({self.API: (302, other, b""), other: (200, None, b"ok")})
        self.assertEqual(body, b"ok")
        self.assertEqual([u for _, u, _ in seen], [self.API, other])
        followed = seen[1][2]
        self.assertNotIn("authorization", followed)
        self.assertNotIn("x-github-api-version", followed)
        self.assertEqual(followed["user-agent"], pr.USER_AGENT)


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

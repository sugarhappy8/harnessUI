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

import copy
import io
import json
import sys
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path

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

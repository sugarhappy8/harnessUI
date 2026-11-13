#!/usr/bin/env python3
"""ARCHITECTURE.md against the files it describes (0.0.6).

The sentences about rea's tests and CI (9.3, 13.6, 16 and 17) are checked against
`.github/workflows/reverse.yml` and `ci.yml`, `e2e/scripted_agent.py` and the files they
name, and no `TODO(lead)` marker may be left in ARCHITECTURE.md. A failure names the
sentence to correct.

    python3 -I scripts/docs_test.py

Standard library only.
"""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WORKFLOWS = ROOT / ".github" / "workflows"
FILE_SUFFIXES = (".rs", ".py", ".yml", ".json", ".ts", ".tsx", ".sh", ".md")


def read(rel: str) -> str:
    return (ROOT / rel).read_text(encoding="utf-8")


def architecture() -> list[str]:
    return read("ARCHITECTURE.md").splitlines()


def section(heading: str) -> list[str]:
    """The lines under `heading` (a whole heading line) up to the next heading of any level."""
    lines = architecture()
    start = lines.index(heading) + 1
    end = next((i for i in range(start, len(lines)) if lines[i].startswith("#")), len(lines))
    return lines[start:end]


def paragraph(lines: list[str], start: str) -> str:
    """The one line (each paragraph and bullet is one line) that starts with `start`."""
    found = [line for line in lines if line.startswith(start)]
    if len(found) != 1:
        raise AssertionError(f"expected one line starting with {start!r}, found {len(found)}")
    return found[0]


def backticked(text: str) -> list[str]:
    return re.findall(r"`([^`]+)`", text)


def reverse_sentences() -> dict[str, str]:
    """The sentences this test keeps in step with the code, by where they are."""
    return {
        "9.3 e2e and contract tests": paragraph(section("### 9.3 Built-in servers"), "The e2e scenario `reverse-engineering`"),
        "13.6 stays offline": paragraph(section("### 13.6 Network use"), "- **What 0.0.6 adds that stays offline.**"),
        "13.6 test": paragraph(section("### 13.6 Network use"), "- **Test:**"),
        "16 reverse CI": paragraph(section("## 16. Packaging, CI and release"), "- **Reverse engineering in CI (0.0.6).**"),
        "17 added in 0.0.6": paragraph(section("## 17. Testing"), "**Added in 0.0.6.**"),
    }


def probes() -> set[str]:
    return set(re.findall(r'^@probe\("([a-z0-9-]+)"\)', read("e2e/scripted_agent.py"), re.M))


def jobs(workflow: str) -> set[str]:
    return set(re.findall(r"^  ([a-z0-9-]+):\s*$", (WORKFLOWS / workflow).read_text(encoding="utf-8"), re.M))


class NoMarkersLeft(unittest.TestCase):
    def test_architecture_holds_no_lead_markers(self):
        left = [f"line {n}: {line[:120]}" for n, line in enumerate(architecture(), 1) if "TODO(lead)" in line]
        self.assertEqual(left, [], "confirm each sentence against the code, then drop its marker")


class NamedFilesExist(unittest.TestCase):
    def test_every_file_and_workflow_named_exists(self):
        for where, text in reverse_sentences().items():
            for token in backticked(text):
                if re.fullmatch(r"[\w-]+\.yml", token):
                    self.assertTrue((WORKFLOWS / token).is_file(), f"{where}: no workflow {token}")
                elif re.fullmatch(r"[\w.-]+(/[\w.-]+)+/?", token) and (token.endswith("/") or token.endswith(FILE_SUFFIXES)):
                    self.assertTrue((ROOT / token).exists(), f"{where}: {token} does not exist")

    def test_the_e2e_scenarios_named_exist_and_every_reverse_scenario_is_named(self):
        text = reverse_sentences()["17 added in 0.0.6"]
        listed = text.split("e2e scenarios (`e2e/scripted_agent.py`):", 1)[1].split(";", 1)[0]
        named = set(backticked(listed))
        self.assertEqual(named - probes(), set(), "17 names scenarios scripted_agent.py does not have")
        reverse = {p for p in probes() if p.startswith("reverse-")}
        self.assertEqual(reverse - named, set(), "17 leaves out reverse-engineering scenarios")


class ReverseCiAsTheWorkflowsRunIt(unittest.TestCase):
    def setUp(self):
        self.reverse = (WORKFLOWS / "reverse.yml").read_text(encoding="utf-8")
        self.ci = (WORKFLOWS / "ci.yml").read_text(encoding="utf-8")
        self.sentences = reverse_sentences()
        self.bullet = self.sentences["16 reverse CI"]

    def test_the_programs_blocked_on_windows_are_named(self):
        block = self.reverse.split("$programs = @(", 1)[1].split(")", 1)[0]
        programs = re.findall(r"\\([\w.-]+\.exe)'", block)
        self.assertTrue(programs, "reverse.yml lists no programs to block")
        for program in programs:
            self.assertIn(f"`{program}`", self.bullet)

    def test_each_job_names_the_decompilers_it_must_test(self):
        self.assertEqual(jobs("reverse.yml"), {"linux", "windows", "macos"})
        self.assertEqual(re.findall(r"^\s+REAL_REA_REQUIRE:\s*(\S+)", self.reverse, re.M), ["kuna,ghidra", "ghidra", "kuna"])
        self.assertIn("`REAL_REA_REQUIRE`", self.bullet)

    def test_each_recorder_is_shown_a_control_first(self):
        self.assertGreaterEqual(self.reverse.count("192.0.2.1"), 3)
        self.assertIn("control", self.bullet)

    def test_the_linux_checker_fails_on_name_lookups(self):
        self.assertIn("htons(53)", self.reverse)
        self.assertIn("name lookup", self.bullet)
        self.assertIn("name lookup", self.sentences["13.6 stays offline"])

    def test_the_server_call_path_runs_with_the_real_rea(self):
        self.assertIn("--test fake_rea", self.reverse)
        self.assertIn("`crates/reverse/tests/fake_rea.rs`", self.bullet)

    def test_the_ghidra_digest_is_its_own_ci_job(self):
        self.assertIn("ghidra-digest", jobs("ci.yml"))
        self.assertIn("`ghidra-digest`", self.bullet)

    def test_the_offline_test_names_what_exists(self):
        test = self.sentences["13.6 test"]
        self.assertIn("`e2e/m8_offline.py`", test)
        self.assertIn('"Reverse engineering"', read("e2e/m8_offline.py"))
        if "m8_offline" not in self.ci:
            self.assertIn("not in CI", test)
        self.assertIn("owner", test)


if __name__ == "__main__":
    unittest.main()

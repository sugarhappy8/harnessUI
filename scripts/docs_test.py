#!/usr/bin/env python3
"""ARCHITECTURE.md against the files it describes (0.0.6).

The sentences about rea's tests and CI (9.3, 13.6, 16 and 17) are checked against
`.github/workflows/reverse.yml` and `ci.yml`, `e2e/scripted_agent.py` and the files they
name, and no `TODO(lead)` marker may be left in ARCHITECTURE.md. SECURITY.md, CHANGELOG.md
and docs/dev/0.0.6/item5-spec.md may claim no more of those checks than ARCHITECTURE.md
does. A failure names the sentence to correct.

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


def doc_paragraph(rel: str, needle: str) -> str:
    """The one paragraph of a hard-wrapped document (the lines between blank lines, joined
    with single spaces) that holds `needle`."""
    paragraphs = [" ".join(p.split()) for p in re.split(r"\n\s*\n", read(rel))]
    found = [p for p in paragraphs if needle in p]
    if len(found) != 1:
        raise AssertionError(f"{rel}: expected one paragraph holding {needle!r}, found {len(found)}")
    return found[0]


def doc_line(rel: str, needle: str) -> str:
    """The one line of a document that holds `needle`."""
    found = [line for line in read(rel).splitlines() if needle in line]
    if len(found) != 1:
        raise AssertionError(f"{rel}: expected one line holding {needle!r}, found {len(found)}")
    return found[0]


def spec_bullet(rel: str, start: str) -> str:
    """A bullet that starts with `start`, with its indented continuation lines, joined."""
    lines = read(rel).splitlines()
    first = [i for i, line in enumerate(lines) if line.startswith(start)]
    if len(first) != 1:
        raise AssertionError(f"{rel}: expected one bullet starting with {start!r}, found {len(first)}")
    out = [lines[first[0]]]
    for line in lines[first[0] + 1 :]:
        if not line.startswith("  ") or not line.strip():
            break
        out.append(line)
    return " ".join(" ".join(out).split())


def windows_blocked_programs(workflow: str) -> list[str]:
    """The programs reverse.yml gives outbound block rules on Windows."""
    block = workflow.split("$programs = @(", 1)[1].split(")", 1)[0]
    return re.findall(r"\\([\w.-]+\.exe)'", block)


def macos_blocked_protocols(workflow: str) -> list[str]:
    """The protocols reverse.yml's pf rule blocks for the test user ([] for all)."""
    rule = re.search(r"block drop out log quick on ! lo0 (.*) user reatest", workflow)
    if rule is None:
        raise AssertionError("reverse.yml has no pf rule for the test user")
    protocols = re.search(r"proto \{ ([^}]+) \}", rule.group(1))
    return [p.strip() for p in protocols.group(1).split(",")] if protocols else []


def probes() -> set[str]:
    return set(re.findall(r'^@probe\("([a-z0-9-]+)"\)', read("e2e/scripted_agent.py"), re.MULTILINE))


def jobs(workflow: str) -> set[str]:
    """The job ids of a workflow: the keys one level under `jobs:`."""
    text = (WORKFLOWS / workflow).read_text(encoding="utf-8").split("\njobs:\n", 1)[1]
    return set(re.findall(r"^  ([a-z0-9-]+):\s*$", text, re.MULTILINE))


class NoMarkersLeft(unittest.TestCase):
    def test_architecture_holds_no_lead_markers(self):
        left = [f"line {n}: {line[:120]}" for n, line in enumerate(architecture(), 1) if "TODO(lead)" in line]
        self.assertEqual(left, [], "confirm each sentence against the code, then drop its marker")

    def test_no_release_document_waits_for_wp9e(self):
        for doc in ("SECURITY.md", "README.md", "CHANGELOG.md"):
            left = [n for n, line in enumerate(read(doc).splitlines(), 1) if "confirm after WP9e" in line]
            self.assertEqual(left, [], f"{doc}: lines still waiting for WP9e")


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
        programs = windows_blocked_programs(self.reverse)
        self.assertTrue(programs, "reverse.yml lists no programs to block")
        for program in programs:
            self.assertIn(f"`{program}`", self.bullet)

    def test_each_job_names_the_decompilers_it_must_test(self):
        self.assertEqual(jobs("reverse.yml"), {"linux", "windows", "macos"})
        self.assertEqual(re.findall(r"^\s+REAL_REA_REQUIRE:\s*(\S+)", self.reverse, re.MULTILINE), ["kuna,ghidra", "ghidra", "kuna"])
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


# What each recorder of reverse.yml sees, in the words SECURITY.md uses for its programs.
PLAIN_NAMES = {"node.exe": "Node.js", "java.exe": "Java", "javaw.exe": "Java", "decompile.exe": "decompiler"}


class NoClaimBeyondWhatEachJobChecks(unittest.TestCase):
    """On Windows only the programs with a block rule are blocked and logged, and on macOS pf
    blocks only the protocols its rule names: no document may say more."""

    def setUp(self):
        self.reverse = (WORKFLOWS / "reverse.yml").read_text(encoding="utf-8")
        sentences = reverse_sentences()
        self.offline = sentences["13.6 stays offline"]
        self.bullet = sentences["16 reverse CI"]
        self.security = doc_paragraph("SECURITY.md", "the real decompilers")

    def test_13_6_names_the_programs_blocked_on_windows(self):
        for program in windows_blocked_programs(self.reverse):
            self.assertIn(f"`{program}`", self.offline, "13.6 must name what Windows blocks, as 16 does")
        self.assertNotIn("any attempt to reach another computer", self.offline)

    def test_security_names_the_programs_blocked_on_windows(self):
        for program in windows_blocked_programs(self.reverse):
            self.assertIn(program, PLAIN_NAMES, f"say in SECURITY.md that Windows blocks {program}, then add it here")
            self.assertIn(PLAIN_NAMES[program], self.security)
        self.assertNotIn("any of their processes", self.security)

    def test_the_protocols_pf_blocks_on_macos_are_named(self):
        protocols = macos_blocked_protocols(self.reverse)
        if protocols:
            words = " and ".join(p.upper() for p in protocols)
            for where, text in (("13.6", self.offline), ("16", self.bullet), ("SECURITY.md", self.security)):
                self.assertIn(words, text, f"{where}: pf blocks only {words} on macOS")
            self.assertNotIn("any packet", self.offline)


class ReleaseDocsSayWhetherTheWorkflowRan(unittest.TestCase):
    """While ARCHITECTURE.md 16 says reverse.yml has not run on GitHub, SECURITY.md and
    CHANGELOG.md say so too; once 16 says it has run, neither may still say it has not."""

    def test_security_and_changelog_agree_with_16(self):
        pending = "has not run on GitHub yet" in reverse_sentences()["16 reverse CI"]
        docs = {
            "SECURITY.md": doc_paragraph("SECURITY.md", "the real decompilers"),
            "CHANGELOG.md": doc_line("CHANGELOG.md", "the real rea and its decompilers"),
        }
        for where, text in docs.items():
            self.assertEqual("has not run" in text, pending, f"{where}: say whether reverse.yml has run, as 16 does")


class OfflineCoverAsItIs(unittest.TestCase):
    """The 13.6 Test bullet says what the offline test sees; the testing table and
    item5-spec's N6 may not lean on a release-gate test that records name lookups."""

    def setUp(self):
        self.test = reverse_sentences()["13.6 test"]
        self.ci = (WORKFLOWS / "ci.yml").read_text(encoding="utf-8")

    def test_n6_names_the_cover_that_exists(self):
        n6 = spec_bullet("docs/dev/0.0.6/item5-spec.md", "- **N6.")
        if "not direct connections or name lookups" in self.test:
            self.assertNotIn("release-gate test", n6, "13.6 says no test records name lookups")
            self.assertIn("section 9", n6, "name the owner checklist that records them")

    def test_the_offline_row_says_what_runs_today(self):
        row = paragraph(section("## 17. Testing"), "| Offline |")
        notes = row.split("|")[3]
        self.assertIn("`e2e/m8_offline.py`", notes)
        self.assertIn("13.6", notes)
        if "m8_offline" not in self.ci:
            self.assertIn("by hand", notes)
            self.assertIn("planned", notes)


if __name__ == "__main__":
    unittest.main()

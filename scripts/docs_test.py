#!/usr/bin/env python3
"""ARCHITECTURE.md against the files it describes (0.0.6).

The sentences about rea's tests and CI (9.3, 13.6, 16 and 17) are checked against
`.github/workflows/reverse.yml` and `ci.yml`, `e2e/scripted_agent.py` and the files they
name, and no `TODO(lead)` marker may be left in ARCHITECTURE.md. SECURITY.md, CHANGELOG.md
and docs/dev/0.0.6/item5-spec.md may claim no more of those checks than ARCHITECTURE.md
does. ARCHITECTURE.md and docs/dev/0.0.6/item6-spec.md state the `llama-fit-params` time
limit the code sets. What the docs say about Studio (0.1.0: hosts, money rules, audit row,
CSP, the engine pin, the local rules and the scope note) is checked against the code that
keeps it, and CHANGELOG.md's 0.1.0 entry must say what Studio adds, local first. A failure
names the sentence to correct.

    python3 -I scripts/docs_test.py

Standard library only.
"""

import json
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
        self.assertIn("htons(53)", read("scripts/check_strace.py"))
        self.assertIn("scripts/check_strace.py", self.reverse)
        self.assertIn("name lookup", self.bullet)
        self.assertIn("UDP socket to port 0", self.bullet)
        # A send on such a socket is checked by its peer (final review, area 4, I1).
        self.assertIn("checked by its peer", self.bullet)
        self.assertIn("a send on a UDP socket connected to port 0", self.bullet)
        self.assertIn("udp-port-0-send", self.reverse)
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

    def test_the_nightly_run_is_said_where_it_runs(self):
        # Final review, area 4, I2: the workflow runs every night, and passed on all three.
        self.assertIn("schedule:", (WORKFLOWS / "reverse.yml").read_text(encoding="utf-8"))
        self.assertIn("runs every night on the default branch", reverse_sentences()["16 reverse CI"])
        self.assertIn("It runs every night on GitHub, on Linux, Windows and macOS.", doc_paragraph("SECURITY.md", "the real decompilers"))
        self.assertNotIn("unverified", reverse_sentences()["13.6 stays offline"])


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


def fit_params_timeout_secs() -> int:
    """The limit `crates/runtime/src/llama/fit_params.rs` sets for one measurement, in seconds."""
    found = re.findall(
        r"pub const FIT_PARAMS_TIMEOUT: Duration = Duration::from_secs\((\d+)\);",
        read("crates/runtime/src/llama/fit_params.rs"),
    )
    if len(found) != 1:
        raise AssertionError(f"fit_params.rs: expected one FIT_PARAMS_TIMEOUT, found {len(found)}")
    return int(found[0])


class FitParamsLimitAsTheCodeSetsIt(unittest.TestCase):
    """Every place ARCHITECTURE.md and item6-spec.md state `FIT_PARAMS_TIMEOUT` gives the
    number of seconds the code sets."""

    def setUp(self):
        self.secs = fit_params_timeout_secs()

    def test_item6_spec_states_the_limit_the_code_sets(self):
        spec = "docs/dev/0.0.6/item6-spec.md"
        sketch = re.findall(r"FIT_PARAMS_TIMEOUT: Duration = Duration::from_secs\((\d+)\)", read(spec))
        self.assertEqual(sketch, [str(self.secs)], f"{spec}: the fit_params.rs interface sketch")
        self.assertIn(f"(`FIT_PARAMS_TIMEOUT`, {self.secs} s", spec_bullet(spec, "- **I6-14 "), f"{spec}: I6-14")

    def test_architecture_states_the_limit_the_code_sets(self):
        rows = [r for r in architecture() if r.startswith("| `llama-fit-params` (0.0.6) |")]
        self.assertEqual(len(rows), 2, "the process table row (4) and the threat table row (13)")
        process_row, threat_row = rows
        self.assertIn(f"or {self.secs} s (`FIT_PARAMS_TIMEOUT`)", process_row, "4: the process table row")
        self.assertIn(f"scrubbed environment, {self.secs} s,", threat_row, "13: the threat table row")
        measurement = paragraph(architecture(), "2. *The measurement,*")
        self.assertIn(f"killed after {self.secs} s", measurement, "7.7: the measurement")


class StudioDocs(unittest.TestCase):
    """What the docs say about Studio matches the code: hosts, money rules, audit row, CSP, scope note,
    and the 0.1.0 changelog entry."""

    def test_13_6_has_a_studio_row_with_its_hosts(self):
        row = paragraph(section("### 13.6 Network use"), "| Studio |")
        for words in ("`openrouter.ai`", "`api.openai.com`", "guarded fetcher", "without the key"):
            self.assertIn(words, row)

    def test_pickle_files_and_the_formats_studio_reads_are_said_exactly(self):
        # Final review, area 4, I3 with area 1, M-3 and area 2, M4, by controller ruling (design
        # section 3): pickle files are never read, parsed or run and are only a plain download;
        # Studio reads .safetensors, .sft and .gguf, plus a tokenizer's .json.
        index = read("crates/models/src/sd/index.rs")
        self.assertIn('PICKLE_EXTENSIONS: [&str; 4] = [".ckpt", ".pt", ".pth", ".bin"]', index)
        self.assertIn('READ_EXTENSIONS: [&str; 3] = [".safetensors", ".sft", ".gguf"]', index)
        self.assertIn('TOKENIZER_EXTENSION: &str = ".json"', read("crates/models/src/gen_catalog.rs"))
        security = doc_paragraph("SECURITY.md", "Studio reads `.safetensors`, `.sft` and `.gguf` model files")
        files = paragraph(section("### 7.9 Studio on this computer (0.1.0)"), "- **Model files.**")
        for where, text in (("SECURITY.md", security), ("ARCHITECTURE.md 7.9", files)):
            for words in (
                "`.safetensors`, `.sft` and `.gguf`",
                "tokenizer's `.json`",
                "(`.ckpt`, `.pt`, `.pth`, `.bin`)",
                "never reads, parses or runs",
                "plain file download for another app",
                "*Download file anyway*",
            ):
                self.assertIn(words, text, where)
            for stale in ("refused everywhere", "must be `.gguf` or `.safetensors`", "`.gguf` and `.safetensors` only"):
                self.assertNotIn(stale, text, where)

    def test_the_input_delay_names_no_removed_confirmation(self):
        # Final review, area 4, M4: G4 removed the Allow all and Full access confirmations.
        aside = paragraph(architecture(), "- *Stepping aside (0.0.7):*")
        self.assertNotIn("Allow all / Full access confirmation", aside)
        self.assertIn("every approval card and the buttons that set the agent going again", aside)

    def test_the_out_of_memory_retry_flags_are_the_codes(self):
        run = read("crates/runtime/src/sd/run.rs")
        self.assertIn("Err(SdFailure::OutOfMemory) if !job.offload_to_cpu => {}", run)
        self.assertIn("offload_to_cpu: true,\n            vae_tiling: job.tiling_allowed,", run)
        process = paragraph(section("### 7.9 Studio on this computer (0.1.0)"), "- **One process per job.**")
        self.assertIn(
            "a run that ran out of memory without `--offload-to-cpu` is tried once more with memory saving: "
            "`--offload-to-cpu`, and `--vae-tiling` unless the recipe disables tiling",
            process,
        )
        self.assertNotIn("(`--vae-tiling --offload-to-cpu`)", process)

    def test_the_money_rules_are_the_codes(self):
        spend = read("src-tauri/src/studio/spend.rs")
        for name, value in (("CONFIRM_AT_USD", "1.0"), ("DEFAULT_DAILY_LIMIT_USD", "10.0"), ("UNKNOWN_PRICE_RESERVE_USD", "5.0")):
            self.assertRegex(spend, rf"pub const {name}: f64 = {re.escape(value)};")
        for doc in ("SECURITY.md", "README.md"):
            self.assertIn("Spending guards are off by default", read(doc), doc)
            self.assertIn("$10", read(doc), doc)

    def test_the_audit_row_keeps_the_prompt_length_only(self):
        bullet = paragraph(section("### 13.7 Audit log"), "- *Studio* (0.1.0)")
        self.assertIn("length", bullet)
        self.assertIn("not the prompt", bullet)

    def test_the_spec_says_the_scope_changed(self):
        self.assertIn("*(Changed in 0.1.0 at the owner's request: image and video generation on this computer, with cloud models "
                      "as the backup; a picture or video model can run beside the chat model when both fit; "
                      "see [docs/dev/0.1.0/README.md](dev/0.1.0/README.md).)*", read("docs/SPEC.md"))

    def test_the_csp_the_docs_describe_is_the_apps(self):
        csp = json.loads(read("src-tauri/tauri.conf.json"))["app"]["security"]["csp"]
        self.assertIn("media-src blob:", csp)
        self.assertIn(f"**CSP:** `{csp}`", paragraph(section("### 13.2 Webview lockdown"), "- **CSP:**"))
        self.assertIn("media-src blob:", read("SECURITY.md"))

    def test_the_offline_test_visits_studio(self):
        self.assertIn('"Studio"', read("e2e/m8_offline.py"))

    def test_13_6_has_a_row_for_studio_on_this_computer(self):
        row = paragraph(section("### 13.6 Network use"), "| Studio on this computer |")
        for words in ("`huggingface.co`", "GitHub", "on a click", "nothing while it runs"):
            self.assertIn(words, row)

    def test_the_engine_pin_the_docs_name_is_the_locks(self):
        tag = json.loads(read("sd.lock.json"))["sd_cpp"]["tag"]
        pin = "\n".join(section("### 7.1 Pinning and layout"))
        self.assertIn(f"`{tag}`", pin)
        self.assertIn("`sd.lock.json`", pin)
        self.assertIn("re-recorded", pin)
        self.assertEqual(read("crates/runtime/tests/fixtures/sd_fixtures_build.txt").strip(), tag)

    def test_7_9_states_the_local_rules_the_code_keeps(self):
        local = "\n".join(section("### 7.9 Studio on this computer (0.1.0)"))
        for words in ("one `sd-cli` process per job", "no `sd-server`", "`--prompt-file`", "1 MiB", "16,384 characters",
                      "262,144 characters", "`.ckpt`",
                      "`--disable-image-metadata`", "never `resources/llama/main`", "Unload the chat model",
                      "installing an update stops it"):
            self.assertIn(words, local)
        self.assertNotIn("4,000 characters", local)
        args = read("crates/runtime/src/sd/args.rs")
        self.assertRegex(args, r"pub const PROMPT_MAX_BYTES: usize = 1_048_576;")
        self.assertRegex(args, r"pub const LOCAL_PROMPT_MAX_CHARS: usize = 16_384;")
        self.assertRegex(args, r"pub const NEGATIVE_MAX_CHARS: usize = PROMPT_MAX_BYTES / 4;")

    def test_security_says_a_local_run_sends_nothing(self):
        self.assertIn("A local Studio run sends nothing", read("SECURITY.md"))

    def test_security_states_the_prompt_bounds_the_code_keeps(self):
        engine = doc_paragraph("SECURITY.md", "The prompt is never on its command line")
        for words in ("at most 16,384 characters, so the child-safety check reads all of it",
                      "at most 262,144 characters", "longer than 1 MiB"):
            self.assertIn(words, engine)
        self.assertNotIn("4,000 characters", engine)

    def test_the_readme_leads_studio_with_this_computer(self):
        studio = doc_paragraph("README.md", "Studio makes pictures and short videos")
        self.assertLess(studio.index("on this computer"), studio.index("OpenRouter"))

    def test_the_notices_name_the_studio_engine(self):
        notices = read("THIRD-PARTY-NOTICES.md")
        self.assertIn("stable-diffusion.cpp", notices)
        self.assertIn("The Studio engine is not part of the Windows installer.", notices)

    def test_no_document_promises_a_local_time_before_the_benchmark(self):
        if any(e.get("speed") for e in json.loads(read("catalog/models.json")).get("generators", {}).values()):
            self.skipTest("the catalog has measured speed points: time figures may be written")
        for doc in ("README.md", "CHANGELOG.md"):
            self.assertNotRegex(read(doc), r"\b(s|seconds?|min|minutes?) (a|per|for a) (picture|clip)", doc)

    def test_the_changelog_tells_users_what_0_1_0_does(self):
        entries = read("CHANGELOG.md").split("\n## ")[1:]
        entry = next((item for item in entries if item.startswith("0.1.0 (")), "")
        self.assertTrue(entry, "missing 0.1.0 changelog entry")
        for words in ("Studio", "On this computer", "stable-diffusion.cpp", "Hugging Face", "$10", "$1 or more", "### Added", "### Known limits"):
            self.assertIn(words, entry)
        added = entry.split("### Added", 1)[1].split("\n### ", 1)[0]
        self.assertLess(added.index("On this computer"), added.index("OpenRouter"))

    def test_the_changelog_tells_users_what_0_2_0_does(self):
        entries = read("CHANGELOG.md").split("\n## ")[1:]
        entry = next((item for item in entries if item.startswith("0.2.0 (")), "")
        self.assertTrue(entry, "missing 0.2.0 changelog entry")
        for words in ("Hugging Face", "LoRA", "Works in Studio", "off by default", "### Added", "### Changed", "### Known limits"):
            self.assertIn(words, entry)
        for words in ("Task ", ".rs", "CI"):
            self.assertNotIn(words, entry)

    def test_the_0_2_0_entry_says_where_settings_are_and_what_an_update_turns_off(self):
        # Final review: area 3, I6 and area 4, I4 (the model sheet only picks a model) and
        # area 4, I5 (an update from 0.1.x turns the old default spending prefs off once).
        entries = read("CHANGELOG.md").split("\n## ")[1:]
        entry = next((item for item in entries if item.startswith("0.2.0 (")), "")
        sheet = next(line for line in entry.splitlines() if line.startswith("- **The model sheet.**"))
        self.assertNotIn("its settings and your saved setups sit in one sheet", sheet)
        for words in ("for picking a model", "*Settings* side panel", "*My setups*"):
            self.assertIn(words, sheet)
        spending = next(line for line in entry.splitlines() if line.startswith("- **The spending limits are off by default.**"))
        for words in ("update from 0.1.x", "$10", "every video", "$1 or more", "turned off once", "you changed yourself is kept"):
            self.assertIn(words, spending)
        prefs = read("src-tauri/src/studio/prefs.rs")
        for code in ("DEFAULT_DAILY_LIMIT_USD", "CONFIRM_AT_USD", "prefs.confirm_every_video = false"):
            self.assertIn(code, prefs)


class StaleFrom0_1_xDocs(unittest.TestCase):
    """The 0.1.0 review's follow-ups (docs/dev/0.1.0/follow-ups.md, items 1 to 8): the process and
    trust tables, the sidecar, the signing claims, README's first sentence, the VC++ DLLs and
    7.9's removed 0.1.x gates."""

    def test_section_4_lists_sd_cli(self):
        row = paragraph(section("## 4. Process model"), "| `sd-cli`")
        for words in ("one process per local Studio job", "spawn helper"):
            self.assertIn(words, row)

    def test_13_1_has_a_studio_engine_row(self):
        lines = section("### 13.1 Trust boundaries")
        row = paragraph(lines, "| `sd-cli`")
        for words in ("Pinned build only", "the prompt by file", "`.ckpt`", "the checksum proves origin, not safety"):
            self.assertIn(words, row)
        self.assertIn("`sd-cli`", paragraph(lines, "| Model files"))

    def test_13_6_names_what_runs_at_launch_and_what_the_engine_is_given(self):
        row = paragraph(section("### 13.6 Network use"), "| Studio |")
        for words in ("`resume_interrupted`", "still waiting or downloading", "the endpoint's key"):
            self.assertIn(words, row)
        bullet = paragraph(section("### 13.6 Network use"), "- **Studio (0.1.0).**")
        for words in ("the pictures added to the run", "`--lora-model-dir`", "`--tokenizer`", "its prompt file"):
            self.assertIn(words, bullet)
        args = read("crates/runtime/src/sd/args.rs")
        for flag in ("--lora-model-dir", "--tokenizer", "--prompt-file", "--negative-prompt-file", "--end-img", "--control-image"):
            self.assertIn(f'"{flag}"', args)

    def test_the_sidecar_is_said_to_hold_the_prompt(self):
        security = doc_paragraph("SECURITY.md", "Results are kept in the gallery folder")
        sentence = security.split("Results are kept in the gallery folder", 1)[1].split("Studio has no tool", 1)[0]
        self.assertIn("record of its settings and its prompt beside it that never holds a key", sentence)
        self.assertIn("The prompt is never written into a local result's file", sentence)
        for doc in ("README.md", "CHANGELOG.md"):
            self.assertIn("a small record of each run's settings and its prompt", doc_line(doc, "a small record of each run"), doc)

    def test_the_signed_claims_wait_for_the_developer_id(self):
        self.assertIn("once the Apple Developer ID exists", paragraph(section("### 7.1 Pinning and layout"), "- **The Studio engine (0.1.0)"))
        self.assertIn("once the Apple Developer ID exists", doc_paragraph("SECURITY.md", "Studio's local pictures and videos are made by one more program"))

    def test_the_readme_first_sentence_names_studio(self):
        text = read("README.md")
        first = text.split("\n\n", 2)[1]
        self.assertIn("chat, tools, skills and MCP", first)
        self.assertIn("Studio", first)
        self.assertNotIn("early test builds", text)

    def test_the_changelog_keeps_no_build_machine_sentence(self):
        self.assertNotIn("The build machine has no graphics card", read("CHANGELOG.md"))

    def test_the_vc_bullet_says_who_needs_which_dll(self):
        bullet = paragraph(section("### 7.1 Pinning and layout"), "- **VC++ runtime.**")
        self.assertIn("llama.cpp needs the first three; the Studio engine also needs `msvcp140_codecvt_ids.dll` and `vcomp140.dll`", bullet)
        self.assertNotIn("Every llama.cpp binary needs them", bullet)

    def test_7_9_has_no_0_1_0_licence_gate(self):
        local = "\n".join(section("### 7.9 Studio on this computer (0.1.0)"))
        self.assertNotIn("Not supported yet", local)
        self.assertNotIn("after its licence is shown", local)
        self.assertNotIn("once its licence is accepted", "\n".join(section("### 7.1 Pinning and layout")))

    def test_the_dll_names_are_the_locks(self):
        lock = read("llama.lock.json")
        for dll in ("msvcp140_codecvt_ids.dll", "vcomp140.dll"):
            self.assertIn(dll, lock)

    def test_17_credits_each_local_and_cloud_e2e_to_its_script(self):
        text = "\n".join(section("## 17. Testing"))
        self.assertIn("cloud in `e2e/m12_studio.py`", text)
        self.assertIn("`e2e/m8_offline.py`", text)
        self.assertIn("`jiff`", "\n".join(section("## 18. Dependencies")))


class Studio0_2_0Docs(unittest.TestCase):
    """0.2.0: any model the engine can run (7.10), what is read before a download, and the
    fewer blocks, checked against the code and the lock they describe."""

    def test_7_10_names_the_caps_the_code_keeps(self):
        index = read("crates/models/src/sd/index.rs")
        self.assertRegex(index, r"pub const SAFETENSORS_HEADER_CAP: u64 = 100 \* 1024 \* 1024;")
        self.assertRegex(index, r"pub const GGUF_HEADER_CAP: u64 = 64 \* 1024 \* 1024;")
        self.assertRegex(read("crates/models/src/sd/remote.rs"), r"pub const MAX_FILES_PER_REPO: usize = 24;")
        self.assertRegex(read("src-tauri/src/studio/detect.rs"), r"pub const CACHE_ENTRIES: usize = 512;")
        text = "\n".join(section("### 7.10 Any model the engine can run (0.2.0)"))
        for words in ("100 MiB", "64 MiB", "24 files", "512 entries", "Range"):
            self.assertIn(words, text)

    def test_7_10_names_the_recipes_and_their_commit(self):
        lock = json.loads(read("sd.lock.json"))["sd_cpp"]
        text = "\n".join(section("### 7.10 Any model the engine can run (0.2.0)"))
        self.assertIn("`catalog/sd-recipes.json`", text)
        self.assertIn("`scripts/vendor_sd_recipes.py`", text)
        self.assertIn(f"`{lock['tag'].rsplit('-', 1)[1]}`", text)
        vendored = json.loads(read("catalog/sd-recipes.json"))
        self.assertEqual(vendored["sdCommit"], lock["commit"])
        # 94 run in Studio and 18 do not: the engine's 112.
        self.assertEqual(len(vendored["recipes"]), 112)
        for words in ("94 of the engine's 112 recipes", "the other 18"):
            self.assertIn(words, text)

    def test_7_10_reads_one_header_per_group_of_quantisations(self):
        text = "\n".join(section("### 7.10 Any model the engine can run (0.2.0)"))
        self.assertIn("one header read for each group of quantisations", text)
        self.assertIn("smallest file", text)
        self.assertIn("labels every quantisation", text)
        self.assertNotIn("one per quantisation", text)
        cand = read("crates/models/src/sd/candidates.rs")
        self.assertIn("/// The smallest member; for a split file, its first shard.", cand)

    def test_7_10_says_what_the_code_does_with_a_file(self):
        text = "\n".join(section("### 7.10 Any model the engine can run (0.2.0)"))
        for words in ("`--lora-model-dir`", "`<models>/@loras`", "`<lora:NAME:W>`", "`--prompt-file`", "`--vae-tiling`",
                      "`--offload-to-cpu`", "`@imported`", "Set it up yourself", "untested set-up",
                      "Lumina 2", "LTX-Video 0.9", "HunyuanVideo", "never parsed"):
            self.assertIn(words, text)
        lora = read("crates/runtime/src/sd/lora_tags.rs")
        self.assertRegex(lora, r"pub const MAX_LORAS: usize = 8;")
        self.assertRegex(lora, r"LORA_WEIGHT_RANGE: RangeInclusive<f32> = -1\.0\.\.=2\.0;")

    def test_7_9_says_a_fit_only_warns(self):
        local = "\n".join(section("### 7.9 Studio on this computer (0.1.0)"))
        self.assertIn("May not fit", local)
        self.assertNotIn("is refused unless it is a picture", local)
        self.assertNotIn("Not for this computer", local)
        self.assertNotIn("family_from_tensors", local)

    def test_13_6_says_when_studio_reads_file_headers(self):
        row = paragraph(section("### 13.6 Network use"), "| Studio on this computer |")
        for words in ("Range", "when a result opens", "each time Get models opens", "Popular"):
            self.assertIn(words, row)

    def test_13_6_qualifies_what_is_sent_with_nothing_clicked(self):
        lines = section("### 13.6 Network use")
        quiet = paragraph(lines, "- **No telemetry, no crash upload, no account.**")
        for words in ("resume_interrupted", "cloud video"):
            self.assertIn(words, quiet)
        studio = paragraph(lines, "- **Studio (0.1.0).**")
        self.assertIn("`resume_interrupted`", studio)
        self.assertIn("Get models", studio)
        self.assertNotIn("Opening Studio, Get local models or Settings", studio)

    def test_13_6_names_every_settings_page_the_offline_test_visits(self):
        self.assertIn('"Search"', read("e2e/m8_offline.py"))

    def test_13_1_says_headers_are_untrusted_and_pickles_never_parsed(self):
        lines = section("### 13.1 Trust boundaries")
        row = paragraph(lines, "| Model file headers (0.2.0)")
        for words in ("Untrusted input", "never executed", "100 MiB", "never parsed"):
            self.assertIn(words, row)

    def test_16_and_17_name_the_detection_fixtures_and_the_capture_script(self):
        for heading in ("## 16. Packaging, CI and release", "## 17. Testing"):
            text = "\n".join(section(heading))
            self.assertIn("`scripts/capture_sd_fixtures.py`", text, heading)
            self.assertIn("not run in CI", text, heading)
        self.assertIn("`scripts/vendor_sd_recipes_test.py`", "\n".join(section("## 16. Packaging, CI and release")))
        self.assertIn("scripts/vendor_sd_recipes_test.py", read(".github/workflows/ci.yml"))

    def test_the_llama_server_signing_claim_waits_for_the_developer_id(self):
        bullet = paragraph(section("### 7.1 Pinning and layout"), "- **macOS.**")
        self.assertIn("once the Apple Developer ID exists", bullet)

    def test_security_names_the_header_caps_and_the_prompt_bound(self):
        text = read("SECURITY.md")
        for words in ("100 MiB", "64 MiB", "24 files", "1 MiB", "16,384"):
            self.assertIn(words, text)
        self.assertNotIn("after its licence is shown", text)
        folder = doc_paragraph("SECURITY.md", "Studio's LoRAs are kept")
        for words in ("@loras", "never run"):
            self.assertIn(words, folder)

    def test_security_says_a_click_is_ignored_again_when_the_window_returns(self):
        cards = doc_paragraph("SECURITY.md", "Cards are built to be read safely")
        self.assertIn("window", cards)

    def test_the_readme_says_any_model_the_engine_runs(self):
        studio = doc_paragraph("README.md", "Studio makes pictures and short videos")
        for words in ("LoRA", "Hugging Face", "Add a model file"):
            self.assertIn(words, studio)
        self.assertLess(studio.index("on this computer"), studio.index("OpenRouter"))
        self.assertNotIn("Not for this computer", read("README.md"))


if __name__ == "__main__":
    unittest.main()

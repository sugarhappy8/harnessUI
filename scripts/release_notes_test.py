#!/usr/bin/env python3
"""Tests for scripts/release_notes.py, which release.yml runs for every release.

They check that the version's CHANGELOG.md entry is found and shortened as release.yml
needs, that the app's text (latest.json `notes`, shown under *What's new*) is plain text
without the download paragraph, that the release page gets the entry as written plus that
paragraph, and that release.yml runs it that way.

    python3 -I scripts/release_notes_test.py

Standard library only.
"""

import contextlib
import io
import json
import os
import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import release_notes as rn  # noqa: E402

CHANGELOG = """# Changelog

All notable changes, by milestone. Dates are UTC.

## 9.8.7 (2026-12-01)

The first version that does a thing.

### Added
- **A box.** It says "Update ready", with *Restart and update* and `harnessUI-setup.exe`.
  - ***Settings → General → Updates* shows it.** See [SECURITY.md](SECURITY.md).
- Globs such as `*.dmg` and `v*` stay; so does 2 * 3, a*b and \\*literal\\*.

## 9.8.6 (2026-11-30)

- **Older.**
"""


class TheEntry(unittest.TestCase):
    def test_the_lines_under_the_version_up_to_the_next_version(self):
        notes = rn.entry(CHANGELOG, "9.8.7")
        self.assertTrue(notes.startswith("The first version that does a thing."))
        self.assertIn("### Added", notes)
        self.assertNotIn("Older", notes)
        self.assertNotIn("9.8.7", notes)

    def test_a_heading_without_a_date_counts(self):
        self.assertEqual(rn.entry("## 1.0.0\n\nText.\n", "1.0.0"), "Text.")

    def test_a_version_without_an_entry_or_with_an_empty_one_stops(self):
        for text, version in ((CHANGELOG, "9.8.8"), (CHANGELOG, "9.8"), ("## 1.0.0\n\n## 0.9.0\n- x\n", "1.0.0")):
            with self.subTest(version=version):
                with self.assertRaises(ValueError) as e:
                    rn.entry(text, version)
                self.assertIn(version, str(e.exception))

    def test_a_long_entry_is_shortened_at_a_line_end(self):
        lines = [f"- line {i} " + "x" * 90 for i in range(400)]
        notes = rn.entry("## 1.0.0\n\n" + "\n".join(lines) + "\n", "1.0.0")
        self.assertTrue(notes.endswith("\n\n" + rn.SHORTENED))
        kept = notes[: -len(rn.SHORTENED)].rstrip()
        self.assertLessEqual(len(kept), rn.LIMIT)
        self.assertTrue(kept.splitlines()[-1] in lines, "it is cut after a whole line")


class TheAppsText(unittest.TestCase):
    """latest.json `notes`, which the update box shows as plain text."""

    def test_no_markdown_marks_are_left(self):
        text = rn.app_notes(CHANGELOG, "9.8.7")
        self.assertEqual(
            text.splitlines(),
            [
                "The first version that does a thing.",
                "",
                "Added",
                '- A box. It says "Update ready", with Restart and update and harnessUI-setup.exe.',
                "  - Settings → General → Updates shows it. See SECURITY.md.",
                "- Globs such as *.dmg and v* stay; so does 2 * 3, a*b and *literal*.",
            ],
        )

    def test_it_has_no_download_paragraph(self):
        text = rn.app_notes(CHANGELOG, "9.8.7")
        self.assertNotIn("Download", text)
        self.assertNotIn("Run anyway", text)
        self.assertNotIn("by hand", text)

    def test_the_real_changelog_converts_without_marks(self):
        changelog = rn.CHANGELOG.read_text(encoding="utf-8")
        version = json.loads(rn.APP_CONFIG.read_text(encoding="utf-8"))["version"]
        if f"## {version}" not in changelog:
            self.skipTest(f"CHANGELOG.md has no entry for {version} yet")
        text = rn.app_notes(changelog, version)
        for mark in ("**", "`", "](", "\n#"):
            self.assertNotIn(mark, text)


class ThePagesText(unittest.TestCase):
    def test_the_entry_as_written_then_the_download_paragraph(self):
        text = rn.page_notes(CHANGELOG, "9.8.7", "harnessUI-setup.exe", "harnessUI-universal.dmg")
        entry = rn.entry(CHANGELOG, "9.8.7")
        self.assertTrue(text.startswith(entry + "\n\n**Download.**"))
        self.assertTrue(text.endswith("\n"))
        self.assertIn("`harnessUI-setup.exe`", text)
        self.assertIn("`harnessUI-universal.dmg`", text)
        self.assertIn("*Run anyway*", text)
        self.assertIn("Privacy & Security", text)
        self.assertIn("Applications folder", text)


class CommandLine(unittest.TestCase):
    def setUp(self):
        self.dir = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.dir)
        changelog = self.dir / "CHANGELOG.md"
        changelog.write_text(CHANGELOG, encoding="utf-8")
        app_config = self.dir / "tauri.conf.json"
        app_config.write_text('{\n  "version": "9.8.7"\n}\n', encoding="utf-8")
        for name, value in (("CHANGELOG", changelog), ("APP_CONFIG", app_config)):
            patcher = mock.patch.object(rn, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)

    def run_main(self, argv, env):
        out = io.StringIO()
        with mock.patch.dict(os.environ, env), contextlib.redirect_stdout(out):
            code = rn.main(argv)
        return code, out.getvalue()

    def test_app_appends_the_body_to_github_output(self):
        output = self.dir / "github_output"
        output.write_text("earlier=1\n", encoding="utf-8")
        code, out = self.run_main(["app"], {"GITHUB_OUTPUT": str(output)})
        self.assertEqual(code, 0, out)
        written = output.read_bytes().decode("utf-8")
        self.assertNotIn("\r", written)
        first, rest = written.split("\n", 1)
        self.assertEqual(first, "earlier=1")
        start, rest = rest.split("\n", 1)
        self.assertTrue(start.startswith("body<<EOF_"))
        delim = start[len("body<<") :]
        self.assertEqual(rest, rn.app_notes(CHANGELOG, "9.8.7") + "\n" + delim + "\n")

    def test_page_writes_the_file(self):
        target = self.dir / "notes.md"
        env = {"STABLE_WINDOWS": "a-setup.exe", "STABLE_MAC": "b.dmg"}
        code, out = self.run_main(["page", str(target)], env)
        self.assertEqual(code, 0, out)
        self.assertEqual(target.read_bytes().decode("utf-8"), rn.page_notes(CHANGELOG, "9.8.7", "a-setup.exe", "b.dmg"))

    def test_missing_settings_or_entry_fail_with_an_error_line(self):
        code, out = self.run_main(["page", str(self.dir / "notes.md")], {"STABLE_WINDOWS": "", "STABLE_MAC": ""})
        self.assertEqual(code, 1)
        self.assertTrue(out.startswith("::error::"))
        rn.APP_CONFIG.write_text('{"version": "9.9.9"}', encoding="utf-8")
        code, out = self.run_main(["app"], {"GITHUB_OUTPUT": str(self.dir / "github_output")})
        self.assertEqual(code, 1)
        self.assertIn("::error::CHANGELOG.md has no entry for 9.9.9", out)
        self.assertFalse((self.dir / "github_output").exists(), "nothing is written for a missing entry")


class TheWorkflow(unittest.TestCase):
    def setUp(self):
        self.workflow = (rn.ROOT / ".github" / "workflows" / "release.yml").read_text(encoding="utf-8")

    def job(self, name, following):
        return self.workflow.split(f"\n  {name}:\n", 1)[1].split(f"\n  {following}:\n", 1)[0]

    def test_the_build_gets_the_apps_text(self):
        publish = self.job("publish", "catalog")
        self.assertIn("python3 -I scripts/release_notes.py app", publish)
        self.assertIn("releaseBody: ${{ steps.notes.outputs.body }}", publish)
        self.assertNotIn("Run anyway", publish, "the download paragraph is not in the build's notes")

    def test_the_page_gets_its_text_when_the_release_is_published(self):
        verify = self.workflow.split("\n  verify:\n", 1)[1]
        self.assertIn("python3 -I scripts/release_notes.py page", verify)
        publish_lines = [line.strip() for line in verify.splitlines() if "--draft=false" in line]
        self.assertEqual(len(publish_lines), 1)
        self.assertIn("--notes-file", publish_lines[0])

    def test_ci_runs_these_tests(self):
        ci = (rn.ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
        self.assertIn("python3 -I scripts/release_notes_test.py", ci)


if __name__ == "__main__":
    unittest.main()

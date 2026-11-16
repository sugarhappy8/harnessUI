#!/usr/bin/env python3
"""A release's notes, from the version's entry in CHANGELOG.md (release.yml runs this).

The same entry gives two texts:

- `app`: what the app's update box shows under *What's new*. tauri-action copies the release
  body it is given into latest.json as `notes`, and the app shows those as plain text, so
  this is the entry with its Markdown marks taken out (a heading becomes a plain line; bold,
  italics, backticks and link addresses go) and nothing else. The download paragraph is not
  in it: it is for someone on the release page, not for someone updating inside the app.
  Written to $GITHUB_OUTPUT as `body` (the publish job, before the build).
- `page`: what the release page shows: the entry as written (GitHub shows the Markdown),
  then where to download and how to open the builds, which are not code-signed. The verify
  job sets it when it publishes the release, so latest.json keeps the app's text.

    python3 -I scripts/release_notes.py app          (needs GITHUB_OUTPUT)
    python3 -I scripts/release_notes.py page FILE    (needs STABLE_WINDOWS and STABLE_MAC)

The version is the one in src-tauri/tauri.conf.json. A version without an entry stops with
an error. An entry longer than 20,000 characters is shortened at a line end: an action's
input must fit in one environment variable, which Windows limits to 32,767 characters.

Standard library only.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CHANGELOG = ROOT / "CHANGELOG.md"
APP_CONFIG = ROOT / "src-tauri" / "tauri.conf.json"
LIMIT = 20000
SHORTENED = "(Shortened here: the whole list is in CHANGELOG.md.)"

HEADING = re.compile(r"^#{1,6}\s+(.*?)(?:\s+#+)?\s*$")
CODE = re.compile(r"`([^`\n]+)`")
LINK = re.compile(r"\[([^\]\n]+)\]\([^()\s]+\)")
# One `*` on each side of a word or phrase: not inside a word, not around a space.
ITALIC = re.compile(r"(?<![\w*])\*(?=\S)([^*\n]*?\S)\*(?![\w*])")
# A backslash before ASCII punctuation is Markdown's way of writing that mark as itself.
ESCAPED = re.compile(r"\\([!-/:-@\[-`{-~])")


# --- Pure helpers (scripts/release_notes_test.py tests these) -----------------------------


def entry(changelog: str, version: str) -> str:
    """The version's entry: the lines under `## <version>` or `## <version> (date)` up to the
    next `## ` heading, shortened past LIMIT characters. ValueError when there is none."""
    lines = changelog.splitlines()
    start = next((i for i, line in enumerate(lines) if line == f"## {version}" or line.startswith(f"## {version} (")), None)
    if start is None:
        raise ValueError(
            f"CHANGELOG.md has no entry for {version} (a line '## {version} (date)'). "
            "It is what the release page and the app's What's new show."
        )
    end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("## ")), len(lines))
    notes = "\n".join(lines[start + 1 : end]).strip()
    if not notes:
        raise ValueError(f"CHANGELOG.md's entry for {version} is empty. It is what the release page and the app's What's new show.")
    if len(notes) > LIMIT:
        cut = notes.rfind("\n", 0, LIMIT)
        notes = notes[: cut if cut > 0 else LIMIT].rstrip() + "\n\n" + SHORTENED
    return notes


def _plain_words(text: str) -> str:
    """Markdown outside a code span, without its marks."""
    held: list[str] = []

    def hold(m: re.Match) -> str:
        held.append(m.group(1))
        return f"\0{len(held) - 1}\0"

    text = ESCAPED.sub(hold, text)
    text = LINK.sub(r"\1", text)
    text = text.replace("**", "")
    text = ITALIC.sub(r"\1", text)
    return re.sub(r"\0(\d+)\0", lambda m: held[int(m.group(1))], text)


def plain(markdown: str) -> str:
    """The entry as plain text, line for line: headings without their #, no bold or italic
    marks, code without backticks, links as their words. Bullets and indents stay, because
    the app keeps the text's line breaks and spaces."""
    out = []
    for line in markdown.splitlines():
        heading = HEADING.match(line)
        if heading:
            line = heading.group(1)
        # Split around code spans: the odd parts are code, kept as written.
        parts = CODE.split(line)
        out.append("".join(part if i % 2 else _plain_words(part) for i, part in enumerate(parts)))
    return "\n".join(out)


def download_paragraph(windows: str, mac: str) -> str:
    """Where to download and how to open the builds, for the release page only."""
    return (
        f"**Download.** Windows 10 or 11 (64-bit): `{windows}`. "
        f"Mac with macOS 13.3 or later (Apple silicon or Intel): `{mac}`; drag harnessUI into the "
        "Applications folder and open it from there, so it can update itself. The files with the "
        "version in their names are the same installers. These builds are not code-signed: on "
        "Windows choose *More info*, then *Run anyway*; on a Mac open the app once, then choose "
        "*Open Anyway* in *System Settings → Privacy & Security* (on macOS 14 and older, "
        "right-clicking the app and choosing *Open* also works). harnessUI 0.0.8 and later update "
        "themselves; 0.0.7 and older cannot, so install this version once by hand. See README.md "
        "for details."
    )


def app_notes(changelog: str, version: str) -> str:
    return plain(entry(changelog, version))


def page_notes(changelog: str, version: str, windows: str, mac: str) -> str:
    return f"{entry(changelog, version)}\n\n{download_paragraph(windows, mac)}\n"


# --- Command line --------------------------------------------------------------------------


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="A release's notes from its CHANGELOG.md entry.")
    sub = parser.add_subparsers(dest="text", required=True)
    sub.add_parser("app", help="the plain text for latest.json and What's new, into $GITHUB_OUTPUT as body")
    page = sub.add_parser("page", help="the release page's text, into FILE")
    page.add_argument("file")
    args = parser.parse_args(argv)

    version = json.loads(APP_CONFIG.read_text(encoding="utf-8"))["version"]
    changelog = CHANGELOG.read_text(encoding="utf-8")
    # Messages are ASCII: a Windows runner's output may not take other characters.
    try:
        if args.text == "app":
            output = os.environ.get("GITHUB_OUTPUT")
            if not output:
                print("::error::GITHUB_OUTPUT is not set; this runs in a GitHub Actions step.")
                return 1
            text = app_notes(changelog, version)
            delim = "EOF_" + os.urandom(8).hex()
            with open(output, "a", encoding="utf-8", newline="\n") as out:
                out.write(f"body<<{delim}\n{text}\n{delim}\n")
            print(f"What's new for {version}: its CHANGELOG.md entry as plain text ({len(text)} characters).")
        else:
            windows, mac = os.environ.get("STABLE_WINDOWS", ""), os.environ.get("STABLE_MAC", "")
            if not windows or not mac:
                print("::error::STABLE_WINDOWS and STABLE_MAC must be set (release.yml sets both).")
                return 1
            text = page_notes(changelog, version, windows, mac)
            with open(args.file, "w", encoding="utf-8", newline="\n") as out:
                out.write(text)
            print(f"Release page for {version}: its CHANGELOG.md entry and the download paragraph ({len(text)} characters).")
    except ValueError as e:
        print(f"::error::{e}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

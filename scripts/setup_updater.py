#!/usr/bin/env python3
"""Sets up in-app updates once: makes the updater key pair and commits its public half.

Every release must be able to update itself (README.md, "In-app updates"). The release
workflow signs each release's update files with a private key kept as a repository secret,
and an installed copy accepts an update only when its signature matches the public key
that copy was built with. Run this once, on your own computer, from the repository root:

    python3 -I scripts/setup_updater.py               (Windows: py scripts\\setup_updater.py)
    python3 -I scripts/setup_updater.py --key PATH    (keep the key file somewhere else)
    python3 -I scripts/setup_updater.py --check       (release.yml: is a usable public key committed?)

What it does:
- Stops, changing nothing, when src-tauri/tauri.updater.conf.json already has a public key:
  a new key would leave every installed copy unable to update.
- Runs the Tauri CLI's `tauri signer generate -w <key file>` (run `npm ci` first), which asks
  for a password. The key file is ~/.tauri/harnessui.key unless --key says otherwise. A key
  file that already exists is never overwritten; when it and its .pub both exist, it offers
  to use them. The CLI's script is started with node, not with `npx`: on Windows npx is a
  batch file that cmd.exe runs, and cmd.exe would take characters such as & or ^ in the key
  file's path (a Windows user name may hold them) as its own.
- Writes the public key (the .pub file's contents) into plugins.updater.pubkey of
  src-tauri/tauri.updater.conf.json.
- With the GitHub CLI (`gh`) installed, offers to save the two repository secrets; otherwise
  it says where to add them.

It never prints the private key: the key file is read only to hand it to `gh` on its input.

Standard library only.
"""

from __future__ import annotations

import argparse
import base64
import binascii
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Callable

ROOT = Path(__file__).resolve().parent.parent
CONFIG = ROOT / "src-tauri" / "tauri.updater.conf.json"
APP_CONFIG = ROOT / "src-tauri" / "tauri.conf.json"
# The Tauri CLI's own script (its package's `bin`), run with node.
TAURI_CLI = ROOT / "node_modules" / "@tauri-apps" / "cli" / "tauri.js"
DEFAULT_KEY = Path("~") / ".tauri" / "harnessui.key"
COMMENT = "untrusted comment:"
PRIVATE_SECRET = "TAURI_SIGNING_PRIVATE_KEY"
PASSWORD_SECRET = "TAURI_SIGNING_PRIVATE_KEY_PASSWORD"


# --- Pure helpers (scripts/setup_updater_test.py tests these) ---------------------------


def current_pubkey(config_text: str) -> str:
    """The public key committed in tauri.updater.conf.json, or "" when there is none."""
    value = json.loads(config_text)["plugins"]["updater"].get("pubkey") or ""
    return value.strip()


def with_pubkey(config_text: str, pubkey: str) -> str:
    """tauri.updater.conf.json's text with `pubkey` as plugins.updater.pubkey, in the file's
    own layout (two-space indents, keys in their order, its line endings, the final newline
    kept)."""
    config = json.loads(config_text)
    config["plugins"]["updater"]["pubkey"] = pubkey
    text = json.dumps(config, indent=2, ensure_ascii=False)
    if config_text.endswith("\n"):
        text += "\n"
    return text.replace("\n", "\r\n") if "\r\n" in config_text else text


def repo_of(config_text: str) -> str:
    """owner/name of the GitHub repository the updater reads latest.json from."""
    for url in json.loads(config_text)["plugins"]["updater"].get("endpoints") or []:
        m = re.match(r"^https://github\.com/([\w.-]+)/([\w.-]+)/releases/", url)
        if m:
            return f"{m.group(1)}/{m.group(2)}"
    raise ValueError("tauri.updater.conf.json names no GitHub release address in plugins.updater.endpoints")


def _key_bytes(line: str) -> bytes:
    """The bytes of minisign's public key line: "Ed", the key id (8 bytes), the key (32)."""
    try:
        raw = base64.b64decode(line.strip(), validate=True)
    except (binascii.Error, ValueError):
        raise ValueError("its key line is not base64") from None
    if len(raw) != 42 or raw[:2] != b"Ed":
        raise ValueError("its key line is not a minisign public key")
    return raw


def key_id(raw: bytes) -> str:
    """The key id as minisign shows it (16 hexadecimal digits)."""
    return raw[2:10][::-1].hex().upper()


def parse_pubkey(text: str) -> tuple[str, str]:
    """The value for plugins.updater.pubkey from a public key file's text, and the key id.

    `tauri signer generate` writes the public key as base64 of minisign's two lines (a
    comment, then the key); that is the form the updater reads, and it is returned
    unchanged. Minisign's two lines themselves, or the key line alone, are turned into that
    form. Anything else raises ValueError with the reason in plain words, a private key
    included. The text itself is never part of the message.
    """
    t = text.strip()
    if not t:
        raise ValueError("the public key is empty")
    if t.startswith(COMMENT):
        box, as_written = t, False
    else:
        try:
            decoded = base64.b64decode(t, validate=True)
        except (binascii.Error, ValueError):
            raise ValueError("this is not a public key: it is neither base64 nor minisign's text") from None
        try:
            decoded_text = decoded.decode("utf-8")
        except UnicodeDecodeError:
            decoded_text = ""
        if decoded_text.startswith(COMMENT):
            box, as_written = decoded_text, True
        elif len(decoded) == 42 and decoded[:2] == b"Ed":
            box, as_written = f"{COMMENT} minisign public key: {key_id(decoded)}\n{t}\n", False
        else:
            raise ValueError("this is not a minisign public key")
    lines = [line.strip() for line in box.strip().splitlines()]
    if "secret key" in lines[0].lower() or "private key" in lines[0].lower():
        raise ValueError("this is a private key, not the public key; it must never go into tauri.updater.conf.json")
    if len(lines) < 2:
        raise ValueError("the key line after the comment is missing")
    raw = _key_bytes(lines[1])
    if as_written:
        return t, key_id(raw)
    value = base64.b64encode(f"{lines[0]}\n{lines[1]}\n".encode()).decode()
    return value, key_id(raw)


def check_config(config_text: str) -> tuple[bool, str]:
    """Whether the committed public key is one the updater can use, and what to say."""
    value = current_pubkey(config_text)
    if not value:
        return False, (
            "src-tauri/tauri.updater.conf.json has no public key (plugins.updater.pubkey). "
            "Every release must be able to update itself: run python3 -I scripts/setup_updater.py "
            "(on Windows: py scripts\\setup_updater.py or python scripts\\setup_updater.py) once "
            "on your own computer (README.md, \"In-app updates\")."
        )
    try:
        written, kid = parse_pubkey(value)
    except ValueError as e:
        return False, f"The public key in src-tauri/tauri.updater.conf.json cannot be used: {e}."
    if written != value:
        return False, (
            "The public key in src-tauri/tauri.updater.conf.json must be the .pub file's contents "
            "exactly as `tauri signer generate` wrote them (one line of base64)."
        )
    return True, f"The updater public key is committed (key id {kid})."


def version_of(app_config_text: str) -> str:
    return json.loads(app_config_text)["version"]


# --- Talking to the person and to node and gh ----------------------------------------------


def ask(question: str) -> bool:
    """A yes/no question; anything but y or yes, or no answer at all, is no."""
    try:
        return input(question).strip().lower() in ("y", "yes")
    except EOFError:
        print()
        return False


class Stop(Exception):
    """Ends the setup with a message and a failure code; nothing more is changed."""


def generate(key: Path, which: Callable, run: Callable) -> None:
    """Makes a new key pair with the Tauri CLI, which asks for the password itself."""
    node = which("node")
    if node is None:
        raise Stop("node was not found. Install Node.js 22, run `npm ci` in the repository, then run this again.")
    if not TAURI_CLI.is_file():
        raise Stop("The Tauri CLI is not installed here. Run `npm ci` in the repository, then run this again.")
    key.parent.mkdir(parents=True, exist_ok=True)
    print(f"Making a new key pair in {key}. The Tauri CLI asks for a password: choose a strong one")
    print("and keep it; every release needs it.")
    # CI=true in the environment would make the CLI skip the password question.
    env = {k: v for k, v in os.environ.items() if k != "CI"}
    sys.stdout.flush()
    done = run([node, str(TAURI_CLI), "signer", "generate", "-w", str(key)], cwd=str(ROOT), env=env)
    if done.returncode != 0:
        raise Stop("`tauri signer generate` did not finish. Nothing was written into tauri.updater.conf.json.")


def save_secrets(key: Path, repo: str, which: Callable, run: Callable, ask: Callable) -> bool:
    """Saves the two repository secrets with gh when the person agrees. True when both are
    taken care of; False when they must be added by hand."""
    gh = which("gh")
    if gh is None:
        return False
    if not ask(f"Save the private key and its password as repository secrets of {repo} with gh now? [y/N] "):
        return False
    sys.stdout.flush()
    done = run([gh, "secret", "set", PRIVATE_SECRET, "--repo", repo], input=key.read_bytes())
    if done.returncode != 0:
        print(f"gh could not save {PRIVATE_SECRET} (is `gh auth login` done?).")
        return False
    print(f"Saved {PRIVATE_SECRET}.")
    if ask(f"Save {PASSWORD_SECRET} too? gh asks you to paste the password. Answer n only if the key has no password. [y/N] "):
        sys.stdout.flush()
        done = run([gh, "secret", "set", PASSWORD_SECRET, "--repo", repo])
        if done.returncode != 0:
            print(f"gh could not save {PASSWORD_SECRET}.")
            return False
        print(f"Saved {PASSWORD_SECRET}.")
    else:
        print(f"{PASSWORD_SECRET} was not saved. If the key has a password, add it at")
        print(f"https://github.com/{repo}/settings/secrets/actions before you release.")
    return True


def secrets_by_hand(key: Path, repo: str) -> None:
    print()
    print(f"Add two repository secrets at https://github.com/{repo}/settings/secrets/actions")
    print("(New repository secret):")
    print(f"- {PRIVATE_SECRET}: everything in {key} (open it in a text editor and copy it all).")
    print(f"- {PASSWORD_SECRET}: the password you chose for the key (leave it out if the key")
    print("  has no password).")


def setup(
    key: Path,
    config: Path = CONFIG,
    app_config: Path = APP_CONFIG,
    which: Callable = shutil.which,
    run: Callable = subprocess.run,
    ask: Callable = ask,
) -> int:
    # Read and written as bytes, so the file keeps its line endings on Windows too.
    config_text = config.read_bytes().decode("utf-8")
    if current_pubkey(config_text):
        print("In-app updates are already set up: src-tauri/tauri.updater.conf.json has a public key.")
        print("Nothing was changed. Replacing that key would leave every installed copy unable to")
        print("update, because each one accepts only updates signed with the key it was built with.")
        return 0
    repo = repo_of(config_text)
    pub = key.with_name(key.name + ".pub")
    try:
        if key.exists() and pub.exists():
            if not ask(f"{key} and {pub} already exist. Use this key pair? [y/N] "):
                raise Stop("Nothing was changed. This script never overwrites a key file; to make a new key pair, choose another file with --key PATH.")
        elif key.exists() or pub.exists():
            there, missing = (key, pub) if key.exists() else (pub, key)
            raise Stop(f"{there} exists but {missing} does not. Nothing was changed. Choose another file with --key PATH, or put the missing file back.")
        else:
            generate(key, which, run)
        if not key.is_file() or not pub.is_file():
            raise Stop(f"The key pair is not where it should be ({key} and {pub}). Nothing was written into tauri.updater.conf.json.")
        try:
            value, kid = parse_pubkey(pub.read_text(encoding="utf-8"))
        except (ValueError, UnicodeDecodeError) as e:
            raise Stop(f"{pub} cannot be used: {e}. Nothing was written into tauri.updater.conf.json.") from None
    except Stop as e:
        print(e)
        return 1
    config.write_bytes(with_pubkey(config_text, value).encode("utf-8"))
    print(f"Wrote the public key (key id {kid}) into src-tauri/tauri.updater.conf.json.")

    if not save_secrets(key, repo, which, run, ask):
        secrets_by_hand(key, repo)

    version = version_of(app_config.read_text(encoding="utf-8"))
    print()
    print("Next steps:")
    print(f"1. Back up {key} and its password in your password manager. If you lose either one,")
    print("   installed copies can never update again.")
    print("2. Commit src-tauri/tauri.updater.conf.json and push it.")
    print("3. Release: on GitHub open Actions, then Release, then Run workflow on the commit to release,")
    print(f"   or push the tag v{version}.")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Set up in-app updates: the updater key pair and its public key.")
    parser.add_argument("--key", default=str(DEFAULT_KEY), help=f"where the private key file goes (default {DEFAULT_KEY})")
    parser.add_argument("--check", action="store_true", help="only check that a usable public key is committed")
    args = parser.parse_args(argv)
    if args.check:
        ok, message = check_config(CONFIG.read_text(encoding="utf-8"))
        print(message)
        return 0 if ok else 1
    return setup(Path(args.key).expanduser())


if __name__ == "__main__":
    sys.exit(main())

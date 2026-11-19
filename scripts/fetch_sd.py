#!/usr/bin/env python3
"""Build or check the pinned stable-diffusion.cpp engine (Studio's local pictures and video).

Everything is driven by sd.lock.json at the repository root, as fetch_llama.py is by
llama.lock.json: one commit, its three submodules by SHA, and the Windows pack by size
and SHA-256. Nothing follows "latest".

Usage:
  python3 scripts/fetch_sd.py macos           # macOS: build sd-cli from the pinned commit for
                                              # Apple silicon and for Intel, and join them into a
                                              # universal file in src-tauri/binaries/
  python3 scripts/fetch_sd.py linux           # Linux (development only): build a CPU-only sd-cli
                                              # into .cache/sd-linux/ and print the directory
  python3 scripts/fetch_sd.py windows-check   # Windows, after fetch_llama.py windows: unpack the
                                              # pinned pack as the app installs it and run its
                                              # sd-cli.exe with a bare PATH
  python3 scripts/fetch_sd.py verify-packs    # check the pack URL answers with the pinned size

The Windows engine is not part of the installer: the app downloads the pack with the first
local model. Standard library only, so it runs on any CI runner without setup.
"""

from __future__ import annotations

import json
import ntpath
import os
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from fetch_llama import (  # noqa: E402
    build_macos_sidecars,
    checkout_commit,
    cmake_build_command,
    cmake_configure_command,
    download,
    macos_plan,
    run,
    verify_pack_files,
)

ROOT = Path(__file__).resolve().parent.parent
LOCK = json.loads((ROOT / "sd.lock.json").read_text(encoding="utf-8"))
CACHE = Path(os.environ.get("SD_FETCH_CACHE", ROOT / ".cache" / "sd"))
PIN = "sd_cpp"


def log(msg: str) -> None:
    print(f"[fetch_sd] {msg}", flush=True)


def checkout(git: str, commit: str) -> Path:
    """The pinned commit with its release tag and exactly the lock's submodules, each at
    the lock's SHA.

    The tag is fetched because upstream's CMake names the version with `git describe
    --tags`; without it `sd-cli --version` says "version unknown". Only `ggml`,
    `thirdparty/libwebp` and `thirdparty/libwebm` are fetched: without them CMake has no
    ggml and SD_WEBP=ON stops ("WebP support enabled but no source found"); cutlass and the
    server's web frontend are never needed.
    """
    if commit != LOCK[PIN]["commit"]:
        raise SystemExit(f"the build commit {commit} is not the pinned stable-diffusion.cpp commit")
    src = CACHE / f"stable-diffusion.cpp-{commit[:12]}"
    checkout_commit(src, git, commit)
    tag = f"refs/tags/{LOCK[PIN]['tag']}"
    run(["git", "fetch", "-q", "--depth", "1", "origin", f"{tag}:{tag}"], cwd=src)
    paths = list(LOCK["submodules"])
    run(["git", "submodule", "update", "--init", "--depth", "1", *paths], cwd=src)
    status = subprocess.run(
        ["git", "submodule", "status", "--", *paths], cwd=src, capture_output=True, text=True, check=True
    ).stdout
    check_submodules(status)
    return src


def check_submodules(status: str) -> None:
    """`git submodule status` must show every submodule of the lock checked out at its SHA.

    Each line is a status character (space when checked out at the recorded commit, `-`
    when not checked out, `+` when at another commit), the SHA and the path.
    """
    found = {}
    for line in status.splitlines():
        if not line.strip():
            continue
        flag, rest = line[0], line[1:].split()
        if len(rest) >= 2:
            found[rest[1]] = (flag, rest[0])
    for path, sha in LOCK["submodules"].items():
        flag, got = found.get(path, ("-", ""))
        if flag == "-":
            raise SystemExit(f"submodule {path} is not checked out, expected {sha} (sd.lock.json)")
        if got != sha:
            raise SystemExit(f"submodule {path} is at {got}, expected {sha} (sd.lock.json)")


ASCII_LOWER = str.maketrans("ABCDEFGHIJKLMNOPQRSTUVWXYZ", "abcdefghijklmnopqrstuvwxyz")


def keeps(name: str, patterns: list[str]) -> bool:
    """Whether the app keeps `name` when it unpacks the Windows pack: the rule of
    runtime::packs::glob_match, so ASCII letters match ignoring case and the first `*` is
    the only wildcard (`?`, `[...]` and a second `*` are plain characters)."""
    base = name.rsplit("/", 1)[-1].translate(ASCII_LOWER)
    for pattern in patterns:
        head, star, tail = pattern.translate(ASCII_LOWER).partition("*")
        if not star:
            if head == base:
                return True
        elif len(base) >= len(head) + len(tail) and base.startswith(head) and base.endswith(tail):
            return True
    return False


def macos() -> None:
    plan = macos_plan(LOCK, pin_key=PIN)
    src = checkout(plan["git"], plan["commit"])
    out_dir = ROOT / "src-tauri" / "binaries"
    build_macos_sidecars(plan, src, out_dir)
    log(f"Studio engine ready in {out_dir}")


def linux() -> None:
    spec = LOCK["dev"]["linux-x86_64"]
    src = checkout(spec["git"], spec["commit"])
    build = src / "build-dev-linux"
    run(cmake_configure_command(src, build, spec["cmake_flags"]))
    run(cmake_build_command(build, spec["targets"], os.cpu_count() or 4))
    out = ROOT / ".cache" / "sd-linux"
    out.mkdir(parents=True, exist_ok=True)
    for name in spec["targets"]:
        built = build / "bin" / name
        if not built.exists():
            raise SystemExit(f"build did not produce {built}")
        shutil.copy2(built, out / name)
        os.chmod(out / name, 0o755)
    print(out)


def windows_check() -> None:
    """Install the pinned pack the way the app does (only keep_files, plus the Visual C++
    runtime from llama.cpp's installed folder) and run its sd-cli.exe from there with
    PATH = that folder and %SystemRoot%\\System32 only, so a DLL the pack lacks is found
    nowhere else on the runner."""
    pack = next(p for p in LOCK["packs"] if p["platform"] == "windows-x86_64")
    llama_main = ROOT / "src-tauri" / "resources" / "llama" / "main"
    missing = [n for n in LOCK["vc_runtime_files"] if not (llama_main / n).is_file()]
    if missing:
        raise SystemExit(f"{llama_main} has no {', '.join(missing)}: run fetch_llama.py windows first")
    archives = [download(entry, cache=CACHE) for entry in pack["files"]]
    with tempfile.TemporaryDirectory() as tmp:
        folder = Path(tmp) / pack["dir"]
        folder.mkdir()
        for archive in archives:
            with zipfile.ZipFile(archive) as z:
                for info in z.infolist():
                    name = info.filename.rsplit("/", 1)[-1]
                    if info.is_dir() or not keeps(name, LOCK["keep_files"]):
                        continue
                    with z.open(info) as src, (folder / name).open("wb") as dst:
                        shutil.copyfileobj(src, dst)
        for name in LOCK["vc_runtime_files"]:
            shutil.copy2(llama_main / name, folder / name)
        log(f"{pack['id']}: {len(list(folder.iterdir()))} files in {folder}")
        system_root = os.environ.get("SystemRoot", r"C:\Windows")
        env = {"PATH": f"{folder};{ntpath.join(system_root, 'System32')}", "SystemRoot": system_root}
        engine = folder / "sd-cli.exe"
        for argument in ("--version", "--list-devices"):
            run([str(engine), argument], cwd=folder, env=env)


def verify_packs() -> None:
    verify_pack_files(LOCK)


def main() -> None:
    commands = {"macos": macos, "linux": linux, "windows-check": windows_check, "verify-packs": verify_packs}
    if len(sys.argv) != 2 or sys.argv[1] not in commands:
        print(__doc__)
        raise SystemExit(2)
    commands[sys.argv[1]]()


if __name__ == "__main__":
    main()

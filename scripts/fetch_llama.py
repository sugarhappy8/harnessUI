#!/usr/bin/env python3
"""Fetch or build the pinned llama.cpp runtime before `tauri build`.

Everything is driven by llama.lock.json at the repository root. Nothing follows
"latest": every download is checked against the SHA-256 in the lock file.

Usage:
  python3 scripts/fetch_llama.py windows          # Windows: unpack the Vulkan+CPU build into
                                                  # src-tauri/resources/llama/main/
  python3 scripts/fetch_llama.py macos            # macOS: build llama-server and llama-fit-params
                                                  # from the pinned commit into src-tauri/binaries/
  python3 scripts/fetch_llama.py verify-packs     # check every pack URL answers with the pinned size
  python3 scripts/fetch_llama.py linux            # Linux (development only): unpack the CPU build
                                                  # into .cache/llama-linux/ and print the directory

Standard library only, so it runs on any CI runner without setup.
"""

from __future__ import annotations

import fnmatch
import glob
import hashlib
import json
import os
import shutil
import subprocess
import sys
import urllib.request
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LOCK = json.loads((ROOT / "llama.lock.json").read_text(encoding="utf-8"))
CACHE = Path(os.environ.get("LLAMA_FETCH_CACHE", ROOT / ".cache" / "llama"))
USER_AGENT = "app-build-script/1"


def log(msg: str) -> None:
    print(f"[fetch_llama] {msg}", flush=True)


def sha256_of(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def download(entry: dict) -> Path:
    """Download one lock-file entry into the cache and verify size and SHA-256."""
    CACHE.mkdir(parents=True, exist_ok=True)
    dest = CACHE / entry["name"]
    if dest.exists() and dest.stat().st_size == entry["size"] and sha256_of(dest) == entry["sha256"]:
        log(f"cached: {entry['name']}")
        return dest
    url = entry["url"]
    if not url.startswith("https://github.com/"):
        raise SystemExit(f"refusing non-GitHub URL in lock file: {url}")
    log(f"downloading {url}")
    tmp = dest.with_suffix(dest.suffix + ".part")
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(req, timeout=120) as resp, tmp.open("wb") as out:
        shutil.copyfileobj(resp, out, length=1 << 20)
    size = tmp.stat().st_size
    digest = sha256_of(tmp)
    if size != entry["size"] or digest != entry["sha256"]:
        tmp.unlink(missing_ok=True)
        raise SystemExit(
            f"checksum mismatch for {entry['name']}: got {size} bytes sha256 {digest}, "
            f"expected {entry['size']} bytes sha256 {entry['sha256']}"
        )
    tmp.replace(dest)
    return dest


def keep(name: str, patterns: list[str]) -> bool:
    base = name.rsplit("/", 1)[-1]
    return any(fnmatch.fnmatchcase(base, p) for p in patterns)


def find_vc_runtime() -> Path | None:
    """Locate the Visual C++ redistributable DLLs shipped with Visual Studio."""
    explicit = os.environ.get("VC_RUNTIME_DIR")
    if explicit:
        return Path(explicit)
    vswhere = Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")) / (
        r"Microsoft Visual Studio\Installer\vswhere.exe"
    )
    if not vswhere.exists():
        return None
    out = subprocess.run(
        [str(vswhere), "-latest", "-products", "*", "-property", "installationPath"],
        capture_output=True,
        text=True,
        check=False,
    ).stdout.strip()
    if not out:
        return None
    matches = sorted(
        glob.glob(os.path.join(out, r"VC\Redist\MSVC\*\x64\Microsoft.VC*.CRT\vcruntime140.dll"))
    )
    return Path(matches[-1]).parent if matches else None


def windows() -> None:
    spec = LOCK["installer"]["windows-x86_64"]
    target = ROOT / "src-tauri" / "resources" / "llama" / spec["dir"]
    if target.exists():
        shutil.rmtree(target)
    target.mkdir(parents=True)
    for entry in spec["files"]:
        archive = download(entry)
        with zipfile.ZipFile(archive) as z:
            for info in z.infolist():
                if info.is_dir() or not keep(info.filename, LOCK["keep_files"]):
                    continue
                name = info.filename.rsplit("/", 1)[-1]
                with z.open(info) as src, (target / name).open("wb") as dst:
                    shutil.copyfileobj(src, dst)
    vc = find_vc_runtime()
    if vc is None:
        if sys.platform == "win32":
            raise SystemExit("could not find the Visual C++ runtime DLLs (set VC_RUNTIME_DIR)")
        log("warning: not on Windows, skipping the Visual C++ runtime DLLs")
    else:
        for name in LOCK["vc_runtime_files"]:
            shutil.copy2(vc / name, target / name)
        log(f"copied Visual C++ runtime from {vc}")
    shutil.copy2(ROOT / "third_party" / "licenses" / "llama.cpp-LICENSE", target / "LICENSE-llama.cpp")
    (target / "BUILD").write_text(
        f"llama.cpp {LOCK['llama_cpp']['tag']} ({LOCK['llama_cpp']['commit']})\n", encoding="utf-8"
    )
    required = ["llama-server.exe", "llama-server-impl.dll", "ggml-vulkan.dll", "ggml-base.dll"]
    missing = [r for r in required if not (target / r).exists()]
    if missing:
        raise SystemExit(f"runtime directory is missing {missing}")
    total = sum(p.stat().st_size for p in target.iterdir())
    log(f"windows runtime ready: {target} ({len(list(target.iterdir()))} files, {total / 1e6:.1f} MB)")


def run(cmd: list[str], cwd: Path | None = None) -> None:
    log("$ " + " ".join(cmd))
    subprocess.run(cmd, cwd=cwd, check=True)


def macos() -> None:
    spec = LOCK["installer"]["macos-aarch64"]["build_from_source"]
    src = CACHE / f"llama.cpp-{spec['commit'][:12]}"
    build = src / "build-app"
    if not (src / ".git").exists():
        src.mkdir(parents=True, exist_ok=True)
        run(["git", "init", "-q"], cwd=src)
        run(["git", "remote", "add", "origin", spec["git"]], cwd=src)
    run(["git", "fetch", "-q", "--depth", "1", "origin", spec["commit"]], cwd=src)
    run(["git", "checkout", "-q", "--force", spec["commit"]], cwd=src)
    head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=src, capture_output=True, text=True, check=True).stdout.strip()
    if head != spec["commit"]:
        raise SystemExit(f"checked out {head}, expected {spec['commit']}")
    run(["cmake", "-S", str(src), "-B", str(build), *spec["cmake_flags"]])
    jobs = str(os.cpu_count() or 4)
    run(["cmake", "--build", str(build), "--config", "Release", "-j", jobs, "--target", *spec["targets"]])
    out_dir = ROOT / "src-tauri" / "binaries"
    out_dir.mkdir(parents=True, exist_ok=True)
    triple = "aarch64-apple-darwin"
    for name in spec["targets"]:
        built = build / "bin" / name
        if not built.exists():
            raise SystemExit(f"build did not produce {built}")
        dest = out_dir / f"{name}-{triple}"
        shutil.copy2(built, dest)
        os.chmod(dest, 0o755)
        check_macos_binary(dest)
    log(f"macOS runtime ready in {out_dir}")


def check_macos_binary(path: Path) -> None:
    """The sidecar must link only system libraries and target macOS 13.3."""
    libs = subprocess.run(["otool", "-L", str(path)], capture_output=True, text=True, check=True).stdout
    for line in libs.splitlines()[1:]:
        lib = line.strip().split(" ")[0]
        if lib and not (lib.startswith("/usr/lib/") or lib.startswith("/System/")):
            raise SystemExit(f"{path.name} links a non-system library: {lib}")
    load = subprocess.run(["otool", "-l", str(path)], capture_output=True, text=True, check=True).stdout
    minos = [l.split()[-1] for l in load.splitlines() if l.strip().startswith("minos")]
    if not minos or minos[0] not in ("13.3", "13.3.0"):
        raise SystemExit(f"{path.name} has minimum OS {minos}, expected 13.3")
    run([str(path), "--version"])


def linux() -> None:
    import tarfile

    entry = LOCK["dev"]["linux-x86_64"]
    archive = download(entry)
    out = ROOT / ".cache" / "llama-linux"
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    with tarfile.open(archive) as tar:
        for member in tar.getmembers():
            name = member.name.split("/", 1)[-1]
            if not name or ".." in Path(name).parts or member.isdev():
                continue
            member.name = name
            if member.issym() or member.islnk():
                if Path(member.linkname).is_absolute() or ".." in Path(member.linkname).parts:
                    continue
            tar.extract(member, out, filter="data")
    print(out)


def verify_packs() -> None:
    for pack in LOCK["packs"]:
        for entry in pack["files"]:
            req = urllib.request.Request(entry["url"], method="HEAD", headers={"User-Agent": USER_AGENT})
            with urllib.request.urlopen(req, timeout=60) as resp:
                size = int(resp.headers.get("Content-Length", "0"))
            status = "ok" if size == entry["size"] else f"SIZE MISMATCH ({size} != {entry['size']})"
            log(f"{pack['id']}: {entry['name']}: {status}")
            if size != entry["size"]:
                raise SystemExit(1)


def main() -> None:
    commands = {"windows": windows, "macos": macos, "linux": linux, "verify-packs": verify_packs}
    if len(sys.argv) != 2 or sys.argv[1] not in commands:
        print(__doc__)
        raise SystemExit(2)
    commands[sys.argv[1]]()


if __name__ == "__main__":
    main()

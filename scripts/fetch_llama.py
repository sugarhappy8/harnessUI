#!/usr/bin/env python3
"""Fetch or build the pinned llama.cpp runtime before `tauri build`.

Everything is driven by llama.lock.json at the repository root. Nothing follows
"latest": every download is checked against the SHA-256 in the lock file.

Usage:
  python3 scripts/fetch_llama.py windows          # Windows: unpack the Vulkan+CPU build into
                                                  # src-tauri/resources/llama/main/
  python3 scripts/fetch_llama.py macos            # macOS: build llama-server and llama-fit-params
                                                  # from the pinned commit for Apple silicon and
                                                  # for Intel, and join them into universal files
                                                  # in src-tauri/binaries/
  python3 scripts/fetch_llama.py check-macos-app <App.app>
                                                  # macOS: every program in a built app bundle has
                                                  # both processor parts and links system libraries only
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
import platform
import shutil
import subprocess
import sys
import urllib.request
import zipfile
from pathlib import Path
from typing import Sequence

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


# Where pinned files may come from. Everything is on GitHub except the Vulkan runtime,
# which its publisher (LunarG, for the Khronos Group) serves from its own site.
GITHUB = "https://github.com/"
LUNARG = "https://sdk.lunarg.com/"


def download(entry: dict, allowed: tuple[str, ...] = (GITHUB,)) -> Path:
    """Download one lock-file entry into the cache and verify size and SHA-256."""
    CACHE.mkdir(parents=True, exist_ok=True)
    dest = CACHE / entry["name"]
    if dest.exists() and dest.stat().st_size == entry["size"] and sha256_of(dest) == entry["sha256"]:
        log(f"cached: {entry['name']}")
        return dest
    url = entry["url"]
    if not url.startswith(allowed):
        raise SystemExit(f"refusing a URL outside {allowed} in the lock file: {url}")
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


def install_vulkan_runtime(target: Path) -> Path:
    """Put the pinned Vulkan runtime (vulkan-1.dll) into <target>/vulkan/.

    A Windows graphics driver installed without its maker's full package can lack the
    Vulkan runtime; llama-server then loads no Vulkan backend and lists no GPU, silently.
    The app adds this folder to the END of llama-server's PATH, where Windows looks last,
    so a runtime that came with the driver is always preferred (see
    crates/runtime/src/llama/backend.rs).
    """
    spec = LOCK["vulkan_runtime"]
    archive = download(spec, allowed=(LUNARG,))
    folder = target / spec["dir"]
    folder.mkdir(parents=True, exist_ok=True)
    out = folder / "vulkan-1.dll"
    with zipfile.ZipFile(archive) as z:
        data = z.read(spec["member"])
    digest = hashlib.sha256(data).hexdigest()
    if digest != spec["member_sha256"]:
        raise SystemExit(
            f"{spec['member']} has sha256 {digest}, expected {spec['member_sha256']}"
        )
    out.write_bytes(data)
    shutil.copy2(ROOT / "third_party" / "licenses" / "VulkanRT-License.txt", folder / "LICENSE-VulkanRT.txt")
    log(f"vulkan runtime {spec['version']} ready: {out} ({len(data) / 1e6:.1f} MB)")
    return out


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
    install_vulkan_runtime(target)
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


# macOS: one build per processor, joined into universal files (ARCHITECTURE.md 7.1).
# Each row: the lock-file platform key, the processor's name for clang, lipo and otool,
# and the Rust target triple.
MACOS_SLICES = (
    ("macos-aarch64", "arm64", "aarch64-apple-darwin"),
    ("macos-x86_64", "x86_64", "x86_64-apple-darwin"),
)
MACOS_ARCHS = tuple(arch for _, arch, _ in MACOS_SLICES)
# `tauri build --target universal-apple-darwin` bundles `<name>-universal-apple-darwin`.
MACOS_UNIVERSAL_TRIPLE = "universal-apple-darwin"
MACOS_MIN_OS = "13.3"


def sidecar_name(program: str, triple: str) -> str:
    """The file name Tauri looks for in src-tauri/binaries/ for an externalBin entry.

    The universal build needs three files per program: one per processor, because the app
    is compiled once for each and its build script copies that processor's file, and the
    universal one, which is what goes into the app bundle.
    """
    return f"{program}-{triple}"


def macos_plan(lock: dict) -> dict:
    """What to build on macOS, from the lock file's two macOS entries.

    Both must build the same programs from the pinned commit, as static binaries for
    macOS 13.3, each for its own processor.
    """
    slices = []
    for platform_key, arch, triple in MACOS_SLICES:
        spec = (lock["installer"].get(platform_key) or {}).get("build_from_source")
        if not spec:
            raise SystemExit(f"llama.lock.json has no build_from_source for {platform_key}")
        flags = list(spec["cmake_flags"])
        arch_flags = [f for f in flags if f.startswith("-DCMAKE_OSX_ARCHITECTURES=")]
        if arch_flags != [f"-DCMAKE_OSX_ARCHITECTURES={arch}"]:
            raise SystemExit(f"{platform_key}: cmake_flags must set CMAKE_OSX_ARCHITECTURES={arch}, found {arch_flags}")
        for required in (f"-DCMAKE_OSX_DEPLOYMENT_TARGET={MACOS_MIN_OS}", "-DBUILD_SHARED_LIBS=OFF", "-DGGML_NATIVE=OFF"):
            if required not in flags:
                raise SystemExit(f"{platform_key}: cmake_flags must contain {required}")
        slices.append(
            {"platform": platform_key, "arch": arch, "triple": triple, "cmake_flags": flags,
             "git": spec["git"], "commit": spec["commit"], "targets": list(spec["targets"])}
        )
    first = slices[0]
    for other in slices[1:]:
        for key in ("git", "commit", "targets"):
            if other[key] != first[key]:
                raise SystemExit(f"{other['platform']} and {first['platform']} differ in {key}")
    if first["commit"] != lock["llama_cpp"]["commit"]:
        raise SystemExit("the macOS build commit is not the pinned llama.cpp commit")
    if not first["targets"]:
        raise SystemExit("the macOS build has no targets")
    return {"git": first["git"], "commit": first["commit"], "targets": first["targets"], "slices": slices}


def cmake_configure_command(src: Path, build: Path, flags: Sequence[str]) -> list[str]:
    return ["cmake", "-S", str(src), "-B", str(build), *flags]


def cmake_build_command(build: Path, targets: Sequence[str], jobs: int) -> list[str]:
    return ["cmake", "--build", str(build), "--config", "Release", "-j", str(jobs), "--target", *targets]


def lipo_create_command(parts: Sequence[Path], output: Path) -> list[str]:
    return ["lipo", "-create", "-output", str(output), *(str(p) for p in parts)]


def linked_libraries(otool_libraries: str) -> list[str]:
    """Library paths from `otool -L` output. Header lines (the file name, with or without
    an architecture) start at the first column; library lines are indented."""
    libs = []
    for line in otool_libraries.splitlines():
        if line[:1] in (" ", "\t") and line.strip():
            libs.append(line.split()[0])
    return libs


def non_system_libraries(otool_libraries: str) -> list[str]:
    """Linked libraries that do not come with macOS (Homebrew, @rpath, a build folder)."""
    return [lib for lib in linked_libraries(otool_libraries) if not lib.startswith(("/usr/lib/", "/System/"))]


def minimum_os_versions(otool_load_commands: str) -> list[str]:
    """Minimum macOS versions named in `otool -l` output (one per LC_BUILD_VERSION or
    LC_VERSION_MIN_MACOSX load command)."""
    versions = []
    command = None
    for line in otool_load_commands.splitlines():
        parts = line.split()
        if len(parts) == 2 and parts[0] == "cmd":
            command = parts[1]
        elif len(parts) == 2 and parts[0] == "minos" and command == "LC_BUILD_VERSION":
            versions.append(parts[1])
        elif len(parts) == 2 and parts[0] == "version" and command == "LC_VERSION_MIN_MACOSX":
            versions.append(parts[1])
    return versions


def version_command(path: Path, arch: str, host_arch: str) -> tuple[list[str], bool] | None:
    """How to run one processor's part of `path` on this Mac: the command, and whether it
    runs natively. An Apple silicon Mac can try the Intel part through Rosetta; an Intel
    Mac cannot run the Apple silicon part at all (None)."""
    if arch == host_arch:
        return [str(path), "--version"], True
    if host_arch == "arm64" and arch == "x86_64":
        return ["arch", "-x86_64", str(path), "--version"], False
    return None


def tool_output(cmd: list[str]) -> str:
    done = subprocess.run(cmd, capture_output=True, text=True, check=False)
    if done.returncode != 0:
        raise SystemExit(f"{' '.join(cmd)} failed with exit code {done.returncode}: {done.stderr.strip()}")
    return done.stdout


def macos_archs(path: Path) -> list[str]:
    return tool_output(["lipo", "-archs", str(path)]).split()


def check_macos_linkage(path: Path, archs: Sequence[str]) -> dict[str, list[str]]:
    """`path` must contain exactly `archs`, and each part must link only system libraries.
    Returns each part's minimum macOS versions."""
    found = macos_archs(path)
    if sorted(found) != sorted(archs):
        raise SystemExit(f"{path.name} is built for {found}, expected {list(archs)}")
    minimum = {}
    for arch in archs:
        bad = non_system_libraries(tool_output(["otool", "-arch", arch, "-L", str(path)]))
        if bad:
            raise SystemExit(f"{path.name} ({arch}) links a non-system library: {', '.join(bad)}")
        minimum[arch] = minimum_os_versions(tool_output(["otool", "-arch", arch, "-l", str(path)]))
    return minimum


def check_macos_binary(path: Path, archs: Sequence[str]) -> None:
    """A sidecar must contain exactly `archs`; each part must link only system libraries
    and target macOS 13.3; and the part for this Mac's processor must run."""
    for arch, minos in check_macos_linkage(path, archs).items():
        if not minos or any(v not in (MACOS_MIN_OS, MACOS_MIN_OS + ".0") for v in minos):
            raise SystemExit(f"{path.name} ({arch}) has minimum OS {minos}, expected {MACOS_MIN_OS}")
    host = platform.machine()
    for arch in archs:
        command = version_command(path, arch, host)
        if command is None:
            log(f"not run here: the {arch} part of {path.name} (this Mac is {host})")
            continue
        cmd, native = command
        if native:
            run(cmd)
            continue
        # Rosetta may be missing, and before macOS 15 it has no AVX. The Intel Mac job in
        # CI runs this part on a real Intel processor.
        log("$ " + " ".join(cmd))
        code = subprocess.run(cmd, check=False).returncode
        if code != 0:
            log(f"warning: the {arch} part of {path.name} did not run through Rosetta (exit {code})")


def macos() -> None:
    plan = macos_plan(LOCK)
    src = CACHE / f"llama.cpp-{plan['commit'][:12]}"
    if not (src / ".git").exists():
        src.mkdir(parents=True, exist_ok=True)
        run(["git", "init", "-q"], cwd=src)
        run(["git", "remote", "add", "origin", plan["git"]], cwd=src)
    run(["git", "fetch", "-q", "--depth", "1", "origin", plan["commit"]], cwd=src)
    run(["git", "checkout", "-q", "--force", plan["commit"]], cwd=src)
    head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=src, capture_output=True, text=True, check=True).stdout.strip()
    if head != plan["commit"]:
        raise SystemExit(f"checked out {head}, expected {plan['commit']}")
    jobs = os.cpu_count() or 4
    out_dir = ROOT / "src-tauri" / "binaries"
    out_dir.mkdir(parents=True, exist_ok=True)
    parts: dict[str, list[Path]] = {name: [] for name in plan["targets"]}
    for piece in plan["slices"]:
        build = src / f"build-app-{piece['arch']}"
        run(cmake_configure_command(src, build, piece["cmake_flags"]))
        run(cmake_build_command(build, plan["targets"], jobs))
        for name in plan["targets"]:
            built = build / "bin" / name
            if not built.exists():
                raise SystemExit(f"build did not produce {built}")
            dest = out_dir / sidecar_name(name, piece["triple"])
            shutil.copy2(built, dest)
            os.chmod(dest, 0o755)
            check_macos_binary(dest, [piece["arch"]])
            parts[name].append(dest)
    for name, files in parts.items():
        dest = out_dir / sidecar_name(name, MACOS_UNIVERSAL_TRIPLE)
        dest.unlink(missing_ok=True)
        run(lipo_create_command(files, dest))
        os.chmod(dest, 0o755)
        check_macos_binary(dest, MACOS_ARCHS)
    log(f"macOS runtime ready in {out_dir}")


def check_macos_app(app: Path) -> None:
    """Every program in a built universal app bundle (the app itself and the llama.cpp
    sidecars) must have both processor parts and link only system libraries: libgit2's
    OpenSSL must be built in, never loaded from Homebrew (ARCHITECTURE.md 9.3)."""
    programs = sorted(p for p in (app / "Contents" / "MacOS").iterdir() if p.is_file())
    names = [p.name for p in programs]
    for sidecar in LOCK["installer"][MACOS_SLICES[0][0]]["build_from_source"]["targets"]:
        if sidecar not in names:
            raise SystemExit(f"{app.name} does not contain {sidecar}")
    if len(programs) < 3:
        raise SystemExit(f"{app.name} should contain the app and its sidecars, found {names}")
    for program in programs:
        minimum = check_macos_linkage(program, MACOS_ARCHS)
        log(f"{program.name}: {' '.join(MACOS_ARCHS)}, system libraries only, minimum macOS {minimum}")


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
    if len(sys.argv) == 3 and sys.argv[1] == "check-macos-app":
        check_macos_app(Path(sys.argv[2]))
        return
    if len(sys.argv) != 2 or sys.argv[1] not in commands:
        print(__doc__)
        raise SystemExit(2)
    commands[sys.argv[1]]()


if __name__ == "__main__":
    main()

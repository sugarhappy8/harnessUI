#!/usr/bin/env python3
"""Tests for scripts/fetch_sd.py that run on any computer.

The Mac build needs a Mac and the Windows check needs Windows. These tests check what
does not: how sd.lock.json turns into two Mac builds, which submodules a checkout
fetches, and dry runs of the `macos`, `linux` and `windows-check` commands with
stand-ins for git, cmake, lipo, otool and the engine itself.

    python3 scripts/fetch_sd_test.py

Standard library only.
"""

import copy
import os
import stat
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import fetch_llama  # noqa: E402
import fetch_llama_test  # noqa: E402
import fetch_sd  # noqa: E402

LISTING = Path(__file__).resolve().parent.parent / "crates" / "runtime" / "tests" / "fixtures" / "sd_win_pack_listing.txt"

# Like fetch_llama_test's git, and it also answers `git submodule status` with the lines
# in $FAKE_SUBMODULES (one "<status><sha> <path>" a line, `|` between lines).
FAKE_GIT = """#!/bin/sh
echo "git $*" >> "$FAKE_LOG"
case "$1" in
  init) mkdir -p .git ;;
  rev-parse) echo "$FAKE_COMMIT" ;;
  submodule)
    if [ "$2" = "status" ]; then printf '%s\\n' "$FAKE_SUBMODULES" | tr '|' '\\n'; fi ;;
esac
exit 0
"""
# Configure remembers the processor (none on Linux); build writes one small program per
# target that names its processor in a comment the lipo stand-in reads.
FAKE_CMAKE = """#!/bin/sh
echo "cmake $*" >> "$FAKE_LOG"
if [ "$1" = "-S" ]; then
  build="$4"
  mkdir -p "$build/bin"
  echo "host" > "$build/arch"
  for a in "$@"; do
    case "$a" in -DCMAKE_OSX_ARCHITECTURES=*) echo "${a#*=}" > "$build/arch" ;; esac
  done
elif [ "$1" = "--build" ]; then
  build="$2"
  arch=$(cat "$build/arch")
  seen=0
  for a in "$@"; do
    if [ "$seen" = 1 ]; then
      printf '#!/bin/sh\\n# archs: %s\\necho "stable-diffusion.cpp stand-in (%s)"\\n' "$arch" "$arch" > "$build/bin/$a"
      chmod 644 "$build/bin/$a"
    fi
    if [ "$a" = "--target" ]; then seen=1; fi
  done
fi
exit 0
"""


def submodule_status(lock: dict, **changed: str) -> str:
    """`git submodule status` lines for the lock's submodules, some SHAs changed."""
    lines = []
    for path, sha in lock["submodules"].items():
        lines.append(f" {changed.get(path, sha)} {path} (heads/master)")
    return "|".join(lines)


class Plan(unittest.TestCase):
    def test_the_plan_builds_sd_cli_for_both_processors(self):
        plan = fetch_llama.macos_plan(fetch_sd.LOCK, pin_key="sd_cpp")
        self.assertEqual(plan["targets"], ["sd-cli"])
        self.assertEqual([s["arch"] for s in plan["slices"]], ["arm64", "x86_64"])
        self.assertEqual([s["triple"] for s in plan["slices"]], ["aarch64-apple-darwin", "x86_64-apple-darwin"])
        self.assertEqual(plan["commit"], fetch_sd.LOCK["sd_cpp"]["commit"])
        self.assertEqual(plan["git"], fetch_sd.LOCK["sd_cpp"]["repository"])

    def test_a_slice_on_another_commit_stops_the_build(self):
        lock = copy.deepcopy(fetch_sd.LOCK)
        lock["installer"]["macos-x86_64"]["build_from_source"]["commit"] = "0" * 40
        with self.assertRaises(SystemExit):
            fetch_llama.macos_plan(lock, pin_key="sd_cpp")
        both = copy.deepcopy(fetch_sd.LOCK)
        for key in ("macos-aarch64", "macos-x86_64"):
            both["installer"][key]["build_from_source"]["commit"] = "1" * 40
        with self.assertRaises(SystemExit) as raised:
            fetch_llama.macos_plan(both, pin_key="sd_cpp")
        self.assertIn("pinned", str(raised.exception))

    def test_the_flags_keep_the_app_rules(self):
        plan = fetch_llama.macos_plan(fetch_sd.LOCK, pin_key="sd_cpp")
        arm, intel = (s["cmake_flags"] for s in plan["slices"])
        for flags in (arm, intel):
            for flag in ("-DBUILD_SHARED_LIBS=OFF", "-DSD_BUILD_SHARED_LIBS=OFF", "-DGGML_NATIVE=OFF",
                         "-DSD_METAL=ON", "-DGGML_METAL=ON", "-DGGML_METAL_EMBED_LIBRARY=ON",
                         "-DSD_WEBP=ON", "-DSD_WEBM=ON", "-DSD_SERVER_BUILD_FRONTEND=OFF",
                         "-DCMAKE_OSX_DEPLOYMENT_TARGET=13.3", "-DCMAKE_BUILD_TYPE=Release"):
                self.assertIn(flag, flags)
        self.assertIn("-DCMAKE_OSX_ARCHITECTURES=arm64", arm)
        self.assertIn("-DCMAKE_OSX_ARCHITECTURES=x86_64", intel)
        # llama.cpp's Intel instruction sets, named one by one, so both engines run on the
        # same Intel Macs; nothing newer.
        llama_intel = fetch_llama.LOCK["installer"]["macos-x86_64"]["build_from_source"]["cmake_flags"]
        x86 = ("-DGGML_SSE42=ON", "-DGGML_AVX=ON", "-DGGML_AVX2=ON", "-DGGML_FMA=ON", "-DGGML_F16C=ON",
               "-DGGML_BMI2=ON", "-DGGML_AVX512=OFF")
        for flag in x86:
            self.assertIn(flag, intel)
            self.assertIn(flag, llama_intel)
            self.assertNotIn(flag, arm)
        # The two builds differ only in the processor and its instruction sets.
        common = lambda flags: sorted(f for f in flags if f not in x86 and not f.startswith("-DCMAKE_OSX_ARCHITECTURES="))  # noqa: E731
        self.assertEqual(common(arm), common(intel))

    def test_the_dev_build_is_the_pin_on_the_processor_only(self):
        dev = fetch_sd.LOCK["dev"]["linux-x86_64"]
        self.assertEqual((dev["git"], dev["commit"]), (fetch_sd.LOCK["sd_cpp"]["repository"], fetch_sd.LOCK["sd_cpp"]["commit"]))
        self.assertEqual(dev["targets"], ["sd-cli"])
        self.assertFalse([f for f in dev["cmake_flags"] if "METAL" in f or "VULKAN" in f or "CUDA" in f])

    def test_the_lock_names_exactly_the_three_submodules(self):
        self.assertEqual(list(fetch_sd.LOCK["submodules"]), ["ggml", "thirdparty/libwebp", "thirdparty/libwebm"])
        for sha in fetch_sd.LOCK["submodules"].values():
            self.assertRegex(sha, r"^[0-9a-f]{40}$")


class Keep(unittest.TestCase):
    """windows-check keeps the files the app keeps when it unpacks the pack
    (runtime::packs::glob_match): names match ignoring case, and one `*` is the only
    wildcard."""

    def test_names_match_the_lock_ignoring_case(self):
        patterns = fetch_sd.LOCK["keep_files"]
        for name in ("sd-cli.exe", "SD-CLI.EXE", "Stable-Diffusion.DLL", "GGML-CPU-HASWELL.DLL", "ggml.dll",
                     "LibWebPMux.dll", "LIBSHARPYUV.DLL", "Webm.dll", "GGML.TXT", "pack/stable-diffusion.cpp.txt"):
            self.assertTrue(fetch_sd.keeps(name, patterns), name)
        for name in ("sd-server.exe", "SD-SERVER.EXE", "start-server.bat", "start-server.sh", "llama-server.exe",
                     "ggml.dll.bak", "my-ggml.dll"):
            self.assertFalse(fetch_sd.keeps(name, patterns), name)

    def test_star_is_the_only_wildcard(self):
        self.assertTrue(fetch_sd.keeps("ggml.dll", ["ggml*.dll"]))
        self.assertTrue(fetch_sd.keeps("ggml-cpu-x64.dll", ["ggml*.dll"]))
        self.assertFalse(fetch_sd.keeps("ggml.dl", ["ggml*.dll"]))
        # `?` and `[...]` are plain characters, as in the app
        self.assertFalse(fetch_sd.keeps("ggml1.dll", ["ggml?.dll"]))
        self.assertTrue(fetch_sd.keeps("GGML?.DLL", ["ggml?.dll"]))
        self.assertFalse(fetch_sd.keeps("ggml1.dll", ["ggml[0-9].dll"]))
        self.assertTrue(fetch_sd.keeps("ggml[0-9].dll", ["GGML[0-9].DLL"]))
        # only the first `*` is a wildcard; a second one is a plain character
        self.assertTrue(fetch_sd.keeps("axb*c", ["a*b*c"]))
        self.assertFalse(fetch_sd.keeps("axbyc", ["a*b*c"]))


@unittest.skipIf(os.name == "nt", "the stand-in tools are shell scripts")
class DryRun(unittest.TestCase):
    """Runs the real `macos`, `linux` and `windows-check` code with stand-ins."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        tools = self.root / "tools"
        tools.mkdir()
        stand_ins = (("git", FAKE_GIT), ("cmake", FAKE_CMAKE), ("lipo", fetch_llama_test.FAKE_LIPO),
                     ("otool", fetch_llama_test.FAKE_OTOOL), ("arch", fetch_llama_test.FAKE_ARCH))
        for name, text in stand_ins:
            tool = tools / name
            tool.write_text(text, encoding="utf-8")
            tool.chmod(tool.stat().st_mode | stat.S_IXUSR)
        self.log = self.root / "commands.log"
        self.saved = (dict(os.environ), fetch_sd.ROOT, fetch_sd.CACHE, fetch_sd.run, fetch_sd.download,
                      fetch_llama.platform.machine)
        os.environ["PATH"] = f"{tools}{os.pathsep}{os.environ['PATH']}"
        os.environ["FAKE_LOG"] = str(self.log)
        os.environ["FAKE_COMMIT"] = fetch_sd.LOCK["sd_cpp"]["commit"]
        os.environ["FAKE_SUBMODULES"] = submodule_status(fetch_sd.LOCK)
        os.environ.pop("SystemRoot", None)
        fetch_sd.ROOT = self.root
        fetch_sd.CACHE = self.root / "cache" / "sd"
        fetch_llama.platform.machine = lambda: "x86_64"

    def tearDown(self):
        env, fetch_sd.ROOT, fetch_sd.CACHE, fetch_sd.run, fetch_sd.download, fetch_llama.platform.machine = self.saved
        os.environ.clear()
        os.environ.update(env)
        self.tmp.cleanup()

    def commands(self):
        return self.log.read_text(encoding="utf-8").splitlines()

    def test_macos_joins_both_parts_into_the_universal_sd_cli(self):
        fetch_sd.macos()
        out = self.root / "src-tauri" / "binaries"
        self.assertEqual(
            sorted(p.name for p in out.iterdir()),
            ["sd-cli-aarch64-apple-darwin", "sd-cli-universal-apple-darwin", "sd-cli-x86_64-apple-darwin"],
        )
        self.assertEqual(fetch_llama.macos_archs(out / "sd-cli-aarch64-apple-darwin"), ["arm64"])
        self.assertEqual(fetch_llama.macos_archs(out / "sd-cli-x86_64-apple-darwin"), ["x86_64"])
        self.assertEqual(fetch_llama.macos_archs(out / "sd-cli-universal-apple-darwin"), ["arm64", "x86_64"])
        self.assertTrue(os.access(out / "sd-cli-universal-apple-darwin", os.X_OK))
        commands = self.commands()
        configures = [c for c in commands if c.startswith("cmake -S")]
        self.assertEqual(len(configures), 2)
        self.assertIn("-DCMAKE_OSX_ARCHITECTURES=arm64", configures[0])
        self.assertIn("-DCMAKE_OSX_ARCHITECTURES=x86_64", configures[1])
        self.assertTrue(all(c.endswith("--target sd-cli") for c in commands if c.startswith("cmake --build")))
        joins = [c for c in commands if c.startswith("lipo -create")]
        self.assertEqual(
            joins,
            [f"lipo -create -output {out / 'sd-cli-universal-apple-darwin'} "
             f"{out / 'sd-cli-aarch64-apple-darwin'} {out / 'sd-cli-x86_64-apple-darwin'}"],
        )
        # Each part and both parts of the universal file had their libraries and minimum
        # macOS read (-L and -l), as for llama.cpp's sidecars.
        self.assertEqual(len([c for c in commands if c.startswith("otool -arch")]), 8)

    def test_the_mac_build_fetches_its_three_submodules(self):
        fetch_sd.macos()
        commands = self.commands()
        checkout = commands.index(f"git checkout -q --force {fetch_sd.LOCK['sd_cpp']['commit']}")
        update = commands.index("git submodule update --init --depth 1 ggml thirdparty/libwebp thirdparty/libwebm")
        self.assertLess(checkout, update)
        self.assertLess(update, next(i for i, c in enumerate(commands) if c.startswith("cmake -S")))
        self.assertFalse([c for c in commands if c.startswith("git") and ("cutlass" in c or "frontend" in c)])
        self.assertFalse([c for c in commands if c.startswith("git submodule update") and c != commands[update]])

    def test_a_submodule_on_another_commit_stops_the_build(self):
        os.environ["FAKE_SUBMODULES"] = submodule_status(fetch_sd.LOCK, ggml="f" * 40)
        with self.assertRaises(SystemExit) as raised:
            fetch_sd.macos()
        self.assertIn("ggml", str(raised.exception))
        self.assertIn("f" * 40, str(raised.exception))
        self.assertFalse([c for c in self.commands() if c.startswith("cmake")])

    def test_a_missing_submodule_stops_the_build(self):
        lock = fetch_sd.LOCK
        os.environ["FAKE_SUBMODULES"] = "|".join(
            f"{'-' if path == 'thirdparty/libwebm' else ' '}{sha} {path}" for path, sha in lock["submodules"].items()
        )
        with self.assertRaises(SystemExit) as raised:
            fetch_sd.linux()
        self.assertIn("submodule thirdparty/libwebm is not checked out", str(raised.exception))

    def test_the_checkout_fetches_the_pinned_tag(self):
        # upstream's CMake names the version with `git describe --tags`: without the tag in
        # the shallow checkout the built sd-cli says "version unknown"
        fetch_sd.linux()
        commands = self.commands()
        tag = fetch_sd.LOCK["sd_cpp"]["tag"]
        fetch_tag = commands.index(f"git fetch -q --depth 1 origin refs/tags/{tag}:refs/tags/{tag}")
        self.assertLess(commands.index(f"git checkout -q --force {fetch_sd.LOCK['sd_cpp']['commit']}"), fetch_tag)
        self.assertLess(fetch_tag, next(i for i, c in enumerate(commands) if c.startswith("cmake -S")))

    def test_the_checkout_must_be_the_pinned_commit(self):
        os.environ["FAKE_COMMIT"] = "e" * 40
        with self.assertRaises(SystemExit) as raised:
            fetch_sd.linux()
        self.assertIn("checked out", str(raised.exception))

    def test_linux_builds_the_dev_engine_from_the_pin(self):
        fetch_sd.linux()
        engine = self.root / ".cache" / "sd-linux" / "sd-cli"
        self.assertTrue(engine.is_file())
        self.assertEqual(stat.S_IMODE(engine.stat().st_mode), 0o755)
        commands = self.commands()
        configures = [c for c in commands if c.startswith("cmake -S")]
        self.assertEqual(len(configures), 1)
        self.assertTrue(configures[0].endswith(" ".join(fetch_sd.LOCK["dev"]["linux-x86_64"]["cmake_flags"])))
        self.assertNotIn("METAL", configures[0])
        self.assertTrue([c for c in commands if c.startswith("cmake --build") and c.endswith("--target sd-cli")])
        self.assertIn("git submodule update --init --depth 1 ggml thirdparty/libwebp thirdparty/libwebm", commands)

    def windows_pack(self, names=None):
        """A stand-in for the pinned zip: the listing's 23 names (or `names`), each holding
        its own name."""
        archive = self.root / "pack.zip"
        names = names or LISTING.read_text(encoding="utf-8").split()
        with zipfile.ZipFile(archive, "w") as z:
            for name in names:
                z.writestr(name, name)
        return archive, names

    def run_windows_check(self, archive):
        """Runs the real windows_check on `archive` with llama's runtime folder in place;
        returns each recorded run with the files beside the engine at that moment."""
        llama_main = self.root / "src-tauri" / "resources" / "llama" / "main"
        llama_main.mkdir(parents=True)
        for name in fetch_llama.LOCK["vc_runtime_files"]:
            (llama_main / name).write_text(name, encoding="utf-8")
        fetch_sd.download = lambda entry, allowed=(fetch_llama.GITHUB,), cache=None: archive
        runs = []

        def record(cmd, cwd=None, env=None):
            folder = Path(cmd[0]).parent
            runs.append((cmd, cwd, env, sorted(p.name for p in folder.iterdir())))

        fetch_sd.run = record
        fetch_sd.windows_check()
        return runs

    def test_windows_check_runs_the_kept_files_with_a_bare_path(self):
        archive, names = self.windows_pack()
        self.assertEqual(len(names), 23)
        runs = self.run_windows_check(archive)
        self.assertEqual([r[0][1:] for r in runs], [["--version"], ["--list-devices"]])
        cmd, cwd, env, files = runs[0]
        folder = Path(cmd[0]).parent
        self.assertEqual(Path(cmd[0]).name, "sd-cli.exe")
        self.assertEqual(cwd, folder)
        self.assertEqual(env["PATH"], f"{folder};C:\\Windows\\System32")
        self.assertEqual(set(env), {"PATH", "SystemRoot"})
        kept = sorted(n for n in names if fetch_sd.keeps(n, fetch_sd.LOCK["keep_files"]))
        self.assertEqual(files, sorted(kept + fetch_sd.LOCK["vc_runtime_files"]))
        for dropped in ("sd-server.exe", "start-server.bat", "start-server.sh"):
            self.assertNotIn(dropped, files)
        self.assertEqual(len(kept), 20)
        self.assertFalse(folder.exists(), "the check works in a temporary folder")

    def test_windows_check_keeps_a_file_whatever_its_case(self):
        # the app keeps GGML-VULKAN.DLL for the pattern ggml*.dll, so the check must too
        listing = LISTING.read_text(encoding="utf-8").split()
        names = [n.upper() if n.endswith(".dll") else n for n in listing]
        archive, _ = self.windows_pack(names)
        files = self.run_windows_check(archive)[0][3]
        for name in ("GGML-VULKAN.DLL", "GGML-BASE.DLL", "STABLE-DIFFUSION.DLL", "LIBSHARPYUV.DLL", "WEBM.DLL"):
            self.assertIn(name, files)
        self.assertNotIn("sd-server.exe", files)
        self.assertEqual(len(files), 20 + len(fetch_sd.LOCK["vc_runtime_files"]))

    def test_windows_check_needs_llamas_runtime_folder(self):
        archive, _ = self.windows_pack()
        fetch_sd.download = lambda entry, allowed=(fetch_llama.GITHUB,), cache=None: archive
        fetch_sd.run = lambda cmd, cwd=None, env=None: self.fail("nothing runs without the Visual C++ runtime")
        with self.assertRaises(SystemExit) as raised:
            fetch_sd.windows_check()
        self.assertIn("fetch_llama.py windows", str(raised.exception))


if __name__ == "__main__":
    unittest.main()

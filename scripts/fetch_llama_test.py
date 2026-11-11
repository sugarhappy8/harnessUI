#!/usr/bin/env python3
"""Tests for scripts/fetch_llama.py that run on any computer.

The macOS build itself needs a Mac. These tests check what does not: how the lock file
turns into two builds, the file names Tauri expects, how `otool` output is read, and a
dry run of the whole macOS command with stand-ins for git, cmake, lipo and otool.

    python3 scripts/fetch_llama_test.py

Standard library only.
"""

import copy
import os
import stat
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import fetch_llama  # noqa: E402

THIN = """/tmp/llama-server:
\t/System/Library/Frameworks/Accelerate.framework/Versions/A/Accelerate (compatibility version 1.0.0, current version 4.0.0)
\t/System/Library/Frameworks/Metal.framework/Versions/A/Metal (compatibility version 1.0.0, current version 370.64.2)
\t/usr/lib/libc++.1.dylib (compatibility version 1.0.0, current version 1700.255.5)
\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1351.0.0)
"""
ONE_ARCH_OF_FAT = THIN.replace("/tmp/llama-server:", "/tmp/llama-server (architecture x86_64):")
HOMEBREW = THIN + "\t/opt/homebrew/opt/openssl@3/lib/libssl.3.dylib (compatibility version 3.0.0, current version 3.0.0)\n"
RPATH = THIN + "\t@rpath/libllama.dylib (compatibility version 0.0.0, current version 0.0.0)\n"

BUILD_VERSION = """Load command 10
      cmd LC_BUILD_VERSION
  cmdsize 32
 platform 1
    minos 13.3
      sdk 26.0
   ntools 1
     tool 3
  version 1221.4
Load command 11
      cmd LC_SOURCE_VERSION
  cmdsize 16
  version 0.0
"""
VERSION_MIN = """Load command 8
      cmd LC_VERSION_MIN_MACOSX
  cmdsize 16
  version 13.3
      sdk 14.0
Load command 9
      cmd LC_SOURCE_VERSION
  cmdsize 16
  version 0.0
"""

FAKE_GIT = """#!/bin/sh
echo "git $*" >> "$FAKE_LOG"
case "$1" in
  init) mkdir -p .git ;;
  rev-parse) echo "$FAKE_COMMIT" ;;
esac
exit 0
"""
# Configure records the processor; build writes one small program per target that says
# which processor it is "for" in a comment the lipo stand-in reads.
FAKE_CMAKE = """#!/bin/sh
echo "cmake $*" >> "$FAKE_LOG"
if [ "$1" = "-S" ]; then
  build="$4"
  mkdir -p "$build/bin"
  for a in "$@"; do
    case "$a" in -DCMAKE_OSX_ARCHITECTURES=*) echo "${a#*=}" > "$build/arch" ;; esac
  done
elif [ "$1" = "--build" ]; then
  build="$2"
  arch=$(cat "$build/arch")
  seen=0
  for a in "$@"; do
    if [ "$seen" = 1 ]; then
      printf '#!/bin/sh\\n# archs: %s\\necho "version: stand-in (%s)"\\n' "$arch" "$arch" > "$build/bin/$a"
      chmod +x "$build/bin/$a"
    fi
    if [ "$a" = "--target" ]; then seen=1; fi
  done
fi
exit 0
"""
FAKE_LIPO = """#!/bin/sh
echo "lipo $*" >> "$FAKE_LOG"
if [ "$1" = "-archs" ]; then
  sed -n 's/^# archs: //p' "$2"
elif [ "$1" = "-create" ]; then
  out="$3"
  shift 3
  archs=""
  for f in "$@"; do archs="$archs $(sed -n 's/^# archs: //p' "$f")"; done
  printf '#!/bin/sh\\n# archs:%s\\necho "version: stand-in (universal)"\\n' "$archs" > "$out"
fi
exit 0
"""
FAKE_OTOOL = """#!/bin/sh
# otool -arch <arch> -L|-l <file>
echo "otool $*" >> "$FAKE_LOG"
arch="$2"
if [ "$3" = "-L" ]; then
  printf '%s (architecture %s):\\n' "$4" "$arch"
  printf '\\t/usr/lib/libc++.1.dylib (compatibility version 1.0.0, current version 1700.255.5)\\n'
  printf '\\t/System/Library/Frameworks/Metal.framework/Versions/A/Metal (compatibility version 1.0.0, current version 370.64.2)\\n'
  if [ -n "$FAKE_BAD_LIB" ] && [ "$arch" = "$FAKE_BAD_ARCH" ]; then
    printf '\\t%s (compatibility version 1.0.0, current version 1.0.0)\\n' "$FAKE_BAD_LIB"
  fi
else
  printf 'Load command 9\\n      cmd LC_BUILD_VERSION\\n  cmdsize 32\\n platform 1\\n    minos %s\\n      sdk 26.0\\n' "${FAKE_MINOS:-13.3}"
fi
exit 0
"""


# `arch -x86_64 <program>` runs the Intel part through Rosetta on an Apple silicon Mac.
FAKE_ARCH = """#!/bin/sh
echo "arch $*" >> "$FAKE_LOG"
if [ -n "$FAKE_NO_ROSETTA" ]; then
  echo "arch: posix_spawnp: Bad CPU type in executable" >&2
  exit 1
fi
shift
exec "$@"
"""


class VulkanRuntime(unittest.TestCase):
    """The Windows fallback copy of vulkan-1.dll (llama.lock.json "vulkan_runtime")."""

    def setUp(self):
        self.spec = fetch_llama.LOCK["vulkan_runtime"]

    def test_the_pin_is_complete_and_from_the_publisher(self):
        for key in ("sha256", "member_sha256"):
            self.assertRegex(self.spec[key], r"^[0-9a-f]{64}$")
        self.assertGreater(self.spec["size"], 1_000_000)
        self.assertTrue(self.spec["url"].startswith(fetch_llama.LUNARG))
        self.assertTrue(self.spec["url"].endswith("/" + self.spec["name"]))
        self.assertTrue(self.spec["member"].endswith("/x64/vulkan-1.dll"))
        self.assertEqual(self.spec["dir"], "vulkan")

    def test_only_the_named_host_is_allowed_for_a_download(self):
        entry = dict(self.spec, name="not-cached-" + self.spec["name"])
        # The default allow-list is GitHub only: the LunarG address is refused there.
        with self.assertRaises(SystemExit) as refused:
            fetch_llama.download(entry)
        self.assertIn("refusing", str(refused.exception))
        elsewhere = dict(entry, url="https://example.com/" + entry["name"])
        with self.assertRaises(SystemExit):
            fetch_llama.download(elsewhere, allowed=(fetch_llama.LUNARG,))

    def test_a_changed_file_inside_the_archive_stops_the_build(self):
        import io, tempfile, zipfile as zf
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            archive = tmp / "rt.zip"
            with zf.ZipFile(archive, "w") as z:
                z.writestr(self.spec["member"], b"not the real loader")
            original = fetch_llama.download
            fetch_llama.download = lambda entry, allowed=(): archive
            try:
                with self.assertRaises(SystemExit) as stopped:
                    fetch_llama.install_vulkan_runtime(tmp / "main")
            finally:
                fetch_llama.download = original
            self.assertIn("expected " + self.spec["member_sha256"], str(stopped.exception))
            self.assertFalse((tmp / "main" / "vulkan" / "vulkan-1.dll").exists())


class Naming(unittest.TestCase):
    def test_sidecar_names_are_the_ones_tauri_looks_for(self):
        self.assertEqual(fetch_llama.sidecar_name("llama-server", "aarch64-apple-darwin"), "llama-server-aarch64-apple-darwin")
        self.assertEqual(fetch_llama.sidecar_name("llama-server", "x86_64-apple-darwin"), "llama-server-x86_64-apple-darwin")
        self.assertEqual(
            fetch_llama.sidecar_name("llama-fit-params", fetch_llama.MACOS_UNIVERSAL_TRIPLE),
            "llama-fit-params-universal-apple-darwin",
        )

    def test_commands(self):
        self.assertEqual(
            fetch_llama.cmake_configure_command(Path("/s"), Path("/s/b"), ["-DA=1", "-DB=2"]),
            ["cmake", "-S", "/s", "-B", "/s/b", "-DA=1", "-DB=2"],
        )
        self.assertEqual(
            fetch_llama.cmake_build_command(Path("/s/b"), ["llama-server", "llama-fit-params"], 8),
            ["cmake", "--build", "/s/b", "--config", "Release", "-j", "8", "--target", "llama-server", "llama-fit-params"],
        )
        self.assertEqual(
            fetch_llama.lipo_create_command([Path("/o/a-arm"), Path("/o/a-x86")], Path("/o/a-universal")),
            ["lipo", "-create", "-output", "/o/a-universal", "/o/a-arm", "/o/a-x86"],
        )


class Plan(unittest.TestCase):
    def test_the_lock_file_describes_both_processors(self):
        plan = fetch_llama.macos_plan(fetch_llama.LOCK)
        self.assertEqual(plan["commit"], fetch_llama.LOCK["llama_cpp"]["commit"])
        self.assertEqual(plan["targets"], ["llama-server", "llama-fit-params"])
        self.assertEqual([(s["arch"], s["triple"]) for s in plan["slices"]],
                         [("arm64", "aarch64-apple-darwin"), ("x86_64", "x86_64-apple-darwin")])
        arm, intel = (s["cmake_flags"] for s in plan["slices"])
        for flags in (arm, intel):
            for flag in ("-DBUILD_SHARED_LIBS=OFF", "-DGGML_NATIVE=OFF", "-DGGML_METAL=ON",
                         "-DGGML_METAL_EMBED_LIBRARY=ON", "-DGGML_BLAS=ON", "-DLLAMA_OPENSSL=OFF",
                         "-DCMAKE_OSX_DEPLOYMENT_TARGET=13.3", "-DCMAKE_BUILD_TYPE=Release"):
                self.assertIn(flag, flags)
        self.assertIn("-DCMAKE_OSX_ARCHITECTURES=arm64", arm)
        self.assertIn("-DCMAKE_OSX_ARCHITECTURES=x86_64", intel)
        # What every Intel Mac that runs macOS 13 has, named one by one; nothing newer.
        for flag in ("-DGGML_AVX=ON", "-DGGML_AVX2=ON", "-DGGML_FMA=ON", "-DGGML_F16C=ON", "-DGGML_ACCELERATE=ON"):
            self.assertIn(flag, intel)
        self.assertNotIn("-DGGML_AVX512=ON", intel)
        # The two builds differ only in the processor and its instruction sets.
        common = lambda flags: sorted(  # noqa: E731
            f for f in flags
            if not f.startswith(("-DCMAKE_OSX_ARCHITECTURES=", "-DGGML_SSE42=", "-DGGML_AVX", "-DGGML_FMA=",
                                 "-DGGML_F16C=", "-DGGML_BMI2=", "-DGGML_ACCELERATE="))
        )
        self.assertEqual(common(arm), common(intel))

    def test_the_app_lists_the_cpu_first_on_an_intel_mac(self):
        installer = fetch_llama.LOCK["installer"]
        self.assertEqual(installer["macos-aarch64"]["backends"], ["metal", "cpu"])
        self.assertEqual(installer["macos-x86_64"]["backends"], ["cpu", "metal"])

    def broken(self, change):
        lock = copy.deepcopy(fetch_llama.LOCK)
        change(lock)
        with self.assertRaises(SystemExit) as raised:
            fetch_llama.macos_plan(lock)
        return str(raised.exception)

    def test_a_missing_or_inconsistent_entry_stops_the_build(self):
        intel = lambda lock: lock["installer"]["macos-x86_64"]["build_from_source"]  # noqa: E731
        self.assertIn("macos-x86_64", self.broken(lambda lock: lock["installer"].pop("macos-x86_64")))

        def wrong_arch(lock):
            flags = intel(lock)["cmake_flags"]
            flags[flags.index("-DCMAKE_OSX_ARCHITECTURES=x86_64")] = "-DCMAKE_OSX_ARCHITECTURES=arm64"

        self.assertIn("CMAKE_OSX_ARCHITECTURES=x86_64", self.broken(wrong_arch))
        self.assertIn("commit", self.broken(lambda lock: intel(lock).update(commit="0" * 40)))
        self.assertIn("targets", self.broken(lambda lock: intel(lock).update(targets=["llama-server"])))
        self.assertIn("BUILD_SHARED_LIBS", self.broken(lambda lock: intel(lock)["cmake_flags"].remove("-DBUILD_SHARED_LIBS=OFF")))
        self.assertIn("GGML_NATIVE", self.broken(lambda lock: intel(lock)["cmake_flags"].remove("-DGGML_NATIVE=OFF")))
        self.assertIn(
            "DEPLOYMENT_TARGET",
            self.broken(lambda lock: intel(lock)["cmake_flags"].remove("-DCMAKE_OSX_DEPLOYMENT_TARGET=13.3")),
        )
        self.assertIn("pinned", self.broken(lambda lock: lock["llama_cpp"].update(commit="1" * 40)))


class OtoolOutput(unittest.TestCase):
    def test_library_lines(self):
        self.assertEqual(len(fetch_llama.linked_libraries(THIN)), 4)
        self.assertEqual(fetch_llama.linked_libraries(ONE_ARCH_OF_FAT), fetch_llama.linked_libraries(THIN))
        self.assertEqual(fetch_llama.non_system_libraries(THIN), [])
        self.assertEqual(fetch_llama.non_system_libraries(ONE_ARCH_OF_FAT), [])
        self.assertEqual(fetch_llama.non_system_libraries(HOMEBREW), ["/opt/homebrew/opt/openssl@3/lib/libssl.3.dylib"])
        self.assertEqual(fetch_llama.non_system_libraries(RPATH), ["@rpath/libllama.dylib"])
        self.assertEqual(fetch_llama.linked_libraries(""), [])

    def test_minimum_os(self):
        self.assertEqual(fetch_llama.minimum_os_versions(BUILD_VERSION), ["13.3"])
        self.assertEqual(fetch_llama.minimum_os_versions(VERSION_MIN), ["13.3"])
        self.assertEqual(fetch_llama.minimum_os_versions("Load command 1\n      cmd LC_SOURCE_VERSION\n  version 13.3\n"), [])

    def test_which_parts_can_run_on_which_mac(self):
        p = Path("/b/llama-server")
        self.assertEqual(fetch_llama.version_command(p, "arm64", "arm64"), (["/b/llama-server", "--version"], True))
        self.assertEqual(fetch_llama.version_command(p, "x86_64", "x86_64"), (["/b/llama-server", "--version"], True))
        self.assertEqual(
            fetch_llama.version_command(p, "x86_64", "arm64"),
            (["arch", "-x86_64", "/b/llama-server", "--version"], False),
        )
        self.assertIsNone(fetch_llama.version_command(p, "arm64", "x86_64"))


@unittest.skipIf(os.name == "nt", "the stand-in tools are shell scripts")
class DryRun(unittest.TestCase):
    """Runs the real `macos` and `check-macos-app` code with stand-ins for the Mac tools."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        tools = self.root / "tools"
        tools.mkdir()
        stand_ins = (("git", FAKE_GIT), ("cmake", FAKE_CMAKE), ("lipo", FAKE_LIPO), ("otool", FAKE_OTOOL), ("arch", FAKE_ARCH))
        for name, text in stand_ins:
            tool = tools / name
            tool.write_text(text, encoding="utf-8")
            tool.chmod(tool.stat().st_mode | stat.S_IXUSR)
        self.log = self.root / "commands.log"
        self.saved = (dict(os.environ), fetch_llama.ROOT, fetch_llama.CACHE, fetch_llama.platform.machine)
        os.environ["PATH"] = f"{tools}{os.pathsep}{os.environ['PATH']}"
        os.environ["FAKE_LOG"] = str(self.log)
        os.environ["FAKE_COMMIT"] = fetch_llama.LOCK["llama_cpp"]["commit"]
        fetch_llama.ROOT = self.root
        fetch_llama.CACHE = self.root / "cache"
        fetch_llama.platform.machine = lambda: "x86_64"

    def tearDown(self):
        env, fetch_llama.ROOT, fetch_llama.CACHE, fetch_llama.platform.machine = self.saved
        os.environ.clear()
        os.environ.update(env)
        self.tmp.cleanup()

    def commands(self):
        return self.log.read_text(encoding="utf-8").splitlines()

    def test_macos_builds_both_processors_and_joins_them(self):
        fetch_llama.macos()
        out = self.root / "src-tauri" / "binaries"
        self.assertEqual(
            sorted(p.name for p in out.iterdir()),
            [
                "llama-fit-params-aarch64-apple-darwin",
                "llama-fit-params-universal-apple-darwin",
                "llama-fit-params-x86_64-apple-darwin",
                "llama-server-aarch64-apple-darwin",
                "llama-server-universal-apple-darwin",
                "llama-server-x86_64-apple-darwin",
            ],
        )
        for name in ("llama-server", "llama-fit-params"):
            self.assertEqual(fetch_llama.macos_archs(out / f"{name}-aarch64-apple-darwin"), ["arm64"])
            self.assertEqual(fetch_llama.macos_archs(out / f"{name}-x86_64-apple-darwin"), ["x86_64"])
            self.assertEqual(fetch_llama.macos_archs(out / f"{name}-universal-apple-darwin"), ["arm64", "x86_64"])
            self.assertTrue(os.access(out / f"{name}-universal-apple-darwin", os.X_OK))
        commands = self.commands()
        configures = [c for c in commands if c.startswith("cmake -S")]
        self.assertEqual(len(configures), 2)
        self.assertIn("build-app-arm64", configures[0])
        self.assertIn("-DCMAKE_OSX_ARCHITECTURES=arm64", configures[0])
        self.assertIn("build-app-x86_64", configures[1])
        self.assertIn("-DCMAKE_OSX_ARCHITECTURES=x86_64", configures[1])
        self.assertIn("-DGGML_AVX2=ON", configures[1])
        self.assertNotIn("-DGGML_AVX2=ON", configures[0])
        self.assertEqual(len([c for c in commands if c.startswith("cmake --build")]), 2)
        joins = [c for c in commands if c.startswith("lipo -create")]
        self.assertEqual(len(joins), 2)
        self.assertTrue(joins[0].endswith("llama-server-aarch64-apple-darwin " + str(out / "llama-server-x86_64-apple-darwin")))
        # Every part of every file had its libraries and minimum OS read: 2 programs x
        # (arm64 + x86_64 + both parts of the universal file) x (-L and -l).
        self.assertEqual(len([c for c in commands if c.startswith("otool -arch")]), 16)
        self.assertEqual(len([c for c in commands if c.startswith("otool -arch x86_64")]), 8)

    def test_on_apple_silicon_the_intel_part_is_tried_through_rosetta(self):
        fetch_llama.platform.machine = lambda: "arm64"
        fetch_llama.macos()
        out = self.root / "src-tauri" / "binaries"
        tried = [c for c in self.commands() if c.startswith("arch ")]
        self.assertEqual(
            tried,
            [f"arch -x86_64 {out / (name + '-' + triple)} --version"
             for triple in ("x86_64-apple-darwin",) for name in ("llama-server", "llama-fit-params")]
            + [f"arch -x86_64 {out / (name + '-universal-apple-darwin')} --version"
               for name in ("llama-server", "llama-fit-params")],
        )

    def test_a_mac_without_rosetta_still_finishes_the_build(self):
        fetch_llama.platform.machine = lambda: "arm64"
        os.environ["FAKE_NO_ROSETTA"] = "1"
        fetch_llama.macos()
        self.assertEqual(len(list((self.root / "src-tauri" / "binaries").iterdir())), 6)
        self.assertEqual(len([c for c in self.commands() if c.startswith("arch ")]), 4)

    def test_macos_stops_when_the_intel_part_links_a_homebrew_library(self):
        os.environ["FAKE_BAD_LIB"] = "/opt/homebrew/lib/libomp.dylib"
        os.environ["FAKE_BAD_ARCH"] = "x86_64"
        with self.assertRaises(SystemExit) as raised:
            fetch_llama.macos()
        self.assertIn("(x86_64) links a non-system library: /opt/homebrew/lib/libomp.dylib", str(raised.exception))

    def test_macos_stops_on_the_wrong_minimum_os(self):
        os.environ["FAKE_MINOS"] = "15.0"
        with self.assertRaises(SystemExit) as raised:
            fetch_llama.macos()
        self.assertIn("expected 13.3", str(raised.exception))

    def test_macos_stops_on_the_wrong_commit(self):
        os.environ["FAKE_COMMIT"] = "f" * 40
        with self.assertRaises(SystemExit) as raised:
            fetch_llama.macos()
        self.assertIn("checked out", str(raised.exception))

    def app(self, programs):
        folder = self.root / "Demo.app" / "Contents" / "MacOS"
        folder.mkdir(parents=True)
        for name, archs in programs.items():
            (folder / name).write_text(f"#!/bin/sh\n# archs: {archs}\n", encoding="utf-8")
        return self.root / "Demo.app"

    def test_app_check_passes_a_universal_bundle(self):
        both = "arm64 x86_64"
        fetch_llama.check_macos_app(self.app({"Demo": both, "llama-server": both, "llama-fit-params": both}))
        self.assertEqual(len([c for c in self.commands() if c.startswith("otool -arch") and " -L " in c]), 6)

    def test_app_check_fails_a_one_processor_program(self):
        both = "arm64 x86_64"
        with self.assertRaises(SystemExit) as raised:
            fetch_llama.check_macos_app(self.app({"Demo": both, "llama-server": "arm64", "llama-fit-params": both}))
        self.assertIn("llama-server is built for ['arm64']", str(raised.exception))

    def test_app_check_fails_without_a_sidecar_or_with_a_foreign_library(self):
        both = "arm64 x86_64"
        with self.assertRaises(SystemExit) as raised:
            fetch_llama.check_macos_app(self.app({"Demo": both, "llama-server": both}))
        self.assertIn("does not contain llama-fit-params", str(raised.exception))
        (self.root / "Demo.app" / "Contents" / "MacOS" / "llama-fit-params").write_text(
            f"#!/bin/sh\n# archs: {both}\n", encoding="utf-8"
        )
        os.environ["FAKE_BAD_LIB"] = "/opt/homebrew/opt/openssl@3/lib/libssl.3.dylib"
        os.environ["FAKE_BAD_ARCH"] = "arm64"
        with self.assertRaises(SystemExit) as raised:
            fetch_llama.check_macos_app(self.root / "Demo.app")
        self.assertIn("(arm64) links a non-system library", str(raised.exception))


if __name__ == "__main__":
    unittest.main()

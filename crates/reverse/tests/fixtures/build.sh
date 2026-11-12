#!/bin/sh
# Rebuilds the reverse-engineering test files in this folder (item5-spec.md WP9c): the
# programs rea opens and checks in the tests of WP9e and WP9h. The outputs are committed;
# CI never runs this script.
#
# Needs: gcc (the Linux ELF file); clang, lld-link and llvm-dlltool (the Windows PE file,
# with no Windows SDK and no C library); mcs, Mono's C# compiler (the .NET library).
# The catalog fixture (rea-<version>-catalog.json) is written by scripts/pin_reverse.py.
set -eu
cd "$(dirname "$0")"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# Linux x86-64 ELF. Symbols are kept, so rea finds `main`; no build id, so a rebuild with
# the same compiler gives the same bytes.
gcc -O1 -fno-asynchronous-unwind-tables -Wl,--build-id=none -o hello-linux-x64 hello.c

# Windows x86-64 PE that imports three functions from kernel32.dll.
cat > "$work/kernel32.def" <<'DEF'
LIBRARY kernel32.dll
EXPORTS
GetStdHandle
WriteFile
ExitProcess
DEF
llvm-dlltool -m i386:x86-64 -d "$work/kernel32.def" -l "$work/kernel32.lib"
clang --target=x86_64-pc-windows-msvc -O1 -ffreestanding -fno-builtin -fno-stack-protector \
  -c hello.c -o "$work/hello.obj"
lld-link /nologo /brepro /entry:mainCRTStartup /subsystem:console /nodefaultlib \
  "$work/hello.obj" "$work/kernel32.lib" /out:hello-win-x64.exe

# .NET library with one class and two methods.
mcs -target:library -optimize+ -out:hello-dotnet.dll Hello.cs

for f in hello-linux-x64 hello-win-x64.exe hello-dotnet.dll; do
  size=$(wc -c < "$f")
  if [ "$size" -ge 20480 ]; then
    echo "$f is $size bytes, over 20 KB" >&2
    exit 1
  fi
done
sha256sum hello-linux-x64 hello-win-x64.exe hello-dotnet.dll

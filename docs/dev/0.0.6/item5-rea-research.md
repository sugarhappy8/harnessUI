# Item 5: rea with a free decompiler (research, 2026-11-12)

Research only; nothing is chosen. The owner decides. Not legal advice. Every licence was read
in the project's own LICENSE file or official page (links below).

## What rea is
- https://github.com/morluto/rea, "REA: Reverse Engineer Anything". An **MCP server (stdio)
  and a command-line program** in one npm package, `rea-agents` (6.9.0; `npx rea-agents mcp`).
  Its setup writes its configuration into other agents (Claude Code, Codex, Cursor, Gemini
  CLI). Not a library.
- TypeScript on Node. It drives outside tools as separate processes: Ghidra (Java scripts
  run through `analyzeHeadless`), Hopper and Cutter (Python bridges), IDA (through an IDA
  MCP server), Kuna (command line), JADX, Rizin, objdump, GDB, Frida, Binwalk, tshark,
  Playwright and others. Many inspectors are built in and need no decompiler (Electron and
  JavaScript, .NET metadata, PE/ELF/Mach-O headers, Go and Rust metadata).
- Pseudocode needs one of **Hopper** (paid), **Ghidra**, **IDA** (paid, or IDA Free through
  a third-party plugin) or **Kuna**. No RetDec, angr or radare2 support.
- **Licence: MIT** ([LICENSE](https://github.com/morluto/rea/blob/main/LICENSE)). Parts keep
  their own ([THIRD_PARTY_NOTICES.md](https://github.com/morluto/rea/blob/main/THIRD_PARTY_NOTICES.md)):
  a Windows native addon built with MinGW (GPLv3 with the GCC runtime exception), optional
  Frida (LGPL-2.0 with the wxWindows exception). It bundles no decompiler.
- **Needs:** Node.js 22.23.4+, 24.22.1+ or 26.12.1+ and npm. Ghidra 12.1 or later with a
  64-bit JDK (21; Ghidra 12.2 needs JDK 25). Ghidra support on Windows is "experimental":
  Windows 10+ x64, read-only, PE files only. Kuna verified on macOS arm64 only. JADX needs a JDK.
- **Activity:** very active (commits on 2026-11-12, many contributors, versions ship
  through npm, the README says it "changes quickly"). Star counts could not be checked.
- **What makes shipping it hard:**
  1. It needs Node, which the app does not ship: bundle Node or ask the owner to install it.
  2. The package unpacks to about 190 MB in about 10,600 files, with native addons.
  3. It never installs a decompiler: the app would ship and update one (and a JDK for Ghidra).
  4. Ghidra on Windows (the owner's PC) is experimental and read-only in rea.
  5. Fast releases with strict version checks of the decompilers.
  6. As an MCP server it fits the app's existing MCP client and policy gate (every call
     a tool action with a card, the audit log, three approval modes), but it starts many
     programs of its own (debuggers, Frida, Playwright) that the gate sees only as one MCP
     call. Which of its tools to offer, and which always ask, is a design question.

## Free decompilers that may be shipped
| Decompiler | Licence (source) | Needs / size | Systems | Ship inside the app? | Used by rea? |
|---|---|---|---|---|---|
| **Ghidra** 12.x | Apache-2.0 ([LICENSE](https://github.com/NationalSecurityAgency/ghidra/blob/master/LICENSE), [NOTICE](https://github.com/NationalSecurityAgency/ghidra/blob/master/NOTICE)); modules under `GPL/` are GPLv3 | JDK 21+ (25+ for 12.2); 543 MB zip plus about 200 MB JDK (estimate) | Windows, macOS, Linux | Yes: keep LICENSE and NOTICE; offer source for the GPL modules or leave them out; the bundled OpenJDK (GPLv2 with Classpath exception) needs its notices | Yes, its main provider (Windows experimental) |
| **Kuna** 1.997 | Apache-2.0 ([LICENSE](https://github.com/Noelo-Lab/kuna/blob/main/LICENSE), [NOTICE](https://github.com/Noelo-Lab/kuna/blob/main/NOTICE)) | One Rust program, about 25 MB, no Java | Windows x64, macOS, Linux | Yes: keep LICENSE and NOTICE | Yes (verified on macOS arm64 only); calls itself "an experiment" |
| **RetDec** | MIT ([LICENSE](https://github.com/avast/retdec/blob/master/LICENSE)) plus third-party notices | Python; about 5-6 GB installed | all three | Yes | No; upstream in "limited maintenance mode" |
| **radare2** + r2dec / r2ghidra | radare2 mostly LGPLv3, GPL parts optional ([COPYING.md](https://github.com/radareorg/radare2/blob/master/COPYING.md)); r2dec BSD-3-Clause ([LICENSES](https://github.com/wargio/r2dec-js/tree/master/LICENSES)); r2ghidra LGPL-3.0-only ([LICENSE.md](https://github.com/radareorg/r2ghidra/blob/master/LICENSE.md)) | native, small | all three | Yes as separate programs: licence text and source offer for the LGPL parts | No |
| **Rizin** + rz-ghidra | Rizin GPL-3.0 ([COPYING](https://github.com/rizinorg/rizin/blob/dev/COPYING)); rz-ghidra LGPL-3.0+ ([COPYING](https://github.com/rizinorg/rz-ghidra/blob/dev/COPYING)) | native, small | all three | Yes as a separate program: GPL text and source offer | Partly (Rizin commands, not as a pseudocode provider) |
| **angr** | BSD-style ([LICENSE](https://github.com/angr/angr/blob/master/LICENSE)) | Python, heavy | all three | Yes | No |
| **JADX** (Android apps) | Apache-2.0 ([LICENSE](https://github.com/skylot/jadx/blob/master/LICENSE)) | Java 11+ | all three | Yes | Yes (through an Apache-2.0 MCP bridge) |
| **ILSpy / ilspycmd** (.NET) | MIT ([LICENSE](https://github.com/icsharpcode/ILSpy/blob/master/LICENSE)) | .NET runtime | all three | Yes | Optional |
| **Reko** | GPL-2.0 ([COPYING](https://github.com/uxmal/reko/blob/master/COPYING)) | .NET 9 | all three | Separate program with source offer | No |
| Binary Ninja Free | proprietary, no redistribution ([licence](https://docs.binary.ninja/about/license/free.html)) | | | **No** | No |
| IDA Free | proprietary EULA, no redistribution, cloud-only decompiler ([EULA](https://hex-rays.com/eula)) | | | **No** | Only through a third-party plugin |
| Snowman, Boomerang, dnSpy | Snowman: history replaced, no licence found; Boomerang: inactive since 2020; dnSpy(Ex): GPL-3.0, Windows GUI | | | Not recommended | No |

GPL and LGPL programs run as separate processes are generally treated as aggregation: the
app keeps its licence, ships the licence text and offers the source. Linking them into the
app's binary is not an option.

## Realistic options (not ranked)
1. **rea + Ghidra + a bundled JDK.** Deepest support in rea, Apache-2.0. About 750 MB more
   per system; Windows support in rea is experimental and read-only.
2. **rea + Kuna.** Apache-2.0, one 25 MB program on all three systems, no Java. Young
   ("an experiment"), verified by rea only on macOS arm64, fewer operations than Ghidra.
3. **rea + both, the owner picks per computer.** Two things to keep up to date; rea must be
   told which one to use.
4. **No rea: the app's own small tool around one decompiler** (Ghidra headless, Kuna or
   radare2 + r2dec). Smaller and fully under the policy gate, but it is not rea.
5. Not shippable: IDA Free, Binary Ninja Free. Paid tools (Hopper, IDA Pro, JEB) only if
   the owner points rea at a copy they own.

Whatever the decompiler, rea itself needs Node (bundled, about 30-40 MB per system, or
installed by the owner) and its own 190 MB package. "Download on first use" (like models)
instead of inside the installer keeps the installer small and needs a network connection once.

## Questions for the owner
- rea itself (Node, 190 MB, fast-moving), or a smaller tool of the app's own with a decompiler?
- Which decompiler: Ghidra (big, most capable), Kuna (small, young), or both?
- Inside the installer, or downloaded on first use?
- Is reverse engineering only of files the owner points at, inside approved folders (as the
  file tools are), and are debugging and Frida (attaching to running programs) in or out?

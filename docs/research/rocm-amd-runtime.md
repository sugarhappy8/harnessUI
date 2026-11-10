# ROCm on Windows — amd-runtime

## Summary

Angle B (AMD ROCm/HIP on Windows), verified 2026-11-10. The brief's premise is out of date in two ways that change the architecture.

**1. Upstream no longer ships `hip-radeon`, and its replacement cannot be bundled as-is.** The asset is now `llama-<tag>-bin-win-rocm-10.0-x64.zip` (b12242: 255 MB). Since b10356 (2026-08-10) it contains only `ggml-hip.dll`, `amdhip64_7.dll`, `rocm_kpack.dll` and `amd_comgr.dll` on top of the CPU zip. `hipblas.dll`, `rocblas.dll`, hipBLASLt and the kernel libraries are deliberately left out, so on a clean PC `--list-devices` prints `(none)` and the server silently runs on CPU. The issue is still open.

**2. The only self-contained Windows ROCm build is the AMD-sponsored `lemonade-sdk/llamacpp-rocm`.** For the RX 9070 XT that is `llama-b1372-windows-rocm-gfx120X-x64.zip`: 688,519,474 bytes zipped, 995,337,813 bytes and 882 files unpacked. It is built nightly against an alpha TheRock ROCm (10.3.0a20261109), and only the gfx1151 archive is tested on real hardware.

Other points that matter for the design:

- **Official support:** the RX 9070 XT (gfx1201) is supported on Windows by HIP SDK 6.4.2 through 7.2 and by ROCm 7.14, 10.0.0 and 10.1.0. ROCm 10.x lists Windows 11 25H2 only; Windows 10 is not listed. Required drivers are Adrenalin 26.8.1 for ROCm 10.0.0 and the ROCm-specific 26.10.41.05 for 10.1.0.
- **Licensing:** the bundled runtime is permissively licensed (MIT, BSD-2-Clause, Apache-2.0 with LLVM exception), so redistribution is allowed with notices. The lemonade zip carries only llama.cpp's LICENSE, so the app must add the ROCm notices itself. I could not read the HIP SDK installer EULA (amd.com returned 503).
- **Performance:** ROCm is not faster than Vulkan for token generation on RDNA3/RDNA4. On the RX 9070 XT, Vulkan leads generation by about 8–45%; HIP leads prompt processing by about 30–50% in the Windows reports (the upstream scoreboards put Vulkan ahead on both). A maintainer states HIP "is not expected to be faster than the vulkan backed".
- **iGPU hazard on the owner's machine:** on Windows the HIP runtime enumerates the Ryzen desktop iGPU (gfx1036, PCI 0x164E on a 7900X) as a ROCm device with about 50 GB of "VRAM". Its index is not stable, and llama.cpp splits work onto it and crashes. The fix that classified it as integrated was reverted on 2026-09-08. Masking with `HIP_VISIBLE_DEVICES` is required.

Limits of this research: github.com downloads were blocked by the session proxy, so I did not unpack any zip. Asset names, sizes and hashes come from the GitHub release pages; the lemonade manifest and upstream workflows were read as raw files. Nothing was written to the repository; working copies are under <temp workspace>

## Findings

### Upstream ggml-org/llama.cpp no longer publishes a `win-hip-radeon` asset; the Windows AMD asset is now `llama-<tag>-bin-win-rocm-10.0-x64.zip`.

Confidence: high

Release b12242 (2026-11-10) lists `llama-b12242-bin-win-rocm-10.0-x64.zip`, 255 MB, sha256 9693208f71f08937b05dd8e67e6c8f74041da4e7df79911b98f9a831103d7130. Sibling assets for sizing: `win-vulkan-x64` 36.1 MB, `win-cpu-x64` 23.8 MB, `win-cuda-12.4-x64` 276 MB plus `cudart-llama-bin-win-cuda-12.4-x64.zip` 373 MB, `win-cuda-13.5-x64` 163 MB plus cudart 13.5 407 MB. All b-tags are pre-releases; the 'Latest' release is v0.9.0 (2026-11-04), which points to nightly b12110. Sizes are GitHub's rounded display values.

Source: https://github.com/ggml-org/llama.cpp/releases/expanded_assets/b12242 ; https://github.com/ggml-org/llama.cpp/releases/tag/v0.9.0 ; https://raw.githubusercontent.com/ggml-org/llama.cpp/master/.github/workflows/release-publish.yml

### The upstream Windows ROCm zip is not self-contained: it omits hipBLAS, rocBLAS, hipBLASLt and their kernel files, and needs a separately installed ROCm on PATH.

Confidence: high

The `windows-rocm` job in release.yml builds only the `ggml-hip` target and packs four files: `ggml-hip.dll`, `amdhip64_7.dll`, `rocm_kpack.dll`, `amd_comgr.dll`. Its comment says "rocblas/hipblaslt kernels resolve fine via PATH and are not copied." The release-publish "Merge artifacts" step then injects the win-cpu zip (llama-server plus ggml-cpu). `ggml-hip` links `hip::host roc::rocblas roc::hipblas`. The PR author (slojosic-amd) wrote that "hipblas/rocblas/hipblaslt dlls and kernel files are not bundled" and that users "still have to install appropriate ROCm 7.14/10.0 build" and "add ROCM_BIN path to system's PATH". AMD's matching tarball `therock-dist-windows-gfx120X-all-10.0.0.tar.gz` is 2,282,922,923 bytes.

Source: https://raw.githubusercontent.com/ggml-org/llama.cpp/master/.github/workflows/release.yml (lines 940-1100) ; https://raw.githubusercontent.com/ggml-org/llama.cpp/master/ggml/src/ggml-hip/CMakeLists.txt ; https://github.com/ggml-org/llama.cpp/pull/26973

### Upstream's Windows ROCm packaging has been broken for clean machines since b10356 (2026-08-10), and the issue is still open.

Confidence: high

Timeline: PR #25775 (merged 2026-08-10) moved Windows from the HIP SDK installer to TheRock wheels (`rocm[libraries,devel]`), ROCm 7.14. PR #26973 (merged 2026-08-27) added the three HIP runtime DLLs. PR #27803 (merged 2026-09-02) moved to ROCm 10.0.0. Issue #26964 "Latest Windows ROCM not using GPU" is open; the latest report (2026-10-23, b11836, RX 6900 XT) says the zip lacks hipblas.dll/rocblas.dll and `--list-devices` shows none. An RX 9070 XT user confirmed on 2026-09-23 ("offloaded into RAM fully"; Vulkan works). Issue #26996 was closed as stale; the AMD assignee said "It pretty much needs all dlls under ROCm 7.14 installation". b10355 is reported as the last self-contained `hip-radeon` build.

Source: https://github.com/ggml-org/llama.cpp/issues/26964 ; https://github.com/ggml-org/llama.cpp/issues/26996 ; https://github.com/ggml-org/llama.cpp/issues/26929 ; https://github.com/ggml-org/llama.cpp/pull/25775 ; https://github.com/ggml-org/llama.cpp/pull/27803

### Upstream's ROCm build targets 20 gfx architectures from stable ROCm 10.0.0 wheels, including gfx1201 (RX 9070 XT) and gfx1036 (Ryzen desktop iGPU).

Confidence: high

Matrix: `ROCM_VERSION: "10.0.0"`, `gpu_targets: gfx1010;gfx1011;gfx1012;gfx1030;gfx1031;gfx1032;gfx1033;gfx1034;gfx1035;gfx1036;gfx1100;gfx1101;gfx1102;gfx1103;gfx1150;gfx1151;gfx1152;gfx1153;gfx1200;gfx1201`. Wheels come from `https://stable.repo.amd.com/rocm/whl-next/`. Flags: `-DGGML_HIP=ON -DGGML_BACKEND_DL=ON -DGGML_NATIVE=OFF -DGGML_CPU=OFF`, built with ROCm's clang on windows-2022. Release artifacts are attested with `actions/attest`.

Source: https://raw.githubusercontent.com/ggml-org/llama.cpp/master/.github/workflows/release.yml ; https://raw.githubusercontent.com/ggml-org/llama.cpp/master/.github/actions/windows-setup-rocm/action.yml ; https://raw.githubusercontent.com/ggml-org/llama.cpp/master/.github/workflows/release-publish.yml

### ROCm on Windows is now a first-class TheRock release: 7.14 was the first production TheRock release, 10.0.0 shipped 2026-08-26 and 10.1.0 on 2026-10-05; the RX 9070 XT (gfx1201) is officially supported.

Confidence: high

The 10.1.0 release notes (dated 2026-10-05) say "Since ROCm 7.14, ROCm uses TheRock as its build and release system." Both the 10.0.0 and 10.1.0 compatibility matrices list "AMD Radeon RX 9070 XT (gfx1201)" with RDNA 4, RDNA 3 (gfx1100/1101/1102) and RDNA 2 (gfx1030, PRO W6800/V620 only). "Supported Windows version: Windows 11 25H2" is the only Windows entry. Install is by tarball or pip; the gfx120X tarball for 10.1.0 is 2,244,477,973 bytes and the multiarch one 4,873,909,736 bytes. The Ryzen 7000 desktop iGPU is not in the supported list. Lemonade nightlies are already numbered 10.3.0a.

Source: https://rocm.docs.amd.com/en/docs-10.1.0/compatibility/compatibility-matrix.html ; https://rocm.docs.amd.com/en/docs-10.0.0/compatibility/compatibility-matrix.html ; https://rocm.docs.amd.com/en/docs-10.1.0/about/release-notes.html ; https://rocm.docs.amd.com/en/docs-10.1.0/install/rocm.html ; HEAD requests to https://stable.repo.amd.com/rocm/core/tarball/

### The classic AMD HIP SDK for Windows installer is the legacy path: latest is 7.2.0, Windows 11 only, and AMD tells users to uninstall it before installing ROCm 10.x.

Confidence: medium

HIP SDK 7.2.0 docs (dated 2026-07-31): supported OS is Windows 11 x86-64 (22H2 validated). RX 9070 XT / 9070 / 9070 GRE / 9060 XT / 9060 are supported for Runtime, HIP SDK and ROCm Debugger; all RDNA2 cards are unsupported. The installer bundles "AMD Radeon Software PRO 25.30". HIP SDK 6.4.2 docs list Windows 10, Windows 11 and Server 2022 and already list the RX 9070 XT. The amd.com download page reportedly lists 7.2 and 7.1.1 (Windows 11) and 6.4.2, 6.2.4, 6.1.2, 5.7.1, 5.5.1 (Windows 10 and 11); I could not fetch that page (HTTP 503), so the version list is from search snippets. The ROCm 10.1 install doc says "Remove any existing HIP SDK for Windows installations and other conflicting AMD graphics software." One user links the b8149→b8152 breakage to a move from HIP SDK 25.Q3 to 26.Q1 (ROCm 6.4 → 7.1); that is unconfirmed.

Source: https://rocm.docs.amd.com/projects/install-on-windows/en/latest/reference/system-requirements.html ; https://rocm.docs.amd.com/projects/install-on-windows/en/latest/about/releasenotes.html ; https://rocm.docs.amd.com/projects/install-on-windows/en/latest/install/install.html ; https://rocm.docs.amd.com/projects/install-on-windows/en/docs-6.4.2/reference/system-requirements.html ; https://github.com/ggml-org/llama.cpp/issues/21106

### Minimum Adrenalin driver for Radeon on Windows: 26.8.1 for ROCm 10.0.0 and the ROCm-specific 26.10.41.05 for ROCm 10.1.0.

Confidence: medium

AMD's matrices state "Supported Adrenalin Driver version 26.8.1" (10.0.0) and "26.10.41.05" (10.1.0, linked to release note RN-RAD-ROCM-10-01). Secondary sources give the Windows Driver Store versions: Adrenalin 26.8.1 = driver 26.10.41.01 = 32.0.31041.1004 (2026-08-20); the ROCm 10.1 package 26.10.41.05 = 32.0.31041.5005 (2026-10-05), a separate download covering RX 9000, AI PRO R9700 and RX 7900/7800/7700/7600, with driver-level support stated for Windows 10 1809+ and Windows 11 21H2+. Mainstream 26.9.1 carries 26.10.41.03. The Driver Store mapping is corroborated by an issue reporter's "32.0.31041.1004 (dated 2026-08-16)". I could not read AMD's own release note (503). No minimum driver is published for the alpha ROCm 10.3 runtime in the lemonade nightlies; TheRock only says to use the latest Adrenalin.

Source: https://rocm.docs.amd.com/en/docs-10.1.0/compatibility/compatibility-matrix.html ; https://rocm.docs.amd.com/en/docs-10.0.0/compatibility/compatibility-matrix.html ; https://mixed-news.com/en/?p=35994 ; https://www.pcgameshardware.de/AMD-Radeon-Grafikkarte-255597/News/ROCm-101-Neuer-Compute-KI-Treiber-1555888/ ; https://www.4gamer.net/games/022/G002212/20260821019/ ; https://raw.githubusercontent.com/ROCm/TheRock/main/RELEASES.md

### Windows 10 is outside AMD's stated support for ROCm 10.x and HIP SDK 7.x, but community reports show ROCm 10 builds running on Windows 10 with an RX 9070 XT.

Confidence: medium

ROCm 10.0/10.1 list only Windows 11 25H2; HIP SDK 7.2 lists only Windows 11. The ROCm 10.1 driver package itself claims Windows 10 1809+ (secondary source). lemonade-sdk/llamacpp-rocm issue #155 (2026-10-31) is from a Windows 10 + RX 9070 XT user running ROCm builds, and a commenter in llama.cpp #28676 reports Win10 with an RX 9070 and R9700. So on Windows 10 the ROCm backend is unsupported but works in practice.

Source: https://rocm.docs.amd.com/en/docs-10.1.0/compatibility/compatibility-matrix.html ; https://github.com/lemonade-sdk/llamacpp-rocm/issues/155 ; https://github.com/ggml-org/llama.cpp/issues/28676

### Every ROCm runtime component in the lemonade archive is under a permissive licence that allows redistribution with notices.

Confidence: high

AMD's licence table plus the LICENSE files fetched from ROCm repos: HIP / hipamd / CLR (`amdhip64_7.dll`) MIT; rocBLAS MIT plus appended BSD notices (UT ICL/MAGMA, STFC, LAPACK, KAUST, Eric Fiselier/LLVM); hipBLAS MIT plus LAPACK BSD notice; hipBLASLt, Tensile, origami and kpack MIT; TheRock MIT; rocSOLVER BSD-2-Clause plus LAPACK/MAGMA BSD notices; Comgr "Apache License v2.0 with LLVM Exceptions" with a legacy NCSA section (AMD's table still says NCSA). MIT text: "The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software." rocSOLVER: "Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the following disclaimer in the documentation and/or other materials provided with the distribution." Apache-2.0 §4(a): "You must give any other recipients of the Work or Derivative Works a copy of this License". The LLVM exception waives 4(a), 4(b), 4(d) only for portions embedded by compiling your own code, so it does not cover shipping `amd_comgr.dll` itself.

Source: https://rocm.docs.amd.com/en/docs-10.1.0/about/license.html ; https://raw.githubusercontent.com/ROCm/rocm-libraries/develop/projects/rocblas/LICENSE.md ; https://raw.githubusercontent.com/ROCm/rocm-libraries/develop/projects/hipblas/LICENSE.md ; https://raw.githubusercontent.com/ROCm/rocm-libraries/develop/projects/rocsolver/LICENSE.md ; https://raw.githubusercontent.com/ROCm/rocm-systems/develop/projects/hip/LICENSE.md ; https://raw.githubusercontent.com/ROCm/llvm-project/amd-staging/amd/comgr/LICENSE.txt ; https://raw.githubusercontent.com/ROCm/TheRock/main/LICENSE

### AMD's own deployment guidance tells ISVs to ship the HIP libraries in their installer, and AMD-authored packaging now also ships the HIP runtime next to the exe.

Confidence: high

"Application deployment guidelines for HIP SDK" (2026-02-19): "We recommend that ISV applications using the HIP SDK libraries distribute the HIP SDK Library as part of its installer package"; doing so "avoids having your end user required to agree to the licensing agreements for the entire HIP SDK"; "AMD does not support static linking to any components distributed in the HIP SDK." The same page says "Applications do not distribute the HIP runtime. Instead, end users use the HIP runtime provided by an AMD GPU Driver." TheRock-era practice differs: upstream PR #26973 (author slojosic-amd) copies `amdhip64_7.dll`, `rocm_kpack.dll` and `amd_comgr.dll` next to the binaries "so the correct HIP runtime loads over the driver's copy in System32", and the lemonade archives bundle the full runtime.

Source: https://rocm.docs.amd.com/projects/install-on-windows/en/latest/conceptual/deployment-guidelines.html ; https://github.com/ggml-org/llama.cpp/pull/26973 ; https://raw.githubusercontent.com/lemonade-sdk/llamacpp-rocm/main/README.md

### The HIP SDK installer's click-through EULA could not be read, so its redistribution terms are unconfirmed.

Confidence: low

AMD docs confirm the gate exists: "Clicking the HIP SDK download link takes you to a license page that you must accept before the download will begin." amd.com returned HTTP 503 to my fetches and I did not try to get around it. Whether DLLs extracted from an installed HIP SDK (for example `C:\Program Files\AMD\ROCm\7.1\bin`) may be redistributed is unknown. This is avoidable: the TheRock tarballs and the lemonade archives contain the same open-source components without that installer.

Source: https://rocm.docs.amd.com/projects/install-on-windows/en/latest/install/install.html ; https://www.amd.com/en/developer/resources/rocm-hub/hip-sdk.html (not retrievable)

### The lemonade Windows archive ships only llama.cpp's MIT LICENSE, so the app must add the ROCm third-party notices itself.

Confidence: medium

The manifest shows a single `LICENSE` of 1,099 bytes; the build workflow says "llama.cpp is MIT-licensed, which requires its notice to travel with every copy" and copies `llama.cpp\LICENSE`. No notice files for the bundled ROCm DLLs are in the manifest. Notices to ship, on my reading of the licences (not legal advice): llama.cpp MIT ("Copyright (c) 2023-2026 The ggml authors"); the full LICENSE.md files of rocBLAS, hipBLAS and rocSOLVER (they carry extra BSD notices); AMD MIT notices for HIP/CLR, hipBLASLt, Tensile, origami, kpack/TheRock; the full Apache-2.0-with-LLVM-exception text for Comgr and the LLVM it embeds; and an AMD trademark attribution line.

Source: https://raw.githubusercontent.com/lemonade-sdk/llamacpp-rocm/qualification-data/qualification/latest/windows-rocm-gfx120X.json ; https://raw.githubusercontent.com/lemonade-sdk/llamacpp-rocm/main/.github/workflows/build-llamacpp-rocm.yml ; https://raw.githubusercontent.com/ggml-org/llama.cpp/master/LICENSE

### lemonade-sdk/llamacpp-rocm publishes self-contained, per-GPU-family Windows and Ubuntu builds roughly nightly, each release gated by automated checks.

Confidence: high

Latest is b1372 (2026-11-09 14:21 UTC), ROCm 10.3.0a20261109, llama.cpp commit 0eebb6eddcd5dc70e5bd3ee440a47f36f6fb1ba8 (binary reports build 12228, 0.9.0-dev, Clang 24). Windows assets: gfx120X 657 MB (sha256 a98bbeee2e130ccffe73bea94492e097ac1dc753dcd7b7c1a42479dbe29ba897, 688,519,474 bytes, cross-checked against the qualification record), gfx110X 506 MB, gfx103X 506 MB, gfx117X 445 MB, gfx90a 319 MB, gfx908 213 MB, gfx1151 185 MB, gfx1150 182 MB, gfx1152 and gfx1153 180 MB each; plus `checksums.txt` and per-archive qualification JSON. Cadence: cron `0 13 * * *`; ten releases between Nov 3 and Nov 9. A release is published only if all 20 archives pass static checks and the gfx1151 archives pass inference on Strix Halo hardware; the gfx120X record says `hardware_validated: false`. README: "We're not set up to provide comprehensive technical support" and structure is "subject to change". The parent project states "This project is sponsored by AMD", and AMD-suffixed accounts triage its issues.

Source: https://github.com/lemonade-sdk/llamacpp-rocm/releases/expanded_assets/b1372 ; https://raw.githubusercontent.com/lemonade-sdk/llamacpp-rocm/main/README.md ; https://raw.githubusercontent.com/lemonade-sdk/llamacpp-rocm/main/.github/qualify-rocm/README.md ; https://raw.githubusercontent.com/lemonade-sdk/lemonade/main/README.md

### Exact contents of `llama-b1372-windows-rocm-gfx120X-x64.zip` (the RX 9070 XT build): 882 files, 995,337,813 bytes unpacked, covering gfx1200 and gfx1201.

Confidence: high

Root DLLs: `ggml-hip.dll` 131,302,400; `amd_comgr.dll` 127,697,920; `rocsolver.dll` 29,348,864; `rocblas.dll` 21,258,240; `amdhip64_7.dll` 17,244,160; `libtensilelite-host.dll` 4,882,432; `libhipblaslt.dll` 2,914,816; `hipblas.dll` 809,472; `origami.dll` 424,960; `rocm_kpack.dll` 182,784; plus `llama-server.exe` (10,240-byte launcher) with `llama-server-impl.dll`, `llama.dll`, `llama-common.dll`, `mtmd.dll`, `ggml.dll`, `ggml-base.dll`, `ggml-cpu.dll` (one CPU backend, GGML_NATIVE=OFF), `ggml-rpc.dll`. Folders: `hipblaslt/library` 574 files, 390.9 MiB; `.kpack` 14 files, 184.8 MiB; `rocblas/library` 220 files, 19.3 MiB. ROCm DLLs loaded by llama-server: amdhip64_7, hipblas, libhipblaslt, libtensilelite-host, origami, rocblas, rocm_kpack, rocsolver. `MSVCP140.dll`, `VCRUNTIME140.dll` and `VCRUNTIME140_1.dll` are treated as OS-provided and are not in the zip. Build flags include `-DGGML_HIP_ROCWMMA_FATTN=OFF -DGGML_RPC=ON -DGGML_STATIC=OFF`.

Source: https://raw.githubusercontent.com/lemonade-sdk/llamacpp-rocm/qualification-data/qualification/latest/windows-rocm-gfx120X.json ; https://raw.githubusercontent.com/lemonade-sdk/llamacpp-rocm/main/.github/workflows/build-llamacpp-rocm.yml

### A third source exists: the lemonade-sdk/llama.cpp fork builds a fat Windows zip against stable ROCm, but it too needs a separately staged TheRock runtime.

Confidence: high

Assets for b12109: `llama-b12109-bin-win-rocm-10.1-x64.zip` 218 MB (sha256 be9cd1ca5f6ff983b8dd09c69ed56c6b97d522a55ec778e162e9d451417f768c), `-rocm-10.0-` 210 MB, `-rocm-7.14-` 210 MB; 22 gfx targets including gfx1036 and gfx1201. Its qualification record lists `rocm_runtime_required: ["amdhip64_7.dll", "hipblas.dll"]`. The AMD-sponsored Lemonade app pins `rocm-stable: b12109` with `therock.version: 10.1.0` and `rocm-nightly: b1365`, verifies SHA-256, downloads the TheRock runtime itself, prepends its directories to PATH and copies the HIP runtime into the exe directory because "a stale System32 amdhip64_7.dll shadows the TheRock runtime on PATH".

Source: https://github.com/lemonade-sdk/llama.cpp/releases/expanded_assets/b12109 ; https://raw.githubusercontent.com/lemonade-sdk/llama.cpp/qualification-data/qualification/reports/42725774657-win-rocm-10.1.json ; https://raw.githubusercontent.com/lemonade-sdk/lemonade/main/src/cpp/resources/backend_versions.json ; https://raw.githubusercontent.com/lemonade-sdk/lemonade/main/src/cpp/server/backends/llamacpp/llamacpp_server.cpp

### On RDNA4 under Windows, HIP/ROCm is slower than Vulkan for token generation; it is faster for prompt processing in the Windows reports, though the upstream scoreboards show Vulkan ahead on both.

Confidence: high

Scoreboards (Llama 2 7B Q4_0, RX 9070 XT, OS not stated): Vulkan pp512 5201.65 / tg128 136.56 (FA off) and 5603.63 / 142.00 (FA on); ROCm 5055.19 / 101.27 and 4903.51 / 97.28. Windows 11, RX 9070 XT, b10298 (issue #26663), tg128 Vulkan vs HIP: Qwen3-0.6B 571.9 vs 292.2; 1.7B 321.8 vs 221.7; 4B 174.5 vs 129.2; 8B 104.3 vs 91.1. pp512: 4B 5223 vs 6671; 8B 2866 vs 4199. R9700 (gfx1201), Qwen3.8-27B: Vulkan pp512 743 / tg128 30.50; HIP pp512 1106–1211 / tg128 29.07. Maintainer IMbackK: "the hip backend is not expected to be faster than the vulkan backed", citing NVIDIA-oriented kernels, wave32 on HIP versus wave64 on Vulkan, and BLAS libraries little optimised for RDNA. Linux RX 7900 XTX data shows Vulkan about 20–22% faster in generation on a 7B model.

Source: https://github.com/ggml-org/llama.cpp/discussions/15021 ; https://github.com/ggml-org/llama.cpp/discussions/10879 ; https://github.com/ggml-org/llama.cpp/issues/26663 ; https://github.com/ggml-org/llama.cpp/issues/20934

### On Windows the HIP runtime exposes the Ryzen desktop iGPU as a ROCm device with unstable index, and llama.cpp spreads work onto it and crashes unless it is masked.

Confidence: high

Issue #23152 (Windows, Ryzen 7 9850X3D + RX 9070 XT, `win-hip-radeon` b9174): iGPU was ROCm0 (50246 MiB), dGPU ROCm1 (16304 MiB); crash "ggml_cuda_compute_forward: MUL_MAT failed" at model load. `-dev ROCm1` alone still crashed; `HIP_VISIBLE_DEVICES=1` fixed it, and the maintainer confirmed "this is the correct fix". In #28676 the iGPU was index 1 (masked with `HIP_VISIBLE_DEVICES=0,2`). In #25836 the Ryzen iGPU showed as gfx1036 / ROCm1. lemonade #96 (Linux, 9950X3D + R9700) shows the KV cache split 2048 MiB on the R9700 and 6144 MiB on the iGPU, then exit -1. PR #24233, which restored the HIP `integrated` flag (merged 2026-07-16), was reverted by PR #28604 (2026-09-08), so current master again treats every HIP device as discrete. The owner's Ryzen 9 7900X iGPU is PCI 1002:164E (Raphael). Applying this to the owner's exact 7900X + 9070 XT system is my extrapolation from near-identical setups.

Source: https://github.com/ggml-org/llama.cpp/issues/23152 ; https://github.com/ggml-org/llama.cpp/issues/28676 ; https://github.com/ggml-org/llama.cpp/issues/25836 ; https://github.com/lemonade-sdk/llamacpp-rocm/issues/96 ; https://github.com/ggml-org/llama.cpp/pull/24233 ; https://github.com/ggml-org/llama.cpp/pull/28604 ; https://raw.githubusercontent.com/pciutils/pciids/master/pci.ids

### Other open or recent ROCm-on-Windows defects affect gfx1201 specifically.

Confidence: high

(a) lemonade #155 (open, 2026-10-31, RX 9070 XT, Windows 10): prompt processing fell from about 1100 t/s on b1356 to about 400–480 t/s on b1357; AMD's benrichard-amd: "Unable to repro on gfx11... Might be specific to gfx12". Lemonade's own pin b1365 and the current b1372 are after that point. (b) llama.cpp #25836: BF16/F16 models crash with "ROCm error: invalid argument" (MUL_MAT) on gfx1201 Windows from a rocBLAS GEMM bug; quantised models are unaffected; workaround `GGML_CUDA_CUBLAS_COMPUTE_TYPE=f32`; closed stale, unfixed. (c) #28676 (open): dual-GPU HIP on Windows 11 with ROCm 10.0 gives fluent gibberish that llama-bench does not detect; single GPU is fine; building with `-DGGML_CUDA_NO_PEER_COPY=ON` avoids it. ROCm 10.1 notes claim a fix for multi-GPU LLM issues on Windows. (d) #29551: builds b11222–b11248 crashed on Windows unless `-dev` was given; fixed in b11249. (e) Lemonade's qualification README: "A broken ROCm build does not look broken" — the server finds zero GPUs and continues on CPU, and the "offloaded N/N layers" log line appears regardless.

Source: https://github.com/lemonade-sdk/llamacpp-rocm/issues/155 ; https://github.com/ggml-org/llama.cpp/issues/25836 ; https://github.com/ggml-org/llama.cpp/issues/28676 ; https://github.com/ggml-org/llama.cpp/issues/29551 ; https://rocm.docs.amd.com/en/docs-10.1.0/about/release-notes.html ; https://raw.githubusercontent.com/lemonade-sdk/llamacpp-rocm/main/.github/qualify-rocm/README.md

### Vulkan on the RX 9070 family has its own reported instabilities, but the headline '5–7x slower than HIP' claim was withdrawn.

Confidence: medium

Issue #26663 (RX 9070 XT, Windows 11, driver 32.0.22042.14002) first claimed Vulkan generation was 4.7–6.7x slower than HIP for hidden size ≥ 4096. The author could not reproduce it on b10298 ("The 5–7× claim was not reproduced") and the issue was closed as not planned. Another user on an RX 9070 (driver 32.0.31041.1004, b10816) saw a transient state where pp512 fell from 3868 to 821 t/s until reboot. In the ROCm bug threads, Vulkan was the working fallback every time.

Source: https://github.com/ggml-org/llama.cpp/issues/26663 ; https://github.com/ggml-org/llama.cpp/issues/26964 ; https://github.com/ggml-org/llama.cpp/issues/28676

### Runtime detection can be done with a PCI-ID pre-filter (vendor 0x1002) followed by an authoritative `--list-devices` probe whose stdout, not exit code, is parsed.

Confidence: medium

pci.ids: 7550 = Navi 48 [RX 9070/9070 XT/9070 GRE]; 7551 = Navi 48 [Radeon AI PRO R9700]; 7590 = Navi 44 [RX 9050 / 9060 XT]. RDNA3 dGPUs: 744c, 7448, 7449, 744a, 744b, 745e, 7460, 7461, 7470, 747e, 7480, 7483, 7489, 7499, 749f, 73f0. iGPUs to exclude: 164e Raphael (Ryzen 7000 desktop), 13c0 Granite Ridge, 15bf/15c8 Phoenix, 1900/1901 Hawk Point, 150e Strix, 1586 Strix Halo, 1114/1902 Krackan. Lemonade does it by name instead: WMI `Win32_VideoController` names containing "AMD" and "Radeon"; discrete if the name contains "rx ", "xt", "pro w", "pro v", "radeon pro", "firepro" or "fury"; "9070"/"9060"/"r9700" → gfx120X; discrete preferred over integrated; driver version read via SetupAPI. The probe prints "Available devices:" then `ROCmN: <name> (<MiB>...)` lines, or "(none)" with exit code 0. Reading the adapter's `DriverVersion` from the display class registry key (or DXGI `CheckInterfaceSupport`) is from my memory of Windows and was not verified here.

Source: https://raw.githubusercontent.com/pciutils/pciids/master/pci.ids ; https://raw.githubusercontent.com/lemonade-sdk/lemonade/main/src/cpp/server/system_info.cpp ; https://raw.githubusercontent.com/lemonade-sdk/lemonade/main/src/cpp/server/system_info_utils.h ; https://github.com/ggml-org/llama.cpp/issues/26929 ; https://github.com/ggml-org/llama.cpp/issues/23152

### The bundled HIP runtime DLLs must sit in the same directory as `llama-server.exe`; putting them in a subfolder or on PATH is not safe.

Confidence: high

The Adrenalin driver installs its own `amdhip64_7.dll` (and amd_comgr) in `C:\Windows\System32`, which Windows searches before PATH but after the application directory. A mismatched copy produced "cudaMemGetInfo failed (invalid argument), returning 0/0" and a 0 MiB device (#26963); copying only `amdhip64_7.dll` was not enough, all three of amdhip64_7 / rocm_kpack / amd_comgr were needed. Separately, llama.cpp loads backends with plain `LoadLibraryW`, so a backend's dependencies are not resolved from the backend's own folder (error 126) unless that folder is the exe directory or on PATH (#28009, closed unfixed 2026-10-25).

Source: https://raw.githubusercontent.com/ggml-org/llama.cpp/master/.github/workflows/release.yml ; https://github.com/ggml-org/llama.cpp/issues/26963 ; https://github.com/ggml-org/llama.cpp/issues/28009 ; https://raw.githubusercontent.com/lemonade-sdk/lemonade/main/src/cpp/server/backends/llamacpp/llamacpp_server.cpp

### AMD's ROCm-on-Windows install guide tells users to turn off Smart App Control and Defender Application Guard.

Confidence: medium

ROCm 10.1 install doc, "Prepare Windows for ROCm installation": "Disable the following Windows security features as they can interfere with ROCm functionality: Turn off WDAG (Windows Defender Application Guard) ... Turn off SAC (Smart App Control)". Whether the lemonade-built or TheRock DLLs are Authenticode-signed was not checked. How this affects a shipped app on machines with Smart App Control enabled is untested.

Source: https://rocm.docs.amd.com/en/docs-10.1.0/install/rocm.html

## Recommendations

- Do not plan around upstream's `win-hip-radeon` or `win-rocm-10.0` asset as the bundled ROCm backend: the first no longer exists and the second is not self-contained. Record this in ARCHITECTURE.md as a correction to the original assumption.
- Source the ROCm backend from `lemonade-sdk/llamacpp-rocm`, asset `llama-<tag>-windows-rocm-gfx120X-x64.zip`. Pin an exact tag and store its SHA-256 in the app's own build manifest (b1372: a98bbeee2e130ccffe73bea94492e097ac1dc753dcd7b7c1a42479dbe29ba897, 688,519,474 bytes). Never follow 'latest'.
- Before fixing the pin, benchmark b1356 against b1372 on the owner's RX 9070 XT (llama-bench pp512 and tg128, plus a text sanity check). The open prompt-processing regression (lemonade #155) started at b1357 and is suspected to be gfx12-specific.
- Keep Vulkan as the default backend on AMD GPUs, including the RX 9070 XT. Offer ROCm as an opt-in 'faster prompt processing' option, and fall back to Vulkan automatically whenever the ROCm probe or the post-load check fails.
- Keep each backend in its own directory with its own `llama-server.exe` and DLL set, and spawn it with that directory as the application directory. The lemonade archive is a different llama.cpp build from the upstream Vulkan/CUDA/CPU zips, so do not mix DLLs across them. Keep the HIP runtime DLLs beside the exe so they win over System32.
- Always mask the iGPU for ROCm: run `llama-server.exe --list-devices` from the ROCm directory with a 10–15 s timeout, parse the `ROCmN:` lines, pick the entry matching the discrete card (name plus memory close to dedicated VRAM, e.g. about 16,304 MiB), then launch with `HIP_VISIBLE_DEVICES=<N>` and an explicit `--device ROCm0`. Treat "(none)", a timeout or a crash as 'ROCm unavailable'.
- Add a post-load check, because a failed ROCm load silently runs on CPU: require the log to show `using device ROCm0` and a non-zero `ROCm0 model buffer size` (needs `-lv 4`), or measure throughput. Do not trust the "offloaded N/N layers" line.
- Gate the ROCm option on a cheap pre-filter before probing: a DXGI adapter with VendorId 0x1002 and DeviceId in an allow-list (0x7550, 0x7551, 0x7590 for the gfx120X archive), and the adapter's driver version at or above 32.0.31041.1004 (Adrenalin 26.8.1). Below that, show an 'update your AMD driver' hint. Validate the threshold on the owner's machine, since no minimum is published for the nightly ROCm runtime.
- Ship a THIRD-PARTY-NOTICES file with the ROCm backend containing the licence texts listed in the findings, and make sure the Microsoft VC++ 2015–2022 x64 runtime is installed or shipped app-local, since the lemonade archive does not include it.
- Reconsider bundling ROCm inside the installer. The gfx120X archive alone is 657 MB compressed and 949 MiB on disk; adding RDNA3 (gfx110X) is another 506 MB compressed. A first-run download of the pinned, hash-verified archive, as Lemonade does, keeps the installer small and lets the pin move without a new installer. If it must be bundled, ship gfx120X only and state that RDNA3 and RDNA2 use Vulkan.
- Document Windows 10 as best-effort for ROCm (AMD lists only Windows 11 25H2 for ROCm 10.x) and state that the supported path on Windows 10 is Vulkan.
- Do not take DLLs from an installed HIP SDK; its EULA was not reviewed. Use the TheRock-based artifacts only. Have someone open the HIP SDK licence page and RN-RAD-ROCM-10-01 in a browser if those terms matter to the project.

## Risks

- The lemonade archives are built against alpha TheRock nightlies (10.3.0a…), not a stable ROCm release, and the gfx120X archive is never run on real RDNA4 hardware before publishing. A pinned build can contain an RDNA4-only regression, as the open issue #155 on the RX 9070 XT shows.
- Supply-chain trust: the lemonade release has `checksums.txt` and qualification JSON in the same release but, as far as I could see, no independent build attestation, and the README says structure and support are subject to change. Upstream llama.cpp has attestations but its ROCm zip is unusable alone. The app must pin hashes itself and be ready for asset names to change.
- iGPU hazard on the owner's 7900X + RX 9070 XT: if masking is skipped or the wrong index is chosen, llama.cpp can place layers or KV cache on the gfx1036 iGPU (which reports about 50 GB of shared memory) and crash at warm-up. The gfx120X archive has no gfx1036 device code at all, so the failure mode there may differ and is untested.
- Silent failure: a missing DLL, a System32 HIP runtime mismatch or a too-old driver gives exit code 0, "(none)" devices and a working but CPU-only server. Without the post-load check, users would see only slow output.
- Driver coupling: the Adrenalin driver ships its own HIP runtime in System32 and AMD now issues ROCm-specific driver packages. A future driver update could break a pinned user-mode runtime (issue #23152 began after a driver upgrade), and the minimum driver for the nightly runtime is not documented.
- Performance expectations: ROCm will be slower than Vulkan for generation on the RX 9070 XT (roughly 8–45% in the cited data). It only helps prompt-processing-heavy use, and that advantage is currently in doubt on gfx1201 because of issue #155.
- Size and installer limits: Vulkan + CPU + CUDA 13.5 (163 MB + 407 MB cudart) + ROCm gfx120X (657 MB) is about 1.3 GB compressed before any RDNA3 or CUDA 12 variant. NSIS-based installers are commonly cited as limited to about 2 GB; I did not verify that limit or Tauri's bundler behaviour in this research.
- Windows security features: AMD's guide says Smart App Control and Defender Application Guard can interfere with ROCm. The bundled ROCm DLLs may be unsigned (not checked), so the backend may be blocked on machines with Smart App Control enabled.
- Unverified items: I could not download or unpack any zip (github.com blocked by the session proxy), could not read amd.com pages (HIP SDK download list, HIP SDK EULA, driver release note RN-RAD-ROCM-10-01), and did not test anything on Windows. The registry/DXGI driver-version lookup and the Driver Store version thresholds rest on secondary sources or memory.
- Known correctness bugs remain in the HIP path: BF16/F16 models crash on gfx1201 Windows through rocBLAS, and multi-GPU HIP on Windows produced silently corrupted text under ROCm 10.0. The app should restrict ROCm to a single discrete GPU and to quantised GGUF models unless tested otherwise.

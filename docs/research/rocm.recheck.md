# ROCm on Windows — re-check and recommendation

## Summary

Skeptic pass, 2026-11-10. I re-checked the decision-critical claims against upstream release assets, source at tag b12242, AMD and NVIDIA documents, and a real makensis run. Nothing was run on Windows or on a GPU, and the repository (which contains only .git) was not touched.

**What holds.** Both researchers' core facts are correct: asset names and byte sizes, the missing `win-hip-radeon` asset, the upstream ROCm zip not being self-contained, the lemonade gfx120X zip being self-contained, RX 9070 XT (gfx1201) support in ROCm 10.0/10.1 on Windows 11 25H2 only, and permissive ROCm licences.

**What I correct.**
1. "All four backends cannot go in one NSIS installer" is too strong. I built test installers with makensis 3.09:
   - solid LZMA (Tauri's default), 2.74 GB payload: compiler crashes;
   - solid LZMA, 1.97 GB: builds;
   - non-solid LZMA, 2.74 GB: builds (600 MB exe).
   So four backends fit either with CUDA 13.5 + lemonade ROCm under the stock template (1.82 GB unique payload), or with CUDA 12.4 under a custom non-solid template. The installer would be about 1.0–1.2 GB.
2. ROCm can legally be bundled. AMD's own deployment guide recommends that ISVs ship the HIP libraries in their installer. NVIDIA's CUDA EULA v13.5 lists cudart, cublas and cublasLt as redistributable.

**What both researchers missed.**
- No binary I checked carries an Authenticode signature: upstream llama.cpp, AMD's ROCm DLLs, all 73 PE files in the lemonade pack, and `cudart64_12.dll`.
- The lemonade pack's longest relative path is 140 characters, so the install root must stay under about 118 characters (MAX_PATH).
- The lemonade build hard-links `ggml-hip.dll` into `ggml.dll`; it is not a plugin build and must live in its own directory.
- `--device <name>` with an unknown name is a hard error, which turns silent CPU fallback into a detectable failure.
- tauri-plugin-updater buffers the whole installer in RAM, and GitHub caps release files at 2 GiB.

**Recommendation.** Put Vulkan + CPU in the installer and deliver CUDA 12.4 and ROCm (lemonade gfx120X) as pinned, hash-verified backend packs, each a complete directory with its own `llama-server.exe`. Keep Vulkan the default on AMD and ROCm opt-in. If the owner insists on all four inside the installer, use one of the two configurations above. If ROCm proves impractical, ship without it: Vulkan generates tokens faster on the RX 9070 XT in the Windows data I checked.

My working files are in <temp workspace> (NSIS results in <temp workspace> zip listings in <temp workspace>).

## Findings

### Upstream Windows x64 asset names and byte sizes at b12242 are exactly as Researcher A reported; Researcher B's smaller figures are the same sizes in MiB.

Confidence: high

My own HEAD requests and central-directory listings:
- llama-b12242-bin-win-cpu-x64.zip: 24,931,401 bytes (52 files, 64,352,541 unpacked)
- llama-b12242-bin-win-vulkan-x64.zip: 37,891,857 (106,358,557 unpacked; adds ggml-vulkan.dll 42,006,016)
- llama-b12242-bin-win-cuda-12.4-x64.zip: 288,964,446 (adds ggml-cuda.dll 614,017,024)
- cudart-llama-bin-win-cuda-12.4-x64.zip: 391,443,627 (cudart64_12.dll 553,984; cublas64_12.dll 100,033,536; cublasLt64_12.dll 473,551,360)
- llama-b12242-bin-win-cuda-13.5-x64.zip: 170,994,072 (adds ggml-cuda.dll 160,740,864)
- cudart-llama-bin-win-cuda-13.5-x64.zip: 426,314,199 (cudart64_13.dll 551,024; cublas64_13.dll 55,212,656; cublasLt64_13.dll 499,102,832)
- llama-b12242-bin-win-rocm-10.0-x64.zip: 267,536,302 (1,258,678,045 unpacked)

The 52 base files have identical CRC-32 in the cpu, vulkan, cuda-12.4, cuda-13.5 and rocm zips, so each backend zip is the CPU zip plus its backend DLLs. B's "255 MB" and "657 MB" are GitHub's MiB display of 267,536,302 and 688,519,474 bytes; there is no real disagreement. I could not open the release's asset page or the GitHub API for ggml-org (session proxy returned 403), so I probed explicit asset names rather than enumerating the release.

Source: HEAD and HTTP range requests on https://github.com/ggml-org/llama.cpp/releases/download/b12242/<asset>; listings saved under <temp workspace>

### There is no win-hip-radeon asset any more, and asset names change with CI toolkit bumps.

Confidence: high

- b12242 win-hip-radeon: 404.
- b10355 llama-b10355-bin-win-hip-radeon-x64.zip: 200, 324,764,934 bytes. It is the last self-contained upstream AMD build (2,024 files, 1,296,725,819 bytes unpacked, includes gfx1201 kernels), but it is from 2026-08-10.
- b10356: hip-radeon 404; win-rocm-7.14 exists (196,498,977).
- b10767: win-rocm-10.0 exists (244,211,806).
- b12242: no win-rocm-10.1 and no cuda-11.x asset.
- Semver tag v0.9.0 is the same commit (cd959bae) as build tag b12110. That build ships win-cuda-13.4, not 13.5, which shows the churn in a nine-day window. The cuda-12.4, cpu and vulkan names are the same at b12110 and b12242.

Source: HEAD requests on release download URLs; git ls-remote --tags https://github.com/ggml-org/llama.cpp

### The upstream win-rocm-10.0 zip cannot work on a clean PC: ggml-hip.dll imports hipblas.dll, which is not in the zip.

Confidence: high

The zip adds only four files to the base: ggml-hip.dll (1,055,838,720), amd_comgr.dll (121,750,016), amdhip64_7.dll (16,555,520) and rocm_kpack.dll (181,248). There is no hipblas, rocblas, rocsolver, hipblaslt, .kpack or kernel directory.

I extracted ggml-hip.dll and read its import table: ggml-base.dll, hipblas.dll, amdhip64_7.dll plus the MSVC runtime.

The release workflow comment says "rocblas/hipblaslt kernels resolve fine via PATH and are not copied." The PR author (slojosic-amd) wrote that users "still have to install appropriate ROCm 7.14/10.0 build and to add ROCM_BIN path to system's PATH." Issue #26964 is still open, last comment 2026-10-23, with an RX 9070 XT user confirming models load to RAM while Vulkan works. I found no upstream PR that bundles the full runtime, but I only read the first page of PR search results.

Source: remote zip listing; llvm-objdump -p on the extracted DLL; .github/workflows/release.yml at b12242 (lines 940-1100); github.com/ggml-org/llama.cpp/issues/26964, /issues/26929, /pull/26973

### lemonade-sdk/llamacpp-rocm b1372 gfx120X is self-contained for the RX 9070 XT, but it is an alpha-ROCm nightly that was never run on RDNA4 hardware before publishing.

Confidence: high

llama-b1372-windows-rocm-gfx120X-x64.zip:
- 688,519,474 bytes; SHA-256 a98bbeee2e130ccffe73bea94492e097ac1dc753dcd7b7c1a42479dbe29ba897 (I downloaded and hashed it; it matches the release's checksums.txt).
- 889 zip entries = 882 files + 7 directory entries, which resolves the 889 vs 882 discrepancy between the researchers.
- 995,337,813 bytes unpacked: root files 371,415,627; .kpack 193,793,456; hipblaslt/library/gfx1200 240,219,569; hipblaslt/library/gfx1201 169,630,485; rocblas/library 20,278,676.

A static import-closure walk from llama-server.exe finds every dependency inside the folder except MSVCP140.dll, VCRUNTIME140.dll and VCRUNTIME140_1.dll.

The qualification record says rocm_version 10.3.0a20261109, llama.cpp commit 0eebb6edd (build 12228), hardware_validated: false, static checks only. Issue #155 (RX 9070 XT, Windows 10, prompt processing down from about 1100 to 400-480 t/s since b1357) is open; AMD's reply on 2026-11-05 was "Unable to repro on gfx11. Might be specific to gfx12." The AMD-sponsored Lemonade app itself pins rocm-nightly to b1365, not latest.

Source: https://github.com/lemonade-sdk/llamacpp-rocm/releases/download/b1372/ (zip, checksums.txt); qualification/latest/windows-rocm-gfx120X.json on the qualification-data branch; github.com/lemonade-sdk/llamacpp-rocm/issues/155; lemonade-sdk/lemonade backend_versions.json

### New: the lemonade build is not a plugin build, so it must live in its own directory and fails differently from upstream.

Confidence: high

In the lemonade zip, ggml.dll imports ggml-cpu.dll, ggml-hip.dll and ggml-rpc.dll directly. Upstream's ggml.dll imports only ggml-base.dll and loads backends at run time.

Consequences:
- lemonade DLLs cannot be mixed with the upstream base (different commit and different linking).
- If ggml-hip.dll or one of its dependencies cannot load, llama-server.exe fails to start at all (a loader error) instead of quietly listing no devices. The probe must treat a non-zero exit or a start failure as "ROCm unavailable".
- It ships one generic ggml-cpu.dll, not upstream's 14 CPU variants, so it is not a good CPU fallback.

Source: llvm-objdump -p on ggml.dll from both zips

### RX 9070 XT (gfx1201) is officially supported by ROCm on Windows, but only on Windows 11 25H2.

Confidence: high

AMD's ROCm 10.0.0 and 10.1.0 compatibility matrices both list "9070 XT (gfx1201)", "Supported Windows version: Windows 11 25H2" and Adrenalin 26.8.1 (10.0.0) or 26.10.41.05 (10.1.0). Windows 10 is not listed.

AMD's stable wheel index has Windows wheels for 10.0.0 and 10.1.0, including rocm_sdk_device_gfx1201. Upstream's build targets include gfx1201, and the lemonade pack contains gfx1200 and gfx1201 kernels. AMD's install guide also says to remove any HIP SDK install and to turn off Smart App Control and Defender Application Guard "as they can interfere with ROCm functionality".

Source: https://rocm.docs.amd.com/en/docs-10.0.0/compatibility/compatibility-matrix.html ; https://rocm.docs.amd.com/en/docs-10.1.0/compatibility/compatibility-matrix.html ; https://rocm.docs.amd.com/en/docs-10.1.0/install/rocm.html ; https://stable.repo.amd.com/rocm/whl-next/

### The ROCm runtime may legally be bundled, and so may the CUDA runtime DLLs, subject to notices and conditions.

Confidence: medium

ROCm: AMD's licence table lists HIP, hipamd, CLR, hipBLAS, hipBLASLt, rocBLAS and Tensile as MIT, rocSOLVER as BSD-2-Clause and Comgr as University of Illinois/NCSA. AMD's deployment guide says: "We recommend that ISV applications using the HIP SDK libraries distribute the HIP SDK Library as part of its installer package." It also says applications should use the HIP runtime from the driver; upstream and lemonade nevertheless ship amdhip64_7.dll next to the exe so it wins over the System32 copy.

CUDA: EULA v13.5 section 1.1.2 requires that "Your application must have material additional functionality, beyond the included portions of the SDK" and that "The distributable portions of the SDK shall only be accessed by your application." Attachment A lists cudart.dll and "cublas.dll, cublasLt.dll" for Windows, including version-suffixed names. Section 1.2 forbids modifying the SDK. I read the current v13.5 text, not the archived 12.4-era EULA.

Neither the lemonade zip (one 1,099-byte LICENSE) nor the upstream zips carry ROCm or NVIDIA notices, so the app must add its own third-party notices file. This is my reading, not legal advice. I did not read the HIP SDK installer EULA, which is irrelevant if only TheRock-based artifacts are used.

Source: https://rocm.docs.amd.com/en/docs-10.1.0/about/license.html ; https://rocm.docs.amd.com/projects/install-on-windows/en/latest/conceptual/deployment-guidelines.html ; https://docs.nvidia.com/cuda/eula/index.html

### Correction: the NSIS 2 GiB limit applies to uncompressed payload only in solid mode; four backends can fit in one installer.

Confidence: high

Tauri's template uses `SetCompressor /SOLID "{{compression}}"` (or `SetCompress off` for "none"), bundles NSIS 3.13, and accepts a custom template via bundle.windows.nsis.template. NSIS source at tag v313 says "NSIS offset limit is 31 bits right now"; in solid mode files are appended raw to the datablock.

I ran makensis 3.09 (Ubuntu package, extracted locally) on dummy payloads:
- solid LZMA, 2,743,407,104 bytes: crash (SIGBUS, exit 135) after 5 s, no installer.
- SetCompress off, same payload: crash.
- solid LZMA, 1,969,855,744 bytes: success, 304,411,286-byte installer.
- non-solid LZMA (`SetCompressor lzma`), 2,743,407,104 bytes: success, 600,401,636-byte installer.

So Researcher A's reading of the source is right for Tauri's stock template, but the conclusion "cannot fit" is wrong in general. I tested compilation on Linux only; I did not run the resulting >2 GiB non-solid installer on Windows.

Source: raw.githubusercontent.com/tauri-apps/tauri/dev/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi and mod.rs; raw.githubusercontent.com/kichik/nsis/v313/Source/mmap.h, mmap.cpp, build.cpp; local makensis runs logged in <temp workspace>

### Exact uncompressed bundle sizes against the solid-mode limit of 2,147,483,647 bytes.

Confidence: high

Building blocks: base 64,352,541; ggml-vulkan.dll 42,006,016; CUDA 12.4 set 1,188,155,904; CUDA 13.5 set 715,607,376; lemonade gfx120X 995,337,813; upstream ROCm DLLs 1,194,325,504; AMD 10.0.0 BLAS runtime for gfx1201 282,562,100 (I re-listed the two AMD wheels and A's file sizes are exact).

Unique-file totals:
- CPU + Vulkan: 106,358,557 (fits)
- CPU + Vulkan + CUDA 12.4: 1,294,514,461 (fits)
- CPU + Vulkan + lemonade ROCm: 1,101,696,370 (fits)
- CPU + Vulkan + CUDA 13.5 + lemonade ROCm: 1,817,303,746 (fits, 330 MB spare); as four complete directories 1,946,008,828 (fits, 201 MB spare)
- CPU + Vulkan + CUDA 12.4 + lemonade ROCm: 2,289,852,274 (over by 142 MB); as four complete directories 2,418,557,356
- CPU + Vulkan + CUDA 12.4 + upstream ROCm + AMD runtime: 2,771,402,065 (over by 624 MB)

The app's own files (exe, WebView2 bootstrapper) come on top.

Source: sums over my remote zip and wheel listings

### A four-backend installer would be roughly 1.0-1.2 GB; a Vulkan + CPU installer adds only about 25-45 MB.

Confidence: medium

Measured: the whole lemonade gfx120X pack compresses to 619,225,780 bytes with xz -6 (688.5 MB as zip). Its kernel and kpack files alone are 561,732,848 compressed, so they barely shrink.

Not re-measured (Researcher A's xz figures): CUDA 12.4 set about 363 MB; ggml-cuda.dll 13.5 about 137 MB. The cudart 13.5 zip is 426 MB deflated and I have no LZMA figure for it. Base is 24.9 MB deflated and ggml-vulkan.dll about 4.4 MB with xz.

Estimates: CUDA 12.4 + ROCm non-solid about 1.0-1.1 GB; CUDA 13.5 + ROCm solid up to about 1.2 GB. The CUDA 12.4 directory installed on disk is 1,252,508,445 bytes and the ROCm directory 995,337,813.

Source: local xz -6 run on the extracted lemonade zip; Researcher A's xz measurements; zip listings

### New: none of the bundled third-party binaries is Authenticode-signed.

Confidence: medium

The PE security directory is empty in every file I checked:
- upstream llama-server.exe, llama-server-impl.dll, ggml.dll, ggml-base.dll, ggml-vulkan.dll, ggml-hip.dll, amdhip64_7.dll, amd_comgr.dll, rocm_kpack.dll;
- cudart64_12.dll from upstream's cudart zip;
- all 73 .exe/.dll files in the lemonade pack.
I did not check ggml-cuda.dll, cublas64_12.dll or cublasLt64_12.dll.

Tauri's NSIS bundler signs every unsigned .exe/.dll resource when signing is configured (should_sign then try_sign). Binaries shipped inside the installer would therefore get the owner's signature, including NVIDIA's DLLs, which sits awkwardly with the EULA's no-modify clause. Packs downloaded at run time stay unsigned unless the owner's CI signs and re-hosts them. Combined with AMD's Smart App Control warning, unsigned GPU DLLs are a real blocking risk on Windows 11; I did not test Smart App Control behaviour.

Source: PE header parsing of files extracted from the release zips; raw.githubusercontent.com/tauri-apps/tauri/dev/crates/tauri-bundler/src/bundle/windows/sign.rs and nsis/mod.rs

### New: the ROCm pack has a path-length budget; the install root must stay short.

Confidence: medium

The longest relative path in the lemonade gfx120X zip is 140 characters (hipblaslt/library/gfx1201/TensileLibrary_..._gfx1201.dat.zlib); the longest rocBLAS path is 102. With the 259-usable-character MAX_PATH limit, the directory holding the pack must be at most about 118 characters (156 if only rocBLAS files matter).

The AMD-sponsored Lemonade app has explicit code for this: it measures the rocBLAS path depth and refuses with "ROCm cannot run from this cache directory ... needs it to be N or fewer." A default per-user install path is normally well inside the budget, but a custom or deep install directory is not. I did not test at what length loading actually fails.

Source: my listing of the lemonade zip; raw.githubusercontent.com/lemonade-sdk/lemonade/main/src/cpp/server/backends/backend_utils.cpp

### New: a large installer is costly for auto-update, and GitHub caps release files at 2 GiB.

Confidence: high

tauri-plugin-updater's download() appends every chunk to a Vec<u8>, verifies the signature on that buffer, then writes a temp file. A 1.0-1.2 GB installer is therefore held fully in RAM and re-downloaded on every app update. GitHub's documentation: "Each file included in a release must be under 2 GiB." Both four-backend configurations stay under that cap.

Source: raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/updater/src/updater.rs (lines 819-893); https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases

### Backend loading and device selection behave as the researchers described, with two additions useful for the design.

Confidence: high

Confirmed in source at b12242:
- Load order is cuda, hip, ..., vulkan, ..., cpu. Release builds are silent when a backend DLL fails to load.
- `--list-devices` prints "Available devices:" then "  (none)" or "  %s: %s (%zu MiB, %zu MiB free)" per non-CPU device and exits 0.
- Default selection skips a second device with the same device id, so explicit selection is needed when two backends see one card.

Additions:
- parse_device_list throws "invalid device: <name>" when the named device does not exist. Launching with an explicit `--device Vulkan0`, `CUDA0` or `ROCm0` therefore fails fast instead of silently running on CPU. `--device none` disables offload.
- The loader searches the executable directory and the current directory, and also loads one extra backend from the GGML_BACKEND_PATH environment variable. A shared base with per-backend plugin folders is therefore possible in principle. I did not test it on Windows, and it does not help HIP, whose runtime DLLs must sit in the exe directory to beat the System32 copies.

Source: common/arg.cpp (lines 1126-1171, 2708-2723), ggml/src/ggml-backend-reg.cpp (lines 480-600), ggml-backend-dl.cpp, src/llama.cpp (lines 186-260) at b12242

### CUDA choice resolved: 12.4 for breadth; 13.5 only if everything must fit Tauri's stock solid installer.

Confidence: medium

ggml-cuda/CMakeLists.txt builds 50-virtual, 61-virtual, 70-virtual, 75-virtual, 80-virtual, 86-real, 89-real, 90-virtual for CUDA below 13. For 13.x it drops 50/61/70 and adds 120a-real and 121a-real.

NVIDIA's release notes give minor-version-compatibility driver floors of ">= 525" for 12.x and ">= 580" for 13.x, and ">=551.61" for 12.4 GA on Windows.

So the 12.4 build covers GTX 900 onward and older drivers but is 1.19 GB on disk. The 13.5 build is 0.72 GB on disk but needs Turing or newer and driver 580+. RTX 50 cards on the 12.4 build rely on PTX JIT; that is an inference nobody tested, and the owner has no NVIDIA card.

Source: ggml/src/ggml-cuda/CMakeLists.txt at b12242; https://docs.nvidia.com/cuda/cuda-toolkit-release-notes/index.html; zip listings

### On the RX 9070 XT under Windows, Vulkan generates tokens faster than HIP while HIP processes prompts faster; HIP also has an iGPU hazard that matters on a Ryzen desktop.

Confidence: medium

Issue #26663 (Windows 11, RX 9070 XT, b10298), tg128 Vulkan vs HIP: 571.9 vs 292.2 (Qwen3-0.6B), 321.8 vs 221.7 (1.7B), 174.5 vs 129.2 (4B), 104.3 vs 91.1 (8B). pp512: 5223 vs 6671 (4B) and 2866 vs 4199 (8B). The original "5-7x slower Vulkan" claim was withdrawn by its author.

Issue #23152 (Ryzen 7 9850X3D + RX 9070 XT, Windows): the log showed "ROCm0 : AMD Radeon(TM) Graphics (50246 MiB...)" and "ROCm1 : AMD Radeon RX 9070 XT (16304 MiB...)", then crashed with "ggml_cuda_compute_forward: MUL_MAT failed". `-dev ROCm1` alone did not help; `HIP_VISIBLE_DEVICES=1` did, and the maintainer replied "Yes, this is the correct fix." The string HIP_VISIBLE_DEVICES is present in lemonade's amdhip64_7.dll.

I did not re-verify B's other benchmark sources, the maintainer quote that HIP "is not expected to be faster", or the revert in PR #28604.

Source: github.com/ggml-org/llama.cpp/issues/26663 and /issues/23152; strings on amdhip64_7.dll

### Self-assembling a ROCm pack from the upstream zip plus AMD's stable 10.0.0 wheels is possible on paper but riskier and much larger than the lemonade pack.

Confidence: low

It needs the upstream ROCm DLLs (1,194,325,504 bytes, of which ggml-hip.dll is 1.06 GB because it carries 20 GPU targets) plus 282,562,100 bytes of hipblas/rocblas/rocsolver/hipblaslt/origami DLLs, the blas kpack and the kernel directories. That is about 1.48 GB against lemonade's 0.995 GB.

Field reports in issue #26964 are mixed: one RX 7900 XTX user added the DLLs from TheRock and still saw no GPU; another got RDNA4 working by copying the whole TheRock 10.1.0 bin directory into the llama.cpp folder. The lemonade DLLs show the runtime looks for ".kpack/blas_lib_@GFXARCH@.kpack" and honours ROCBLAS_TENSILE_LIBPATH, HIPBLASLT_TENSILE_LIBPATH and ROCM_KPACK_PATH, so a layout can be forced, but nobody in this research validated one.

Source: listings of rocm_sdk_libraries-10.0.0 and rocm_sdk_device_gfx1201-10.0.0 Windows wheels; github.com/ggml-org/llama.cpp/issues/26964; strings on lemonade rocblas.dll, libhipblaslt.dll, rocm_kpack.dll

### Every llama.cpp binary needs the Microsoft VC++ runtime, which no zip includes.

Confidence: medium

llama-server-impl.dll, ggml*.dll, ggml-vulkan.dll, ggml-hip.dll and all the ROCm DLLs import MSVCP140.dll, VCRUNTIME140.dll and/or VCRUNTIME140_1.dll. The upstream base does include libomp.dll. Tauri's NSIS installer does not install the VC++ redistributable by default; that last point and the permission to ship those three DLLs app-local are from my memory, not checked in this session.

Source: llvm-objdump -p on extracted binaries; static import-closure walk of the lemonade pack

## Recommendations

- Sourcing: pin one upstream ggml-org/llama.cpp build for CPU, Vulkan and CUDA. b12110 is the build behind tag v0.9.0; b12242 is the newest. Take `llama-<tag>-bin-win-cpu-x64.zip`, `llama-<tag>-bin-win-vulkan-x64.zip`, `llama-<tag>-bin-win-cuda-12.4-x64.zip` and `cudart-llama-bin-win-cuda-12.4-x64.zip`. Record URL, byte size and SHA-256 per asset in a manifest checked by the build; never follow 'latest', because names such as cuda-13.4/13.5 and rocm-7.14/10.0 change.
- ROCm sourcing: use `lemonade-sdk/llamacpp-rocm`, asset `llama-<tag>-windows-rocm-gfx120X-x64.zip`, pinned by hash (b1372: 688,519,474 bytes, SHA-256 a98bbeee2e130ccffe73bea94492e097ac1dc753dcd7b7c1a42479dbe29ba897). Do not use upstream `win-rocm-10.0` on its own. Before fixing the pin, benchmark b1356 against b1372 on the owner's RX 9070 XT because of the open prompt-processing regression (lemonade issue #155).
- Layout: one complete directory per backend, each with its own `llama-server.exe` and DLLs, used exactly as the upstream or lemonade zip unpacks. Ship them through Tauri `bundle.resources` (not `externalBin`) and spawn by absolute path with the working directory set to that folder. Do not mix lemonade DLLs with upstream ones. Keep the ROCm root path under about 118 characters and check that at run time.
- Packaging (preferred): put `cpu/` (64,352,541 bytes) and `vulkan/` (106,358,557 bytes) in the installer, and deliver CUDA 12.4 (1,252,508,445 bytes installed) and ROCm gfx120X (995,337,813 bytes installed) as backend packs downloaded on first run or from settings into the per-user app-data folder. Offer the pack that matches the detected GPU vendor. Host the packs on the app's own release channel, verify SHA-256 before unpacking, and sign the unsigned llama.cpp and ROCm binaries in CI if the app is code-signed; leave NVIDIA's DLLs unmodified.
- If all four must be inside the installer, use one of two configurations. (a) Tauri's stock template: CPU + Vulkan + CUDA 13.5 + lemonade ROCm, 1,817,303,746 bytes unique payload (1,946,008,828 as four full directories), accepting that GTX 10-series and drivers below 580 fall back to Vulkan. (b) A custom NSIS template with non-solid `SetCompressor lzma` via `bundle.windows.nsis.template`, which allows CUDA 12.4 (2.29-2.42 GB payload). Expect a 1.0-1.2 GB installer either way, and do one real Windows install of the result before committing to it.
- Write the backend manager so it only sees 'a backend directory plus a manifest entry', regardless of whether the directory came from the installer or from a downloaded pack. That keeps the choice between a full installer and a small installer with packs a packaging decision, not a code change, and lets the ROCm pin move without a new app release.
- Detection: enumerate display adapters first (vendor 0x10DE NVIDIA, 0x1002 AMD, 0x8086 Intel) only to decide which backends are candidates. Then probe each candidate directory with `llama-server.exe --list-devices` under a 15-second timeout and parse stdout with `^  (Vulkan|CUDA|ROCm)(\d+): (.+) \((\d+) MiB, (\d+) MiB free\)$`. Mark a backend unusable on a start failure, non-zero exit, timeout, "(none)", a missing prefix or 0 MiB. Cache the result per backend version and GPU driver version.
- Launch: always pass an explicit `--device <Name>` (or `--device none` for CPU). An unknown device name is a hard error in llama.cpp, so a backend that failed to load exits at once instead of silently serving from CPU. For ROCm, pick the `ROCmN` entry that matches the discrete card (name and about 16 GB), set `HIP_VISIBLE_DEVICES=N`, re-probe to confirm a single device, then launch with `--device ROCm0`. With Vulkan on an iGPU + dGPU machine, pass the discrete card's `VulkanN` explicitly too.
- Fallback chain: NVIDIA: CUDA, then Vulkan, then CPU. AMD: Vulkan by default; ROCm only as an opt-in labelled as faster prompt processing; on any ROCm failure go to Vulkan, then CPU. Intel and others: Vulkan, then CPU. Treat a process exit, a health-check timeout or implausibly low throughput after load as a failure, step down one level, and remember the failure for that backend version and driver version. Show the active backend and device name in the UI and allow a manual override.
- Gate the ROCm option: Windows 11 only (AMD lists only Windows 11 25H2 for ROCm 10.x), an RX 9000-series card for the gfx120X pack, a single discrete GPU, and quantised GGUF models. On Windows 10, and on RDNA2/RDNA3 cards unless further lemonade packs (gfx110X, gfx103X, about 530 MB each zipped) are added, the supported AMD path is Vulkan.
- If ROCm cannot practically be bundled (size, an unstable nightly, Smart App Control blocks, path length): ship without it and keep Vulkan as the AMD backend. Offer ROCm only as an optional pack, or as an advanced 'use my installed ROCm' mode that runs the upstream `win-rocm-10.0` build with the user's ROCm bin directory on PATH. Do not fall back to requiring AMD's 2.28 GB tarball for normal users. There is no legal blocker: the ROCm components are MIT, BSD-2-Clause and Apache-2.0-with-LLVM-exception/NCSA.
- Ship a THIRD-PARTY-NOTICES file covering llama.cpp (MIT), LLVM OpenMP (libomp.dll), the ROCm components (HIP/CLR, rocBLAS, hipBLAS, hipBLASLt, Tensile, rocSOLVER, Comgr/LLVM) and a reference to the NVIDIA CUDA EULA. Make sure the VC++ 2015-2022 x64 runtime is present, either app-local in each backend directory or installed by the installer.
- Validate before writing the design down as fixed: (1) on the RX 9070 XT, run the lemonade pack's `--list-devices`, a model load with the iGPU masked, and llama-bench against Vulkan; (2) do one `tauri build` with the ROCm directory as a resource and confirm the `.kpack` dot-directory and the 140-character kernel paths survive into the installed tree; (3) find an NVIDIA tester for the CUDA pack, since the owner has no NVIDIA card.

## Risks

- Nothing was executed on Windows, on the RX 9070 XT or on any NVIDIA GPU. Device listings, ROCm behaviour and CUDA behaviour on RTX 50 cards come from source code, binary inspection, AMD/NVIDIA documents and GitHub issue reports.
- The NSIS tests used makensis 3.09 on Linux with dummy files. Tauri uses NSIS 3.13 on Windows; the limiting code is the same at tag v313, but I did not install the >2 GiB non-solid test installer on Windows, and under the stock solid template the CUDA 13.5 + ROCm configuration has only 200-330 MB of headroom, which a larger lemonade nightly could consume.
- The lemonade ROCm pack is built on alpha ROCm (10.3.0a) nightlies, is not run on RDNA4 hardware before release, has an open RX 9070 XT performance regression, and comes with no support commitment or independent build attestation. The app must pin by hash and be ready to hold back or drop the ROCm backend.
- All third-party binaries I checked are unsigned. Smart App Control or antivirus may block them, especially packs downloaded at run time. Signing them through Tauri fixes that for llama.cpp and ROCm files but would also modify NVIDIA's DLLs, which the CUDA EULA's no-modify clause may not allow; this needs a decision by the owner, not an engineering guess.
- A deep install or app-data path can push ROCm kernel files past MAX_PATH (the pack needs a root of at most about 118 characters). The exact failure behaviour was not tested.
- On a Ryzen desktop with an iGPU plus the RX 9070 XT, an unmasked HIP runtime can list the iGPU as a ROCm device with about 50 GB of memory and crash at model load. The gfx120X pack contains no gfx1036 code, so its behaviour with that iGPU present is unknown until tested on the owner's machine.
- ROCm on Windows is officially supported only on Windows 11 25H2 with Adrenalin 26.8.1 or the ROCm-specific 26.10.41.05 driver. A bundled HIP runtime can break after a driver update, and no minimum driver is published for the nightly runtime.
- A four-backend installer of 1.0-1.2 GB is re-downloaded and held in RAM by tauri-plugin-updater on every update. Installed size is 2.3-2.8 GB for all four backends.
- CUDA cannot be tested by the owner. The 12.4 build's coverage of RTX 50 cards rests on PTX JIT and is an inference; the 13.5 build drops GTX 10-series and needs driver 580 or newer.
- Upstream bNNNN releases are development builds published several times a day, and asset names embed toolkit versions that change (cuda-13.4 at b12110, cuda-13.5 at b12242; rocm-7.14 then rocm-10.0). A pin can contain regressions, and a re-pin can silently change asset names.
- Redistribution conclusions are my reading of the licence texts and the current CUDA EULA v13.5, not legal advice. I did not read the archived EULA that shipped with CUDA 12.4 or the HIP SDK installer EULA.
- Items I did not re-verify and took from the researchers: LZMA sizes for the CUDA DLLs, the PCI device-ID allow-list for RDNA4, the driver-store version thresholds, the maintainer statement that HIP is not expected to beat Vulkan, and the log lines proposed for a post-load ROCm check.
- I could not open the GitHub API or the release asset page for ggml-org/llama.cpp from this session (proxy 403), so asset existence was checked by probing explicit names; an asset with a name nobody guessed would have been missed.

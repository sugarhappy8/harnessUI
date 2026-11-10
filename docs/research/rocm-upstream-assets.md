# ROCm on Windows — upstream-assets

## Summary

Checked against llama.cpp build b12242 (published 2026-11-10, commit 3b4d6ab3, version 0.9.0-dev) by reading its release workflows, listing every Windows zip remotely, and inspecting the DLL import tables. Nothing was run on Windows or on an AMD/NVIDIA GPU, so runtime behaviour comes from source code and upstream issue reports.

Three results change the plan:

1. **There is no `win-hip-radeon-x64` asset any more.** It ended at b10355 (2026-08-10). The AMD asset is now `llama-b12242-bin-win-rocm-10.0-x64.zip`, built against ROCm 10.0.0 wheels (not the HIP SDK) for 20 targets including gfx1201 (RX 9070 XT).

2. **The upstream ROCm zip is not self-contained.** `ggml-hip.dll` imports `hipblas.dll`, which is not in the zip, and neither are `rocblas.dll`, `rocsolver.dll`, `libhipblaslt.dll`, `origami.dll` or the kernel files. Without them the backend silently fails to load and `--list-devices` prints `(none)`. Upstream tells users to install AMD's ROCm tarball (2.28 GB for gfx120X) and put it on PATH.

3. **All four backends cannot go in one Tauri NSIS installer.** Tauri builds with `SetCompressor /SOLID lzma`, and in solid mode NSIS holds the uncompressed payload in a 31-bit buffer (about 2.147 GB). CPU + Vulkan + CUDA 12.4 with its runtime + ROCm is 2.49 GB of unique uncompressed files, or 2.77 GB with the missing ROCm BLAS runtime. Compressed size is not the problem (roughly 0.7 GB with LZMA).

Other points:
- Every Windows zip is the same base (`llama-server.exe` stub plus DLLs plus CPU variants) with one backend DLL on top.
- CUDA 12.4 alone covers GTX 900 through RTX 50; CUDA 13.5 is not needed as well.
- `--list-devices` prints `  <Name><idx>: <description> (<total> MiB, <free> MiB free)` on stdout, with names `Vulkan0`, `CUDA0`, `ROCm0`.

Scratch outputs (asset listings, workflow copies) are in <temp workspace> the repository working directory was not touched.

## Findings

### Latest upstream build is b12242; GitHub releases are now dev/nightly builds, and asset names embed toolkit versions.

Confidence: high

Highest tag is b12242, commit 3b4d6ab3f79651935fba1dd93732f29022a7f5f5 (2026-11-10), assets uploaded 02:01-02:04Z. CMakeLists sets version 0.9.0 with a -dev suffix. release-publish.yml creates each bNNNN release with prerelease: true. docs/release.md says semver tags (latest v0.9.0, 2026-11-04, first contained in b12110) are tags only: 'Currently releases are not published to github releases, only nightly/development builds are available there'. Asset names carry the toolkit version (cuda-12.4, cuda-13.5, rocm-10.0, openvino-2026.4.1) and change when CI bumps it: CUDA 13 went 13.1 -> 13.3 -> 13.4 -> 13.5 (13.5 since b12169, 2026-11-06); ROCm went 7.14 -> 10.0.

Source: git ls-remote https://github.com/ggml-org/llama.cpp; raw.githubusercontent.com/ggml-org/llama.cpp/b12242/.github/workflows/release.yml and release-publish.yml; docs/release.md at b12242

### Complete list of Windows assets in b12242 with exact byte sizes (14 files; 9 are x64).

Confidence: high

x64:
- llama-b12242-bin-win-cpu-x64.zip: 24,931,401
- llama-b12242-bin-win-vulkan-x64.zip: 37,891,857
- llama-b12242-bin-win-cuda-12.4-x64.zip: 288,964,446
- cudart-llama-bin-win-cuda-12.4-x64.zip: 391,443,627 (no build tag in the name)
- llama-b12242-bin-win-cuda-13.5-x64.zip: 170,994,072
- cudart-llama-bin-win-cuda-13.5-x64.zip: 426,314,199
- llama-b12242-bin-win-rocm-10.0-x64.zip: 267,536,302
- llama-b12242-bin-win-sycl-x64.zip: 156,252,488
- llama-b12242-bin-win-openvino-2026.4.1-x64.zip: 104,927,492

arm64:
- llama-b12242-bin-win-cpu-arm64.zip: 13,281,295
- llama-b12242-bin-win-vulkan-arm64.zip: 25,818,815
- llama-b12242-bin-win-cuda-13.5-arm64.zip: 158,478,534
- cudart-llama-bin-win-cuda-13.5-arm64.zip: 278,821,062
- llama-b12242-bin-win-opencl-adreno-arm64.zip: 14,290,413

There is no CUDA 11 build and no x64 OpenCL build. Vulkan is built with Vulkan SDK 1.4.357.0.

Source: HTTP HEAD Content-Length on https://github.com/ggml-org/llama.cpp/releases/download/b12242/<asset>; asset names cross-checked with releases/expanded_assets/b12242 and release-publish.yml

### No win-hip-radeon-x64 asset exists today; it was replaced by win-rocm-<version>-x64 at b10356.

Confidence: high

llama-b12242-bin-win-hip-radeon-x64.zip returns 404.
- Last hip-radeon build: b10355 (324,764,934 bytes). It was built with the AMD HIP SDK installer 26.Q1 for gfx1150, gfx1151, gfx1200, gfx1201, gfx1100, gfx1101, gfx1102, gfx1030, gfx1031, gfx1032, and it was self-contained (rocblas.dll, libhipblas.dll, libhipblaslt.dll, rocblas/library with 892 files, hipblaslt/library with 1,079 files; 1,296,725,819 bytes unpacked).
- Commit 0666ad2b ('ci : target ROCm 7.14 for build and release', #25775, 2026-08-10) replaced the windows-hip job with windows-rocm. b10356 has llama-b10356-bin-win-rocm-7.14-x64.zip (196,498,977 bytes) and no hip-radeon asset.
- The name became win-rocm-10.0-x64 at b10767 (commit cff18443, #27803, 2026-09-02).
- The first commit adding a prebuilt Windows HIP binary was e948a7da (#9467, 2024-09-21, first tag b3797). I did not check what the asset was called that early.

Source: HEAD requests on b10355/b10356/b10661/b10767 assets; git log -S on .github/workflows/release.yml in a blobless clone; git show 0666ad2b

### The Windows ROCm asset is compiled against ROCm 10.0.0 wheels for 20 GPU targets, including gfx1200 and gfx1201.

Confidence: high

The windows-rocm job runs on windows-2022 and installs `rocm[libraries,devel]==10.0.0` from https://stable.repo.amd.com/rocm/whl-next/ via the windows-setup-rocm action. It builds only the ggml-hip target with ROCm's clang, with -DGGML_HIP=ON -DGGML_BACKEND_DL=ON -DGGML_NATIVE=OFF -DGGML_CPU=OFF.

AMDGPU_TARGETS = gfx1010; gfx1011; gfx1012; gfx1030; gfx1031; gfx1032; gfx1033; gfx1034; gfx1035; gfx1036; gfx1100; gfx1101; gfx1102; gfx1103; gfx1150; gfx1151; gfx1152; gfx1153; gfx1200; gfx1201.

AMD's compatibility matrix lists the RX 9070 XT as gfx1201. The bundled amdhip64_7.dll and rocm_kpack.dll are byte-identical (SHA-256) to those in rocm_sdk_core-10.0.0-py3-none-win_amd64.whl. ROCm 10.1.0 wheels already exist in the same index and the workflow has a comment about 10.1, so another rename is plausible.

Source: release.yml at b12242 (windows-rocm job, lines 940-1102); .github/actions/windows-setup-rocm/action.yml; sha256 comparison of DLLs from the zip and the AMD wheel; rocm.docs.amd.com compatibility matrix

### The ROCm zip unpacks to 1,258,678,045 bytes (56 files) and does NOT contain hipBLAS, rocBLAS or kernel files.

Confidence: high

Contents beyond the shared base:
- ggml-hip.dll: 1,055,838,720 bytes
- amd_comgr.dll: 121,750,016 (version 3.3.0.0)
- amdhip64_7.dll: 16,555,520 (file version 10.0.3581.0)
- rocm_kpack.dll: 181,248

There is no hipblas.dll, rocblas.dll, rocsolver.dll, libhipblaslt.dll, rocblas/library, hipblaslt/library or .kpack directory. ggml-hip.dll's PE import table lists amdhip64_7.dll and hipblas.dll, so loading fails unless hipblas.dll and its dependencies can be found.

The workflow comment says: 'rocblas/hipblaslt kernels resolve fine via PATH and are not copied'. PR #26973 states 'hipblas/rocblas/hipblaslt dlls and kernel files are not bundled with llama.cpp binaries'. Discussion #27047 (by a collaborator) tells Windows users to extract AMD's therock-dist-windows tarball to C:\TheRock\build and add C:\TheRock\build\bin to PATH. therock-dist-windows-gfx120X-all-10.0.0.tar.gz is 2,282,922,923 bytes. Issue #26929 shows the failure: the prebuilt ROCm binary printed 'Available devices: (none)' until ROCm was installed separately.

Source: remote central-directory listing of llama-b12242-bin-win-rocm-10.0-x64.zip; llvm-objdump -p on extracted DLLs; release.yml 'Bundle HIP runtime DLLs' step; github.com/ggml-org/llama.cpp/pull/26973, /discussions/27047, /issues/26929; HEAD on stable.repo.amd.com/rocm/core/tarball/

### The missing ROCm runtime for gfx1201 is about 283 MB unpacked and can be taken from AMD's ROCm 10.0.0 Windows wheels; the directory layout it needs is inferred, not tested.

Confidence: medium

Import chain: hipblas.dll -> rocblas.dll + rocsolver.dll; rocblas.dll -> amdhip64_7.dll + libhipblaslt.dll; libhipblaslt.dll -> origami.dll.

From rocm_sdk_libraries-10.0.0-py3-none-win_amd64.whl (116,831,755 bytes):
- hipblas.dll: 801,280 (hipBLAS 3.6)
- rocblas.dll: 21,020,160 (rocBLAS 5.6)
- rocsolver.dll: 24,983,040
- libhipblaslt.dll: 6,301,696
- origami.dll: 368,128

From rocm_sdk_device_gfx1201-10.0.0-py3-none-win_amd64.whl (312,303,802 bytes):
- .kpack/blas_lib_gfx1201.kpack: 42,924,354
- bin/rocblas/library: 110 files, 10,444,234
- bin/hipblaslt/library/gfx1201: 292 files, 175,719,208

Total 282,562,100 bytes, about 223 MB deflated (the kernel files barely compress).

Layout, from strings in the DLLs only: rocblas.dll 10.0.0 looks for '../.kpack/blas_lib_@GFXARCH@.kpack' (the parent of the DLL directory). rocm_kpack.dll reads ROCM_KPACK_PATH, ROCM_KPACK_PATH_PREFIX, ROCM_KPACK_DISABLE and ROCM_KPACK_DEBUG. rocBLAS reads ROCBLAS_TENSILE_LIBPATH and hipBLASLt reads HIPBLASLT_TENSILE_LIBPATH. I did not verify whether llama.cpp needs the hipblaslt kernel files at run time.

Source: remote listing of the two AMD wheels at https://stable.repo.amd.com/rocm/whl-next/; llvm-objdump -p and strings on the extracted DLLs

### lemonade-sdk/llamacpp-rocm ships self-contained Windows zips per GPU family, built on nightly (alpha) ROCm.

Confidence: high

Latest release b1372 (2026-11-09): ROCm 10.3.0a20261109, llama.cpp commit 0eebb6ed. The notes say 'Each archive bundles its ROCm runtime, so nothing else needs to be installed.'

Windows assets:
- llama-b1372-windows-rocm-gfx120X-x64.zip: 688,519,474 bytes (995,337,813 unpacked, 889 files)
- gfx110X: 530,911,618
- gfx103X: 530,883,357
- gfx1151: 193,706,021
- gfx1150, gfx1152, gfx1153, gfx117X, gfx908, gfx90a also exist (about 180-445 MB as displayed)

The gfx120X zip is flat: a 131,302,400-byte ggml-hip.dll (gfx1200/1201 only), amd_comgr.dll, amdhip64_7.dll, rocm_kpack.dll, hipblas.dll, rocblas.dll, rocsolver.dll, libhipblaslt.dll, origami.dll, libtensilelite-host.dll, a .kpack/ directory, rocblas/library and hipblaslt/library/gfx1200|gfx1201. Its build numbers (b1372) are its own, not llama.cpp's. Repo licence is MIT; the README says nothing about the licence of the bundled ROCm binaries.

Source: github.com/lemonade-sdk/llamacpp-rocm releases (expanded_assets/b1372), README; HEAD and remote zip listing of the gfx120X asset

### Every Windows zip is the same base plus one backend DLL, and backends are plugins loaded from the exe directory.

Confidence: high

release-publish.yml unzips the windows-cpu zip into every other llama-bin-win-*-x64.zip, so the base is identical across backends: 52 files, 64,352,541 bytes. It holds a 9,728-byte llama-server.exe stub that imports llama-server-impl.dll (10,158,592), plus llama.dll, llama-common.dll, mtmd.dll, ggml.dll, ggml-base.dll, libomp.dll and 14 ggml-cpu-*.dll variants (x64, sse42, sandybridge, ivybridge, piledriver, haswell, skylakex, cannonlake, cascadelake, cooperlake, icelake, alderlake, zen4, sapphirerapids). llama-server.exe cannot be shipped as a single file.

Backend additions:
- Vulkan: ggml-vulkan.dll, 42,006,016 (imports vulkan-1.dll from the GPU driver)
- CUDA 12.4: ggml-cuda.dll, 614,017,024 (imports cudart64_12.dll, cublas64_12.dll, nvcuda.dll)
- CUDA 13.5: ggml-cuda.dll, 160,740,864 (imports cublas64_13.dll)
- ROCm: the four DLLs listed in the ROCm finding

Loader behaviour (ggml-backend-reg.cpp): searches the executable directory and the current directory; tries cuda, hip, ..., vulkan, ..., cpu in that order; in release builds a DLL that fails to load is skipped with no message and no error dialog. Both CUDA builds use the name ggml-cuda.dll, so they cannot share a directory. There is no command-line option to choose a backend directory. All binaries import MSVCP140.dll, VCRUNTIME140.dll and VCRUNTIME140_1.dll, which are not in the zips.

Source: release-publish.yml 'Merge artifacts' step; remote zip listings; llvm-objdump -p; ggml/src/ggml-backend-reg.cpp and ggml-backend-dl.cpp at b12242

### CUDA 12.4 alone covers the widest range of NVIDIA cards; shipping CUDA 13.5 as well is not needed.

Confidence: medium

ggml-cuda CMakeLists compiles, for CUDA below 13: 50-virtual, 61-virtual, 70-virtual, 75-virtual, 80-virtual, 86-real, 89-real, 90-virtual. That is Maxwell (GTX 900) through RTX 40 natively, with newer cards served by PTX JIT. For CUDA 13 and later it drops 50/61/70 and adds 120a-real and 121a-real, so the 13.5 build is Turing-or-newer only.

NVIDIA's release notes give the minor-version-compatibility driver floor as 525 or later for 12.x and 580 or later for 13.x (12.4 GA itself lists Windows driver 551.61 or later).

Runtime zips:
- cudart 12.4: cudart64_12.dll 553,984 (12.4.127); cublas64_12.dll 100,033,536 (12.4.5); cublasLt64_12.dll 473,551,360. Total 574,138,880 unpacked.
- cudart 13.5: cudart64_13.dll 551,024; cublas64_13.dll 55,212,656; cublasLt64_13.dll 499,102,832. Total 554,866,512 unpacked.

That RTX 50 cards work well through the 12.4 build's PTX path is my inference from the architecture list, not something I tested.

Source: ggml/src/ggml-cuda/CMakeLists.txt at b12242; .github/actions/windows-setup-cuda/action.yml; remote listing of cudart zips; DLL version resources; docs.nvidia.com/cuda/cuda-toolkit-release-notes

### Total size of a four-backend bundle: about 1.0 GB as upstream zips and 2.5-2.8 GB unpacked.

Confidence: high

Four upstream zips unpacked side by side (CPU, Vulkan, CUDA 12.4 + cudart, ROCm):
- download 1,010,767,633 bytes
- unpacked 2,681,897,588 bytes
- adding CUDA 13.5 + cudart: download 1,608,075,904; unpacked 3,461,857,505

Unique files only (base stored once):
- base 64,352,541 + ggml-vulkan 42,006,016 + CUDA 12.4 (614,017,024 + 574,138,880) + ROCm DLLs 1,194,325,504 = 2,488,839,965 bytes
- plus the gfx1201 BLAS runtime (282,562,100) = 2,771,402,065 bytes

Subsets under 2 GiB uncompressed: CPU + Vulkan + CUDA 12.4 is about 1.29 GB; CPU + Vulkan + ROCm with the gfx1201 runtime is about 1.58 GB. CUDA and ROCm together do not fit.

Source: sums of the per-file sizes from the remote zip and wheel listings above

### A Tauri NSIS installer cannot hold more than about 2 GiB of uncompressed payload, so the four-backend bundle does not fit even though it compresses well.

Confidence: medium

Tauri's installer.nsi template sets `SetCompressor /SOLID "{{compression}}"` (default lzma) and the bundler downloads NSIS 3.13.

In NSIS's build.cpp, add_db_data only compresses per file when solid mode is off; in solid mode files are appended raw to the datablock, whose length is a signed 32-bit int. mmap.cpp says: 'Right now the mapping is still limited to 31 bits.' The installer stub also tracks positions in the decompressed stream as int.

NSIS bug #1284, still open, is titled 'Unable to generate a solid compressed installer when size of installed files exceeds 2 GiB' and fails with 'Internal compiler error #12345: error mmapping file ... is out of range'. The NSIS maintainer's comment there is 'We are limited to 2gb of compressed data', so the uncompressed reading rests on the source code and the bug title; I did not run makensis to confirm it. The NSIS wiki pages returned 403 and were seen only through search snippets.

Source: raw.githubusercontent.com/tauri-apps/tauri/dev/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi and mod.rs; raw.githubusercontent.com/kichik/nsis/master/Source/build.cpp, mmap.cpp, exehead/fileform.c; sourceforge.net/p/nsis/bugs/1284

### The large backend DLLs compress far better with LZMA than in the upstream zips, so on-demand download packs can be much smaller.

Confidence: medium

Measured with `xz -6` (8 MiB dictionary, the same as NSIS's LZMA default):
- ggml-hip.dll: 1,055,838,720 -> 28,427,644 (195,249,216 in the upstream zip)
- ggml-cuda.dll 12.4: 614,017,024 -> 104,427,796 (263,279,224 in the zip)
- ggml-cuda.dll 13.5: 160,740,864 -> 137,165,124
- amd_comgr.dll: 121,750,016 -> 29,903,916
- ggml-vulkan.dll: 42,006,016 -> 4,376,872
- cublasLt64_12.dll: 473,551,360 -> 210,462,120
- cublas64_12.dll: 100,033,536 -> 48,035,588

A CUDA 12.4 pack would be roughly 363 MB with LZMA against 655 MB as upstream zips. The ROCm DLLs would be roughly 62 MB, plus about 220 MB of BLAS kernel files that barely compress. The whole four-backend set would be around 0.7 GB; that total is an estimate, not a measurement.

Source: local xz -T0 -6 runs on DLLs extracted from the b12242 assets

### `llama-server.exe --list-devices` prints one line per non-CPU device on stdout and exits 0.

Confidence: high

From common/arg.cpp (common_print_available_devices): it loads all backends, prints `Available devices:`, then either `  (none)` or, per device, `  %s: %s (%zu MiB, %zu MiB free)` with name, description, total and free memory.

Names are the backend name plus an index: `Vulkan0` (GGML_VK_NAME), `CUDA0`, and `ROCm0` for the HIP build (GGML_CUDA_NAME is "ROCm"). The description is the driver's device name (cudaDeviceProp.name or the Vulkan deviceName). Logs go to stderr; only this list is on stdout.

Devices appear in backend load order (cuda, hip, ..., vulkan). When several backends in one directory see the same card, default selection in src/llama.cpp keeps the first one registered with a given PCI id and skips the rest, so ROCm0 would win over Vulkan0 unless `--device Vulkan0` (or env LLAMA_ARG_DEVICE) is passed. `--device none` disables offload.

I have no real output from an RX 9070 XT. An illustrative line: `  Vulkan0: AMD Radeon RX 9070 XT (16xxx MiB, 15xxx MiB free)`. A matching regex: `^  ([A-Za-z]+)(\d+): (.+) \((\d+) MiB, (\d+) MiB free\)$`.

Source: common/arg.cpp lines 1148-1171 and 2708-2723, common/log.cpp, ggml/include/ggml-cuda.h, ggml-vulkan.h, src/llama.cpp lines 186-312 at b12242; issue #26929 for the '(none)' output

### A probe must treat a missing device or a 0 MiB device as an unusable backend, and HIP runtime DLLs must sit next to llama-server.exe.

Confidence: medium

Missing dependencies (hipblas.dll, cudart, vulkan-1.dll, nvcuda.dll) make the backend DLL fail to load silently, so the device is simply absent from the list.

Issue #26963 reports `ggml_backend_cuda_device_get_memory: cudaMemGetInfo failed (invalid argument), returning 0/0` on Windows ROCm when an older amdhip64_7.dll from C:\Windows\System32 (shipped by the Adrenalin driver) was loaded instead of the matching one. The workflow comment explains that the exe directory is searched before System32, which is why upstream copies amdhip64_7.dll, rocm_kpack.dll and amd_comgr.dll next to the binaries. Placing them only in the working directory or on PATH would not override the System32 copy. That issue was closed by the stale bot without maintainer confirmation.

Source: ggml-backend-dl.cpp and ggml-backend-reg.cpp at b12242; release.yml comment in the 'Bundle HIP runtime DLLs' step; github.com/ggml-org/llama.cpp/issues/26963

### AMD supports ROCm 10.0 on Windows 11 25H2 only, with Adrenalin 26.8.1; the ROCm components llama.cpp needs are open source.

Confidence: medium

The ROCm 10.0.0 compatibility matrix lists 'Supported Windows version: Windows 11 25H2' and 'Supported Adrenalin Driver version 26.8.1' for the Radeon RX 9000 series (the 10.1.0 matrix lists 26.10.41.05). Windows 10 is not listed. The 10.0.0 install guide tells users to remove any HIP SDK install and to turn off Smart App Control and Defender Application Guard 'as they can interfere with ROCm functionality'.

Licences per AMD's licence page: HIP, hipamd, hipBLAS, rocBLAS, hipBLASLt and Tensile are MIT; rocSOLVER is BSD-2-Clause; Comgr is listed as University of Illinois/NCSA, and the LICENSE.txt inside the wheel says Apache 2.0 with LLVM exceptions. The wheels' METADATA files carry no licence field. I did not review the AMD HIP SDK EULA (no longer used by upstream) or the NVIDIA CUDA EULA terms for redistributing cudart/cublas.

Source: rocm.docs.amd.com/en/docs-10.0.0/compatibility/compatibility-matrix.html and /install/rocm.html; rocm.docs.amd.com/en/latest/about/license.html; LICENSE.txt inside rocm_sdk_core-10.0.0 wheel

## Recommendations

- Do not put all four backends in the NSIS installer. Ship the base plus Vulkan (106,358,557 bytes unpacked; CPU is already included) and download CUDA and ROCm as separate packs on first run, after detecting the GPU vendor.
- If the owner insists on a single installer, first run a test `tauri build` with dummy files totalling about 2.5 GB to confirm the NSIS solid-mode limit; that limit is read from source, not tested.
- For AMD, decide between two options. (a) Assemble a pack from the upstream win-rocm-10.0 zip plus hipblas.dll, rocblas.dll, rocsolver.dll, libhipblaslt.dll, origami.dll, blas_lib_gfx1201.kpack, rocblas/library and hipblaslt/library from AMD's ROCm 10.0.0 wheels; this matches upstream exactly but the layout must be validated on the RX 9070 XT. (b) Use lemonade-sdk's llama-bNNNN-windows-rocm-gfx120X-x64.zip (688,519,474 bytes), which is already self-contained but built on nightly alpha ROCm and has its own build numbering.
- Keep Vulkan as the default on the RX 9070 XT and treat ROCm as opt-in: it needs Windows 11 25H2 and Adrenalin 26.8.1 or later, and upstream's own Windows ROCm zip had detection and 0-MiB issues as recently as August-October 2026.
- Offer only the CUDA 12.4 pack (ggml-cuda.dll plus cudart64_12.dll, cublas64_12.dll, cublasLt64_12.dll). Skip CUDA 13.5 unless a smaller on-disk DLL matters more than supporting GTX 10-series and drivers older than 580.
- Give each backend its own complete directory (base + backend DLLs + runtime DLLs next to llama-server.exe) so a broken driver in one backend cannot affect another and the HIP DLLs beat the System32 copies. If a single shared directory is used instead, always pass `--device <Name>` explicitly because ROCm0 would otherwise take precedence over Vulkan0.
- Probe each backend with `llama-server.exe --list-devices` under a timeout, parse stdout with the regex in the findings, and mark the backend unusable when the expected prefix (Vulkan/CUDA/ROCm) is missing or total memory is 0 MiB.
- Pin one llama.cpp build number in the app and store asset names and SHA-256 hashes in a manifest; asset names change whenever CI bumps CUDA or ROCm (for example win-rocm-7.14 to win-rocm-10.0, cuda-13.4 to cuda-13.5), and a ROCm 10.1 bump looks likely.
- Install or bundle the Microsoft VC++ 2015-2022 x64 runtime; every llama.cpp binary imports MSVCP140.dll, VCRUNTIME140.dll and VCRUNTIME140_1.dll and the zips do not include them.
- Ship the whole base directory as Tauri resources rather than a single externalBin sidecar, because llama-server.exe is a 9,728-byte stub that needs llama-server-impl.dll and the other DLLs beside it.

## Risks

- Nothing here was executed on Windows, on the RX 9070 XT or on any NVIDIA card; device-listing output, ROCm runtime layout and RTX 50 behaviour on the CUDA 12.4 build are from source code, DLL inspection and issue reports.
- The upstream ROCm zip used as-is will silently run on CPU (or show no device) on any machine without a matching ROCm install on PATH; users would see slow inference with no error.
- The ROCm runtime layout for a self-assembled pack (kpack in the parent directory or via ROCM_KPACK_PATH, Tensile library directories) is inferred from strings in the DLLs and may need adjustment; whether the 175.7 MB hipblaslt kernel directory is required is unknown.
- The NSIS 2 GiB uncompressed limit in solid mode is supported by NSIS source and open bug #1284 but contradicts a loose reading of the maintainer's 'compressed data' comment; if it is wrong, the installer would still be roughly 0.7 GB and install 2.5-2.8 GB.
- ROCm 10.0 officially supports only Windows 11 25H2; on Windows 10 the ROCm backend is outside AMD's support matrix. AMD also says Smart App Control and Defender Application Guard can interfere with ROCm.
- A bundled amdhip64_7.dll that is newer or older than what the user's Adrenalin driver expects may misbehave; AMD has only stated a plan to keep the driver and runtime interfaces backward compatible on Windows.
- Upstream GitHub releases are dev builds published several times a day; a pinned build can contain regressions, and the tagged semver releases (v0.9.0) have no GitHub release assets.
- Redistribution terms for NVIDIA's cudart/cublas DLLs (CUDA EULA) were not reviewed; upstream redistributes them, but the app owner should check before bundling or mirroring.
- Installed size is large whichever route is taken: ggml-hip.dll alone is 1.06 GB on disk and the CUDA 12.4 set is 1.19 GB.
- The latest build (b12242) was minutes old at the time of checking; sizes and names apply to that build only.

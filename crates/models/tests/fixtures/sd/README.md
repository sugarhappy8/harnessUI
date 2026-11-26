# Detection fixtures (0.2.0 Studio)

The tensor tables (name, dtype, shape as written in the file) of 25 real Hugging Face
files, trimmed. `models::sd::testing::fixture` loads one as a `TensorIndex` whose
`parameters_hint` is the full file's parameter count (`parameters`), and `fixture_source`
returns its pinned source. No weights are committed.

Captured with scripts/capture_sd_fixtures.py on 2026-11-24; not run in CI. It reads only the
headers, through HTTP Range requests on the revision-pinned resolve URL, after checking the
file's LFS oid and size on the Hub.

## Format

```json
{"source": {"repo": "", "revision": "", "path": "", "size": 0, "sha256": "", "capturedOn": ""},
 "format": "safetensors", "metadata": {}, "tensors": [["name", "dtype", [0]]], "parameters": 0}
```

Safetensors tensors are sorted by name (the engine's visiting order); GGUF tensors are in
file order. GGUF dtypes are ggml's names.

## Trim rule

- Keep every tensor whose name contains a marker of the detection table (the literals of
  `ModelLoader::get_sd_version`) or of R6's tables and R7's facts: the `MARKERS` list of
  the script. In the LoRA files a marker is matched against the module part of the name,
  without the `.lora_down.weight`, `.lora_up.weight`, `.alpha` and similar suffixes.
- Keep the first tensor of each top-level name prefix (the text before the first `.`), and
  the first tensor of the lowest and the highest N of each numbered block (`...blocks.N.`,
  `...layers.N.`).
- In the LoRA files, also keep every `attn2_to_k`, `lora_te2_` and `te2.` name.
- Keep the `__metadata__` keys `modelspec.*` (but not the `modelspec.thumbnail` image), `ss_base_model_version`,
  `ss_network_module`, `general.architecture` and `general.name`.

## Sources

| Fixture | Repo @ revision | File | Size | SHA-256 |
|---|---|---|---|---|
| `sd15` | stable-diffusion-v1-5/stable-diffusion-v1-5 @ 451f4fe16113bff5a5d2269ed5ad43b0592e9a14 | `v1-5-pruned-emaonly.safetensors` | 4265146304 | 6ce0161689b3853acaa03779ec93eafe75a02f4ced659bee03f50797806fa2fa |
| `sdxl` | stabilityai/stable-diffusion-xl-base-1.0 @ 462165984030d82259a11f4367a4eed129e94a7b | `sd_xl_base_1.0.safetensors` | 6938078334 | 31e35c80fc4829d14f90153f4c74cd59c90b779f6afe05a74cd6120b893f7e5b |
| `sd35-medium` | second-state/stable-diffusion-3.5-medium-GGUF @ 58b78c305a43ddfcffe1ab54d7022995f61667ac | `sd3.5_medium.safetensors` | 5107104286 | 11fe06e22364b823dfeedc275912336b932b32a293a0b2f35ffac071990cc4de |
| `flux1-dev` | Comfy-Org/Flux-1-Dev @ 10cf201a3d49f7ff4de84d6a97d6a32cdfa8cb86 | `flux1-dev.safetensors` | 23802932552 | 4610115bb0c89560703c892c59ac2742fa821e60ef5871b33493ba544683abd7 |
| `flux1-kontext` | QuantStack/FLUX.1-Kontext-dev-GGUF @ 2d083027732f81e5548620b57ac47da22d30aa5a | `flux1-kontext-dev-Q4_K_M.gguf` | 6931817760 | ebd4e92c44c47f104ad64e4353692f37addf06eb99420c9b459334d871c8b750 |
| `chroma` | silveroxides/Chroma1-HD-GGUF @ d3b77bf4eb5b84c0b50e9a1e83f6e8074ed85f82 | `Chroma1-HD-Q4_0.gguf` | 5432053920 | 1aa9f3c059a415320bb9b8b6231ea14e118df26653b5bb937c92e00cf0cc127e |
| `qwen-image` | QuantStack/Qwen-Image-GGUF @ 257f261fa92593bed760aa6fa3f7921a49fea00f | `Qwen_Image-Q4_0.gguf` | 11852773920 | ab0f20d548f3bfb197ec0f0f61c15acc2f0f865495fee2941c658fef4391cd61 |
| `qwen-image-edit-2509` | QuantStack/Qwen-Image-Edit-2509-GGUF @ 84a3006979126011422eeeefe0c9485ddf431ef5 | `Qwen-Image-Edit-2509-Q4_0.gguf` | 11928271392 | 4f6cda402e1dbc36ee4b601b10b9ee0da2dbefedfbfa53eae3efb0ddff48c3e2 |
| `z-image-turbo` | Comfy-Org/Z-Image-Turbo @ 6fc90a3b1b653e935a0d175e260736de25b84df5 | `split_files/diffusion_models/z_image_turbo_bf16.safetensors` | 12309866400 | 2407613050b809ffdff18a4ac99af83ea6b95443ecebdf80e064a79c825574a6 |
| `flux2-klein-4b` | black-forest-labs/FLUX.2-klein-4B @ e7b7dc27f91deacad38e78976d1f2b499d76a294 | `flux-2-klein-4b.safetensors` | 7751105712 | ec3d4e733a771f61c052fb4856c48b336c55eaf2c65487c2a1faeb9bbda7a343 |
| `wan21-t2v-1.3b` | Comfy-Org/Wan-2.1 @ d6258323e18f14c60d37899da97ca555f3118259 | `split_files/diffusion_models/wan2.1_t2v_1.3B_fp16.safetensors` | 2838303560 | be531024cd9018cb5b48c40cfbb6a6191645b1c792eb8bf4f8c1c6e10f924dc5 |
| `wan21-i2v-14b` | city96/Wan2.1-I2V-14B-480P-gguf @ b4b6b3b3f0f64975fb33b1d5d1a46ca24e065f03 | `wan2.1-i2v-14b-480p-Q4_0.gguf` | 10247552384 | cc572fdf84417905550363f16c53f20fd8d5aee046e1562d2546fae7222c2a0a |
| `wan22-ti2v-5b` | QuantStack/Wan2.2-TI2V-5B-GGUF @ 57437632ddd08bdcbd1508c866aa22e126ed51d2 | `Wan2.2-TI2V-5B-Q4_K_M.gguf` | 3433116000 | 95b19697b7f98e65b0a543640e9ca7b4dfec32e2a6e3731e8e10708be52655e2 |
| `wan22-t2v-a14b-high` | QuantStack/Wan2.2-T2V-A14B-GGUF @ 73eafba53a1a8f29254e4c77f92e74ea27d7cd6f | `HighNoise/Wan2.2-T2V-A14B-HighNoise-Q4_K_M.gguf` | 9650090496 | e0c490c6e316fd91ff52034e4ca66b825717e33ff11624585c0ccfcb5d410c59 |
| `wan22-t2v-a14b-low` | QuantStack/Wan2.2-T2V-A14B-GGUF @ 73eafba53a1a8f29254e4c77f92e74ea27d7cd6f | `LowNoise/Wan2.2-T2V-A14B-LowNoise-Q4_K_M.gguf` | 9650090496 | 091a5bae02e14aa016bc9b10a7892efda4c629346b81c5dcebbe30ea2ac8923a |
| `hunyuan-video-15` | Comfy-Org/Hunyuan-Video-1.5 @ 1405a2b738c29b890cf6d03fc038d8d3e92eeed7 | `split_files/diffusion_models/hunyuanvideo1.5_720p_t2v_fp16.safetensors` | 16653368128 | 4f05e2a31b51b7ddee3ef336d234e0932001cbdc85887da15451069fda3e1d05 |
| `ltx23` | unsloth/LTX-2.3-GGUF @ 96e8ed4925ead3db9ff4d0084f165ef6a74f28d0 | `ltx-2.3-22b-dev-Q4_K_M.gguf` | 14326856736 | e053e3d7827f3a69ecd00e55395d3a8f8616ab10d3a394e8d2b65ae204d490e0 |
| `sdxl-lora-pixel-art` | nerijs/pixel-art-xl @ 8bf4a4d9ea283e00a51fafda8e0539f8248ea037 | `pixel-art-xl.safetensors` | 170543052 | 4234637cb80c998f41e348e6a6cb6bc20d8d038b2b0f256b6129b3b5e353eef7 |
| `flux-lora-realism` | XLabs-AI/flux-lora-collection @ 9ddb42ad970b5d31f50ac02d223adea81413ba4c | `realism_lora_comfy_converted.safetensors` | 22433528 | 379e73dccfb57822ee3b12f374e141ce1c79a13b7ff19da4219ef2a2a610038e |
| `lumina2` | Comfy-Org/Lumina-Image-2 @ 5b072540ef86570fecb8249c505f23d5bdeb88cd | `split_files/diffusion_models/lumina_2_model_bf16.safetensors` | 5219585680 | 736e7a524186c943568dfe688d87ce3556cfbd97f7026125065b0aa44c351f6a |
| `sd14-diffusers-unet` | CompVis/stable-diffusion-v1-4 @ 133a221b8aa7292a167afc5127cb63fb5005638b | `unet/diffusion_pytorch_model.safetensors` | 3438167534 | 145a07e0f05ec5bbe6e2e9faf608bdb311caf708895cac8c8ed713c59864e1e8 |
| `sd14-diffusers-text-encoder` | CompVis/stable-diffusion-v1-4 @ 133a221b8aa7292a167afc5127cb63fb5005638b | `text_encoder/model.safetensors` | 492265879 | 7b3a12df205cb3c74dd4eae4354d93f606ae6b3bc29d5d06fd97921cb9ad8a81 |
| `ae-flux1` | lodestones/Chroma @ 2f3b2730d7b5edbd02cdff12f72610af5787300b | `ae.safetensors` | 335304388 | afc8e28272cd15db3919bacdb6918ce9c1ed22e96cb12c4d5ed0fba823529e38 |
| `clip-l` | comfyanonymous/flux_text_encoders @ 6af2a98e3f615bdfa612fbd85da93d1ed5f69ef5 | `clip_l.safetensors` | 246144152 | 660c6f5b1abae9dc498ac2d21e1347d2abdb0cf6c0c0c8576cd796491d9a6cdd |
| `umt5-xxl-q5km` | city96/umt5-xxl-encoder-gguf @ b535255bee98c2b0a59ea7c0ae2dcd0c6657b3b7 | `umt5-xxl-encoder-Q5_K_M.gguf` | 4145878880 | eaea358bb438c5d211721a4feecc162000e3636e9cb96f51e216f1f44ebd12ce |

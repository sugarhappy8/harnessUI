# Tensor table fixtures (0.1.0 Studio)

Each tensor's name and dimensions, one `name d0 d1 …` a line, in the file's order and as
the file writes them (ggml's order: the fastest-changing dimension first). Read on
2026-11-18 from the start of each pinned file through a Range request: the first 256 KiB,
or for the Qwen3 chat model, whose tokenizer comes first, the first 8 MiB.
`models::gguf::testing::shapes` reads them and `testing::diffusion_gguf` writes them back
as a tiny model file (32 bytes of data a tensor, no metadata keys).

leejet's and Green-Sky's files hold no metadata keys at all, so there is no
`general.architecture` to read: the table is what tells the families apart
(`gen_search::family_from_tensors`).

| File | Repository @ revision, file |
|---|---|
| `klein4b-tensors.txt` | leejet/FLUX.2-klein-4B-GGUF @ 3b1f5a9dc3abb32238b053aeb3d823c30afdacbd, `flux-2-klein-4b-Q4_0.gguf` |
| `klein9b-tensors.txt` | leejet/FLUX.2-klein-9B-GGUF @ cc588497a95ffc2937ebbd6b9b3916a11ada6e5b, `flux-2-klein-9b-Q4_0.gguf` |
| `zimage-tensors.txt` | leejet/Z-Image-Turbo-GGUF @ c61c0e422dc8b541b7548cf33a4ef8302b0f8085, `z_image_turbo-Q4_K.gguf` |
| `wan22-ti2v-5b-tensors.txt` | Green-Sky/FastWan2.2-TI2V-5B-FullAttn-GGUF @ 3e8fe5537b1200654868aa24ea8d0f4012fb3a1e, `FastWan2.2-TI2V-5B-q6_k.gguf` |
| `wan21-t2v-1.3b-tensors.txt` | off-grid-ai/Wan2.1-T2V-1.3B-GGUF @ 1ba777b2426d5717dd0fddb1100078604a9d4667, `Wan2.1-T2V-1.3B-Q4_K_S.gguf` (converted with stable-diffusion.cpp: every name starts `model.diffusion_model.`) |
| `qwen3-4b-tensors.txt` | unsloth/Qwen3-4B-GGUF @ 22c9fc8a8c7700b76a1789366280a6a5a1ad1120, `Qwen3-4B-Q4_K_M.gguf` (a chat model, for contrast) |

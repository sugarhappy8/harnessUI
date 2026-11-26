#!/usr/bin/env python3
"""Captures the tensor-table fixtures of crates/models/tests/fixtures/sd from real Hugging Face
files. Run by hand (never in CI or tests):

    python3 -I scripts/capture_sd_fixtures.py [--only NAME] [--out DIR]

Only headers are read, through HTTP Range requests on the revision-pinned resolve URL, with
the caps of crates/models/src/sd/index.rs. Weights are never downloaded. The headers are
untrusted data: they are only parsed as bytes or JSON. No token is sent.
"""
import argparse
import datetime
import json
import re
import struct
import sys
import urllib.parse
import urllib.request
from pathlib import Path

HUB = "https://huggingface.co"
USER_AGENT = "harnessui-fixture-capture/1"
SAFETENSORS_HEADER_CAP = 100 * 1024 * 1024
GGUF_HEADER_CAP = 64 * 1024 * 1024
FIRST_READ = 1 << 20

# (name, repo, revision, path, size, sha256)
FIXTURES = [
    ("sd15", "stable-diffusion-v1-5/stable-diffusion-v1-5", "451f4fe16113bff5a5d2269ed5ad43b0592e9a14", "v1-5-pruned-emaonly.safetensors", 4265146304, "6ce0161689b3853acaa03779ec93eafe75a02f4ced659bee03f50797806fa2fa"),
    ("sdxl", "stabilityai/stable-diffusion-xl-base-1.0", "462165984030d82259a11f4367a4eed129e94a7b", "sd_xl_base_1.0.safetensors", 6938078334, "31e35c80fc4829d14f90153f4c74cd59c90b779f6afe05a74cd6120b893f7e5b"),
    ("sd35-medium", "second-state/stable-diffusion-3.5-medium-GGUF", "58b78c305a43ddfcffe1ab54d7022995f61667ac", "sd3.5_medium.safetensors", 5107104286, "11fe06e22364b823dfeedc275912336b932b32a293a0b2f35ffac071990cc4de"),
    ("flux1-dev", "Comfy-Org/Flux-1-Dev", "10cf201a3d49f7ff4de84d6a97d6a32cdfa8cb86", "flux1-dev.safetensors", 23802932552, "4610115bb0c89560703c892c59ac2742fa821e60ef5871b33493ba544683abd7"),
    ("flux1-kontext", "QuantStack/FLUX.1-Kontext-dev-GGUF", "2d083027732f81e5548620b57ac47da22d30aa5a", "flux1-kontext-dev-Q4_K_M.gguf", 6931817760, "ebd4e92c44c47f104ad64e4353692f37addf06eb99420c9b459334d871c8b750"),
    ("chroma", "silveroxides/Chroma1-HD-GGUF", "d3b77bf4eb5b84c0b50e9a1e83f6e8074ed85f82", "Chroma1-HD-Q4_0.gguf", 5432053920, "1aa9f3c059a415320bb9b8b6231ea14e118df26653b5bb937c92e00cf0cc127e"),
    ("qwen-image", "QuantStack/Qwen-Image-GGUF", "257f261fa92593bed760aa6fa3f7921a49fea00f", "Qwen_Image-Q4_0.gguf", 11852773920, "ab0f20d548f3bfb197ec0f0f61c15acc2f0f865495fee2941c658fef4391cd61"),
    ("qwen-image-edit-2509", "QuantStack/Qwen-Image-Edit-2509-GGUF", "84a3006979126011422eeeefe0c9485ddf431ef5", "Qwen-Image-Edit-2509-Q4_0.gguf", 11928271392, "4f6cda402e1dbc36ee4b601b10b9ee0da2dbefedfbfa53eae3efb0ddff48c3e2"),
    ("z-image-turbo", "Comfy-Org/Z-Image-Turbo", "6fc90a3b1b653e935a0d175e260736de25b84df5", "split_files/diffusion_models/z_image_turbo_bf16.safetensors", 12309866400, "2407613050b809ffdff18a4ac99af83ea6b95443ecebdf80e064a79c825574a6"),
    ("flux2-klein-4b", "black-forest-labs/FLUX.2-klein-4B", "e7b7dc27f91deacad38e78976d1f2b499d76a294", "flux-2-klein-4b.safetensors", 7751105712, "ec3d4e733a771f61c052fb4856c48b336c55eaf2c65487c2a1faeb9bbda7a343"),
    ("wan21-t2v-1.3b", "Comfy-Org/Wan-2.1", "d6258323e18f14c60d37899da97ca555f3118259", "split_files/diffusion_models/wan2.1_t2v_1.3B_fp16.safetensors", 2838303560, "be531024cd9018cb5b48c40cfbb6a6191645b1c792eb8bf4f8c1c6e10f924dc5"),
    ("wan21-i2v-14b", "city96/Wan2.1-I2V-14B-480P-gguf", "b4b6b3b3f0f64975fb33b1d5d1a46ca24e065f03", "wan2.1-i2v-14b-480p-Q4_0.gguf", 10247552384, "cc572fdf84417905550363f16c53f20fd8d5aee046e1562d2546fae7222c2a0a"),
    ("wan22-ti2v-5b", "QuantStack/Wan2.2-TI2V-5B-GGUF", "57437632ddd08bdcbd1508c866aa22e126ed51d2", "Wan2.2-TI2V-5B-Q4_K_M.gguf", 3433116000, "95b19697b7f98e65b0a543640e9ca7b4dfec32e2a6e3731e8e10708be52655e2"),
    ("wan22-t2v-a14b-high", "QuantStack/Wan2.2-T2V-A14B-GGUF", "73eafba53a1a8f29254e4c77f92e74ea27d7cd6f", "HighNoise/Wan2.2-T2V-A14B-HighNoise-Q4_K_M.gguf", 9650090496, "e0c490c6e316fd91ff52034e4ca66b825717e33ff11624585c0ccfcb5d410c59"),
    ("wan22-t2v-a14b-low", "QuantStack/Wan2.2-T2V-A14B-GGUF", "73eafba53a1a8f29254e4c77f92e74ea27d7cd6f", "LowNoise/Wan2.2-T2V-A14B-LowNoise-Q4_K_M.gguf", 9650090496, "091a5bae02e14aa016bc9b10a7892efda4c629346b81c5dcebbe30ea2ac8923a"),
    ("hunyuan-video-15", "Comfy-Org/Hunyuan-Video-1.5", "1405a2b738c29b890cf6d03fc038d8d3e92eeed7", "split_files/diffusion_models/hunyuanvideo1.5_720p_t2v_fp16.safetensors", 16653368128, "4f05e2a31b51b7ddee3ef336d234e0932001cbdc85887da15451069fda3e1d05"),
    ("ltx23", "unsloth/LTX-2.3-GGUF", "96e8ed4925ead3db9ff4d0084f165ef6a74f28d0", "ltx-2.3-22b-dev-Q4_K_M.gguf", 14326856736, "e053e3d7827f3a69ecd00e55395d3a8f8616ab10d3a394e8d2b65ae204d490e0"),
    ("sdxl-lora-pixel-art", "nerijs/pixel-art-xl", "8bf4a4d9ea283e00a51fafda8e0539f8248ea037", "pixel-art-xl.safetensors", 170543052, "4234637cb80c998f41e348e6a6cb6bc20d8d038b2b0f256b6129b3b5e353eef7"),
    ("flux-lora-realism", "XLabs-AI/flux-lora-collection", "9ddb42ad970b5d31f50ac02d223adea81413ba4c", "realism_lora_comfy_converted.safetensors", 22433528, "379e73dccfb57822ee3b12f374e141ce1c79a13b7ff19da4219ef2a2a610038e"),
    ("lumina2", "Comfy-Org/Lumina-Image-2", "5b072540ef86570fecb8249c505f23d5bdeb88cd", "split_files/diffusion_models/lumina_2_model_bf16.safetensors", 5219585680, "736e7a524186c943568dfe688d87ce3556cfbd97f7026125065b0aa44c351f6a"),
    ("sd14-diffusers-unet", "CompVis/stable-diffusion-v1-4", "133a221b8aa7292a167afc5127cb63fb5005638b", "unet/diffusion_pytorch_model.safetensors", 3438167534, "145a07e0f05ec5bbe6e2e9faf608bdb311caf708895cac8c8ed713c59864e1e8"),
    ("sd14-diffusers-text-encoder", "CompVis/stable-diffusion-v1-4", "133a221b8aa7292a167afc5127cb63fb5005638b", "text_encoder/model.safetensors", 492265879, "7b3a12df205cb3c74dd4eae4354d93f606ae6b3bc29d5d06fd97921cb9ad8a81"),
    ("ae-flux1", "lodestones/Chroma", "2f3b2730d7b5edbd02cdff12f72610af5787300b", "ae.safetensors", 335304388, "afc8e28272cd15db3919bacdb6918ce9c1ed22e96cb12c4d5ed0fba823529e38"),
    ("clip-l", "comfyanonymous/flux_text_encoders", "6af2a98e3f615bdfa612fbd85da93d1ed5f69ef5", "clip_l.safetensors", 246144152, "660c6f5b1abae9dc498ac2d21e1347d2abdb0cf6c0c0c8576cd796491d9a6cdd"),
    ("umt5-xxl-q5km", "city96/umt5-xxl-encoder-gguf", "b535255bee98c2b0a59ea7c0ae2dcd0c6657b3b7", "umt5-xxl-encoder-Q5_K_M.gguf", 4145878880, "eaea358bb438c5d211721a4feecc162000e3636e9cb96f51e216f1f44ebd12ce"),
]

LORA_NAMES = {"sdxl-lora-pixel-art", "flux-lora-realism"}

# Name fragments the detection (R5), parameter-label (R6) and matching (R7) tables look at:
# the literals of the engine's ModelLoader::get_sd_version (model_loader.cpp:443-), without
# their "model.diffusion_model." prefix, plus the family-specific names of the other tables.
MARKERS = [
    "perceiver.latent_queries", "emb_in.image.weight", "txt_mode_blocks.0.q_proj.weight",
    "video_embedder.weight", "shared_block_modulation.weight",
    "audio_injector.injector.0.q.weight", "casual_audio_encoder.weights",
    "cond_stage_model.model.token_embedding.weight",
    "cond_stage_model.transformer.text_model.embeddings.token_embedding.weight",
    "conditioner.embedders.0.model.token_embedding.weight",
    "conditioner.embedders.0.transformer.text_model.embeddings.token_embedding.weight",
    "distilled_guidance_layer.in_proj.weight",
    "double_stream_layers.0.img_instruct_attn.processor.img_to_q.weight",
    "dual_time_embed.semantic_embedder.linear_1.weight", "embed_image_indicator.weight",
    "first_stage_model.decoder.proj_out.weight",
    "language_model.model.layers.0.self_attn.q_proj_mot_gen.weight",
    "llm_adapter.blocks.0.cross_attn.q_proj.weight",
    "adaln_single.emb.timestep_embedder.linear_1.bias", "adaln_single.linear.weight",
    "add_embedding.", "audio_patch_proj.weight", "audio_patchify_proj.weight",
    "blocks.0.cross_attn.norm_k.weight", "cap_embedder.0.weight", "condition_proj.weight",
    "context_embedder.weight", "conv_in.weight", "double_blocks.0.img_mlp.gate_proj.weight",
    "double_stream_modulation_img.lin.weight", "down_blocks.0.", "img_emb", "img_in.weight",
    "input_blocks.0.0.weight", "input_blocks.8.0.time_mixer.mix_factor", "joint_blocks.0.",
    "label_emb.0.0.", "layers.0.adaLN_sa_ln.weight", "local_decoder.out_conv.weight",
    "mid_block.resnets.1.", "middle_block.1.", "modulation_cores.adaln_img.weight",
    "nerf_final_layer_conv.", "net.lq_proj.latent_proj.0.weight",
    "output_blocks.3.1.transformer_blocks.1", "output_blocks.7.1", "patch_embedder.weight",
    "patch_embedding.weight", "pos_embed.proj.weight", "sigvq_embedder.1.weight",
    "t_block.1.weight", "text_fusion.projector.weight", "time_text_embed.addition_t_embedding.weight",
    "transformer_blocks.0.attn.norm_added_q.weight", "transformer_blocks.0.img_mlp.w1.weight",
    "transformer_blocks.0.img_mod.1.weight",
    "txt_in.individual_token_refiner.blocks.0.adaLN_modulation.1.weight",
    "txt_in.text_norm.weight", "txt_in.weight", "txtfusion.projector.weight",
    "up_blocks.1.attentions.0.transformer_blocks.1", "up_blocks.2.attentions.1",
    "x_embedder.proj.weight", "y_embedder.layer_pool.weight",
    "model.layers.0.self_attn.q_proj.weight", "x_embedder.proj1.weight",
    "net.img_embedder.proj1.weight", "single_blocks.47.linear1.weight",
    "text_encoders.llm.connector.layers.0.self_attn.q_proj.weight",
    "text_model.embeddings.token_embedding.weight", "unet.conv_in.weight",
    "vae.decoder.proj_out.weight",
    # video and other families' own fingerprints (R6 labels, R7 facts)
    "head.head.weight", "img_emb.proj", "time_projection", "text_embedding.0.weight",
    "patch_embedding.", "single_transformer_blocks.0.",
    # fix round 1: the rest of the tables' literals
    "byt5_in.fc1.weight", "blk.0.attn_q.weight", "conditioner.embedders.1", "cond_stage_model.1",
    "te.1", "video_patch_proj.weight", "connector.layers.0.self_attn.q_proj.weight",
    "decoder.proj_out.weight", "text_encoders.clip_l.", "text_encoders.clip_g.",
    "text_encoders.t5xxl.", "text_encoders.llm.", "cond_stage_model.1.",
    "conditioner.embedders.1.", "mm.", "v.blk.",
    "encoder.block.0.layer.0.SelfAttention.q.weight", "wav2vec2.",
    "feature_extractor.conv_layers.", "audio_vae.", "vocoder.", "connectors.",
    "embeddings_connector",
    "context_refiner", "noise_refiner", "token_embd.weight", "shared.weight",
    "encoder.final_layer_norm", "decoder.conv_in", "encoder.conv_in", "quant_conv",
    "logit_scale", "text_projection", "position_ids",
]
LORA_EXTRA = ["attn2_to_k", "lora_te2_", "te2."]
# A LoRA name is its module plus a format suffix; markers match the module part.
LORA_SUFFIX = re.compile(r"\.(lora_down|lora_up|lora_A|lora_B|lora\.down|lora\.up|lora_A\.default|lora_B\.default)(\.weight)?$|\.(alpha|diff|diff_b)$|\.(weight|bias)$")
KEPT_METADATA = re.compile(r"^(modelspec\.(?!thumbnail$).*|ss_base_model_version|ss_network_module|general\.architecture|general\.name)$")
NUMBERED = re.compile(r"^(.*?)\.(\d+)\.")


def http(url, headers=None, data=None, limit=None):
    req = urllib.request.Request(url, data=data, headers={"User-Agent": USER_AGENT, **(headers or {})})
    with urllib.request.urlopen(req, timeout=120) as resp:
        # never take more than asked, even if the server ignores Range
        return resp.read(limit) if limit is not None else resp.read()


def check_pinned(repo, rev, path, size, sha256):
    body = urllib.parse.urlencode({"paths": path, "expand": "true"}).encode()
    url = f"{HUB}/api/models/{repo}/paths-info/{rev}"
    info = json.loads(http(url, {"Content-Type": "application/x-www-form-urlencoded"}, body))
    entry = next((e for e in info if e.get("path") == path), None)
    lfs = (entry or {}).get("lfs") or {}
    if lfs.get("oid") != sha256 or lfs.get("size") != size:
        raise SystemExit(f"{repo}@{rev} {path}: Hub says oid={lfs.get('oid')} size={lfs.get('size')}, "
                         f"table says oid={sha256} size={size}")


def fetch_head(repo, rev, path, n):
    url = f"{HUB}/{repo}/resolve/{rev}/{urllib.parse.quote(path)}"
    return http(url, {"Range": f"bytes=0-{n - 1}"}, limit=n)


class Truncated(Exception):
    pass


def safetensors_header(repo, rev, path, size):
    buf = fetch_head(repo, rev, path, min(FIRST_READ, size))
    n = struct.unpack("<Q", buf[:8])[0]
    if n > SAFETENSORS_HEADER_CAP:
        raise SystemExit(f"{path}: header of {n} bytes is past the cap")
    if len(buf) < 8 + n:
        buf = fetch_head(repo, rev, path, 8 + n)
    meta = json.loads(buf[8:8 + n])
    tensors = []
    metadata = {}
    for name, v in meta.items():
        if name == "__metadata__":
            metadata = {k: x for k, x in v.items() if isinstance(x, str)}
        else:
            tensors.append([name, v["dtype"], [int(d) for d in v["shape"]]])
    tensors.sort(key=lambda t: t[0])  # the engine visits a std::map in name order
    return tensors, metadata


GGML_TYPES = {0: "f32", 1: "f16", 2: "q4_0", 3: "q4_1", 6: "q5_0", 7: "q5_1", 8: "q8_0", 9: "q8_1",
              10: "q2_K", 11: "q3_K", 12: "q4_K", 13: "q5_K", 14: "q6_K", 15: "q8_K",
              16: "iq2_xxs", 17: "iq2_xs", 18: "iq3_xxs", 19: "iq1_s", 20: "iq4_nl", 21: "iq3_s",
              22: "iq2_s", 23: "iq4_xs", 24: "i8", 25: "i16", 26: "i32", 27: "i64", 28: "f64",
              29: "iq1_m", 30: "bf16", 34: "tq1_0", 35: "tq2_0", 39: "mxfp4", 40: "nvfp4",
              41: "q1_0", 42: "q2_0", 43: "f8_e4m3", 44: "f8_e5m2"}
SCALAR = {0: "<B", 1: "<b", 2: "<H", 3: "<h", 4: "<I", 5: "<i", 6: "<f", 7: "<?", 10: "<Q", 11: "<q", 12: "<d"}


class Cursor:
    def __init__(self, buf):
        self.buf, self.pos = buf, 0

    def take(self, n):
        if self.pos + n > len(self.buf):
            raise Truncated()
        out = self.buf[self.pos:self.pos + n]
        self.pos += n
        return out

    def unpack(self, fmt):
        return struct.unpack(fmt, self.take(struct.calcsize(fmt)))[0]

    def string(self):
        return self.take(self.unpack("<Q")).decode("utf-8", "replace")

    def value(self, ty):
        """Reads one metadata value; arrays are skipped, only strings are kept."""
        if ty == 8:
            return self.string()
        if ty == 9:
            inner, count = self.unpack("<I"), self.unpack("<Q")
            if inner in SCALAR:
                self.take(struct.calcsize(SCALAR[inner]) * count)
            else:
                for _ in range(count):
                    self.value(inner)
            return None
        if ty in SCALAR:
            return self.unpack(SCALAR[ty])
        raise SystemExit(f"unknown GGUF metadata type {ty}")


def parse_gguf(buf):
    c = Cursor(buf)
    if c.take(4) != b"GGUF":
        raise SystemExit("not a GGUF file")
    c.unpack("<I")
    tensor_count, kv_count = c.unpack("<Q"), c.unpack("<Q")
    metadata = {}
    for _ in range(kv_count):
        key = c.string()
        v = c.value(c.unpack("<I"))
        if isinstance(v, str):
            metadata[key] = v
    tensors = []
    for _ in range(tensor_count):
        name = c.string()
        dims = [c.unpack("<Q") for _ in range(c.unpack("<I"))]
        ty = c.unpack("<I")
        c.unpack("<Q")
        tensors.append([name, GGML_TYPES.get(ty, f"type{ty}"), dims])
    return tensors, metadata


def gguf_header(repo, rev, path, size):
    want = min(FIRST_READ, size)
    while True:
        buf = fetch_head(repo, rev, path, want)
        try:
            return parse_gguf(buf)
        except Truncated:
            if want >= min(GGUF_HEADER_CAP, size):
                raise SystemExit(f"{path}: table does not fit in {want} bytes")
            want = min(want * 2, GGUF_HEADER_CAP, size)


def parameters(tensors):
    total = 0
    for _, _, dims in tensors:
        p = 1
        for d in dims:
            p *= d
        total += p
    return total


def trim(name, tensors, metadata):
    lora = name in LORA_NAMES
    markers = MARKERS + (LORA_EXTRA if lora else [])
    keep = [False] * len(tensors)
    seen_prefix = set()
    blocks = {}  # key -> {N: first index}
    for i, (tname, _, _) in enumerate(tensors):
        if lora:
            module = LORA_SUFFIX.sub("", tname)
            hit = any(re.sub(r"\.(weight|bias)$", "", m) in module for m in markers)
        else:
            hit = any(m in tname for m in markers)
        if hit:
            keep[i] = True
        prefix = tname.split(".", 1)[0]
        if prefix not in seen_prefix:
            seen_prefix.add(prefix)
            keep[i] = True
        m = NUMBERED.match(tname)
        if m and re.search(r"(blocks|layers|layer)$", m.group(1)):
            blocks.setdefault(m.group(1), {}).setdefault(int(m.group(2)), i)
    for firsts in blocks.values():
        keep[firsts[min(firsts)]] = True
        keep[firsts[max(firsts)]] = True
    kept = [t for t, k in zip(tensors, keep) if k]
    return kept, {k: v for k, v in metadata.items() if KEPT_METADATA.match(k)}


def capture(entry, out, today):
    name, repo, rev, path, size, sha = entry
    check_pinned(repo, rev, path, size, sha)
    if path.endswith(".gguf"):
        fmt, (tensors, metadata) = "gguf", gguf_header(repo, rev, path, size)
    else:
        fmt, (tensors, metadata) = "safetensors", safetensors_header(repo, rev, path, size)
    total = parameters(tensors)
    kept, kept_meta = trim(name, tensors, metadata)
    doc = {
        "source": {"repo": repo, "revision": rev, "path": path, "size": size, "sha256": sha, "capturedOn": today},
        "format": fmt,
        "metadata": kept_meta,
        "tensors": kept,
        "parameters": total,
    }
    text = json.dumps(doc, indent=1)
    (out / f"{name}.json").write_text(text + "\n")
    print(f"{name}: {len(tensors)} tensors, kept {len(kept)}, {len(text) // 1024} KB, {total} parameters")
    return len(text)


def write_readme(out, today):
    rows = "\n".join(
        f"| `{n}` | {r} @ {v} | `{p}` | {s} | {h} |" for n, r, v, p, s, h in FIXTURES)
    (out / "README.md").write_text(f"""# Detection fixtures (0.2.0 Studio)

The tensor tables (name, dtype, shape as written in the file) of 25 real Hugging Face
files, trimmed. `models::sd::testing::fixture` loads one as a `TensorIndex` whose
`parameters_hint` is the full file's parameter count (`parameters`), and `fixture_source`
returns its pinned source. No weights are committed.

Captured with scripts/capture_sd_fixtures.py on {today}; not run in CI. It reads only the
headers, through HTTP Range requests on the revision-pinned resolve URL, after checking the
file's LFS oid and size on the Hub.

## Format

```json
{{"source": {{"repo": "", "revision": "", "path": "", "size": 0, "sha256": "", "capturedOn": ""}},
 "format": "safetensors", "metadata": {{}}, "tensors": [["name", "dtype", [0]]], "parameters": 0}}
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
{rows}
""")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--only")
    ap.add_argument("--out", default="crates/models/tests/fixtures/sd")
    args = ap.parse_args()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    today = datetime.date.today().isoformat()
    todo = [f for f in FIXTURES if args.only in (None, f[0])]
    if not todo:
        raise SystemExit(f"no fixture named {args.only}")
    worst = 0
    for entry in todo:
        worst = max(worst, capture(entry, out, today))
    if args.only is None:
        write_readme(out, today)
    print(f"{len(todo)} fixtures written (largest {worst // 1024} KB)")


if __name__ == "__main__":
    sys.exit(main())

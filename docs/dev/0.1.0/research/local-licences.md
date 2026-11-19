# Licences and distribution constraints for local image/video generation in harnessUI

This is not legal advice. All facts below were checked on 2026-11-17 against Hugging Face API metadata (`/api/models/<repo>`: `gated`, `cardData.license*`, tags), the licence files at each repo's `main`, and a shallow clone of stable-diffusion.cpp at commit 532d2c15 (2026-11-15). I edited no files. The only scratch copies are under /tmp/claude-0/-home-user/f8f6f1ef-33e4-57b5-aed7-1e790c206f77/scratchpad/ (lic/, hf/, gh/).

## 1. Bottom line

- **No engine licence blocks this.** stable-diffusion.cpp and everything bundled with it, plus every MLX option, are MIT, BSD, public-domain or Unlicense code. There is nothing copyleft in the sd.cpp build, and its video output (WebM using VP8, animated WebP, or MJPEG AVI) needs no ffmpeg and no H.264.
- **The risk is in the model terms, and it varies a lot within one family.** Examples:
  - Qwen-Image, -2512 and -Edit-2511 are Apache-2.0, but **Qwen-Image-2.1 (Sept 2026) is research or evaluation only**.
  - FLUX.2 klein 4B is Apache-2.0, but klein 9B is non-commercial.
  - The new **FLUX.3 image klein** uses the "Kommunity" licence: you may use outputs commercially only if your revenue is under $5M.
  - The catalog must store licence terms for each repo and revision, never for each family.
- **Two families should stay out of the catalog because of where they may be used:**
  - **HunyuanVideo / HunyuanVideo 1.5 / HunyuanImage**: the licence excludes the EU, UK and South Korea, and that covers outputs too.
  - **MiniMax-H3 / H3-Image / P-H3-Image**: the licence excludes the EU, UK, South Korea **and the USA**, and requires anyone who makes generation available to build safeguards.
- **A model is several files, and the strictest licence among them applies.** These need separate licence records:
  - LTX-2.3 needs Gemma 3 12B, which is under the Gemma Terms with a Prohibited Use Policy and is gated on google/.
  - HiDream-I1 needs Llama-3.1-8B-Instruct, which is under the Llama 3.1 licence and gated with manual review.
  - The sd.cpp docs fetch the FLUX.1 VAE from FLUX.1-dev (non-commercial and gated) and the FLUX.2 VAE from FLUX.2-dev (non-commercial and gated). Byte-identical Apache-2.0, ungated copies exist and should be used instead (see section 4).
- **The labels on mirror and quantized repos are unreliable.** Take the licence from the upstream base model, not from the mirror's metadata (examples in section 4).
- **A gated repo can only be unlocked in a browser.** Hugging Face says: "Requesting access can only be done from your browser." harnessUI's existing errors (401 means it needs a token, 403 means the user must accept the terms) fit this. The app should open the repo page and say what data the publisher collects (section 5c).
- **Labelling outputs is now a legal question for harnessUI itself, not only for its users.**
  - EU AI Act Art. 50(2) applies from 2 Aug 2026. It requires machine-readable marking of synthetic images and video. The free and open-source exemption does not cover Art. 50. The grace period to 2 Dec 2026 only covers systems on the market before 2 Aug 2026.
  - California's SB 1000 (signed 30 Sep 2026, as reported by law firms) removed the 1M-user threshold. "Covered provider" now means any producer of a GenAI system that is publicly accessible in California.
  - The LTX-2.x and FLUX licences also require disclosure, or forbid removing provenance.
  - Recommendation: harnessUI stamps metadata (C2PA plus IPTC) on every local output, and adds an invisible watermark where it is cheap. Get legal review.

## 2. Engines and bundled dependencies

| Component | Licence | Notes |
|---|---|---|
| stable-diffusion.cpp (leejet) | MIT | https://github.com/leejet/stable-diffusion.cpp. Backends: CPU, CUDA, Vulkan, Metal, OpenCL, SYCL. Writes generation parameters into the PNG as a webui-style text string. |
| ggml (submodule, leejet/ggml fork) | MIT | Same as llama.cpp's ggml. |
| thirdparty/ | nlohmann json MIT; kuba--/zip Unlicense with miniz (public domain / Unlicense); darts-clone BSD-2; cpp-httplib MIT; stb_image/resize/write public domain or MIT; Oniguruma BSD-2; utf8proc MIT plus Unicode data licence | https://github.com/leejet/stable-diffusion.cpp/blob/master/thirdparty/README.md. Its CMake installs the Oniguruma and utf8proc licence files, so ship them in THIRD-PARTY-NOTICES. |
| libwebp, libwebm (submodules) | BSD-3 | WebP and WebM (VP8) output. Video formats: webm (default), animated webp, avi (MJPG). No ffmpeg, no H.264 patent exposure. |
| NVIDIA CUTLASS (submodule, CUDA builds only) | BSD-3 | A CUDA pack also needs NVIDIA's redistributable runtime libraries under the CUDA EULA, the same as the existing llama.cpp CUDA pack. |
| MLX (Apple) | MIT | https://github.com/ml-explore/mlx |
| mflux | MIT | https://github.com/filipstrand/mflux. Supports Z-Image, Krea 2, FLUX.2, FLUX 3 Image Klein, Iris-3B. Python. |
| mlx-video (Prince Canuma) | MIT | https://github.com/Blaizzy/mlx-video. Supports LTX-2/2.3 and Wan 2.1/2.2. It depends on opencv-python and imageio-ffmpeg, whose bundled ffmpeg builds carry LGPL or GPL terms. Check this before shipping it as a pack. It also needs a Python runtime, so it would be a separate download, not part of the 150 MB installer. |
| DiffusionKit (Argmax) | MIT | https://github.com/argmaxinc/DiffusionKit. Older; covers SD3 and FLUX.1. |
| c2pa-rs (for labelling) | MIT or Apache-2.0 | https://github.com/contentauth/c2pa-rs |
| Meta Video Seal / PixelSeal / AudioSeal (watermark) | MIT | https://github.com/facebookresearch/videoseal. Lightricks ships these weights as https://huggingface.co/Lightricks/LTX-2-Marking (MIT). |

Model licences cannot infect the app's MIT code. Weights are data loaded at run time, and none of the candidate licences is copyleft over the software that runs them. The installer cap stays safe as long as no weights and no Python stack go in the installer. Engines go out as packs, the same way ROCm and CUDA packs do.

## 3. Model families

Column meanings:
- **(a) download**: the app fetches the files from Hugging Face for the user.
- **(b) commercial outputs**: whether the user may use what they generate commercially.
- **(c) gate**: Hugging Face's `gated` value.
- **(d) region**: territorial limits.

### Green: permissive and suitable for the default catalog

| Model | Licence | Gate | Commercial outputs | Notes |
|---|---|---|---|---|
| FLUX.1 [schnell] | Apache-2.0 | **auto** (login and click, no form) | Yes | The official repo is gated even though it is Apache. Ungated Apache copies: Comfy-Org/flux1-schnell, city96/FLUX.1-schnell-gguf. |
| FLUX.2 [klein] 4B and base-4B; FLUX.2-small-decoder | Apache-2.0 | none | Yes | The repo includes its own vae/ and Qwen3-4B encoder. The card lists prohibited uses but says it does not modify the licence. https://huggingface.co/black-forest-labs/FLUX.2-klein-4B |
| Z-Image / Z-Image-Turbo (Tongyi) | Apache-2.0 | none | Yes | Encoder Qwen3-4B (Apache). VAE is the FLUX.1 ae (Apache through schnell or Comfy-Org). Supported by sd.cpp and mflux. |
| Qwen-Image, Qwen-Image-2512, Qwen-Image-Edit-2511 | Apache-2.0 | none | Yes | 20B transformer plus Qwen2.5-VL-7B encoder (Apache). **Not 2.1; see Red.** |
| Wan 2.1 / 2.2 (T2V 1.3B, TI2V-5B, A14B, Animate-2, Dancer) | Apache-2.0 | none | Yes | Card: "We claim no rights over your generated contents". UMT5-XXL encoder is Apache. Comfy-Org repackages are Apache. https://huggingface.co/Wan-AI/Wan2.2-TI2V-5B |
| LingBot-Video dense 1.3B / MoE 30B-A3B (robbyant, Jul 2026) | Apache-2.0 | none | Yes | Uses Qwen3-VL-4B (Apache) and the Wan2.1 VAE. Supported by sd.cpp. A small video option. |
| HiDream-O1-Image (May 2026) | MIT | none | Yes | Self-contained, with no external encoder. Its optional prompt agent uses Gemma-4-31B, which is Apache. Supported by sd.cpp. |
| MOSS-Image (OpenMOSS) | Apache-2.0 | none | Yes | Supported by sd.cpp. |
| Mochi 1 preview | Apache-2.0 | none | Yes | Not supported by sd.cpp and very heavy, so low priority. |
| CogVideoX-2B | Apache-2.0 | none | Yes | Not supported by sd.cpp. |
| SDXL base/refiner 1.0, SD 1.5 | CreativeML Open RAIL++-M / OpenRAIL-M | none | Yes. "Licensor claims no rights in the Output", but no use of the output may break Attachment A. | Attachment A covers minors, defamation, harmful false information, automated legal decisions, medical advice and similar. Anyone who redistributes must pass those restrictions on as enforceable terms. https://huggingface.co/stabilityai/stable-diffusion-xl-base-1.0/blob/main/LICENSE.md |

**SDXL fine-tunes vary:**
- RealVisXL V5 and Animagine XL 4.0 are openrail++.
- Juggernaut-XL v9 is tagged creativeml-openrail-m.
- Illustrious is under the Fair AI Public License 1.0-SD, a different licence with copyleft-style terms for derivative models.
- Many popular SDXL and Illustrious merges are tagged `not-for-all-audiences` (for example TheReprinter/WAI-NSFW-illustrious-SDXL).

Curate SDXL entries one by one. Do not search-and-add.

### Amber: allowed with conditions or revenue limits, behind a licence screen

| Model | Licence | Gate | Commercial outputs | Key conditions |
|---|---|---|---|---|
| SD3.5 Large / Medium / Large Turbo; SDXL-Turbo | Stability AI Community License (5 Jul 2024) | auto, with a form: name, email, country, organisation, intended use, marketing opt-in | Yes if you and your affiliates earn **under $1M a year**. Commercial use requires **registering** at stability.ai/community-license. You own outputs. | Must follow the AUP (https://stability.ai/use-policy). No training of foundation models on outputs. Distributors must include the licence, a NOTICE file, and "Powered by Stability AI". SDXL-Turbo's HF metadata says `sai-nc-community`, but its LICENSE.md is the Community License; trust the file. https://huggingface.co/stabilityai/stable-diffusion-3.5-large/blob/main/LICENSE.md |
| FLUX.3 image klein / klein-base (Oct 2026) | **FLUX Kommunity License v1.0** | auto | Only if you are a "Qualifying User": gross annualised revenue with affiliates **under $5M**. Otherwise non-commercial only. | Section 2(e): you must run content filters **or** review outputs before sharing them, and disclose AI generation where the law requires. No use of outputs to improve other models. Export-control clause. Revocable on notice. The klein-pup prompt upsampler is a separate repo and is Apache-2.0. https://huggingface.co/black-forest-labs/flux-3-image-klein/blob/main/LICENSE.md |
| LTX-2 / **LTX-2.3** | LTX-2 Community License (5 Jan 2026) | none | Any purpose for entities **under $10M** annual revenue, affiliates included. Above that, a paid licence; unlicensed use costs double fees as liquidated damages. | Attachment A: item 5, disclose machine-generated content; item 7, no deepfake impersonation without consent; item 19, do not circumvent filters or safety features; **item 20, no use "in any product, service, or application that directly competes with Licensor's commercial products"**. That last item is ambiguous for a free video app; consider asking Lightricks. LTX-2.3 in sd.cpp needs **Gemma 3 12B** (Gemma Terms). https://github.com/Lightricks/LTX-2/blob/main/LICENSE-2 |
| **LTX-2.5** (Jul 2026) | LTX-2.x Community License (11 Aug 2026, amended 29 Oct 2026) | auto. The gate text says the user "consent[s] to receive offers and updates including targeted and personalized advertisements". | Under $10M as above. A non-commercial carve-out exists for larger entities. | "AI Regulations" clause, naming the EU AI Act and the California AI Transparency Act: you must keep, including **within any application** through which LTX-2.x or its outputs are made available, all "metadata, watermarking, content provenance, latent disclosure" features. The licensor may restrict usage remotely. You must make reasonable efforts to use the latest version. Its Gemma-4-12B encoder with projection is bundled in the repo; Gemma 4 is Apache-2.0. https://github.com/Lightricks/LTX-2/blob/main/LICENSE-2_x |
| Krea 2 Raw / Turbo (Jun 2026) | Krea 2 Community License v1 | auto, with a form: name, email, company | Commercial only if revenue is **under $1M** (trailing 12 months). | AUP. Disclose AI generation where required. No circumventing provenance. |
| HiDream-I1 Full / Dev / Fast | MIT for the transformer | **The Llama dependency is manual-gated** | Yes; the card says "You own all content you create". | Needs meta-llama/Llama-3.1-8B-Instruct (Llama 3.1 Community License: AUP, 700M MAU limit; Meta reviews each request by hand). Also T5 (Apache) and the FLUX.1 schnell VAE (Apache). The manual gate makes it a poor fit for one-click download. |
| CogVideoX-5B / 1.5-5B | CogVideoX License | none | Free for academic research. Commercial use requires **registration** at open.bigmodel.cn and fewer than 1M visits a month. | Chinese law and courts. Vague "national security" clause. Not supported by sd.cpp. |
| Chroma / Chroma1-HD | Apache-2.0 (based on FLUX.1-schnell) | none | Yes | **The licence is fine but the content risk is high.** The `lodestones/Chroma` repo is tagged `not-for-all-audiences` and advertises "Fully uncensored". Chroma1-HD "has not been aligned with a specific safety filter". Keep it out of the default list, or show it only behind the content notice. |

### Red: exclude from the default catalog, or show only as "personal or research use" behind an explicit opt-in

| Model | Licence | Gate | Commercial outputs | Why red |
|---|---|---|---|---|
| FLUX.1 [dev], FLUX.1 Kontext [dev], FLUX.1 Krea [dev] | FLUX.1 [dev] Non-Commercial License v1.1.1 | auto | 2(d): "You may use Output for any purpose (including for commercial purposes)". **But** 4(a) bars commercial use of "any data produced by the FLUX.1 [dev] Model". The two clauses conflict, so treat the answer as "unclear". | The model itself is non-commercial. "Non-commercial" excludes any use "in direct interactions with or that has impact on end users". 2(e) requires content filters or review, plus AI disclosure where the law requires it. Revocable on notice. https://huggingface.co/black-forest-labs/FLUX.1-dev/blob/main/LICENSE.md |
| FLUX.2 [dev], FLUX.2 [klein] 9B / base-9B | FLUX Non-Commercial License v2.1 | auto | Same pattern as FLUX.1 dev. | Same as above. https://huggingface.co/black-forest-labs/FLUX.2-dev/blob/main/LICENSE.md |
| **Qwen-Image-2.1 / 2.1-Turbo** (Sep/Oct 2026) | **Qwen Research License** (20 Sep 2026) | **none** | No. "Non-Commercial shall mean for research or evaluation purposes only". | Even hobby use is not clearly covered. Chinese law, Hangzhou courts. Commercial licence via model-business@notice.qwencloud.com. https://huggingface.co/Qwen/Qwen-Image-2.1/blob/main/LICENSE |
| HunyuanVideo, HunyuanVideo-1.5, HunyuanImage-2.1/3.0 | Tencent Hunyuan Community License | none | Tencent claims no rights in outputs, **but** "You must not use, reproduce, modify, distribute, or display the Tencent Hunyuan Works, **Output** or results ... outside the Territory". | **Territory = worldwide except the EU, UK and South Korea.** A licence is needed above 100M MAU. AUP. HunyuanVideo 1.5 is supported by sd.cpp. https://github.com/Tencent-Hunyuan/HunyuanVideo-1.5/blob/master/LICENSE |
| MiniMax-H3 (video), MiniMax-H3-Image, PrunaAI/P-H3-Image | MiniMax H3 Community License (2 Aug 2026) | none | Within the territory only. | **Excluded territories: EU, UK, Republic of Korea and USA, outputs included.** Authorisation needed above $20M revenue. "MiniMax H3" must appear on the UI of commercial products. **"If you provide or make available to any Third Party a product ... that permits the generation of Outputs ... you must ... implement ... safeguards"**, which would bind harnessUI directly. Supported by sd.cpp. https://huggingface.co/MiniMaxAI/MiniMax-H3/blob/main/LICENSE |

Not red, but outside these choices: Tencent Hy-Nomas (Oct 2026) is **Apache-2.0** with no territory limit. Tencent's first such model, as far as I found. It is a multi-segment world model that needs a 4090-class GPU and is not supported by sd.cpp. https://huggingface.co/tencent/Hy-Nomas

I did not review licences for the other model families sd.cpp now supports: Lens, PiD, Ideogram4, ERNIE-Image, Anima, Boogu, Mage-Flow, SeFi, LongCat, Ovis, SenseNova, Fibo 2.0, Iris-3B, Krea2 variants, PixArt and others. Check each before adding it.

## 4. Text encoders, VAEs and mirror pitfalls

**Encoder licences:**
- T5 v1.1 XXL: Apache-2.0.
- UMT5-XXL: Apache-2.0.
- OpenAI CLIP ViT-L/14: MIT (https://github.com/openai/CLIP/blob/main/LICENSE; the HF repo has no licence metadata).
- OpenCLIP ViT-bigG: MIT.
- Qwen2.5-VL-7B, Qwen3-4B/8B, Qwen3-VL-4B/8B: Apache-2.0.
- Mistral Small 3.1/3.2 24B: Apache-2.0.
- Gemma 4: Apache-2.0 (https://gemma.ai/resources/apache-2/).
- **Gemma 3**: Gemma Terms of Use, last modified 1 Apr 2026 (https://gemma.ai/resources/terms/).
  - The Prohibited Use Policy is incorporated into the terms.
  - Google "claims no rights in Outputs".
  - Anyone who redistributes must pass the restrictions on and include a NOTICE file.
  - There is no revenue limit and no region limit.
  - google/gemma-3-12b-it reports `gated: manual`, although its gate text says "Requests are processed immediately".
- **Llama 3.1 8B**: Llama 3.1 Community License. AUP, 700M MAU limit, and "Built with Llama" for distributors. Gated, manual.

**Pitfalls to encode in the catalog:**
1. **FLUX.1 VAE.** The sd.cpp docs for flux, chroma and kontext link `black-forest-labs/FLUX.1-dev/ae.safetensors`, which is non-commercial and gated. The same 335,304,388-byte `ae.safetensors` is in FLUX.1-schnell (Apache, gated auto). It is also in **Comfy-Org/z_image_turbo `split_files/vae/ae.safetensors`** (Apache, ungated, sha256 afc8e282…529e38). Gated repos hide their hashes, so the files match on size only; byte identity is not verified.
2. **FLUX.2 VAE.** The sd.cpp flux2 and flux3 docs link FLUX.2-dev (non-commercial, gated). Use **Comfy-Org/flux2-klein-4B `split_files/vae/flux2-vae.safetensors`** instead (Apache, ungated, 336,211,292 bytes, sha256 868fe7b3…ce8f3; same size as FLUX.2-dev's ae.safetensors). Or use FLUX.2-klein-4B's own `vae/` folder, or FLUX.2-small-decoder.
3. **Ungated mirrors of gated or restricted models are still bound by the original licence:**
   - unsloth/gemma-3-12b-it-GGUF is ungated, but Gemma Terms still apply. sd.cpp's LTX-2.3 docs use this repo.
   - unsloth/LTX-2.5-GGUF is ungated, but the LTX-2.x licence still applies.
   - city96/FLUX.1-dev-gguf is ungated, but the FLUX dev non-commercial licence still applies.
4. **Some mirror licence labels are wrong or missing:**
   - vantagewithai/LTX-2.5-GGUF, which sd.cpp links, labels itself with the *old* LTX-2 licence.
   - leejet/flux-3-image-klein-GGUF and leejet/MiniMax-H3-GGUF have **no** licence metadata.
   - **AtomicChat/Qwen-Image-2.1-Turbo-Uncensored-GGUF** (614k downloads) claims `apache-2.0` for a model whose upstream licence is the research-only Qwen Research License.

   Rule: take the licence from the upstream `base_model` chain. Warn whenever a repo's own label differs from its base model's.

## 5. What this means for (a)–(d)

**(a) The app downloads for the user and does not redistribute.**
- The user downloads directly from the publisher's (or a mirror's) Hugging Face repo and becomes the licensee. That is how ComfyUI and similar managers work.
- The FLUX, Stability, LTX, Hunyuan and MiniMax licences define "Distribution" as providing or making available a copy. harnessUI does neither if it never hosts or mirrors weights.
- Keep it that way: no weights on GitHub releases or a CDN. Otherwise each redistribution duty applies: licence copy, NOTICE files, "Powered by Stability AI", BFL's attribution notice, Gemma's NOTICE, passing on OpenRAIL use restrictions, and territorial limits.
- Three licences reach the *application* anyway:
  - MiniMax H3: required safeguards.
  - LTX-2.x: keep provenance features "within any application".
  - LTX-2 item 20: the competing-product clause.
- For the FLUX dev and Kommunity licences, the user is the licensee who must filter or review outputs. The app should make that easy and visible, but it is not the licensee.
- Always use the official repo for non-permissive models, or record the user's acceptance of the upstream licence in the app. Picking an ungated mirror precisely so the user never sees the terms is the wrong pattern.

**(b) Commercial use of outputs.**
- **Yes:** Apache and MIT models; OpenRAIL models within the use restrictions.
- **Yes below a revenue limit:**
  - SD3.5: $1M, plus registration.
  - Krea 2: $1M.
  - FLUX.3 klein: $5M.
  - LTX-2/2.3/2.5: $10M.
  - CogVideoX-5B: registration and fewer than 1M visits a month.
- **Unclear:** FLUX.1/FLUX.2 dev outputs (clause 2(d) against 4(a)).
- **No:** Qwen-Image-2.1.
- **Outside the territory only:** Hunyuan; MiniMax H3, which also excludes the US.
- Several licences make AI disclosure part of using the output: LTX Attachment A item 5; the FLUX 2(e)(ii) family; Krea.

**(c) Gated repos.**
- Access can be requested only in a logged-in browser. Automatic approval is instant; manual approval can take days or be refused, and authors can revoke access at any time (https://huggingface.co/docs/hub/models-gated).
- Downloads then need the user's token. A fine-grained token must allow reading gated repos; please verify the exact token setting.
- harnessUI already maps HTTP 401 to "add a Hugging Face token in Settings" and 403 to "accept its terms" (crates/models/src/hf.rs, crates/models/src/download.rs). The catalog should also tell the user:
  - **Which form they will meet.** FLUX: just "Agree". SD3.5: name, email, country, organisation, intended use, marketing opt-in. Krea 2: name, email, company. Gemma: "Acknowledge license". Llama: manual review.
  - **That the publisher receives their HF username and email.**
  - **For LTX-2.5, that clicking "Agree and Access" consents to targeted advertising.**
- After the user accepts, retry the download automatically.

**(d) Regional restrictions.**
- Hunyuan: EU, UK and South Korea, outputs included.
- MiniMax H3: EU, UK, South Korea and USA, outputs included.
- FLUX, LTX and Qwen have sanctions or export-control clauses (BFL: "will not disguise your location through IP proxying").
- Publishers can block EU users by IP with `extra_gated_eu_disallowed`, but only on gated repos. None of the candidates use it; Hunyuan and MiniMax are ungated.
- harnessUI should not geolocate users. Keep both families out of the catalog. In search results, show a red notice naming the excluded territories and require explicit confirmation before downloading.

## 6. How the catalog should show licences before download

Reuse the pack pattern:
- The UI dialog in src/components/BackendDownloads.tsx: "Downloading means you accept these terms", "Read the license", "Accept and download".
- The backend refusal in src-tauri/src/packs_cmd.rs, where `download_pack` errors unless `accept_license` is set.

Today `catalog/models.json` stores only `license: {spdx}`, and `LICENSE_WORDS` in src/lib/abilities.ts turns any custom id into "Another license (read the model card)". That is too little for Studio.

**Schema.** For each entry:

```json
"license": {
  "id": "flux-kommunity-1.0",
  "name": "FLUX Kommunity License v1.0",
  "url": "https://huggingface.co/black-forest-labs/flux-3-image-klein/blob/<pinned-rev>/LICENSE.md",
  "sha256": "<sha of LICENSE file at the pinned revision>",
  "commercialOutputs": "yes|under-revenue|registration|unclear|no",
  "revenueLimitUsd": 5000000,
  "excludedRegions": [],
  "obligations": ["content-filter-or-review", "ai-disclosure-where-required", "keep-provenance"],
  "aup": "https://bfl.ai/legal/usage-policy",
  "attribution": null,
  "plainWords": "Free for personal use. Commercial use of pictures only if your yearly revenue is under $5M."
},
"components": [
  {"role": "text_encoder", "repo": "...", "revision": "...", "license": { ... }},
  {"role": "vae", "repo": "...", "revision": "...", "license": { ... }}
],
"gate": {"mode": "auto|manual|none", "collects": ["username", "email", "country", "company", "marketing-consent"]},
"contentNotes": "uncensored|none"
```

**The effective licence is the strictest across all components.**

**Dialog wording.** "Download Z-Image Turbo (12.3 GB) from Hugging Face. Licences: Apache 2.0 (model, text encoder, VAE). Pictures: free for any use, including commercial. [Read the licences] [Accept and download]". For an amber model, add a line such as "Commercial use only if your organisation earns less than $1M a year (registration required)". For a red model: "Personal and research use only. Not licensed in: EU, UK, South Korea."

**Badges and filter.**
- Green: "Free for commercial use".
- Amber: "Commercial use under $X revenue".
- Red: "Non-commercial".
- Grey: "Region-limited".
- The default catalog shows green and amber. "Show non-commercial and region-limited models" is an explicit switch.

**Enforcement.**
- `download_model(id, accepted_license_sha256s)` refuses unless every component's pinned licence hash is accepted.
- Record the acceptance (licence id, hash, timestamp) locally.
- Ask again when a new pinned revision changes a licence file.
- Pin LICENSE files in the catalog alongside the weights, so the text shown matches the revision downloaded.

**Hugging Face search (outside the catalog).**
- Follow `base_model` and show the base model's licence.
- Warn on a label mismatch.
- **Hide repos tagged `not-for-all-audiences` by default** (filter client-side). Today nothing in crates/ or src/ filters on this tag.
- Show the gate mode and the region notice.

## 7. Content safety for local models

**There is no company filter.** Some models are trained to resist abuse:
- BFL filtered FLUX.2 klein's training data with the IWF, and its repo ships optional NSFW and protected-content filters.
- Mochi "limit[s] NSFW content".

Others are openly uncensored: Chroma, and "Uncensored" re-uploads such as the AtomicChat GGUF. The Hugging Face content policy (10 Apr 2025) bans "Underage nudity or any sexual Content involving minors" and content "created without explicit consent" (https://huggingface.co/content-policy).

**Law.**
- AI-generated CSAM is illegal in the US, UK, EU and elsewhere.
- The UK **Crime and Policing Act 2026** (Royal Assent 29 Apr 2026) creates an offence to "adapt, possess, supply or offer to supply a CSA image generator", with up to 5 years in prison. The factsheet says it "will not criminalise AI developers"; commencement is unclear.
- Non-consensual intimate deepfakes are criminal in many places. The US TAKE IT DOWN Act (2025) covers publishing them; https://www.congress.gov/bill/119th-congress/senate-bill/146.
- EU AI Act Art. 50(4) requires deployers (users) to label deepfakes. Separately, Art. 50(2) requires *providers* to mark outputs in a machine-readable way. The Commission's voluntary code expects at least two techniques, for example signed metadata plus an imperceptible watermark, and a free detection mechanism.
- California SB 942, as amended by AB 853 and SB 1000:
  - Latent disclosures must carry the provider name, system name and version, time and date, a unique ID, and whether content was created or altered.
  - A disclosure verification tool is required; a third-party tool may be used.
  - Duties for "GenAI hosting platforms" start 1 Jan 2027. A curated download catalog might count as one; legal review needed.

**What reasonable apps do, and what I recommend for harnessUI:**
1. **One-time Studio notice before the first local download, and in the help.** Suggested words: "Local models run only on your computer. No company checks your prompts or pictures. Don't create sexual images of children, or sexual or intimate images of real people without their consent, and don't impersonate people. These are crimes in many countries. Each model's licence and use policy also applies."
2. **A minimal, always-on local prompt block** for sexual terms combined with terms for minors. It is cheap, and it matches the "reasonable safeguards" language in the MiniMax and Krea licences.
3. **No NSFW or "uncensored" models or LoRAs in the catalog.** Hide NFAA repos in search by default.
4. **For FLUX dev, FLUX.2 9B and FLUX.3 Kommunity models**, tell users their licence requires a content filter *or* a review of outputs before sharing. The gallery's preview-before-export step supports the review option. An optional downloadable NSFW classifier would support the filter option (licence not yet checked).
5. **Label every local output.**
   - A C2PA manifest via c2pa-rs (MIT or Apache).
   - XMP/IPTC `DigitalSourceType = trainedAlgorithmicMedia`, plus PNG and WebM metadata naming harnessUI, the model and the revision.
   - Optionally an invisible PixelSeal / Video Seal mark (MIT). This is the mechanism Lightricks ships for LTX.
   - A "Check a file" verifier.
   - This differs from the cloud design ("No metadata is stamped into them in 0.1.0", docs/dev/0.1.0/studio-design.md). For local output the app is the only party that can mark it.
6. **Update the terms page** (/home/user/harnessUI-website/public/terms.html). It currently says only "Models you download come with their own licences" and "Follow the law". Add a section "Pictures and videos you make" that:
   - incorporates each model's licence and use policy by reference;
   - lists the core prohibitions;
   - says outputs carry an AI label that must not be removed where the law requires it;
   - says local generation has no company filter.

## 8. Open points to verify, ideally with a lawyer

- Whether harnessUI counts as a "provider" under EU AI Act Art. 50(2) and California's law as amended by SB 1000. My sources for both are law-firm alerts, not the Official Journal or leginfo text.
- How the LTX-2/2.x "directly competes" clause (Attachment A item 20) applies to a free local video app. Consider written confirmation from Lightricks before featuring LTX in the default catalog.
- The conflict in the FLUX dev licences between 2(d) and 4(a) on commercial use of outputs.
- Commencement of the UK CSA-image-generator offence.
- Byte identity of the Apache VAE copies against the gated FLUX.1-dev and FLUX.2-dev files (sizes match; gated hashes are hidden).

## Sources

- **Engines:**
  - https://github.com/leejet/stable-diffusion.cpp
  - https://github.com/leejet/stable-diffusion.cpp/blob/master/thirdparty/README.md
  - https://github.com/ml-explore/mlx
  - https://github.com/filipstrand/mflux
  - https://github.com/Blaizzy/mlx-video
  - https://github.com/argmaxinc/DiffusionKit
  - https://github.com/contentauth/c2pa-rs
  - https://github.com/facebookresearch/videoseal
  - https://huggingface.co/Lightricks/LTX-2-Marking
- **FLUX:**
  - https://huggingface.co/black-forest-labs/FLUX.1-schnell
  - https://huggingface.co/black-forest-labs/FLUX.1-dev/blob/main/LICENSE.md
  - https://huggingface.co/black-forest-labs/FLUX.2-dev/blob/main/LICENSE.md
  - https://huggingface.co/black-forest-labs/FLUX.2-klein-4B
  - https://huggingface.co/black-forest-labs/FLUX.2-klein-9B
  - https://huggingface.co/black-forest-labs/flux-3-image-klein/blob/main/LICENSE.md
- **Stability:**
  - https://huggingface.co/stabilityai/stable-diffusion-xl-base-1.0/blob/main/LICENSE.md
  - https://huggingface.co/stabilityai/stable-diffusion-3.5-large/blob/main/LICENSE.md
  - https://huggingface.co/stabilityai/sdxl-turbo/blob/main/LICENSE.md
  - https://stability.ai/use-policy
  - https://stability.ai/community-license
- **Qwen:**
  - https://huggingface.co/Qwen/Qwen-Image
  - https://huggingface.co/Qwen/Qwen-Image-2.1/blob/main/LICENSE
  - https://huggingface.co/AtomicChat/Qwen-Image-2.1-Turbo-Uncensored-GGUF
- **Chroma, Z-Image, HiDream, Llama:**
  - https://huggingface.co/lodestones/Chroma
  - https://huggingface.co/lodestones/Chroma1-HD
  - https://huggingface.co/Tongyi-MAI/Z-Image-Turbo
  - https://huggingface.co/HiDream-ai/HiDream-I1-Full
  - https://huggingface.co/HiDream-ai/HiDream-O1-Image
  - https://huggingface.co/meta-llama/Llama-3.1-8B-Instruct
- **Wan, LingBot:**
  - https://huggingface.co/Wan-AI/Wan2.2-TI2V-5B
  - https://huggingface.co/robbyant/lingbot-video-dense-1.3b
- **LTX:**
  - https://github.com/Lightricks/LTX-2/blob/main/LICENSE-2
  - https://github.com/Lightricks/LTX-2/blob/main/LICENSE-2_x
  - https://huggingface.co/Lightricks/LTX-2.3
  - https://huggingface.co/Lightricks/LTX-2.5
  - https://huggingface.co/Lightricks/LTX-Video
  - https://huggingface.co/vantagewithai/LTX-2.5-GGUF
- **Tencent, MiniMax, Krea:**
  - https://github.com/Tencent-Hunyuan/HunyuanVideo-1.5/blob/master/LICENSE
  - https://huggingface.co/tencent/HunyuanVideo
  - https://huggingface.co/tencent/Hy-Nomas
  - https://huggingface.co/MiniMaxAI/MiniMax-H3/blob/main/LICENSE
  - https://huggingface.co/krea/Krea-2-Turbo
- **CogVideoX, Mochi:**
  - https://huggingface.co/zai-org/CogVideoX-5b/blob/main/LICENSE
  - https://huggingface.co/zai-org/CogVideoX-2b
  - https://huggingface.co/genmo/mochi-1-preview
- **Encoders:**
  - https://huggingface.co/google/t5-v1_1-xxl
  - https://huggingface.co/google/umt5-xxl
  - https://github.com/openai/CLIP/blob/main/LICENSE
  - https://huggingface.co/laion/CLIP-ViT-bigG-14-laion2B-39B-b160k
  - https://huggingface.co/Qwen/Qwen3-VL-4B-Instruct
  - https://huggingface.co/google/gemma-3-12b-it
  - https://huggingface.co/unsloth/gemma-3-12b-it-GGUF
  - https://gemma.ai/resources/terms/
  - https://gemma.ai/resources/apache-2/
- **Apache VAE copies:**
  - https://huggingface.co/Comfy-Org/z_image_turbo
  - https://huggingface.co/Comfy-Org/flux2-klein-4B
  - https://huggingface.co/Comfy-Org/flux1-schnell
- **Hugging Face:**
  - https://huggingface.co/docs/hub/models-gated
  - https://huggingface.co/content-policy
- **EU AI Act:**
  - https://www.paulweiss.com/insights/client-memos/eu-finalises-transparency-rules-for-ai-generated-content
  - https://www.gibsondunn.com/eu-ai-act-omnibus-agreement-postponed-high-risk-deadlines-and-other-key-changes/
  - https://www.joneswalker.com/en/insights/blogs/ai-law-blog/yes-august-2-still-matters-the-eu-approved-a-high-risk-ai-delay-but-most-trans.html
- **California:**
  - https://www.mofo.com/resources/insights/261001-california-revamps-ai-transparency-requirements
  - https://www.lcwlegal.com/news/sb-1000-expands-and-revises-california-ai-transparency-requirements/
  - https://calmatters.digitaldemocracy.org/bills/ca_202520260sb1000
- **UK:**
  - https://www.gov.uk/government/publications/crime-and-policing-act-2026-factsheets/crime-and-policing-act-2026-child-sexual-abuse-material-factsheet
- **US:**
  - https://www.congress.gov/bill/119th-congress/senate-bill/146
- **harnessUI files referenced:**
  - /home/user/harnessUI/src/components/BackendDownloads.tsx
  - /home/user/harnessUI/src-tauri/src/packs_cmd.rs
  - /home/user/harnessUI/catalog/models.json
  - /home/user/harnessUI/crates/models/src/catalog.rs
  - /home/user/harnessUI/crates/models/src/hf.rs
  - /home/user/harnessUI/crates/models/src/download.rs
  - /home/user/harnessUI/src/lib/abilities.ts
  - /home/user/harnessUI/docs/dev/0.1.0/studio-design.md
  - /home/user/harnessUI-website/public/terms.html
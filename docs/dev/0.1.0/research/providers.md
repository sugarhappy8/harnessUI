## Cloud image and video generation APIs, November 2026: findings and a v1 recommendation for harnessUI 0.1.0

I checked everything on 2026-11-17, using official docs where I could. "Observed" means I queried a live public endpoint myself. "Third-party" means it comes from a blog or reseller and needs checking before it is relied on.

### 1. What changed that matters for the design

- **OpenAI no longer has a video API.** The Videos API, sora-2 and sora-2-pro shut down on Sep 24, 2026, with "No one-to-one replacement" (https://developers.openai.com/api/docs/deprecations).
- **OpenAI's image models are moving fast.**
  - Current models: `gpt-image-2.6-sunburst` and `gpt-image-2.6-flare`.
  - Retiring models:
    - `gpt-image-1.5`, `gpt-image-1-mini` and `chatgpt-image-latest` shut down Dec 1, 2026.
    - `gpt-image-2` shuts down Feb 10, 2027.
  - Already gone: `gpt-image-1` (Oct 23, 2026) and dall-e-2/3 (May 12, 2026).
  - Source: same deprecations page.
- **Google's Gemini API changed its video model and its request style.**
  - Veo 3.1 preview IDs were shut down in the Gemini API on Oct 22, 2026. Gemini Omni Flash (`gemini-omni-1.2-flash`) is the video model now (https://aistudio.google.com/docs/video).
  - Image and video generation now use the new Interactions API. `generateContent` is labelled "legacy" but still supported (https://ai.google.dev/gemini-api/docs/migrate-to-interactions).
  - Google's image page calls Imagen legacy and shut down (https://aistudio.google.com/docs/image-generation).
  - Veo 3.1 GA reportedly remains on Google Cloud only (third-party: https://github.com/benderandrew069/google-veo-3-api-status-2026). It is also still sold through OpenRouter, Replicate and Runway.
- **OpenRouter now has dedicated image and video APIs.**
  - Image API launched Jun 23, 2026: POST /api/v1/images. "new image models will be added exclusively to the dedicated Image API". Chat-completions image output keeps working only for older models (https://openrouter.ai/blog/announcements/image-api/).
  - Video API launched Apr 15, 2026: POST /api/v1/videos (https://openrouter.ai/blog/announcements/video-generation/).
  - Both have public catalog endpoints with typed, per-model capabilities and pricing. Observed: 68 image models and 33 video models.
- **Almost every vendor is now an aggregator.** Runway, Ideogram, fal, Replicate and OpenRouter all resell Kling, Seedance, Veo/Omni, GPT Image and Nano Banana.
- **No generation from two of the existing endpoints.** Anthropic has no image output (third-party, consistent: https://www.adityabawankule.io/guides/can-claude-generate-images). For abliteration.ai I found no image or video generation endpoint; it documents chat completions only (https://abliteration.ai/multimodal-llm-api).
- **Model IDs churn monthly.** The UI must come from list endpoints, not hardcoded IDs.

### 2. Provider by provider

#### OpenAI (existing endpoint and key)

**Auth and endpoints**
- Bearer key.
- Images: POST /v1/images/generations and /v1/images/edits. Both are synchronous; edits accept up to about 2 minutes on complex prompts. Optional SSE streaming with `partial_images` 0–3 (each adds 100 output tokens).
- The Responses API also has an `image_generation` tool for multi-turn work (`action`: auto/generate/edit). The 2.6-sunburst model page lists Responses as "Not supported" as a direct model (https://developers.openai.com/api/docs/models/gpt-image-2.6-sunburst).
- Sources: https://developers.openai.com/api/docs/guides/image-generation (both parts of the page).

**Models**
- Sunburst is for precise editing and detail. Flare is for fast everyday images.
- Both 2.6 models do SVG output (generations only, n=1) and animated GIF output (`frame_count` 2–16, `frame_duration_ms` 20–2000).

**Parameters**
- `size`: 1024x1024, 1536x1024, 1024x1536, or a custom WxH:
  - both sides multiples of 16
  - aspect ratio between 1:3 and 3:1
  - longest edge at most 3840
  - total pixels 655,360 to 8,294,400
- `quality`: low, medium, high, xhigh, max, auto (default auto).
- `n` (OpenRouter reports up to 10 for 2.6).
- `output_format`: png, jpeg, webp, svg, gif. `output_compression` 0–100 for jpeg and webp.
- `background`: transparent, opaque, auto. Transparent needs png or webp.
- `moderation`: auto or low.
- `complexity`: low, medium, high; default high, no extra fee.
- Editing:
  - `mask`: alpha PNG, applies to the first image, masking is prompt-guided.
  - `markup`: transparent PNG overlay; cannot be combined with mask.
  - Reference images: OpenRouter's catalog says up to 16 for 2.6.
- `safety_identifier`.
- No seed or negative prompt is documented (unconfirmed that they are absent).

**Output:** base64 (`b64_json`). No URL output appears in the docs.

**Prices**
- GPT Image 2.x: image input $8, image output $30, cached input $2 per 1M tokens. Batch is half price (https://developers.openai.com/api/docs/pricing).
- gpt-image-2 per image at 1024²: low $0.006, medium $0.053, high $0.211 (image guide).
- 2.5/2.6 per image: roughly $0.005–$0.211 (third-party, dated Oct 20, 2026: https://costgoat.com/pricing/openai-images).
- Runway resells 2.6 at 2–19 credits, which is $0.02–$0.19 per image (https://docs.dev.runwayml.com/guides/pricing/).

**Policy and limits**
- All prompts and outputs are filtered. A block returns `error.code` "moderation_blocked" with `moderation_details`.
- API Organization Verification "may" be required.
- Rate limits depend on usage tier; no figures are published.

#### Google Gemini API

**Request style**
- POST https://generativelanguage.googleapis.com/v1beta/interactions, with `Api-Revision: 2026-05-20` in the examples.
- API key auth. The header name was not re-verified; it is usually `x-goog-api-key`.

**Image models** (https://aistudio.google.com/docs/image-generation)
- `gemini-nano-banana-2.1`: main model.
- `gemini-3.1-flash-lite-image`: cheapest; no multiple references, multi-turn or grounding.
- `gemini-3.1-flash-image`: previous generation.
- `gemini-3-pro-image`: premium.
- `gemini-2.5-flash-image`: legacy, shuts down Mar 15, 2027.

**Image parameters**
- `response_format` {type:"image", mime_type jpeg/png, aspect_ratio, image_size}.
  - `image_size`: 1K, 2K, 4K (uppercase K). 512px only on 3.1 Flash.
  - Aspect ratios include 1:4, 4:1, 1:8 and 8:1.
- `thinking_level`.
- Up to 14 reference images.
- `google_search` tool, including image search.
- Multi-turn editing via `previous_interaction_id`.
- No seed or n is documented. OpenRouter shows n max 1.

**Image output:** base64 in `output_image.data`. Every image carries a SynthID watermark.

**Image prices** (https://aistudio.google.com/docs/pricing)

| Model | Price per image |
|---|---|
| nano-banana-2.1 | $0.0336 (1K), $0.0504 (2K), $0.113 (4K); batch half |
| 3.1-flash-image | $0.045–$0.151 |
| flash-lite | $0.0336 |
| 3-pro-image | $0.134 (1K/2K), $0.24 (4K) |
| 2.5-flash-image | $0.039 |

None of these has a free tier.

**Video: `gemini-omni-1.2-flash`** (https://aistudio.google.com/docs/omni)
- Requests to 1.1 route to 1.2 from Dec 3, 2026.
- Synchronous by default (`background=false`). Files over 4MB use `delivery:"uri"` via the Files API.
- `response_format` {type:"video"}:
  - `aspect_ratio`: 16:9 or 9:16.
  - `resolution`: 360p, 720p, 1080p (upscaled), 4k (upscaled).
  - `duration`: "3s"–"15s", default 10s.
- Tasks: text_to_video, image_to_video, reference_to_video, edit, extend.
- Up to 6 reference images.
- First and last frame are supported only for 3–10s clips.
- Extend appends 3–10s per turn, up to 30s total.
- Native audio is always on.
- Negative prompts and temperature are not supported.

**Video price:** about $0.07/s at 720p, $0.023/s at 360p, $0.104/s at 1080p, $0.209/s at 4K (pricing page).

**Restrictions**
- SynthID on all outputs.
- No editing or extending of uploaded videos, and no images of minors, in the EEA, Switzerland and the UK.
- Some recognisable people are unsupported.

#### OpenRouter (existing endpoint and key)

**Images** (https://openrouter.ai/docs/guides/overview/multimodal/image-generation)
- POST /api/v1/images. Synchronous; optional SSE streaming on models that support it.
- Parameters:
  - `model`, `prompt`
  - `n` 1–10 (per-model limits)
  - `resolution` (512/768/1K/1.5K/2K/4K), `aspect_ratio`, `size`
  - `quality`, `output_format` (png/jpeg/webp/svg/gif), `frame_count`, `frame_duration_ms`
  - `background`, `output_compression`, `seed`
  - `input_references` [{type:"image_url",…}], `markup`
  - `provider` (only/order/ignore/sort/allow_fallbacks/zdr/data_collection, plus `options`)
- Response: `data[].b64_json` plus `media_type`, and `usage.cost`.
- Billing is all-or-nothing: failed generations return 502 and are not charged.

**Image catalog**
- GET /api/v1/images/models. Observed: public, no key needed, 68 models.
- Each model lists `supported_parameters` as typed enums and ranges. For example, gpt-image-2.6-sunburst shows quality auto…max, n 1–10, input_references 0–16.
- GET /api/v1/images/models/{id}/endpoints gives per-provider pricing (unit image, megapixel or token, with variants).

**Video** (https://openrouter.ai/docs/guides/overview/multimodal/video-generation)
- POST /api/v1/videos returns 202 {id, polling_url, status}. Poll GET /api/v1/videos/{id}. Download from GET /api/v1/videos/{id}/content?index=0.
- Statuses: pending, in_progress, completed, failed. Webhooks also report cancelled and expired.
- Parameters:
  - `duration`, `resolution`, `aspect_ratio`, `size`
  - `frame_images` (first_frame/last_frame), `input_references`
  - `generate_audio` (default true), `seed`
  - `callback_url`, `provider`
- Output:
  - `unsigned_urls` need the API key.
  - `urls` are signed and work without a key until `expires_at`.
- Video is not eligible for zero data retention.

**Video catalog**
- GET /api/v1/videos/models. Observed: public, 33 models.
- Per model: supported_resolutions, aspect_ratios, sizes, durations, frame_images, generate_audio, seed, `pricing_skus`, and `allowed_passthrough_parameters`.

**Observed video prices** (2026-11-17, from the catalog)

| Model | Price per second |
|---|---|
| Veo 3.1 | $0.40 with audio, $0.20 without; 4K $0.60 |
| Veo 3.1 Fast | $0.10–0.12 |
| Veo 3.1 Lite | $0.05–0.08 |
| Gemini Omni 1.2 Flash | $0.0695 (720p), $0.104 (1080p), $0.209 (4K) |
| Kling 3.0 Pro | $0.112, or $0.168 with audio |
| Kling 3.0 Std | $0.084, or $0.126 with audio |
| Wan 3.0 | $0.05 / $0.10 / $0.20 (480/720/1080p) |
| Hailuo 3 | $0.13 |
| Runway Gen-4.5 | $0.12 |
| FLUX 3 Video | $0.17 (720p) to $0.41 (4K) |
| Grok Imagine Video 1.5 | $0.08 / $0.14 / $0.25 |
| Grok Imagine Video 1.5 Lite | $0.02 / $0.03 / $0.14 |

Seedance is priced per video token.

**Observed image prices**
- FLUX 3 Image $0.041–$0.607 by resolution tier.
- Grok Imagine Image 2.0 $0.04–$0.08, plus $0.01 per input image.
- GPT Image 2.6 at OpenAI's token rates.

**Not seen in the catalogs:** Kling 4, Luma Ray 3.2, MiniMax H3 video, and any Ideogram model.

#### xAI

**Auth:** Bearer key. Base https://api.x.ai/v1, which works with the OpenAI SDK.

**Images** (https://docs.x.ai/developers/model-capabilities/images/generation.md)
- POST /v1/images/generations and /v1/images/edits. Synchronous.
- `n` 1–10. `aspect_ratio` (default auto, includes 21:9 and 5:2). `resolution` 1K/2K. `quality` low/medium/auto. Up to 5 source images for edits.
- Output: temporary URL by default, base64 optional. Expiry is not stated.

**Video** (https://docs.x.ai/developers/model-capabilities/video/generation)
- POST /v1/videos/generations returns {request_id}. Poll GET /v1/videos/{request_id}. Statuses: pending, done, failed.
- Models:
  - `grok-imagine-video-1.5`: native 1080p, lip-synced speech.
  - `-1.5-lite`
  - `grok-imagine-video`: classic model; also the editing model.
- Parameters:
  - `duration` 1–15s (default 8)
  - 9 aspect ratios
  - `resolution` 480p (default) / 720p / 1080p
  - `image` (first frame), `last_frame`, `keyframes` (up to 4)
  - `reference_images` (output capped at 720p), `reference_audios` (up to 3 voices)
  - `generate_audio` (default on)
- Output is a temporary URL. Generation "typically takes up to several minutes". There are also edit and extension endpoints.

**Prices** (https://docs.x.ai/developers/pricing)
- Images: grok-imagine-image $0.02. grok-imagine-image-2.0 $0.04; at low quality $0.06 for 2K and +$0.01 per input image on edits (https://docs.x.ai/developers/migration/imagine-image-quality-nov-2.md).
- Video: classic $0.05/s, 1.5-lite $0.02/s, 1.5 $0.08/s. The page lists one rate per model; OpenRouter shows per-resolution tiers, so xAI's direct tiers are unconfirmed.
- The Batch discount does not apply to image or video.

**Policy and limits**
- "subject to content policy review and is not used for training".
- Only "per-minute" rate limits are mentioned, with no numbers.
- OpenRouter labels xAI's provider "SpaceXAI"; I did not find out why.

#### Black Forest Labs (FLUX)

**Auth and flow** (https://docs.bfl.ml/api_integration/integration_guidelines)
- `x-key` header.
- Hosts: api.bfl.ai (global), api.eu.bfl.ai, api.us.bfl.ai.
- Always asynchronous: POST returns {id, polling_url, cost}, and you must poll the returned `polling_url`.
- Statuses: Pending, then Reasoning and Generating for video, then Ready, Request Moderated, Content Moderated, Error, or Task not found.
- Webhooks are supported.

**Result URLs and limits**
- Image URLs expire in 10 minutes and have no CORS. Video URLs expire about 2 hours after Ready.
- 24 concurrent requests (6 for kontext-max).

**FLUX 3 Image:** POST /v1/flux-3-image (https://docs.bfl.ml/api-reference/utility/generate-an-image-with-flux-3.md)
- `images`: 1–10 reference images, as URL or base64.
- `aspect_ratio`: 15 values plus auto.
- `resolution`: 768sq, 1k, 1.5k, 2k, 4k, match_input.
- `width`/`height`: 256–6256.
- `safety_tolerance`: 0–7, default 2.
- `grounding`: web and image search, default true.
- Bounding-box layout can be written inside the prompt.
- No seed. No negative prompt: BFL tells you to use positive descriptions instead.

**FLUX 3 Video:** POST /v1/flux-3-video
- Modes: t2v, i2v (keyframes), v2v (`start_video`), draft_enhance.
- `duration` 5–20s or auto. `resolution` hd/fhd/qhd/uhd. `generate_audio`. `draft`.
- MP4 at 24 fps. Latency is several minutes (https://docs.bfl.ml/cookbook/video_quickstart.md).

**Prices** (https://bfl.ai/pricing)

| Product | Price |
|---|---|
| FLUX 3 Image | $0.041 / $0.048 / $0.07 / $0.10 / $0.607 (768, 1K, 1.5K, 2K, 4K) |
| Klein | $0.008–$0.036 per image |
| FLUX 3 Video | $0.15 (HD), $0.24 (FHD), $0.28 (QHD), $0.41 (UHD) per second; draft $0.06/s |
| FLUX.2 [pro] | $0.03 first megapixel + $0.015 each extra |

Moderated or failed requests are not charged.

#### fal.ai

**Auth and flow** (https://fal.ai/docs/model-apis/model-endpoints/queue, https://fal.ai/docs/api-reference/inference-api/index.md)
- `Authorization: Key …`.
- Queue: POST https://queue.fal.run/{endpoint_id} returns request_id, status_url, response_url, cancel_url.
- Statuses: IN_QUEUE, IN_PROGRESS, COMPLETED. SSE status stream and `fal_webhook` are available. Sync calls go to https://fal.run.
- The queue retries up to 10 times.
- Results are kept 1 hour (6 minutes for results of 10KB or more unless payload storage is on, which is the default).
- Media is served from public fal.media URLs, with expiry set by the account's storage settings.

**Catalog**
- GET https://api.fal.ai/v1/models. Observed: works without a key; supports a `category` filter. `expand=openapi-3.0` returns each model's input JSON schema; for example Kling 4 has duration 3–30, four aspect ratios, 720p/1080p and `multi_prompt` shots.
- Pricing endpoints need a key.

**Broadest video catalog:** Kling 4 / 4 Flash / v3 / O3, Seedance 2.0/2.5, Wan 3.0, MiniMax H3, Gemini Omni 1.2, Grok Imagine 1.5, FLUX 3 and FLUX 3 Max, Luma Ray 3.2, PixVerse V6, LTX 2.3.

**Prices** (2026-11-17, from each model's https://fal.ai/models/<id>/llms.txt)

| Model | Price |
|---|---|
| Kling 4 | $0.157/s (720p), $0.235/s (1080p) |
| Seedance 2.5 US | about $0.22 / $0.47 / $1.16 per second (480/720/1080p) |
| Wan 3.0 | $0.05 / $0.10 / $0.20 per second |
| Gemini Omni 1.2 | $0.0819/s at 720p, plus $1.50 per 1M input tokens (slightly above Google direct) |
| MiniMax H3 | $0.05–$0.16 per second |
| Luma Ray 3.2 | $0.50 / $1.00 / $2.00 per 5s (540/720/1080p) |
| FLUX 3 Max video | $0.085/s (https://fal.ai/pricing) |
| Ideogram 4.5 | $0.03 / $0.06 / $0.22 (low/medium/high) |
| Recraft V4.1 | $0.035 per image |

Server errors and queue time are not charged.

#### Replicate

**Auth and flow**
- Bearer key.
- POST /v1/models/{owner}/{name}/predictions. Asynchronous by default; `Prefer: wait=1..60` waits for the result. Poll `urls.get`, or use `webhook` with `webhook_events_filter` (https://replicate.com/docs/topics/predictions/create-a-prediction).
- Output files live on replicate.delivery and expire after 1 hour; API outputs are deleted after an hour (https://replicate.com/docs/topics/predictions/output-files).

**Discovery and limits** (https://replicate.com/docs/reference/http)
- GET and QUERY /v1/models, GET /v1/collections/{slug}, and the model's `latest_version.openapi_schema`. Every API call needs a key (I observed a 401 without one).
- Rate limits: 600 prediction creates per minute, 3000 other calls per minute.

**Catalog** (https://replicate.com/collections/text-to-video): Gen-4.5, Veo 3.1/Fast/Lite, Kling 3.0/Omni, Seedance 2.0, Grok Imagine, Vidu Q3, MiniMax H3.

**Prices**
- Shown per model page. The general pricing page examples look stale (https://replicate.com/pricing).
- Third-party November figures: Kling 4 about $0.157/s at 720p, Wan 3.0 about $0.07/s (https://www.teamday.ai/blog/ai-image-video-api-providers-comparison-2026). Unconfirmed.

#### Runway

**Auth and flow** (https://docs.dev.runwayml.com/ai-context.md)
- Base https://api.dev.runwayml.com. Bearer key. `X-Runway-Version: 2024-11-06` is required on every request.
- Asynchronous: POST /v1/text_to_video, /image_to_video, /video_to_video, /text_to_image returns {id}. Poll GET /v1/tasks/{id}.
- Statuses: PENDING, THROTTLED, RUNNING, SUCCEEDED, FAILED, CANCELLED.
- The request body changes by model: `ratio` looks like "1280:720", and duration ranges differ per model.
- Input media: HTTPS URL, data URI, or POST /v1/uploads.
- Output URLs expire within 24–48 hours (https://docs.dev.runwayml.com/assets/outputs.md).

**Moderation**
- A moderated request shows up as a FAILED task, and it is charged.
- Repeated moderated requests can suspend the account.
- `contentModeration.publicFigureThreshold` can be set to low.

**Limits** (https://docs.dev.runwayml.com/usage/tiers.md)
- Concurrency per tier. Extra tasks show THROTTLED and are queued, not rejected.
- Daily generation cap returns 429. There is also a monthly spend cap. No requests-per-minute limit.

**Catalog:** Runway is now a multi-model router: gen4.5, aleph2, Kling 3/4/O3, Veo 3.1, Seedance 2/2.5, Wan 3, Hailuo 3, Grok Imagine, Gemini Omni 1.2, plus GPT Image, Nano Banana and Ideogram.

**Prices** ($0.01 per credit; https://docs.dev.runwayml.com/guides/pricing/)

| Model | Credits |
|---|---|
| gen4.5 | 12 per second ($0.12/s) |
| veo3.1 | 40/s with audio, 20/s without |
| kling_4 720p | 15.7/s list price (promotion until Nov 30) |
| gemini_omni_flash_1.2 720p | 7/s |
| gen4_image | 5 or 8 per image |
| ProRes or PNG sequence output | +5/s |
| HDR | +20/s |

#### Luma

**Auth and flow**
- New Luma Agents API: POST https://agents.lumalabs.ai/v1/generations. Poll GET /v1/generations/{id}. Results come as presigned MP4 URLs.
- Bearer key starting `luma-api-` (https://docs.agents.lumalabs.ai/guides/videos/index.md).

**Video: `ray-3.2`** (https://docs.agents.lumalabs.ai/guides/videos/generation/index.md)
- Types: video, video_edit, video_reframe.
- `resolution`: 360p (draft), 540p, 720p, 1080p. `duration`: 5s or 10s.
- `start_frame` and `end_frame` (not with 10s).
- `keyframes` (1–64) with `keyframe_indexes` on a 24 fps grid.
- `hdr` and `exr_export`.

**Prices**
- In credits: ray-3.2 at 5s costs 20 / 50 / 100 / 400 (360p, 540p, 720p, 1080p). uni-1 images cost 14 credits each (https://docs.agents.lumalabs.ai/guides/pricing/index.md).
- The dollar value of a credit on pay-as-you-go depends on the plan. fal's price implies about $0.01 per credit; that is my inference.

#### Stability AI

**Auth and flow** (https://docs.stability.ai/api-reference/index.md)
- Bearer key. https://api.stability.ai/v2beta/… with multipart/form-data. Mostly synchronous.
- `Accept: image/*` returns raw bytes. `Accept: application/json` returns base64 plus seed and finish_reason.
- `CONTENT_FILTERED` means the output was blurred. HTTP 403 means moderation.
- Rate limit: 150 requests per 10 seconds, with a 60-second block after that.

**Prices** ($0.01 per credit; https://docs.stability.ai/pricing)

| Service | Price |
|---|---|
| Ultra | $0.08 |
| SD3.5 Large | $0.065 |
| Core | $0.03 |
| Inpaint / Erase / Search & Replace | $0.05 |
| Outpaint | $0.04 |
| Upscalers | $0.02–$0.60 |

There is no video service on the pricing page.

#### Ideogram

**Auth and flow** (https://developer.ideogram.ai/v2/api-reference/images/generate/ideogram-4-5.md)
- `Api-Key` header.
- POST https://api.ideogram.ai/v2/image/generate/ideogram-4-5 (multipart). Synchronous by default; `async` polls GET /v2/generations/{id}, or use a `webhook_url` (Ed25519-signed).

**Parameters**
- `magic_prompt`, `background` (transparent), `quality` very_low to high, `num_images`, `seed`, `size`, `mask`.
- The v3 endpoint adds `negative_prompt`, style codes, style reference images and `color_palette` (https://developer.ideogram.ai/api-reference/api-reference/generate-v3).

**Output:** URLs available for a limited time. A blocked image comes back with `is_image_safe` false; a failed safety check returns 422.

**Prices:** direct price not confirmed. Via fal: $0.03 / $0.06 / $0.22.

#### Recraft

**Auth and flow** (https://www.recraft.ai/docs/api-reference/endpoints.md)
- Bearer key. Base https://external.api.recraft.ai/v1, compatible with the OpenAI SDK.
- POST /images/generations, plus /raster and /vector variants.
- `response_format`: url (default), b64_json, or multipart.

**Parameters**
- `size` (WxH or w:h), `style`, `style_id`, `style_match`, `style_reference_urls` (up to 10).
- `random_seed`.
- `negative_prompt` on V2/V3 only.
- V3 editing: inpainting and outpainting.
- Native SVG vector output.

**Prices** (https://www.recraft.ai/docs/api-reference/pricing.md)

| Model | Price per image |
|---|---|
| V4.1 Flash V2 | $0.007 |
| V4.1 raster | $0.035 |
| V4.1 vector | $0.08 |
| V4.1 Pro raster | $0.21 |
| V4.1 Pro vector | $0.30 |

### 3. Recommendation for v1 (0.1.0)

**Ship with OpenRouter and OpenAI direct. Both keys are already in the keychain.**

**OpenRouter is the backbone, for images and video.**
- One key covers nearly every frontier model.
  - Images: GPT Image 2.6, Nano Banana 2.1/Pro, FLUX 3, Seedream 5.5, Recraft, Grok.
  - Video: Veo 3.1, Gemini Omni 1.2, Kling 3, Seedance 2/2.5, Wan 3, Hailuo 3, Gen-4.5, FLUX 3 Video, Grok 1.5.
- Its public catalogs describe each model's parameters as typed enums and ranges, and give prices. That lets harnessUI build the controls automatically and show a cost estimate before a run, and it survives the model churn.
- Images come back as base64 in a synchronous call. Video is asynchronous with a `polling_url`, so no webhook is needed.

**OpenAI direct is worth adding for users who have only an OpenAI key, and because its image API has the deepest controls.**
- The same `/v1/images/generations` request shape also works for xAI and Recraft, which both document OpenAI SDK compatibility. One "OpenAI-compatible images" adapter could add them as cheap extra presets.
- OpenAI has no video.

**Best next step (v1.x): fal.ai.**
- It has the widest video catalog, including Kling 4, Luma Ray 3.2, FLUX 3 Max and MiniMax H3, none of which OpenRouter lists.
- Its public catalog includes each model's input schema, and it uses a queue API.
- It needs one new key.

**Google direct is optional.** Gemini Omni costs about the same on OpenRouter, and Google's Interactions API is a new request style to build.

**Skip for v1**
- Replicate: 1-hour URLs, per-model schemas that differ, key needed even to browse.
- Runway: per-model request bodies; credits. Good for ProRes and HDR later.
- Luma: dollar price per credit is unclear.
- BFL direct: always async, 10-minute URLs; FLUX 3 is on OpenRouter anyway.
- Stability: no video.
- Ideogram: multipart uploads; available via fal.
- abliteration.ai and Anthropic: they don't generate images.

**Controls to always show ("easy to use")**
- Model picker grouped by family, with a price hint.
- Prompt.
- Aspect ratio chips.
- Quality or resolution.
- Number of images.
- Drag-in reference images.
- For video: duration (from `supported_durations`), resolution, start frame, audio on/off.
- A cost estimate, with a confirm step for expensive runs. Examples: Veo 3.1 8s at 4K with audio costs $4.80; Seedance 2.5 10s at 1080p costs about $11.64 on fal.

**"Advanced" drawer ("not dumbed down"), showing only what the selected model declares**
- `seed`.
- Output format and compression.
- Transparent background.
- GIF frame count and timing; SVG output.
- `last_frame` (end frame).
- Number of references, plus reference video and audio.
- Moderation level: OpenAI `moderation=low`, BFL `safety_tolerance`.
- Negative prompt, only on the models that take one (Recraft V2/V3, Ideogram v3, Stability). Many 2026 models don't: Gemini Omni rejects it, and BFL tells you to describe what you want instead.
- OpenRouter provider routing: `provider.order`/`only`, `allow_fallbacks`.
- Raw JSON pass-through for `allowed_passthrough_parameters`, as an escape hatch for experts.
- Mask painting and markup overlay for edits (OpenAI and OpenRouter).
- Live partial-image previews while streaming.
- Later, for video: keyframes (xAI up to 4, Luma up to 64), multi-shot (Kling 4 `multi_prompt`), extend/edit/reframe, and draft-then-final (FLUX 3 draft, Seedance 2.5 draft, Luma 360p).

**Things the implementation has to get right**
- **Download every result immediately into local storage.** Links expire anywhere from 10 minutes (BFL) to 1 hour (Replicate, fal queue) to 24–48 hours (Runway). OpenRouter's `unsigned_urls` need the key.
- **Never send the API key when downloading from a result URL on another host.** This matches the existing rule that keys go only to their own endpoint.
- **Poll; don't use webhooks.** A desktop app has no public URL.
- **Save in-flight video job IDs so they survive an app restart.** The job is billed even if the app closes.
- **Explain moderation outcomes clearly.** Each provider reports a block differently:
  - OpenAI: `moderation_blocked`.
  - BFL: "Request/Content Moderated", not charged.
  - Runway: FAILED, and charged.
  - Stability: 403 or CONTENT_FILTERED.
  - Ideogram: 422 or `is_image_safe` false.
- **Keep the "Cloud" mark and add a few data notes:**
  - OpenRouter video is not eligible for zero data retention.
  - Google outputs carry a SynthID watermark.
  - xAI says outputs are not used for training.

### 4. Unconfirmed items

- Per-image prices for OpenAI 2.5/2.6 (third-party).
- Whether OpenAI's image API really has no seed or negative prompt.
- The Gemini auth header name, and Gemini rate limits.
- Whether xAI charges different per-second rates by resolution (only OpenRouter shows tiers), and the expiry of xAI's URLs.
- Luma's dollar price per credit.
- Ideogram's direct prices.
- Replicate per-model prices (third-party).
- Veo GA on Google Cloud (secondary sources).
- The 2.6 launch date of Nov 4, 2026 (Wikipedia only).
- Rate limits for OpenRouter's image and video APIs (not documented).

### 5. Files

Raw snapshots of the catalogs, kept in the scratchpad (the scratchpad may be cleared):
- /tmp/claude-0/-home-user/f8f6f1ef-33e4-57b5-aed7-1e790c206f77/scratchpad/or/images.json (OpenRouter image catalog)
- /tmp/claude-0/-home-user/f8f6f1ef-33e4-57b5-aed7-1e790c206f77/scratchpad/or/videos.json (OpenRouter video catalog)
- /tmp/claude-0/-home-user/f8f6f1ef-33e4-57b5-aed7-1e790c206f77/scratchpad/fal/t2v.json and /tmp/claude-0/-home-user/f8f6f1ef-33e4-57b5-aed7-1e790c206f77/scratchpad/fal/t2i.json (fal text-to-video and text-to-image catalogs)
- /tmp/claude-0/-home-user/f8f6f1ef-33e4-57b5-aed7-1e790c206f77/scratchpad/bfl/ (FLUX 3 OpenAPI references)

In the repo:
- The ready-made endpoint choices are in /home/user/harnessUI/src/components/settings/endpointPresets.ts.
- The chat model list filters out image models in /home/user/harnessUI/crates/agent/src/cloud.rs, around line 990.

No repo files were edited.
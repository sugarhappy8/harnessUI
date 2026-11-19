<!-- Decisions recorded 2026-11-17 -->
> **Owner's request (verbatim):** "For the cloud update let's make 0.1 let's add image and video gen section for those models, make it easy to use but not dumbed down"
>
> **Owner's decisions (2026-11-17):** 1) Companies in 0.1.0: OpenRouter + OpenAI. 2) No picture tool for chat models in 0.1.0 (Send to chat and Open in Studio only). 3) Spending: confirm every video, any run of $1 or more and any run with an unknown price, plus a default daily limit of $10 that pauses Studio until it is raised.
>
> Provider facts were checked again on 2026-11-17: OpenRouter's public catalogues listed 68 image and 33 video models (`GET /api/v1/images/models`, `GET /api/v1/videos/models`), and OpenAI's deprecations page lists the Videos API, sora-2 and sora-2-pro as shut down on Sep 24, 2026.

# harnessUI 0.1.0, "the cloud update": Studio, a place to make pictures and videos with cloud models

Design proposal, 2026-11-17. Read only: no files were edited. The repo facts come from origin/main at f67d49e, checked again for this proposal. Provider facts come from the 2026-11-17 provider research, including the saved OpenRouter catalog snapshots. Plain English comes first in each section; the parts for engineers are marked **Technical detail**.

---

## 0. Summary

- **What.** A new section called **Studio**, next to Models and Settings. You type what you want, pick a model, and press **Create**. Pictures arrive in seconds. Videos take a few minutes and keep going while you chat or after you close the app. Every result is saved on your computer with the exact settings that made it, so you can redo it, change it or export it.
- **Easy but not dumbed down** means three layers:
  1. A short set of controls that are always visible.
  2. Controls that follow the chosen model: you only see what that model really supports.
  3. A full **Advanced** drawer that lists every setting the model offers, as typed fields built from the company's own parameter list.

  Anything you change in Advanced also shows as a chip next to the prompt, so nothing that affects the result is hidden.
- **Companies in 0.1.0:**
  - **OpenRouter** for pictures and video. It covers about 68 picture models and 33 video models with one key.
  - **OpenAI** for pictures (GPT Image 2.6). OpenAI shut down its video API on Sep 24, 2026.

  Both use keys already saved in Settings → Endpoints, so you add no new key. Anthropic and abliteration.ai do not make pictures or video.
- **Money.** The estimated price is on the Create button before you run. Every video run, every run estimated at $1 or more and every run with an unknown price asks you to confirm first. A daily limit of $10 is on by default: a run that would take today's spending past it pauses until you raise the limit (Settings → Studio) or the day ends. The actual cost is shown afterwards, along with a "spent today" figure.
- **Privacy.** The Cloud mark is always shown, together with the exact host your prompt and pictures go to. Opening Studio sends nothing. Model lists are fetched only when you click. Each run writes one audit log row. Safety refusals are explained in plain words and are never retried automatically.
- **Not in 0.1.0:**
  - A tool that lets chat models make pictures. I recommend 0.1.x.
  - fal.ai.
  - Mask painting.
  - Extending videos.
  - Comparing models side by side.
- **Size of the work.** About 7–8 person-weeks, or roughly 4–5 calendar weeks for two engineers, including tests and docs.

---

## 1. What it is and where it lives

**In one paragraph.** Studio is a workspace for making pictures and short videos with cloud AI models. You describe the picture or clip, optionally add pictures of your own (as references or as the first frame of a video), choose the shape, how many, the quality or length, and press Create. Studio shows what it will cost and where your prompt goes before anything is sent. Results appear as cards above the prompt box, much like chat messages. Each card holds the prompt, the model, the settings, the cost and the results. You can open any result full size, star it, redo it, reuse its settings, turn a picture into a video, send it into a chat, save it anywhere or delete it. All results are downloaded to a normal folder on your computer at once, because the companies' links expire within minutes or hours.

**Navigation.**
- **How you get there:**
  - A third icon button in the sidebar header (a sparkle icon), between Models and Settings.
  - The shortcut **Ctrl/⌘ G**. This is free today: the app uses N, K, L, M, B, `,` and `/`.
  - The window event `app:open-studio`, which other parts of the app can use, for example "Open in Studio" on a chat picture.
- **What happens when you open it.** Studio replaces the chat area and the Workbench. The sidebar stays, so one click on a chat or New chat takes you back. Chat replies keep running in the background.
- **Working jobs.** While a job runs, the Studio icon shows a small count, such as "1". When a video finishes while you are in a chat, a notice says "Your video is ready" with a link to open it.
- **Why a section and not a dialog or a chat mode.** The owner asked for a section. A gallery needs the whole window, and video jobs have to outlive any one chat. The Workbench belongs to one chat, and a dialog (Models is 52 × 38 rem) is too small for a gallery.
- **Name.** "Studio" is a noun like "Models" and "Settings". The button says "Create". In the code it is never called "generation", because `state::Generation` and the store's `generation` already mean a chat reply being written.

### Wireframe 1: Studio, Pictures, simple view

```
+----------------+---------------------------------------------------------------------------------+
| [Models][Studio*][Settings] | Studio  [ Pictures | Video ]   Search...  Starred  Grid   Cloud . $0.84 today |
| + New chat     +--------------------------------------------------------+------------------------+
| Search chats   | -- Today ------------------------------------------    | Model                  |
|                | "lighthouse at dusk, 35mm film"                        | [GPT Image 2.6 Flare v]|
| Chats          | GPT Image 2.6 Flare . OpenRouter . 3:2 . x4 . $0.16    |  via OpenRouter  Cloud |
|  Trip plan     | +-----++-----++-----++-----+                           |  Edit . Refs 16 .      |
|  Rust bug      | | img || img || img || img |                           |  Transparent . GIF     |
|  ...           | +-----++-----++-----++-----+                           |                        |
|                | Again   Reuse   Star   ...                             | Shape                  |
|                |                                                        | [1:1][3:2][2:3][16:9][+]|
|                | "same, in heavy snow"   Nano Banana 2.1 . x2           | How many  [1][2][4][+] |
|                | +-----++- making it . 0:09 -+               [Cancel]   |                        |
|                | | img ||                    |                          | Quality                |
|                | +-----++--------------------+                          | (Auto|Low|Med|High|Max)|
|                |                                                        |                        |
|                | ! OpenAI's safety filter refused the prompt            | My setups  [None    v] |
|                |   Not charged . request req_8f2...                     |                        |
|                |   [Edit prompt]  [Details]                             | > Advanced . 2 changed |
|                +--------------------------------------------------------+------------------------+
|                | [+ Add picture]   Describe the picture you want...                              |
|                |                                                                                 |
|                | seed 81723 [x]   background: transparent [x]                                    |
| (local model)  | Cloud: your prompt and added pictures go to openrouter.ai   ~ $0.16  [ Create ] |
+----------------+---------------------------------------------------------------------------------+
```

The Video tab uses the same layout. Its right panel holds Model, Shape (from the model's list), Length (for example [4 s][6 s][8 s]), Resolution (480p | 720p | 1080p | 4K, as offered), Sound (on/off, when the model makes sound) and a **Start frame** drop zone, when the model takes one.

### Wireframe 2: the Advanced drawer (opens inside the right panel)

```
| v Advanced . 2 changed                          [Reset all] |
|                                                             |
| Seed           [ 81723       ]  [Dice]  [x] Keep it      R  |
| Exact size     [ Automatic             v ]                  |
|                  width [1536] x height [1024]  (checked)    |
| File type      [ Automatic (PNG)       v ]                  |
| Compression    ------o------ 80     only for JPEG and WebP  |
| Background     ( Automatic | Transparent | Opaque )      R  |
| Animated GIF   [ ] on   Frames [8]   Time per frame [120 ms]|
| Safety level   [ Automatic (company default) v ]   (?)      |
| -- OpenRouter ---------------------------------------------- |
| Company that runs it  [ OpenRouter chooses v ]              |
| [x] Allow another company if that one is busy               |
| Extra fields   none set   [Edit JSON]                       |
|                this model accepts: negative_prompt, watermark|
| ------------------------------------------------------------ |
| [See the exact request]   key removed . goes to openrouter.ai|
```

`R` is a per-field reset. Every field reads "Automatic (company default)" until you change it. The header counts the changes, following the existing Fine control pattern in src/components/FineControl.tsx. Fields the model does not support are not shown. `(?)` is a one-line help tooltip.

### Wireframe 3: a video job card, and the confirm step

```
| "slow drone push over the lighthouse, waves crashing"                                   |
| Veo 3.1 Fast . OpenRouter . 8 s . 16:9 . 720p . sound on . start frame [img]   ~ $0.96  |
| [#########............]  Making it . 1:12 so far . usually about 2 min                  |
| You can keep working or close the app; it will appear here.          [Stop watching]   |

+ Make this video? -----------------------------------------------+
| Veo 3.1 Fast through OpenRouter                                 |
| 8 seconds . 1280 x 720 . sound on . 1 video                     |
| Estimated cost   $0.96   (OpenRouter's price list, 17 Nov)      |
| OpenRouter charges for a video once it has started, even if     |
| you stop watching it. Your prompt and start frame go to         |
| openrouter.ai and the company it sends the job to.              |
| [ ] Don't ask again for videos under $1.00                      |
|                                   [Cancel]   [Make video]       |
+-----------------------------------------------------------------+
```

### Wireframe 4: the viewer (click any result)

```
+------------------------------------------------+------------------------------------+
|                                                | Prompt                     [Copy]  |
|                                                | lighthouse at dusk, 35mm film      |
|             [ picture or video ]               | Model    GPT Image 2.6 Flare       |
|                                                |          OpenRouter (runs at OpenAI)|
|        <   2 / 4   >        > || 0:03 / 0:08   | Size     1536 x 1024 . seed 81723  |
|                                                | Quality  medium . PNG . transparent|
|                                                | Cost     $0.04 . took 13.8 s       |
|                                                | Made     17 Nov 2026 14:02         |
|                                                | File     ~/harnessUI/studio/...png |
+------------------------------------------------+ [Reuse all] [Reuse without seed]   |
| Star   Again   Edit   Animate   Use as reference| [Send to chat] [Save as...] [Copy] |
| Show in folder   Delete                         | [See the request]                  |
+------------------------------------------------+------------------------------------+
```

---

## 2. Which companies in 0.1.0, and how keys are reused

### Recommendation: OpenRouter and OpenAI

| Company | Pictures | Video | Key | Work to add | 0.1.0? |
|---|---|---|---|---|---|
| **OpenRouter** | Yes: about 68 models through its picture API (GPT Image 2.6, Nano Banana 2.1 and Pro, FLUX 3, Seedream, Recraft, Grok) | Yes: about 33 models through its video jobs API (Veo 3.1 / Fast / Lite, Gemini Omni 1.2, Kling 3, Seedance 2/2.5, Wan 3, Hailuo 3, Runway Gen-4.5, FLUX 3 Video, Grok 1.5) | Already in Settings → Endpoints | About 9–10 dev-days: model list into typed fields, price reading, the picture call, the video job (start, check, download) | **Yes** |
| **OpenAI** | Yes: GPT Image 2.6 Sunburst (precise edits) and Flare (fast everyday), plus older models until they retire (gpt-image-1.5 on Dec 1, 2026; gpt-image-2 on Feb 10, 2027) | No: the API was shut down Sep 24, 2026 | Already in Settings → Endpoints | About 4–5 dev-days: a hand-written settings list and price table, plus picture editing | **Yes** |
| Anthropic | No API for pictures or video | No | n/a | n/a | Not applicable |
| abliteration.ai | No generation API found (chat only) | No | n/a | n/a | Not applicable |
| xAI, Recraft | Yes, in the same request shape as OpenAI's | xAI only, with its own job API | New key and a new company choice | About 2–3 days each for pictures on the OpenAI adapter; about 4 days for xAI video | Option, see Decision 1 |
| fal.ai | Yes | The widest catalogue: Kling 4, Luma Ray 3.2, MiniMax H3, Seedance 2.5, FLUX 3 Max | New key | About 8–10 days: a queue API, and per-model schemas turned into fields | 0.1.x (Decision 1) |
| Google Gemini direct | Yes | Omni Flash | New key | About 8 days: a new request style (the Interactions API) | Later; the same models are on OpenRouter at about the same price |
| Replicate, Runway, Luma, Black Forest Labs, Stability, Ideogram | n/a | n/a | n/a | n/a | Skip in 0.1 |

Why these companies are skipped in 0.1:
- Replicate: links expire after an hour, and you need a key even to browse.
- Runway: the request changes per model, prices are in credits, and blocked runs are charged.
- Luma: it is unclear what a credit costs in dollars.
- Black Forest Labs: every call is asynchronous, links last 10 minutes, and FLUX 3 is on OpenRouter anyway.
- Stability: no video.
- Ideogram: it is available through fal.

**Why OpenRouter is the backbone.**
- One key covers nearly every leading model.
- Its public model lists describe each model's settings as typed values. Across the 68 picture models there are only three kinds: a choice from a list, a number range, and on/off. The lists also give prices. Studio can therefore build the right controls and a price estimate for models that do not exist yet, which matters because model IDs change every month.
- Pictures come back in one call. Videos are jobs Studio checks on, with no webhook needed.

**Why OpenAI too.**
- Some people have only an OpenAI key.
- GPT Image has the deepest picture controls: quality up to "max", transparent background, animated GIF, custom sizes up to 3840 px, and editing.

### Keys and endpoints

- **No new key and no new secret.** Studio uses the endpoints already in Settings → Endpoints and their keychain entries (`provider-api-key:<id>`). A key still goes only to its own endpoint's scheme, host and port, the promise made in 0.0.7.
- **The endpoint card shows what it can do,** for example "Chat · Pictures · Video" for OpenRouter and "Chat · Pictures" for OpenAI.
- **Fetching the lists.** Clicking **Test** on such an endpoint now also fetches its picture and video model lists. Studio has its own **Update model lists** button, and it shows when each list was last updated.
- **Opening Studio fetches nothing.** If no list has been saved yet, for example on an endpoint added in 0.0.8, Studio shows: "Get OpenRouter's picture and video models [Get the list]".
- **Empty state with no capable endpoint:** "Studio makes pictures and videos with OpenRouter or OpenAI. Add one in Settings → Endpoints. [Open Endpoints]"
- **Missing or broken key.** Models from an endpoint whose key is missing or unreadable are greyed out, with "Add a key in Settings → Endpoints".
- **The two model lists stay separate.** Picture and video models never appear in the chat model picker, and chat models never appear in Studio. The existing chat filters already drop media models (`openai_can_chat`, and the output-modality filter in `list_model_entries`); Studio's list is the reverse of those filters.
- **Generic (self-run) endpoints** get nothing in 0.1.0. A later opt-in, "This server makes pictures (OpenAI style)", is part of Decision 1, option C.

**Technical detail**
- In src/components/settings/endpointPresets.ts, add `media?: { image: boolean; video: boolean }` to `EndpointChoice`: openrouter `{image: true, video: true}`, openai `{image: true, video: false}`. Endpoints.tsx shows the badges.
- In src-tauri/src/providers.rs, `test_provider` also writes `providerMediaModels:<id>` (the same pattern as `providerModels:<id>`, src-tauri/src/settings.rs:94) for kinds that can make media. `delete_provider` clears that key and sets `provider_id` to NULL on the Studio rows, so a run keeps its saved endpoint name and host.
- OpenRouter: `GET /api/v1/images/models` and `GET /api/v1/videos/models` are public and need no key. They are still sent through the endpoint's own client (same origin, so the existing redirect, proxy and key-transport rules apply).
- OpenAI: `GET /v1/models` (key needed), filtered by a new `cloud::openai_media_kind(id)`, the reverse of `openai_can_chat` (crates/agent/src/cloud.rs:991), then matched against the bundled settings list by id prefix.
- Picture prices on OpenRouter are not in the list itself. They come from `GET /api/v1/images/models/{id}/endpoints`, fetched once when you pick a model (that click is the user action) and cached with the list. Until then the picker shows "price shown when picked". Video prices are in the list (`pricing_skus`).

---

## 3. "Easy to use but not dumbed down"

### The rules

1. **Few controls in front, all controls one click away.** Simple settings stay simple, and every setting the model offers is reachable.
2. **The controls follow the model.** Studio shows only what the selected model declares. When you switch models, your choices carry over, are moved to the nearest value the new model supports, and a one-line note says what changed: "Nano Banana 2.1 has no seed setting, so the seed was removed. Shape 21:9 became 16:9."
3. **Nothing that changes the result is hidden.**
   - A changed Advanced value shows as a removable chip under the prompt.
   - "See the exact request" shows exactly what will be sent and to which host, with the key removed.
   - Studio never rewrites your prompt behind your back.
4. **Every result can be reproduced.** All settings are saved with the result. When the model takes a seed, Studio picks a random seed itself and sends it, so even "random" results can be repeated exactly.
5. **There is no hidden "Auto" model.** It would hide where your data goes. The default is the model you used last, or a recommended one on first use.

### Always visible

| Pictures | Video |
|---|---|
| Prompt (Enter creates, Shift+Enter adds a new line) | Prompt |
| Model picker with the Cloud mark and the endpoint | Model picker with the Cloud mark and the endpoint |
| Shape: aspect-ratio chips from the model's list; the first 4–5 show and the rest are under [+] | Shape: from the model's list (often 16:9, 9:16, 1:1) |
| How many: 1, 2, 4, up to the model's maximum; more under [+] | Length: chips from the model's supported lengths; **the default is the shortest length of at least 4 s** |
| Quality and/or Detail (resolution): the model's own values with friendly labels, including the top ones (xhigh, max, 4K); the price change is in the tooltip | Resolution: **the default is 720p**, or the lowest available if 720p is missing |
| + Add picture (references), when the model accepts them, up to the model's limit | Sound on/off, when the model makes sound (on by default where the company defaults to on; the price shows the difference) |
| My setups (saved model plus settings) | Start frame drop zone, when the model takes one |
| Price estimate, the destination host, and Create | Price estimate, the destination host, and Create (always asks to confirm) |

The video defaults keep the first try cheap. Bigger settings are one click away, and "Make full version" on the card (later) can redo a clip at higher settings.

### Under Advanced (only what the selected model declares)

- **Seed**: Random by default. [Dice] gives a new one, "Keep it" locks it, and there is a per-field reset. An honest note in the help: "A seed is not a saved copy; the same seed on another model or version gives a different picture."
- **Exact size**: OpenAI custom sizes, checked as you type (both sides multiples of 16, ratio between 1:3 and 3:1, longest side at most 3840, pixel count in range), or the model's size list.
- **File type and compression.** Compression only appears for JPEG and WebP.
- **Background**: transparent or opaque. Choosing transparent switches the file type to PNG or WebP and says so.
- **Animated GIF**: frames and time per frame (GPT Image 2.6). **SVG output** where offered; it is shown only through `<img>`, where scripts never run.
- **Safety level**: OpenAI `moderation` (Automatic or Low), in plain words: "Low makes OpenAI's filter less strict; it still refuses what its rules forbid."
- **End frame** (video), when the model supports a last frame.
- **More references** beyond the first ones, up to the model's limit.
- **OpenRouter routing**: which company runs the model (`provider.order`/`only`), whether a fallback is allowed, and for pictures, "only companies that keep no data" (`zdr`). The routing request carries this choice.
- **Extra fields (JSON)**: an expert escape hatch. It accepts only the names the model lists in `allowed_passthrough_parameters` (for example `negative_prompt`, `personGeneration`, `enhancePrompt`, `cfg_scale`), lists those names as hints, checks that the input is valid JSON, and refuses names that are not listed. A field set here also shows as a chip.
- **See the exact request**: read-only JSON with the key removed, attached pictures shown as "picture 1 (sha …, 1.2 MB)", and the host it goes to.

**Negative prompt** appears only where the model takes one. Today on OpenRouter that means through Extra fields; Gemini Omni rejects it, and FLUX tells you to describe what you want instead. It is not a standard field, because most 2026 models ignore or refuse it.

### Presets

- **"My setups"** (0.1.0): save the current model and all settings under a name, and pick it again from one menu. This is cheap to build and serves both beginners ("Product shot") and experts.
- **Styles** that add visible text to the prompt (Fooocus-style) come later, in 0.1.x. When they arrive they show as a chip and in the request preview, never hidden.

### Seeds, redo and reuse (on every card and in the viewer)

- **Again**: the same settings with a new seed (or simply again, when the model has no seed).
- **Reuse all**: fills the prompt and every setting, seed included. **Reuse without seed** does the same with a new seed. **Reuse never starts a run by itself.**
- **Retired models.** Reuse on a model that is no longer offered picks the closest model from the same family and lists what changed. If none exists, it says "OpenRouter no longer offers this model". Old results always stay viewable.

### References, picture-to-video and editing

- **The tray.** "+ Add picture" accepts drag and drop, paste, a file, or a result from the gallery ("Use as reference"). Each picture has a role chip: Reference for pictures; Start frame or End frame for video. The roles follow OpenRouter's standard fields (`input_references`, `frame_images` first_frame/last_frame) and OpenAI's `image[]`.
- **Animate** on any picture switches to Video with that picture as the start frame. If the chosen video model takes no start frame, it says so and offers the most recently used model that does.
- **Edit with words** (0.1.0) puts the picture in the tray as the picture to change and focuses the prompt. It uses OpenAI `/v1/images/edits` or OpenRouter `input_references`. If the current model cannot edit, Studio suggests one that can.
- **Later:**
  - Mask painting, with OpenAI's honest warning that edits can spill past the selection.
  - Markup overlay.
  - Extending a video (Omni, up to 30 s).
  - "Save frame" from a video.
  - Prompt enhance using the user's **local** chat model, so the prompt stays on the computer until Create. No competitor has that privacy angle.

**Technical detail: the per-model parameter definition**

The webview renders controls only from a list of `ParamSpec`s, so there is no React code per company:

```ts
type ParamSpec = {
  key: string;                         // the name sent to the company, e.g. "quality"
  canonical?: "shape" | "count" | "quality" | "resolution" | "length" | "sound" | "seed"
            | "references" | "startFrame" | "endFrame" | "size" | "format" | "background";
  label: string; help?: string;
  layer: "simple" | "advanced";
  kind:
    | { type: "enum"; values: { value: string; label: string }[] }
    | { type: "range"; min: number; max: number; step?: number; unit?: string }
    | { type: "boolean" }
    | { type: "seed"; max: number }
    | { type: "images"; max: number; role: "reference" | "start" | "end" }
    | { type: "size"; rules: SizeRules }          // OpenAI custom WxH checks
    | { type: "extra"; allowedKeys: string[] };   // the JSON escape hatch
  companyDefault?: unknown;   // shown as "Automatic (...)"; an unset field is NOT sent
  affectsPrice?: boolean;     // re-estimate when this field changes
  requires?: { key: string; values: string[] }; // e.g. compression needs jpeg/webp
};
```

- **Where the specs come from.**
  - Rust builds them: from OpenRouter's catalogues, where each `supported_parameters` entry is enum, range or boolean, and the video fields are `supported_durations`, `supported_resolutions`, `supported_aspect_ratios`, `supported_frame_images`, `generate_audio`, `seed` and `allowed_passthrough_parameters`.
  - For OpenAI, from a bundled, dated file (crates/agent/src/media/specs/openai_images.json) matched by id prefix.
  - Unknown parameters are not dropped silently: they appear as Extra fields hints.
- **Widgets.** A short enum becomes a segmented control; a long enum becomes a select; a range becomes a slider plus a number box; a boolean becomes a switch; seed becomes a number plus dice and lock; images becomes a drop zone; size becomes width and height boxes with live checks.
- **Carrying values between models** is a pure TypeScript module (src/components/studio/canonical.ts):
  - Shape goes to the nearest ratio by value; count to `min(count, max)`.
  - Quality or resolution keeps the same name, or else moves to the next lower step (cheaper, never more expensive).
  - Length goes to the nearest supported value; seed is kept when supported; references are kept up to the new model's limit.
  - Everything not carried over is listed in the note.

---

## 4. Jobs: how runs work, cost and confirmation

**In plain words.**
- **Pictures** take from a few seconds to about 2 minutes (OpenAI edits on complex prompts). The card shows "making it" with a timer and a Cancel button. Cancel stops waiting, and the card says the company may still charge.
- **Videos** take minutes. The card shows the status and how long it has run against how long that model usually takes on your computer's history ("usually about 2 min"), with a percentage if the company ever reports one.
  - You can keep working, switch to chat, or close harnessUI. On the next start Studio checks on the job and downloads the result.
  - **Stop watching** ends the checking. The card then says "OpenRouter may still finish and charge for this" and offers **Get it anyway** for as long as OpenRouter still has the result.
  - **Esc never cancels a Studio job**; in Studio, Esc only closes the viewer or a popup. The global stop hotkey stops chat turns and tools only, never Studio jobs.
- **Prices before running.**
  - The Create button shows "~ $0.16" for the whole run (price × count × length).
  - The estimate is shown in text, with colour as a second cue: under $1 plain, $1–$2.50 amber, over $2.50 red.
  - When a price cannot be worked out, for example a model priced per "video token", the button says "price unknown" and Studio always asks first.
  - After the run the card shows the **actual** cost the company reported, and the top bar shows "spent today".
- **Confirmation.** By default Studio asks before **every video**, before **any run estimated at $1 or more**, and before **any run whose price is unknown**. The dialog lists the model, company, length, resolution, sound, count, estimate, where the data goes, and that a started video is charged even if you stop watching. "Don't ask again under $X" lets experts skip it. The defaults are Decision 3 (option B, chosen by the owner).
- **Daily limit (on by default, $10).** Spending is counted per local calendar day from the actual costs reported (or the estimate while a run is going and for runs whose actual cost is unknown). When today's spending plus a run's estimate would pass the limit, Create is replaced by "Daily limit reached: $X of $10 spent today" with **Raise the limit** (opens Settings → Studio) and the run is not sent. A run whose price is unknown counts as passing the limit unless the remaining allowance is at least $5 and the user confirms. Settings → Studio changes the amount or turns the limit off.
- **Queue.** Up to 2 picture runs and 2 video jobs at once per endpoint; more wait in line, shown as "waiting". A small strip reads "1 making · 1 waiting · 3 done · 1 stopped".

**Technical detail**
- **The manager.** `StudioManager` in src-tauri/src/studio/manager.rs is modelled on `DownloadManager` (src-tauri/src/downloads.rs:252):
  - `running: HashMap<run_id, Running{token, ended}>`, and a `TaskEnd` guard that tidies up on every exit, panic included.
  - Progress events are throttled: `app.emit_to(MAIN_WINDOW, "studio-progress", RunDto)` at most 4 per second.
  - Because events are not replayed, the state is also readable through `studio_runs`.
- **States:** `queued → sending → waiting(remote_id saved) → downloading → done | failed | blocked | cancelled | interrupted`. The remote id is written to the DB **before** checking starts.
- **Never pay twice.** The request that creates a run or job is sent **at most once**:
  - No retry on a timeout, a 5xx or a dropped connection after the request was sent.
  - It is retried only on 429 with a Retry-After of at most 30 s (the request was not accepted) and on a connect failure before any byte was sent.
  - This needs a `post_once` path in crates/agent/src/client.rs, because the existing `send_json` retries 408, 5xx and 529.
- **Pictures.** One POST:
  - Connect limit 10 s; whole-request limit 5 min (OpenAI edits can take about 2 min).
  - The response is streamed to a temporary file (cap 192 MiB, since base64 pictures far exceed the 8 MiB `MAX_JSON_BODY`). Each `b64_json` is then decoded into its own file.
  - The "How many" chips are capped at 4 in the simple layer.
- **Video checking.**
  - `GET /api/v1/videos/{id}` every 5 s for the first minute, then every 10 s, then every 20 s; each check has a 30 s limit.
  - A network error while checking keeps the job waiting. A 404 means "OpenRouter no longer has this job".
  - After 30 min the card says "still not ready" with [Check again]. The remote id is kept and the job is never thrown away silently.
- **Resume at start.** `studio.resume_interrupted()` is called next to the downloads resume in src-tauri/src/lib.rs, at about line 408:
  - `waiting` and `downloading` rows are picked up again.
  - A run left in `sending` when the app quit becomes `interrupted`: "The app closed while sending; it may or may not have started. Check your activity page on openrouter.ai." Studio cannot know without a remote id.
- **Downloads.**
  - Prefer the same-origin `GET /api/v1/videos/{id}/content?index=0`, or `unsigned_urls`, with the key through the endpoint's own client.
  - If that redirects to another host, or only signed `urls` are given, the redirect is followed by a binary, stream-to-disk variant of the guarded fetcher (crates/search/src/fetch.rs), **without the key**: addresses resolved and pinned, private and loopback refused, redirects followed by hand.
  - The file is written to `<gallery>/.partial/<id>` and renamed into place once its type is checked. Allowed types: PNG, JPEG, WebP, GIF, SVG, MP4, WebM.
  - Caps: 50 MiB per picture, 1 GiB per video.
- **Price model** (crates/agent/src/media/price.rs):

  ```rust
  enum Price {
      PerImage(Table),
      PerSecond(Table),
      PerToken,   // estimate unavailable
      Unknown,
  }
  // Table: price keyed by resolution, quality and with/without sound
  ```

  - It parses OpenRouter's `pricing_skus` key patterns seen on 2026-11-17: `duration_seconds[_with_audio|_without_audio][_<res>]`, `cents_per_video_output_second_<res>`, `cents_per_second_output`, `text_to_video_/image_to_video_duration_seconds_<res>`, `video_tokens*`. Some are in dollars, some in cents.
  - A key it does not understand gives `Unknown`, never a guess.
  - OpenAI prices come from a dated table in the bundled spec. The actual cost is computed from the response's `usage` tokens.
  - OpenRouter reports the actual cost in `usage.cost`.
- **The backend enforces the confirmation too.** `studio_start` takes `confirmed: bool` and refuses with `studio_needs_confirmation` when the rules require a confirmation. A webview bug therefore cannot spend money without the dialog. It also refuses with `studio_daily_limit` when the daily limit would be passed, whatever the webview sends.

---

## 5. Gallery and history

**In plain words.**
- **Where files go.** Every result is saved in a normal, visible folder: **`<home>/harnessUI/studio`**, next to the models folder. You can change it in Settings → Studio. It is kept outside app data because the Windows uninstaller may delete app data; your pictures are yours.
- **Search and filter.** The gallery is the card stream, with a **Grid** toggle for all media. You can search by prompt and filter by Pictures, Video, Starred, model or date.
- **The viewer** (Wireframe 4) moves with the arrow keys and shows every setting, the cost, the time taken, the file location and notes such as "Carries Google's SynthID watermark".
- **Actions:**
  - Star; Again; Reuse all; Reuse without seed; Edit; Animate; Use as reference; Send to chat.
  - **Save as…**: original format, plus an optional "Also save the settings as a .json file", off by default so your prompt is not passed along by accident.
  - Copy (pictures); Show in folder.
  - **Delete**: removes the file and its record. A note says the audit log keeps a line saying a run happened (model, cost), but not your prompt.
  - Select several to delete or save them together.
- **Settings → Studio** (a new tab) shows:
  - "Studio uses 1.2 GB in ~/harnessUI/studio (312 pictures, 14 videos)", with [Open folder].
  - [Delete stopped and failed runs].
  - The confirmation and daily-limit settings.
  - When each model list was last updated, with [Update now].
- **Files are kept exactly as the company sent them.** No metadata is stamped into them in 0.1.0, so the companies' watermarks and content credentials stay untouched.

**Technical detail**
- **Layout on disk.** `<gallery>/YYYY-MM/<run_id>-<n>.<ext>`, a sidecar `<run_id>.json` (the run's full record, key never included) so the history survives a lost database, and `<gallery>/refs/<sha256>.<ext>` for the pictures you sent.
  - Refs cannot reuse the `attachments` table, whose `chat_id` is `NOT NULL`. They do reuse the attachments validators: `sniff`, the 20 MiB cap, and WebP-to-JPEG/PNG through the picture worker.
  - Thumbnails go in `<gallery>/.thumbs/`, a 512 px JPEG made by the existing picture worker (`--internal-picture-worker`), extended to resize.
  - A video's poster frame is captured by the webview the first time the video is shown (canvas) and saved through `studio_save_thumb`. Until then the tile shows the length and the start frame, if there was one.
- **DB migration V3**, appended to `MIGRATIONS` in crates/store/src/schema.rs (never edit V1 or V2):

```sql
CREATE TABLE creation_runs (
  id            TEXT PRIMARY KEY,                       -- uuid
  kind          TEXT NOT NULL CHECK (kind IN ('image','video')),
  provider_id   INTEGER REFERENCES providers(id) ON DELETE SET NULL,
  provider_name TEXT NOT NULL,  provider_host TEXT NOT NULL,
  model         TEXT NOT NULL,  model_label   TEXT NOT NULL,
  prompt        TEXT NOT NULL,
  params_json   TEXT NOT NULL CHECK (json_valid(params_json)),   -- exactly what was sent, no key
  refs_json     TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(refs_json)), -- sha256 + role
  status        TEXT NOT NULL CHECK (status IN ('queued','sending','waiting','downloading',
                                   'done','failed','blocked','cancelled','interrupted')),
  remote_id     TEXT,  progress REAL,
  estimate_usd  REAL,  cost_usd REAL,
  error_json    TEXT,                                   -- AppError + company code, charged?
  audit_id      INTEGER,                                -- to complete the audit row later
  created_at    INTEGER NOT NULL, started_at INTEGER, finished_at INTEGER
);
CREATE TABLE creation_outputs (
  id          TEXT PRIMARY KEY,
  run_id      TEXT NOT NULL REFERENCES creation_runs(id) ON DELETE CASCADE,
  idx         INTEGER NOT NULL,
  path        TEXT NOT NULL,  thumb_path TEXT,
  mime        TEXT NOT NULL,  size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
  width INTEGER, height INTEGER, duration_ms INTEGER, seed INTEGER,
  starred     INTEGER NOT NULL DEFAULT 0 CHECK (starred IN (0,1)),
  created_at  INTEGER NOT NULL
);
CREATE INDEX creation_runs_created ON creation_runs(created_at);
CREATE INDEX creation_runs_live ON creation_runs(status)
  WHERE status IN ('queued','sending','waiting','downloading');
```

- **Accessors.** New crates/store/src/creations.rs: insert, update status and progress, list paginated with filters, delete, spend-today total, usage. Re-exported from lib.rs. A cap like `MAX_OUTPUTS_PER_CHAT`, for example 20,000 runs, keeps the table bounded.
- **Showing media.**
  - A new `studio_output_bytes(id)` returns raw bytes with `tauri::ipc::Response`, which the app does not use yet; the webview turns them into a `blob:` URL.
  - The CSP adds `media-src blob:` (src-tauri/tauri.conf.json), and the pinned-CSP test in src-tauri/src/commands.rs:210 is updated.
  - Blob URLs seek in WebKit without the Range support a custom scheme would need.
  - A main-window-only `studio:` scheme is the fallback if very large videos make blobs too heavy.
- **Export** reuses the `export_artifact` pattern: `saveDialog`, then `studio_export(output_id, path, with_settings)` with `export_target` (an absolute path, the right extension, the app's own folders refused through `permissions::forbidden_for_tools`).
- **Changing the gallery folder** follows the models-folder setting pattern (`hub::models_dir`). Existing files are moved only when the user asks.

---

## 6. Working with chat

**Recommended for 0.1.0:**
- **Send to chat** on any picture puts it into the current chat's message box as an attachment, or into a new chat. It reuses the attachments path with its 20 MiB and 8-picture limits. Videos cannot be sent, because chat models do not take video; the button is hidden for them.
- **Open in Studio** on a picture you attached in a chat puts it in Studio's tray as a reference.

**Recommended for 0.1.x, not 0.1.0: a "make a picture" tool for chat models** (Decision 2).
- It would reuse the same Studio backend, so the result lands in the gallery and the chat.
- It needs a new policy scope, and an approval card that shows the model, the settings and the **price**.
- It must ask **every time, even in Allow all and Full access**, because it spends money and sends data to a company. It also needs a per-chat spending limit and audit rows decided by the user.
- That is a full policy and approval piece of work. Shipping Studio first lets the backend settle.

**Technical detail.** `studio_attach_to_chat(output_id, chat_id)` writes an `attachments` row from the gallery file (it copies to `attachments/<sha>.<ext>`, so deleting from the gallery never breaks a chat). The later tool would be a built-in server, `studio`, with tool `create_picture`, behind a new `Scope` in crates/policy/src/engine.rs, with a decision that ignores Allow all.

---

## 7. Privacy and safety

**What goes where.** Shown in Studio, in SECURITY.md and on the website's privacy policy:

| What | Where it goes | Never |
|---|---|---|
| Your prompt, settings, pictures you add (references, start and end frames) | Only the chosen endpoint's host (`openrouter.ai` or `api.openai.com`). OpenRouter passes the job on to the company that runs the model (for example Google, ByteDance or OpenAI) | No other host; nothing when you only open Studio |
| Your API key | Only its endpoint's own scheme, host and port | Never to a result link on another host (CDN); those are fetched without the key |
| Results | Downloaded from the endpoint, or from the company's file host without the key, then kept only on your computer | Not uploaded anywhere by harnessUI |
| Usage data or telemetry | None | n/a |

- **Cloud mark.**
  - Always shown in Studio's top bar, next to the model in the picker, and on the line above Create: "Cloud: your prompt and added pictures go to openrouter.ai".
  - For OpenRouter the model info adds "OpenRouter sends it on to the company that runs this model (shown in the result details)".
  - It also adds per-model notes from the research: "OpenRouter video is not covered by zero data retention"; "Google adds an invisible SynthID watermark"; "xAI says it does not train on these" (when xAI is added).
- **No automatic requests.** Opening Studio or Settings → Studio sends nothing. Model lists come from the cache and are refreshed only on Test, Get the list, Update model lists, or picking a model (for its picture price). e2e/m8_offline.py adds Studio and Settings → Studio to its visits and must still see zero requests.
- **Docs to update:**
  - SECURITY.md "Secrets and privacy": add prompts, pictures you add and results, and "OpenRouter passes it on".
  - ARCHITECTURE.md 13.6: add a "Studio" row to the network table, with the endpoint hosts and "the company's file host, through the guarded fetcher, without the key".
  - README.md.
  - The harnessui.site privacy policy (the page added in 0.0.8).
  - docs/SPEC.md:121 lists "image generation" as out of scope for v1. Annotate it "(Changed in 0.1.0 at the owner's request: cloud image and video generation, see docs/dev/0.1.0/README.md)", the way 0.0.8 annotated the updater lines.
- **When a company's safety filter says no:**
  - The card stays, with your prompt editable, and a plain message naming the company and, when the company says so, the stage: "OpenAI's safety filter refused the prompt" or "…stopped the picture after it was made".
  - It says whether you were charged, when known (OpenRouter: failed runs are not charged; OpenAI: per its usage), and gives the request id for an appeal.
  - **Never retried automatically**, and there is no "rewrite to get past the filter" helper.
  - Mapped codes: OpenAI `moderation_blocked` (with `moderation_details`), OpenRouter moderation errors and 502 "failed, not charged", and OpenAI "organization must be verified", which links to the OpenAI settings page.
- **Audit log.** One row per run, written with no schema change, like the sharing proxy's rows (src-tauri/src/share/proxy.rs:1956):
  - Server `studio`; tool `picture` or `video`; `chat_id` empty.
  - Arguments: the endpoint name and host, the model, the changed settings, the number of pictures sent, the estimate, and whether the user confirmed.
  - **The prompt length only, not the prompt**, so Delete in the gallery really removes your prompt. The log is append-only.
  - Decision `allowed_without_prompt`, decided by `user`.
  - The row is completed when the run ends (possibly after a restart, through `audit_id`) with the status and the actual cost.

---

## 8. Technical architecture, mapped to files

```
 webview (React)                         Rust (src-tauri)                       crates
 ----------------                        ----------------                       ------
 StudioView ── store/studio.ts ──ipc──►  studio/commands.rs ──► StudioManager ──► agent::media::MediaProvider
   PromptDock, SettingsPanel,            (start/cancel/list/                       ├─ OpenRouterMedia (images+videos)
   ParamField, AdvancedDrawer,            bytes/export/...)       │                └─ OpenAiImages
   ModelPicker(+CloudMark),               ◄── "studio-progress" ──┤                agent::client (redirect/proxy/key rules)
   RunCard, Viewer, ConfirmRun,                                   ├─► store::creations (V3 tables)
   RefTray, Grid                                                  ├─► store::audit (append/complete)
                                                                  └─► search::Fetcher::fetch_to_file (no key)
```

**Rust: the provider adapters behind one trait.** New crates/agent/src/media/{mod.rs, openrouter.rs, openai.rs, spec.rs, price.rs, specs/openai_images.json}:

```rust
#[async_trait]
pub trait MediaProvider: Send + Sync {
    async fn list_models(&self) -> Result<Vec<MediaModel>, ProviderError>;
    async fn model_price(&self, model: &str) -> Result<Price, ProviderError>;   // may be cached
    /// Sent at most once. Pictures come back done; video comes back as a job.
    async fn start(&self, req: &MediaRequest, scratch: &Path) -> Result<Started, ProviderError>;
    async fn check(&self, remote_id: &str) -> Result<JobState, ProviderError>;
    async fn download(&self, job: &JobDone, index: usize, dest: &Path) -> Result<Saved, ProviderError>;
}
pub enum Started { Done(Vec<Saved>, Option<Cost>), Job { remote_id: String } }
pub enum JobState { Pending, Running { progress: Option<f32> }, Done(JobDone), Failed(MediaFailure) }
pub struct MediaFailure { pub stage: Option<Stage>, pub blocked: bool, pub charged: Option<bool>,
                          pub company_code: Option<String>, pub request_id: Option<String> }
pub struct MediaModel { pub id: String, pub display_name: String, pub family: String,
                        pub outputs: Outputs, pub inputs: Inputs, pub params: Vec<ParamSpec>,
                        pub price: Price, pub notes: Vec<String> }
```

- **crates/agent/src/client.rs**:
  - Make `same_origin_redirects`, `authorize`, `read_capped` and `network_error` `pub(crate)`, so media requests inherit the redirect, proxy, key-transport and key-scrubbing rules.
  - Add `post_once` (no retry on create).
  - Add `download_to_file` (streamed, capped, key scrubbed from errors).
  - Add a media JSON cap of 192 MiB, streamed to a temp file.
- **crates/agent/src/cloud.rs**: `openai_media_kind(id)`, the reverse of `openai_can_chat`, and `media_company(kind)`.
- **crates/search/src/fetch.rs**: `fetch_to_file(url, accept: Media, cap, dest)`, a binary variant with the same address checks and no proxy, which never carries a key.
- **crates/store**: schema.rs V3; creations.rs (new); lib.rs re-exports.
- **src-tauri/src/studio/{mod.rs, manager.rs, commands.rs, errors.rs}** (new). The commands:
  - `studio_models`: from the cache, no network.
  - `studio_refresh_models(provider_id)`.
  - `studio_model_price(provider_id, model)`.
  - `studio_estimate(req)`.
  - `studio_start(req, confirmed)`.
  - `studio_cancel(run_id)`.
  - `studio_fetch_anyway(run_id)`.
  - `studio_runs(filter, page)`.
  - `studio_output_bytes(id)` (`ipc::Response`).
  - `studio_save_thumb`.
  - `studio_star`.
  - `studio_delete(ids)`.
  - `studio_export(id, path, with_settings)`.
  - `studio_show_in_folder(id)`.
  - `studio_usage`.
  - `studio_attach_to_chat(output_id, chat_id)`.
  - `studio_get_prefs` / `studio_set_prefs`: last model per kind, Advanced open, confirmation rules, daily limit, setups. These are typed commands, because the generic `set_setting` only takes its `ALLOWED` list.

  The event is `studio-progress`. Errors are four-part `AppError`s, with new codes `studio_blocked`, `studio_needs_confirmation`, `studio_job_lost`, `studio_interrupted`, `studio_no_endpoint` and `studio_org_verification`.
- **Register each command in three places:** src-tauri/commands.list, `allow-<cmd>` in src-tauri/capabilities/main.json (main window only), and `generate_handler!` in src-tauri/src/lib.rs:108. Also in lib.rs: `manage(StudioManager)`, and resume at start next to the downloads resume.
- **src-tauri/src/providers.rs**: cache media lists in `test_provider`; clear them in `delete_provider`. **src-tauri/src/dto.rs**: `MediaModelDto`, `ParamSpecDto`, `RunDto`, `OutputDto`, `EstimateDto`.
- **src-tauri/tauri.conf.json** and **src-tauri/src/commands.rs**: `media-src blob:` and the updated CSP pin test.

**Frontend.**
- **src/App.tsx**:
  - `view: "chat" | "studio"`, rendering `<StudioView/>` in place of ChatView and Workbench.
  - Ctrl/⌘ G, and the `app:open-studio` event.
  - The Esc branch does nothing in Studio view; the viewer handles its own Esc.
  - Check that unmounting ChatView never stops a running reply. If it does, keep ChatView mounted and hidden.
- **src/components/Sidebar.tsx**: the Studio icon button with the running-count badge.
- **src/components/studio/** (new):
  - StudioView.tsx, PromptDock.tsx, SettingsPanel.tsx.
  - ParamField.tsx, the only widget renderer, driven by the spec.
  - AdvancedDrawer.tsx.
  - ModelPicker.tsx: grouped by endpoint and family, with search, filter chips (Picture, Video, Edit, References, Start/End frame, Sound, Transparent), Recent first, badges, a price hint, and the CloudMark.
  - RunCard.tsx (including the job state), Viewer.tsx, ConfirmRun.tsx, RefTray.tsx, Grid.tsx.
  - canonical.ts (carry and snap rules), cost.ts, api.ts, types.ts.
- **src/store/studio.ts** (new zustand store, like src/store/compare.ts): models, a draft per kind, the runs page, live progress by `listenInEffect` (src/lib/listen.ts), filters.
- **src/components/settings/**: endpointPresets.ts media flags; Endpoints.tsx badges; a new Studio settings page, added to SettingsDialog.tsx and to `SETTINGS_PAGES` in e2e/m8_offline.py.
- **src/components/ShortcutsDialog.tsx**:
  - Ctrl/⌘ G opens Studio.
  - In the prompt box: Enter creates, Shift+Enter adds a new line.
  - In the viewer: ←/→ move, F stars, R reuses all, Shift R reuses without seed, A animates, S saves, C copies, Del deletes, Space plays, Esc closes.
- **src/lib/words.ts**: all plain-language copy, including the safety and charge messages.

**Docs and version.**
- ARCHITECTURE.md: 7.5 (providers), 12 (storage: the gallery folder and V3), 13.6 (network row), 13.7 (audit rows), 14 (frontend: Studio view), 17 (tests).
- SECURITY.md; CHANGELOG.md; README.md; docs/SPEC.md (scope note).
- docs/dev/0.1.0/README.md, with the owner's words kept verbatim, like the 0.0.8 plan.
- Version 0.1.0 in Cargo.toml `[workspace.package]`, package.json and src-tauri/tauri.conf.json.
- scripts/docs_test.py keeps the ARCHITECTURE sentences in step.
- No new dependency is required. A system notification when a video is done would need tauri-plugin-notification and a notices update, so 0.1.0 uses the in-app notice instead.

---

## 9. Scope, risks and tests

### In 0.1.0
- The Studio section (Pictures and Video), the sidebar entry and Ctrl/⌘ G.
- OpenRouter pictures and video; OpenAI pictures, including Edit with words.
- Controls built from the model's parameter list: the simple layer, the Advanced drawer with change chips, Extra fields (JSON) and the request preview.
- Carrying settings between models, with a note about what changed.
- References, start and end frames, Animate.
- Seed handling: Again, Reuse all, Reuse without seed. My setups.
- The price estimate, confirmation rules, actual cost, spent today, and an optional daily limit.
- The job queue; video jobs that survive a restart; Stop watching and Get it anyway.
- A local gallery (folder, sidecars, V3 tables), the viewer, star, search and filters, export, delete, disk use.
- Send to chat and Open in Studio.
- The Cloud mark everywhere, audit rows, and clear safety-refusal messages.
- Docs, privacy policy, the SPEC note, version 0.1.0.

### Later (0.1.x / 0.2)
1. The chat-model picture tool (Decision 2).
2. fal.ai (Decision 1).
3. xAI and Recraft presets, and Generic opt-in.
4. Mask painting and markup.
5. Video extend, edit and Save frame.
6. Draft, then "Make full version".
7. Run on several models side by side (borrowing from Models → Compare).
8. Styles.
9. Prompt enhance with a local model.
10. A system notification.
11. Partial-picture previews while streaming.
12. Keyframes and multi-shot video.
13. Google direct.

### Rough effort (two engineers)

| Part | Person-days |
|---|---|
| agent::media: trait, OpenRouter pictures and video, OpenAI pictures and edits, spec and price parsing, downloads, guarded binary fetch | 12–14 |
| Store V3, StudioManager (queue, resume, cancel), commands, audit, errors, CSP | 7–8 |
| Webview: StudioView, prompt box, panel, ParamField, Advanced, picker, cards, viewer, confirm, grid, Settings → Studio, Endpoints badges | 12–14 |
| Tests (fixtures, mock server, vitest, e2e), docs, privacy policy, version | 6–7 |
| **Total** | **about 37–43 (7–8 person-weeks; about 4–5 calendar weeks)** |

### Risks and how the design handles them

- **Paying twice.**
  - The create request is never retried after it was sent.
  - The backend enforces the confirmation.
  - A run interrupted while sending says honestly that it may have been charged.
- **A started video is charged even if you stop.**
  - The confirmation and the card both say so.
  - Get it anyway recovers the result.
- **Wrong price estimates.** OpenRouter's price keys are free-form, mixing dollars and cents, and some are per token.
  - Only known patterns are parsed; anything else is "price unknown", which always asks.
  - The actual cost from `usage.cost` is always shown.
  - Table tests cover every pattern in the saved catalogue.
- **The catalogue shape or the models change every month.**
  - Parsing is tolerant: unknown parameters become Extra fields hints.
  - Fixtures are recorded from the real catalogues.
  - OpenAI matching uses id prefixes. An unknown future `gpt-image-*` gets the basic fields and the note "settings may not match this new model".
  - OpenAI retires three models on Dec 1, 2026, two weeks after the expected release, so the bundled OpenAI spec must lead with the 2.6 models.
- **Result links on other hosts, and redirects to a CDN.**
  - Guarded fetcher, never the key.
  - A recorded fixture of a real redirect must be checked before release.
- **Large responses.** Base64 pictures and videos are streamed to disk with caps, never buffered through `read_capped`.
- **Links that expire while the app is closed.** OpenRouter's signed URLs carry `expires_at`.
  - The same-origin content endpoint is preferred.
  - If a result has expired, the card says so plainly.
  - Expiry for OpenRouter's content endpoint is not documented: a risk to check.
- **Safety false positives and account risk.**
  - Clear messages, no automatic retry, no bypass helper.
  - Runway (charged and suspends accounts on repeated blocks) is excluded from 0.1 partly for this reason.
- **Disk growth.** Disk use is shown, there is bulk delete and "Delete stopped and failed runs", and the folder is visible.
- **The offline promise.** m8_offline visits Studio. The lists come from the cache.
- **WebKit video playback.** The blob route avoids the Range problem. 1 GiB per video is the ceiling.
- **Scope creep.** The "later" list above is explicit. The chat tool and fal are owner decisions, not defaults.

### Test plan

- **Rust unit tests** (inline `#[cfg(test)]`):
  - Turning the catalogue into specs, using the real 2026-11-17 OpenRouter snapshots (68 picture and 33 video models) committed as fixtures under crates/agent/tests/fixtures/media/.
  - The price parser: one case per `pricing_skus` pattern, plus "unknown".
  - The OpenAI size checks.
  - Error mapping per company code (`moderation_blocked`, 502 not charged, 403, org verification).
  - The StudioManager state machine with a fake `MediaProvider`: queue limits, resume after restart, `sending` becoming `interrupted`, cancel, Get it anyway, the 30-minute timeout, refusal without confirmation.
  - Store V2 → V3 upgrade from a V2 database, FK `SET NULL` on provider delete, cascade on run delete.
- **Mock-server tests:** crates/agent/tests/media_endpoints.rs, on the existing `common/mod.rs` MockServer:
  - A picture run (b64 decode, several outputs).
  - A video create, then check, check, done, download.
  - **No second create on a 500 or a timeout**; a retry on 429 with Retry-After.
  - A redirect to a foreign host **carries no key**.
  - The key is scrubbed from error text.
  - Oversized body caps; a file type that does not match its content is refused.
  - A signed-URL fallback through the guarded fetcher, where a private address is refused.
- **Frontend tests** (vitest, `vi.mock("../lib/ipc")`, like ChatView.cloud.test.tsx and settings/Endpoints.test.tsx):
  - Controls render from a spec, and unsupported fields are hidden.
  - Advanced changes count up and show as chips; reset works.
  - Carrying settings between models produces the right note.
  - The confirmation appears for video, at $1 or more, and for an unknown price.
  - "Don't ask again" works.
  - Esc does not cancel a run.
  - The Cloud mark text names the right host.
  - Reuse fills the form without starting a run.
  - canonical.ts and cost.ts have pure unit tests.
- **e2e** (by hand, tauri-driver on Linux):
  - A scripted scenario adds an OpenRouter-kind endpoint pointing at a local mock (`http://127.0.0.1:<port>/api/v1`) that serves the recorded catalogues, a picture and a short video job.
  - The scenario creates a picture, starts a video, restarts the app mid-job, sees it finish, exports, deletes.
  - m8_offline.py opens Studio and Settings → Studio with every request refused, and expects none.
- **Owner checklist before release.** One real run on each company, under $1 in total: a low-quality GPT Image 2.6 Flare picture on OpenAI; a picture and a 4 s 480p video on OpenRouter (for example Grok Imagine Video 1.5 Lite at about $0.02/s); one safety refusal. Check the shown cost against each company's activity page.

---

## 10. Open decisions for the owner (3)

**Decision 1: Which companies in 0.1.0?**
- **A. OpenRouter + OpenAI (recommended; chosen by the owner).** Both keys are already in the app. This gives about 68 picture and 33 video models, including Veo 3.1, Gemini Omni, Kling 3 and Seedance. It is the smallest, safest build.
- B. A + fal.ai. It adds the widest video set (Kling 4, Luma Ray 3.2, MiniMax H3, FLUX 3 Max), at the cost of a new key and about 2 more weeks.
- C. A + "any OpenAI-style picture server": xAI and Recraft as new company choices, plus an opt-in for Generic endpoints. About 1 more week; mostly pictures.
- *Trade-off:* A ships sooner and keeps the privacy story to two hosts. B and C give more choice, but each new company means one more host in the privacy policy, one more error vocabulary and one more thing to keep current. I'd do B in 0.1.1 if people ask for Kling 4 or Luma.

**Decision 2: Should chat models be able to make pictures in 0.1.0?**
- **A. Not yet (recommended; chosen by the owner).** Ship Send to chat and Open in Studio now, and the tool in 0.1.x.
- B. A picture-only tool in 0.1.0 that asks every time (even in Allow all or Full access) and shows the price on the approval card. About 1 more week, plus policy and approval work.
- C. Pictures and video tools in 0.1.0.
- *Trade-off:* A chat tool is convenient, but it lets a model trigger spending. Building it on a settled Studio backend, with its own spending limit, is safer. C risks surprise video bills.

**Decision 3: Spending guardrails by default**
- A. Confirm every video, any run of $1 or more, and any run with an unknown price; the daily limit is off but offered in Settings → Studio.
- **B. As A, plus a default daily limit of $10 that pauses Studio until you raise it. (Chosen by the owner.)**
- C. Confirm only runs of $5 or more; no limit. This means the fewest interruptions.
- *Trade-off:* A protects against the expensive mistakes (a 10 s 1080p Seedance clip is about $11.64 elsewhere; Veo 3.1 at 8 s, 4K with sound is $4.80) without nagging on cheap pictures. B is the safest for new users but can block someone mid-project. C suits experts but makes accidental spending easy.

*Treated as decided, unless the owner says otherwise:*
- Studio is a separate section; the owner asked for a "section".
- It is named "Studio" with a "Create" button.
- Results go in a visible `~/harnessUI/studio` folder.
- The audit log stores the prompt's length, not the prompt.

---

Relevant files: /home/user/harnessUI/docs/SPEC.md (line 121, the scope note); /home/user/harnessUI/src/App.tsx (view state, the shortcut, Esc); /home/user/harnessUI/src/components/Sidebar.tsx; /home/user/harnessUI/src/components/settings/endpointPresets.ts; /home/user/harnessUI/src/components/FineControl.tsx (the Automatic pattern); /home/user/harnessUI/crates/agent/src/client.rs; /home/user/harnessUI/crates/agent/src/cloud.rs (line 991); /home/user/harnessUI/crates/store/src/schema.rs (MIGRATIONS at line 414; providers at line 202; attachments.chat_id NOT NULL at line 112); /home/user/harnessUI/src-tauri/src/downloads.rs (DownloadManager at line 252, resume_interrupted at line 1020); /home/user/harnessUI/src-tauri/tauri.conf.json (CSP at line 59: no media-src); /home/user/harnessUI/e2e/m8_offline.py (SETTINGS_PAGES at line 38); /home/user/harnessUI/ARCHITECTURE.md (13.6 at line 1224, 13.7 at line 1251). The catalogue snapshots used for the parameter and price analysis are in /tmp/claude-0/-home-user/f8f6f1ef-33e4-57b5-aed7-1e790c206f77/scratchpad/or/images.json and /tmp/claude-0/-home-user/f8f6f1ef-33e4-57b5-aed7-1e790c206f77/scratchpad/or/videos.json. They may be cleared; copy them into the repo as test fixtures.
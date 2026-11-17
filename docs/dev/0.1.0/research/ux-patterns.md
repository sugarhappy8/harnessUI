# UX research: image and video generation for harnessUI 0.1 ("the cloud update")

Read-only research. I changed no files. Sources are listed at the end. Four official doc sites blocked my fetcher with HTTP 403: Midjourney, the OpenAI Help Center, Runway's help center, and the Luma composer pages. Where I describe those products, the claims come from search snippets or third-party guides and are marked "(secondary)".

## 1. Bottom line: what "easy to use but not dumbed down" means in late 2026

The leading tools share one structure, which you could call a **three-layer disclosure**:

1. **Always visible: one prompt dock.** It holds the prompt, a model chip, chips for shape (aspect ratio) and count, and a **Generate button that shows the cost**. Krea, Flow, Ideogram, Midjourney web, Leonardo and the fal Sandbox all put the prompt and model picker in one bar, and keep model-specific settings next to it.
2. **Common settings that follow the model.** The tool only shows what the chosen model supports. Ideogram: "Each model shows only the settings it supports… check the panel after switching". Krea: "not every model supports every setting". fal Sandbox goes further: it maps one **canonical input** (prompt, image, aspect ratio, duration) onto each model's own parameter names, and rounds a duration to the nearest value the model supports.
3. **Advanced: every provider parameter, collapsed but complete.** Ideogram keeps a collapsed "Advanced settings" row for Seed, Background and so on. fal pages have "Additional Settings". Replicate and fal publish a per-model OpenAPI schema, so this layer can be **generated as typed fields**. ComfyUI's 2026 App Mode keeps the full node graph behind a simple form.

Two rules separate "simple" from "dumbed down":

- **Nothing that changes the result is invisible.**
  - A changed Advanced value shows as a chip in the prompt dock.
  - An enhanced prompt is shown and can be undone.
  - The exact request can be inspected.
- **Every output can be reproduced and remixed.** The full settings are stored with each output, with one-click "Reuse all" and "Reuse without seed".
  - InvokeAI: *Use All*, and *Remix Image* (all settings except the seed).
  - Krea: *Reuse parameters*.
  - Midjourney: *Use prompt / style / image* (secondary).
  - fal: *Rerun*.

## 2. Market facts that should shape the design (verified)

- **Models come and go every few months.**
  - OpenAI's **Sora 2 models and Videos API were shut down on Sept 24, 2026**, with "no one-to-one replacement".
  - Google's **Veo 3.1 Preview in the Gemini API was shut down on Oct 22, 2026**. The replacement is **Gemini Omni Flash** (`gemini-omni-1.2-flash`).
  - **Imagen is shut down**; Google says to move to Nano Banana.
  - So: never hard-code the model list. History must outlive a model; "Reuse" on a retired model should offer the nearest model and say which settings changed.
- **OpenRouter, already an endpoint preset in harnessUI, now does images and video.**
  - `POST /api/v1/images`, plus chat completions with image output.
  - `POST /api/v1/videos`, which is asynchronous: submit, poll, download.
  - `GET /api/v1/images/models` and `GET /api/v1/videos/models` list each model's inputs, durations, resolutions, aspect ratios, pricing and model-specific parameters (for example Veo's `personGeneration`).
  - Reference images and first/last frames are standardized across models.
  - This is close to ideal for a UI built from data.
- **Which harnessUI presets can generate today:** OpenAI can do images (GPT Image) but no longer video. OpenRouter can do both. Anthropic does neither. abliteration.ai is unknown.
- **Where per-model schemas come from:**
  - fal: `fal.ai/api/openapi/queue/openapi.json?endpoint_id=…`
  - Replicate: `openapi_schema`; each input has an `x-order` that gives field order.
  - OpenAI and Gemini have fixed APIs and would need small hand-written specs.
- **Video jobs take minutes.**
  - Krea: 1–10 minutes.
  - Gemini app: you can't use that chat while it processes, but you can start another and come back.
  - OpenAI's old job API had `queued / in_progress / completed / failed` and a `progress` percentage ("if available", and not linear).
  - Omni Flash: edit or extend by `previous_interaction_id`. Extension adds 3–10 s at the end, up to 30 s total.
- **Watermarks.** SynthID is on all Gemini images and videos. Don't strip it; mention it in the info panel.

## 3. Product-by-product patterns

- **ChatGPT Images** (secondary; the help center returned 403)
  - Images tab in the sidebar, with trend-based style templates.
  - Aspect presets 1:1, 3:4, 9:16, 4:3, 16:9.
  - "Select" to highlight an area to edit; OpenAI warns the highlight is not exact.
- **Google Flow**
  - Click the model name in the prompt box to pick Video, then Frames or Ingredients.
  - "+ Add start frame / + Add end frame" drop zones.
  - Typing `@` in the prompt searches project assets.
  - The settings pick aspect ratio, number of outputs, model and length; Google says to check the resolution and current credit cost in the prompt box settings.
  - Extend; a History panel of versions with "Save to Project"; "Save frame" to use a frame as an ingredient or start frame; Scenebuilder for trimming and ordering clips.
  - Feature support differs by model; Veo 3.1 Quality, for example, has no ingredients.
- **Midjourney web** (secondary)
  - Imagine bar, with a settings popover whose defaults persist: aspect ratio, model, stylization, weirdness, variety.
  - Hover actions: Vary Subtle/Strong, Rerun, Use prompt, Use style, Use image.
  - Draft mode for cheap exploration.
- **Krea**
  - Prompt box at the bottom, model chip at its bottom left, settings below it that change with the model.
  - The **model picker lists a one-line description, a speed rating (⚡), a quality rating (◆) and the approximate compute cost per generation for each model**, with recent and popular models first and an "Auto" choice.
  - Hovering a result row gives Retry, **Reuse parameters**, Share parameters, Download all.
  - Hovering an image gives Fullscreen, Edit, Upscale & enhance, **Video** (the image becomes the start frame), Download, ♥.
  - Video: duration, resolution, start/end image, and a **Draft mode** (480p, cheaper) with "Generate full video / 4K" later; Extend on hover.
  - Sessions in a left panel, plus an Assets grid.
- **Leonardo**
  - Prompt Enhance (Auto), preset styles, negative prompt under advanced settings.
  - **Flow State:** an endless feed of images with "More like this", a "Scroll to Generate" switch to control spending, and stages you can go back to.
- **Ideogram**
  - Magic Prompt switch, on by default; newer models use "Prompt upsampling" with Auto/On/Off.
  - **The Generate button shows the total cost of the run**, which changes with the number of images and the quality choice.
  - Quality tiers differ by model (Turbo / Default / Quality).
  - Seed under Advanced, "Random" by default, with an honest note that a seed "isn't a saved copy of an image".
  - Settings are drafts for the session only; **Recreate** restores a finished run's inputs.
- **Freepik / Magnific**
  - Auto model selection ("leave it on Auto until you can finish the sentence 'it came out wrong because…'").
  - Settings for Mode, Styles, Effects, Character, Object, Colors.
  - A node canvas (Spaces) for advanced users.
- **fal Sandbox / playground**
  - Media type, then operation (Text to Image, Image to Video, …), then models or **model sets**.
  - **Quantity times models gives the request count.**
  - **A cost estimate in the prompt bar footer, coloured green below $1, yellow from $1 to $2.50, red above $2.50.**
  - Results show Fastest, Cheapest and Free badges, and a full-screen viewer with arrow keys and model, cost and time.
  - "Use as input" sends a result into another operation.
  - History records inputs, cost and duration, with Copy, Rerun, Share and Bookmark.
  - Prompt enhancement is done by an LLM.
- **Replicate playground:** image grids and side-by-side comparison of models and settings, plus "copy the code"; the form follows the model's OpenAPI schema.
- **Runway:** priced in credits per second (12 credits/s for Gen-4.5); 2–10 s clips; aspect ratio follows the input image by default. Explore Mode, a slower queue that uses no credits, was on the Unlimited plan, which reportedly ended in 2026 (secondary).
- **Luma Dream Machine:** Boards; a Draft mode toggle, then "master" to 4K HDR; keyframe chips with drawn directions; EXR export.
- **Cherry Studio "Painting":** a separate page with a provider picker (SiliconFlow, AiHubMix, DMXAPI, TokenFlux) and a `?` tooltip on every parameter. Gemini image generation is left to chat because it "does not support parameter tuning".
- **Msty:** image generation is a per-model "purpose". Parameters like `size` and `n` go in as raw model parameters, a power-user approach that is easy to get wrong. **Media Studio (2.7, May 2026)** has Spotlight, side-by-side compare, stars, drawing markup used as context for the next version, and feeding an output back as context.
- **LM Studio:** no built-in generation found, only community plugins (Hugging Face, stable-diffusion.cpp, Gemini, Draw Things).
- **Fooocus:** "fewer than three mouse clicks" to a first image; a single performance choice (Speed / Quality / Extreme Speed) replaces tuning steps and CFG by hand; combinable style templates; an advanced tab for the rest.
- **InvokeAI:** boards, stars, a metadata viewer; Use All / Remix / Use Prompt / Use Seed. Recalling settings does not start a run.
- **ComfyUI App Mode (2026):** a builder exposes chosen node inputs as a form (inputs on the right, outputs in the middle, a bottom panel that switches between queue and results, and "Cancel this run"). The workflow can open in App or Graph view.

## 4. Pattern catalogue and what to do in harnessUI

- **Layout.** Put the prompt dock at the bottom, as in chat. Results stream above it as **run cards** (one prompt with its N outputs). A right-hand settings panel offers Simple or All. Sessions sit on the left, plus "Starred" and "All media". This keeps the app's chat muscle memory.
- **Model picker.**
  - Group by provider and endpoint.
  - Show **capability badges taken from the capability listings**: Image, Video, Edit, Refs ×N, Start/End frame, Audio, Text-in-image, Transparent, Max size (4K) or Max length (15 s), Seed, Negative prompt.
  - Show speed and quality ratings and a per-run cost estimate, as Krea does.
  - Add filter chips and search, and "Recent" at the top.
  - Skip a hidden "Auto" router. It hides where data goes, which goes against the app's Cloud-mark promise; default to the last-used or a recommended model instead.
- **Styles and presets.**
  - A short style list that **adds visible text** to the prompt, Fooocus-style templates, shown in the request preview rather than hidden.
  - **"My setups"**: a saved model plus settings plus style.
- **Shape and count.**
  - Aspect chips taken from the model's supported list (1:1, 3:2, 2:3, 16:9, 9:16, plus extra ratios where offered).
  - Count chips 1/2/4, capped by the model.
  - Resolution tier (1K/2K/4K) where offered.
- **Seed, vary, reuse.**
  - Seed under Advanced: "Random", a 🎲 button, and 🔒 to keep it.
  - On each output: Again, Vary (Subtle/Strong where the model supports it, otherwise "Again with a new seed"), Reuse all, Reuse without seed.
  - Reusing fills the dock and **never starts a run by itself**, as in InvokeAI.
- **References and image-to-video.**
  - An "+ add" tray in the dock with role chips: Reference, Style, Character, Start frame, End frame. The roles follow OpenRouter's standardized reference and frame types.
  - Drag-and-drop and paste.
  - "Animate" on any image card sends it to Video as the start frame (Krea, fal "Use as input").
- **Editing (in order of ease).**
  - Edit with a prompt, carrying the conversation where supported (Nano Banana and Omni `previous_interaction_id`).
  - Extend a video (Omni or Krea), and Save frame (Flow).
  - Area editing with a brush, with ChatGPT's warning that edits may spill past the selection.
  - Upscale and expand where a model offers them.
- **Prompt helpers.**
  - ✨Enhance shows the rewritten prompt and lets you undo it, and keeps both prompts in the metadata.
  - **It can run on the user's local chat model**, so the prompt stays on the computer until Generate, a privacy angle no competitor has.
  - "Describe this image" with a local vision model.
  - Negative prompt only where supported, under Advanced.
- **Cost before running.**
  - Show "≈ $0.16" on or beside Generate, computed from the listing price times count times duration.
  - fal-style colours.
  - Ask for confirmation above a threshold (for example over $1, or any video longer than 10 s).
  - A "spent today" line and an optional daily cap.
  - Show the actual cost on the run card afterwards.
- **Progress and queue for video.**
  - Job card: status, % when the provider reports it, elapsed time against typical time, "you can keep working, it will appear here".
  - A system notification when done.
  - A queue strip (running / waiting / done / blocked).
  - Cancel, with the words "the company may still charge for a started job" where that applies.
  - Jobs survive an app restart: keep the job id and poll again.
- **History and metadata.**
  - **Download every output to local disk at once** (provider links expire), with sidecar JSON holding the full request (key removed), the response, the cost, the time, the model id and the endpoint host.
  - Also embed it in PNG text chunks, as InvokeAI does.
  - Search by prompt and filter by model, type, starred and date.
  - Retired models stay readable.
- **Compare.** "Run on several models": the same prompt and canonical settings across 2–4 models (fal Sandbox, Replicate), shown as a grid with time and cost per tile. The app already has a model Compare component to borrow from.
- **Favourites and export.** ★ on hover. Save as… (original format), Copy, Save all (zip), Show in folder, and a "Remove the prompt from the file" option when exporting.
- **Safety blocks.**
  - A plain message that names the company and the stage: "OpenAI's safety filter stopped this picture after it was made" versus "…refused the prompt".
  - Say whether it was charged, when known.
  - Keep the run card with the prompt so it can be edited; show the request id for an appeal.
  - **Never retry automatically** (Gemini guidance), and offer no "rewrite to get past the filter" helper.
  - Map codes such as OpenAI `moderation_blocked` and Gemini `IMAGE_SAFETY`, `IMAGE_PROHIBITED_CONTENT` and `NO_IMAGE`.
- **Keyboard shortcuts.**
  - The app already uses Ctrl N, K, L, M, B, `,` and `/`, plus Enter and Esc.
  - Suggestions:
    - Ctrl G opens Create.
    - In the dock: Enter generates (as in chat, with the cost confirmation protecting expensive runs), Shift+Enter adds a new line, Ctrl E enhances.
    - In the viewer: ←/→ move, F stars, R is Reuse all, Shift R is Reuse without seed, V varies, A animates, S saves, C copies, Del deletes, Space plays or pauses, Esc closes.
  - Add them all to the shortcuts sheet.

## 5. Progressive disclosure, made specific for harnessUI

1. **Match the existing "Fine control" pattern.** Each Advanced field shows "Automatic (company default)" until changed, has a reset button per field, and the drawer header counts changes: "Advanced · 2 changed".
2. **Build the fields from the schema.**
   - Widget mapping: enum → segmented control or select; integer or number with min/max → slider plus number box; boolean → switch; string → text; image or uri → drop zone; seed → number plus 🎲 and 🔒.
   - Field order from `x-order` or the listing order. The description becomes a `?` tooltip, as in Cherry Studio.
   - Sources: OpenRouter `/images/models` and `/videos/models`, fal OpenAPI, Replicate `openapi_schema`, and small hand-written specs for OpenAI GPT Image (size, quality, background, output format, moderation) and for Gemini or Omni if added later.
3. **Common settings form the canonical layer** (fal Sandbox approach). Switching model carries values over, snaps them to the nearest supported value, and **says what was dropped** ("Seed isn't used by Nano Banana 2 · ignored").
4. **Changed values are never hidden.** Each one shows as a removable chip in the dock, such as `seed 81723`, `negative: text, watermark` or `bg transparent`.
5. **An escape hatch for experts.**
   - "Request preview" shows the exact JSON that will be sent, with the key removed, and says which host gets the prompt and any reference images.
   - "Extra fields (JSON)" is checked against the schema.
   - Unknown fields that the server rejects are handled the way 0.0.7 already handles chat (resend without the field, then remember that).
6. **Video defaults keep the first try cheap:** a short clip, 720p, Draft where offered, and "Make full version" on the result (Krea, Luma, Midjourney Draft).

## 6. Recommended layout

Add a **Create** section to the sidebar next to Chats, Models and Settings, with an Images / Video switch at the top. Generation is always cloud in 0.1, so the Cloud mark is always shown and the dock says where the request goes.

Wireframe 1: the Create view (Images)

```
┌ ☰ Chats  ✦ Create  ▣ Models  ⚙ ────────── [ Images | Video ] ───────── ☁ Cloud · $1.84 today ┐
├─────────────┬──────────────────────────────────────────────────┬─────────────────────────────┤
│ SESSIONS    │ ── Today ──────────────────────────────────────  │ Settings     [Simple | All] │
│ + New       │ ┌ "lighthouse at dusk, 35mm film"              ┐ │ Model                       │
│ ● Lighthouse│ │ GPT Image 2 · OpenAI · 3:2 · ×4 · $0.16 · 14s│ │ [GPT Image 2 · OpenAI   ▾]  │
│   Logo ideas│ │ [ img ][ img ][ img ][ img ]                 │ │  ⚡⚡ ◆◆◆  Edit · Text · 4K   │
│   Trailer   │ │ ↻ Again  ⧉ Reuse  ⇄ Compare  ★  ⋯            │ │ Shape [1:1][3:2][2:3][16:9]…│
│             │ └──────────────────────────────────────────────┘ │ How many   [1][2][4]        │
│ ★ Starred   │ ┌ "same, in heavy snow"  Nano Banana 2 · ×2    ┐ │ Quality  ○Fast ●Std ○Best   │
│ ▤ All media │ │ [ img ][ ◌ making… ]                         │ │ Style    [None          ▾]  │
│             │ └──────────────────────────────────────────────┘ │ ▸ Advanced · 2 changed      │
│             │ ┌ ⚠ Stopped by OpenAI's safety filter (prompt)  ┐ │   Seed [81723 ] 🎲 🔒       │
│             │ │ Not charged · id req_8f…  [Edit prompt]      │ │   Background [transparent▾] │
│             │ └──────────────────────────────────────────────┘ │   Output format [Automatic▾]│
│             │                                                  │   Moderation   [Automatic▾] │
│             │                                                  │   { } Request preview       │
├─────────────┴──────────────────────────────────────────────────┴─────────────────────────────┤
│ [+ Reference] [+ Style]   Describe the picture…                                    ✨ Enhance │
│ GPT Image 2 ▾  3:2 ▾  ×4 ▾  Std ▾  · seed 81723 ✕ · bg transparent ✕                         │
│                                   ≈ $0.16 · prompt goes to api.openai.com   [ Generate  ↵ ]  │
└──────────────────────────────────────────────────────────────────────────────────────────────┘
```

Model picker (popover from the model chip): search; filter chips [Image] [Video] [Edit] [Refs] [Start/End frame] [Audio] [Text-in-image] [Transparent]. Each row reads: name · endpoint · ⚡ speed · ◆ quality · badges · "≈ $0.04 each" or "≈ $0.10/s". Recent models first; models without a working key are greyed out with "Add a key in Settings → Endpoints".

Wireframe 2: a video job card, and the viewer with metadata

```
┌ "slow drone push over the lighthouse, waves crashing" ───────────────────────────────┐
│ Veo 3.1 · OpenRouter · 8 s · 16:9 · 720p · audio on · start frame [▣]    ≈ $3.20 ●red│
│ ▓▓▓▓▓▓▓▓▓▓▓░░░░░░░░  making it · 55% · 1:12 so far · usually about 2 min            │
│ You can keep working; it will show up here and you'll get a notification.  [Cancel]  │
└──────────────────────────────────────────────────────────────────────────────────────┘
 Queue: ● 1 making  ○ 1 waiting  ✓ 3 done  ⚠ 1 stopped                         [Show all]

┌────────────────────────────────────────────────┬─────────────────────────────────────┐
│                                                │ Prompt                       [Copy] │
│                                                │ lighthouse at dusk, 35mm film       │
│              [ picture / video ]               │ ▸ Enhanced prompt (by Qwen · local) │
│                                                │ Model   GPT Image 2 · OpenAI        │
│         ◀   2 / 4   ▶          ▶❙❙ 0:03/0:08   │ Size    1536×1024 · seed 81723      │
│                                                │ Quality Std · png · bg transparent  │
│                                                │ Cost    $0.04 · 13.8 s              │
│                                                │ Made    17 Nov 2026 14:02           │
│                                                │ Saved   ~/harnessUI/media/…png      │
│                                                │ Carries the company's AI watermark  │
├────────────────────────────────────────────────┤ [Reuse all  R] [Reuse w/o seed ⇧R] │
│ ★ F  Vary ▾ V  Edit ▾  Upscale  Animate A      │ [Use as reference] [Request JSON]   │
│ Extend (video)  Save frame  Save as… S  Copy C │ [Show in folder]   [Delete]         │
└────────────────────────────────────────────────┴─────────────────────────────────────┘
```

Also: a chat picture artifact gets "Open in Create", and a Create output gets "Send to chat". Both use the same generation backend, which could later be exposed to the chat model as a tool that asks for approval.

## 7. Things to avoid

- Hidden auto-routing or silent prompt rewriting. Ideogram's Magic Prompt is on by default; if you offer anything like it, show the rewritten prompt and allow undo.
- Raw "model parameters" text boxes as the only advanced path (Msty's `size`/`n`): no types, no checking.
- Greying out unsupported controls without saying why. Hide them, and explain anything dropped when the model changes.
- Spending money without a figure next to the button, or a video run with no cost confirmation.
- Keeping only the provider's URLs, which expire, or a history that breaks when a model is retired.
- Retry loops on safety blocks.

## Sources

- Google Flow, create videos: https://support.google.com/flow/answer/16353334?hl=en ; models and features: https://support.google.com/flow/answer/16352836 ; edit and Scenebuilder: https://support.google.com/labs/answer/16935718
- Gemini app videos: https://support.google.com/gemini/answer/16126339
- Gemini API (AI Studio) video, with the Veo 3.1 Preview shutdown: https://aistudio.google.com/docs/video ; Omni Flash: https://aistudio.google.com/docs/omni.md ; Nano Banana: https://aistudio.google.com/docs/image-generation.md ; index, with Imagen shut down: https://aistudio.google.com/llms.txt
- OpenAI video guide, with the Sora 2 shutdown on 2026-09-24: https://developers.openai.com/api/docs/guides/video-generation ; GPT Image reference: https://developers.openai.com/api/reference/resources/images.md ; moderation_blocked reports: https://community.openai.com/t/api-issue-moderation-over-refusals-on-gpt-image-2-with-moderation-low-where-chatgpt-always-succeeds/1388964 ; ChatGPT Images (secondary): https://www.datacamp.com/blog/chatgpt-images , https://help.openai.com/en/articles/9055440-editing-your-images-with-chatgpt-images (403 to my fetcher)
- OpenRouter video: https://openrouter.ai/blog/announcements/video-generation/ ; video models: https://openrouter.ai/collections/video-models
- fal Sandbox: https://fal.ai/docs/documentation/model-apis/sandbox.md ; per-model OpenAPI: https://fal.ai/docs/examples/model-apis/custom-workflow-ui ; example playground: https://fal.ai/models/fal-ai/gpt-image-1.5/playground
- Replicate schemas: https://replicate.com/changelog/2023-03-20-get-model-input-and-output-schemas-via-the-api ; Playground: https://frontend.replicate.com/changelog/2024-10-10-playground-beta
- Krea image: https://www.krea.ai/docs/user-guide/features/krea-image.md ; video: https://www.krea.ai/docs/user-guide/features/video.md ; model overview: https://www.krea.ai/docs/user-guide/features/model-overview.md
- Ideogram render speed and settings: https://docs.ideogram.ai/using-ideogram/generation-settings/render-speed ; Magic Prompt: https://docs.ideogram.ai/using-ideogram/getting-started/prompting-guide/7-creative-tools-in-ideogram/using-magic-prompt
- Leonardo Flow State: https://intercom.help/leonardo-ai/en/articles/10002805-flow-state ; prompt enhance and negative prompt (secondary): https://stacksheriff.com/ai-tools/leonardo-ai-tutorial/
- Freepik models and Auto: https://www.freepik.com/ai/docs/image-ai-models ; Spaces: https://www.freepik.com/blog/introducing-spaces
- Midjourney (secondary): https://docs.midjourney.com/hc/en-us/articles/33390732264589 (403), https://zapier.com/blog/how-to-use-midjourney/
- Runway Gen-4.5 (secondary): https://help.runwayml.com/hc/en-us/articles/46974685288467 (403) ; Unlimited/Explore: https://help.runwayml.com/hc/en-us/articles/18053095835795
- Luma Ray3: https://lumalabs.ai/learning-hub/ray3-user-guide ; https://lumalabs.ai/news/ray3
- Cherry Studio drawing: https://docs.cherry-ai.com/cherry-studio/preview/drawing
- Msty: https://docs.msty.ai/studio/features/image-gen ; https://msty.ai/blog/media-studio-from-prompt-to-asset
- LM Studio plugins: https://lmstudio.ai/altra/image
- ComfyUI App Mode: https://docs.comfy.org/interface/app-mode.md ; https://blog.comfy.org/p/from-workflow-to-app-introducing
- InvokeAI gallery: https://invoke.ai/features/gallery/
- Fooocus: https://stable-diffusion-art.com/fooocus
- Gemini image safety finish reasons (third-party): https://docs.apiyi.com/en/api-capabilities/gemini-image-error-handling

## harnessUI files I read (to align the design)

- /home/user/harnessUI/src/components/ShortcutsDialog.tsx (existing shortcuts)
- /home/user/harnessUI/src/components/CloudMark.tsx
- /home/user/harnessUI/src/components/FineControl.tsx ("Automatic" pattern)
- /home/user/harnessUI/src/components/settings/endpointPresets.ts (Anthropic, OpenAI, OpenRouter, abliteration.ai)
- /home/user/harnessUI/src/components/models/Compare.tsx
- /home/user/harnessUI/CHANGELOG.md
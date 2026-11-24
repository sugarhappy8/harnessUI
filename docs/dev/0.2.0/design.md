<!-- Owner decisions recorded 2026-11-24 -->
> **The owner's words, verbatim.**
>
> - "Work on fixing the app and removing ur set restrictions due to it being very not user friendly. Go from latest build"
> - "I was mostly talking about how nothing from huggingface for video and image models is supported"
> - "Yes it's exactly that and also the studio section is a bit not attractive looking, I care about appearance so that should look nice"
> - "Idk I just don't really like the layout too much especially how you switch or download models. Start working on everything"
>
> **The owner's choices (2026-11-24)**
>
> - Studio restrictions to remove: **spending guards** and **local run blocks**. The notices and other dialogs stay.
> - Chat tool modes: **drop the extra confirms**. Chats still start in Ask first.
> - Nothing else is broken: fix what is found along the way.
> - The play-back of "any Hugging Face picture or video model the engine can run should just work in Studio, with no not-supported dead ends" was confirmed as "exactly that".
> - LoRAs were offered with the recommendation to include them. The owner replied "Yes", so they are in.

# harnessUI 0.2.0: any Hugging Face model in Studio, LoRAs, a better-looking Studio, fewer blocks

The base is release 0.1.2, `main` at e7dc878. This document is the spec that the plan, `docs/dev/0.2.0/plan.md`, argues from.

Research behind it, dated 2026-11-24 (session scratch, summarised here where it matters):
- a map of the 0.1.2 pipeline from Hugging Face search to `sd-cli`;
- what stable-diffusion.cpp `master-1106-532d2c1` can run, read from its source;
- the ungated Hugging Face sources for every companion file, with sizes and SHA-256 checked against the API, and the formats popular repos ship;
- an inventory of the app's 175 user-facing restrictions;
- screenshots and a critique of the current Studio screens.

## 1. Why

Studio 0.1.x can run only the model families written into its code: FLUX.2 klein 4B, Z-Image Turbo, two Wan 2.2 5B variants, Wan 2.1 1.3B and FLUX.3 Image Klein. It recognises them by **name**, accepts only **GGUF** files, and checks each one against five compiled tensor fingerprints.

The pinned engine, however, runs **61 model versions**, and about **two-thirds of popular Hugging Face image and video repos** (134 of 205 sampled) ship files it can run. Most popular picture models are single-file `.safetensors` checkpoints, which Studio cannot read at all. 0.1.2 made every file *downloadable*, but anything outside the families still cannot be *used*.

The owner also finds Studio unattractive, especially switching and getting models, and finds several guards needlessly obstructive.

## 2. Goals

1. **Any model the engine can run works.** Search Hugging Face, see "Works in Studio" with the model type, press one Download (the main file plus the extra files it needs), and the model appears in the model switcher and runs.
2. **Files already on the computer work too.** That includes a file downloaded elsewhere or with 0.1.2's plain file download. Add it to Studio, and the same detection and set-up apply.
3. **LoRAs.** Download one from Hugging Face or add one from disk. Attach it to a model, with a strength slider. Several can be attached at once.
4. **Studio looks good**, and switching and getting models are redesigned (section 10).
5. **The spending guards are off by default.** The daily limit, the confirmations and the $5 hold stay available as optional switches in Settings → Studio.
6. **The local run blocks become warnings.** Studio says what may go wrong, and the person can always try.
7. **Chat tool modes switch without "are you sure".** A saved start mode is not asked again at every launch.
8. **Fixes found along the way:**
   - the scheduled Reverse engineering job, which has failed every day since 2026-11-20;
   - the flaky llama-server contract test that loses the stderr tail;
   - stale docs.

## 3. Non-goals

- **Models the pinned engine cannot run.** These are Lumina 2, Neta Lumina, HiDream-I1, SVD, LTX-Video 0.9.x, HunyuanVideo v1 and SDXL AnimateDiff, plus diffusers-folder repos of any type other than SD 1.x/2.x/XL. Studio says plainly why such a model cannot run, and the file can still be downloaded, as in 0.1.2.
- **Reading pickle formats** (`.ckpt`, `.pt`, `.pth`, `.bin`) to detect what they are. They stay downloadable, and Studio says it reads `.safetensors`, `.sft` and `.gguf` files.
- **Adapters and extra pipelines**, which need UI of their own: ControlNet, IP-Adapter, PhotoMaker, PuLID, AnimateDiff motion modules, upscalers (ESRGAN, SeedVR2, RelayVSR), and audio-driven video (Wan S2V). Engine recipes that need these are not offered.
- **Changing the pinned engine build.**
- **Loosening anything the owner did not pick**: the notices, the chat approval rules other than the confirms, and the link confirmation.

## 4. Anti-goals

These would make the work a failure even if it runs:
- a model that says "Works in Studio" and then cannot start, because a required file is missing or wrong, with no way to fix it in the app;
- a download that silently pulls gigabytes the person did not see listed;
- a redesign that looks better but makes switching or getting a model take more steps than before;
- losing the models people already installed, or their settings, on upgrade;
- weakening the protections that stay: the child-safety prompt check, typed engine arguments with no shell, the prompt passed by file, checked output paths, files pinned by size and SHA-256, the API key never sent to a file host, and update signatures.

## 5. Constraints

- **Engine.** stable-diffusion.cpp commit `532d2c15c430a785a712b2d94281f04e733008a4` (`sd.lock.json`). Facts about it come from its source at that commit and are cited in the plan.
- **Recipes come from that same commit.** A test fails if `catalog/sd-recipes.json` names any other commit than `sd.lock.json`.
- **Downloads stay pinned.** Every downloaded file keeps its repo, revision, path, size and SHA-256. The SHA-256 is Hugging Face's LFS oid, and the download is verified against it. Gated repos still need the person's Hugging Face token.
- **Header reads are bounded.** A remote header read uses an HTTP Range request against the revision-pinned resolve URL. It never follows a redirect off Hugging Face's own hosts and its CDN with the token attached, matching 0.1.x's guarded fetcher. It also has hard caps:
  - a safetensors header of at most 100 MiB;
  - at most 64 MiB of GGUF metadata and tensor table;
  - at most 24 files read per repo opened.
- **Wording and layout.** All on-screen wording lives in `src/lib/words.ts`, with no raw identifiers on screen. Keyboard and screen-reader use stays as good as 0.1.x: roles, labels, focus traps and Esc.
- **Upgrading loses nothing.**
  - Models installed with 0.1.x keep working, mapped to recipes.
  - Settings, gallery and runs keep working.
  - The database migration is additive. Schema 4 can be read by nothing older, as with schema 3.
- **Windows rules stay.** Files that are in use are never replaced or removed, Unicode paths work, and LoRA names are relative to `--lora-model-dir`, because the engine's LoRA name cannot contain `:`.

## 6. Approaches considered

| | Approach | Trade-off | Verdict |
|---|---|---|---|
| A | **Engine recipes plus file-header detection.** Read each file's tensor table, decide the engine version the way the engine itself does, pick the engine's own recipe for that version, and fill its slots from a pinned table of ungated companion files. | One port of the engine's detection (about 60 signatures) and a matching table to keep in step with the pin. In return it covers everything the engine runs, gives correct defaults, and can say "works" before downloading. | **Chosen** |
| B | **Manual set-up only.** The person picks a main file and assigns files to slots (text encoder, VAE, …) themselves. | Very flexible and little code, but people would have to know which encoder and VAE each model needs. That is not "easy to use". | Kept as the fallback "Set it up yourself" (section 7.5), for files detection cannot place or to override a choice |
| C | **Add more families by hand** to the closed `GenFamily` enum. | Quick for five models, but the closed list *is* the problem: every new model would need a release. | Rejected |

## 7. Design: any model the engine can run

### 7.1 Engine recipes, vendored

- Copy the engine's 112 built-in recipes (`examples/server/recipes/*.json` at the pinned commit) into `catalog/sd-recipes.json`: `{ "sdCommit": "<commit>", "recipes": [ … ] }`, with each recipe's JSON kept as is.
  - A script, `scripts/vendor_sd_recipes.py <checkout>`, writes the file.
  - A test checks the commit against `sd.lock.json` and that every recipe parses.
- Recipe schema (version 1, as found):
  - `id`, `name`, `description`, `model_url`;
  - `slots[{id, label?, required}]` and `required_any[[…]]`;
  - `defaults` (sampling_method, scheduler, cfg_scale, steps, width, height, distilled_guidance, flow_shift, video_frames, fps, the high_noise_* fields, moe_boundary, …);
  - `features`, `disabled_features`, `size_multiple`, `video_constraints{frame_step, frame_offset, frame_rounding, min_frames}`, `context{vae_format, model_args}`.
- Slots map one to one onto flags. These are the slots Studio uses:

  | Slot | Flag |
  |---|---|
  | `model` | `-m` |
  | `diffusion_model` | `--diffusion-model` |
  | `high_noise_diffusion_model` | `--high-noise-diffusion-model` |
  | `uncond_diffusion_model` | `--uncond-diffusion-model` |
  | `clip_l` | `--clip_l` |
  | `clip_g` | `--clip_g` |
  | `t5xxl` | `--t5xxl` |
  | `llm` | `--llm` |
  | `llm_vision` | `--llm_vision` |
  | `clip_vision` | `--clip_vision` |
  | `tokenizer` | `--tokenizer` |
  | `vae` | `--vae` |
  | `tae` | `--tae` |
  | `audio_vae` | `--audio-vae` |
  | `audio_encoder` | `--audio-encoder` |
  | `embeddings_connectors` | `--embeddings-connectors` |

  The adapter slots (`control_net`, `ip_adapter`, `motion_module`, `photo_maker`, `pulid_weights`) put a recipe out of scope (section 3).
- The recipes do not say which detected engine version they serve, so harnessUI keeps a **matching table** for that: version → recipes, with name tokens that pick among several. Examples:
  - FLUX → `flux1-dev` by default, `flux1-schnell` for "schnell", `flux1-kontext` for "kontext";
  - Qwen-Image Edit → `qwen-image-edit-2511` for "2511", `qwen-image-edit-2509` for "2509", otherwise `qwen-image-edit`.

  Each recipe also gets a Studio **kind**: picture, picture edit (it takes a reference picture) or video. Video recipes run with `-M vid_gen`.

### 7.2 Reading what a file is

- **A tensor-index reader** for local files and for byte ranges:
  - safetensors: an 8-byte little-endian header length, then JSON with names, shapes, dtypes and an optional `__metadata__`;
  - sharded safetensors: a `*.safetensors.index.json` plus the headers of its shards;
  - GGUF: the existing reader, extended to 5-D tensors and to incremental reading of a remote prefix.
- **Detection is a port of the engine's own version detection**: `src/model_loader.cpp` signature checks, in their evaluation order, with the engine's name normalisation and shape checks. It returns:
  - the engine version;
  - the layout: a **single-file checkpoint**, which takes `-m` and carries its own VAE and encoders, or a **diffusion-only file**, which takes `--diffusion-model`;
  - which components the file embeds;
  - whether it is a **LoRA** (kohya `lora_up`/`lora_down`, PEFT `lora_A`/`lora_B`, LyCORIS `hada_`/`lokr_`, or `.diff`), with a best guess at the base version it targets.
- **Some versions need a second file to be told apart**: Ming-Image needs its `--llm` file and P-H3-Image its `--vae` file. They are detected once their companions are known; until then, name tokens decide.
- **Fixtures from real files.** The port is tested against tensor lists taken from real Hugging Face files, one or more per supported version, fetched once by header Range reads and trimmed to the names and shapes the checks use.

### 7.3 Before downloading

When a search result opens, Studio reads the header of each candidate weight file, up to the caps in section 5. The candidates are `.safetensors`, `.sft` and `.gguf` files of at least 10 MB that aren't obviously companions. Studio detects what each one is and caches the result by repo, revision, path and SHA-256. Each file is then labelled:
- **Works in Studio**, with the model type, for example "SDXL picture model" or "Wan 2.2 video model, 14B, high-noise part";
- **LoRA for <type>**;
- **Extra file**, such as a text encoder or VAE that recipes can use;
- **Can't run in Studio**, with the reason, for example "This is a Lumina 2 model, which the Studio engine can't run" or "Studio can't read .ckpt files".

0.1.2's plain download stays available for every file.

### 7.4 Setting a model up

1. **The recipe** comes from detection plus name tokens (section 7.1). The model's settings show it as **Model type**, and the person can change it among the recipes for that version.
2. **Slots are filled in this order:**
   1. the main file goes into `model` or `diffusion_model`, depending on its layout;
   2. for high-noise/low-noise pairs (Wan 2.2 A14B), the sibling file of the same quantisation in the repo fills the other slot;
   3. each required slot, and `vae` for `required_any [vae, tae]`, is filled from:
      - a file already installed with the same SHA-256, or a known companion already on disk;
      - a suitable file in the same repo;
      - the **companion table**: a new `companions` section in `catalog/models.json`, with id, slot, the versions it suits, repo, revision, path, size, SHA-256 and licence. It is seeded with the ungated sources verified on 2026-11-24, and for each version it names the default, preferring the smaller of equal-quality variants: fp8/Q8 text encoders over fp16 when the engine accepts them;
      - otherwise, the person picks a file, from disk or from a Hugging Face repo.
3. **A plan before download** lists every file, its role in plain words ("Text encoder (T5-XXL)", "Decoder (VAE)"), its size and licence, and whether it is already on this computer. Then it shows the total to download and one Download button. Nothing downloads that the plan doesn't list.
4. **Storage.** The `gen_models` row records the recipe id and the files per slot. Schema 4 adds `recipe_id`, with `source` gaining `import`, and migrates existing rows: each catalog family maps to its recipe.

### 7.5 Set it up yourself

For any detected model, the model's settings can open **Files**:
- the recipe's slots, each showing its file;
- *Change* for each slot, which chooses another installed file, a file from disk, or a file from a Hugging Face repo;
- the model type.

The same screen is the fallback when detection can't place a file but the person believes the engine can run it. They pick a model type and assign the files, and the run then goes ahead with an "untested set-up" note.

### 7.6 Running

The engine arguments come from typed fields, never a shell:
- the mode follows the version;
- every filled slot passes its flag;
- the recipe's defaults are the settings' starting values;
- `size_multiple` and `video_constraints` drive size and frame-count validation;
- `context` passes `--vae-format` and `--model-args` where the recipe sets them;
- the full sampler and scheduler lists of the pinned engine are offered, replacing 0.1.x's whitelist of 7 samplers and 4 schedulers;
- memory: the engine's own auto-fit spreads weights over GPU, RAM and disk. Studio does not pass `--offload-to-cpu` unless the person chooses *Use the processor*, the existing option. It adds `--vae-tiling` for video and for pictures above 1024×1024;
- LoRAs: `--lora-model-dir <Studio LoRA folder>`, with `<lora:NAME:W>` tags appended to the prompt file. The engine reads them from the file (checked at the pinned commit: `--prompt-file` fills the prompt, then `extract_and_remove_lora` runs when the LoRA folder is set).

### 7.7 Fit and time

- **Fit.** Generic models get a memory estimate from the sum of their part sizes, adjusted by recipe kind. The verdicts are the existing words, and they are warnings only (section 9).
- **Time** is shown after the first run on this computer. There is no first guess for generic models.

### 7.8 Importing a file from the computer

*Add a model file* (on Get models and in the model switcher) opens a file picker for `.safetensors`, `.sft` and `.gguf` files. The file is read in place, then copied into Studio's models folder or used where it is. The choice is offered once, and moving is never done silently. It then follows sections 7.2 and 7.4 exactly as a downloaded file does.

## 8. Design: LoRAs

- **Store.** A new schema-4 table, `gen_loras`: id, name, file (repo, revision, path, size, SHA-256 or local path), base-version hint, source and added_at. LoRA files live under one Studio LoRA folder, so that relative names work on every system.
- **Getting LoRAs.** Hugging Face results that are LoRAs show "LoRA for <type>" and download like models. *Add a LoRA file* imports one from disk.
- **Using LoRAs.** The settings panel gets a **LoRAs** section:
  - *Add* lists installed LoRAs, those matching the current model's version first and the others marked "may not fit this model";
  - each attached LoRA shows its name, a **strength** slider (−1 to 2, step 0.05, default 1) and Remove;
  - for Wan 2.2 A14B models, a LoRA also has a "high-noise part" switch, which sends `<lora:|high_noise|NAME:W>`.

  Attached LoRAs belong to the draft. They are saved in My setups and shown in a run's settings line.
- **Engine check.** The engine drops a LoRA it cannot find with a warning. Studio checks that every attached LoRA file exists before starting, and says so if one is missing.

## 9. Design: fewer blocks

### 9.1 Spending guards: off by default, optional switches (the owner's pick)

- **Settings → Studio → Spending**, every switch off by default:
  - *Daily limit*, with an amount;
  - *Ask before every video*;
  - *Ask before runs of $X or more*;
  - *Ask when the price is unknown*.
- **The backend applies exactly what these switches say.** It no longer requires confirmation on its own when they are off. The $5 hold for unknown prices applies only while a daily limit is on.
- **Upgrade.** Prefs that still hold the old defaults (limit $10, ask before video, ask at $1) move to the new defaults once, through a prefs version marker. Values the person changed are kept.
- **Kept regardless:** prices are still shown before every cloud run, and the card still says what a run cost.

### 9.2 Local run blocks: warnings (the owner's pick)

- **"Not for this computer"** no longer greys out Create, refuses the run, or hides Download or Download again. It becomes a warning line ("May not fit this computer: <reason>. It may fail or be very slow.") and the run goes ahead.
- **Local video on computers without a graphics card** (the processor, an Intel Mac) is offered, with the warning "Video on the processor is very slow".
- **The prompt length caps** of 4,000 characters for local runs and 32,000 for cloud runs go. One hidden safety bound of 1 MiB stays, against pathological input.
- **Create no longer waits on a price.** Pressing Create while extra settings wait for a model's price finishes that price request first, then sends. If the request fails, it sends without those settings and says so.
- **Installing an update while a local job runs** stops the job and installs, after a warning line in the update box ("Installing stops the picture being made"). The update is no longer refused.

The engine's real limits stay as validation, because the engine refuses anything else: size multiples, the frame rule, maximum sides and frames.

### 9.3 Chat tool modes (the owner's pick: drop the extra confirms)

- Switching a chat to Allow all or Full access, or making either the start mode for every chat, no longer asks "are you sure".
- A saved start mode is applied at launch without asking again, and Esc no longer resets it.
- Chats still start in Ask first unless the person saved another start mode. Everything else in approvals stays as it is.

## 10. Design: Studio's look, and switching and getting models

The owner's words: "not attractive looking, I care about appearance", and "I just don't really like the layout too much especially how you switch or download models".

### 10.0 What's wrong today

From the 2026-11-24 screenshots and critique of 0.1.2, light and dark at 1280×800:

- **Model, settings, prompt and Create are spread across the far-right column and the bottom edge, with the results in between.** The model switcher is a dropdown in the right panel, the corner farthest from the prompt.
- **The switcher list is cluttered:**
  - a capability badge on every row ("Edit", "Refs 16", "Transparent", "GIF");
  - most rows say "price shown when picked";
  - "Recent" repeats rows;
  - the "On this computer" chip in the top bar looks clickable but isn't.
- **Getting a model is buried and cramped.**
  - It lives under the ⋯ menu and opens in about 40% of the window, beside the settings panel and the prompt box.
  - It opens scrolled to the bottom.
  - Hugging Face search sits below the fold, and its results are bare links that only say what Studio can run once opened one by one.
  - Download, then use, takes 7 clicks: 8 the first time.
- **Download feedback stops at the row.**
  - There's no Pause or Cancel here.
  - Two size formats appear on one card ("6.2 GB" and "of 6.7 GB").
  - A finished model isn't offered for use.
- **There is no hierarchy.** About 60–70% of each screen is 12–14 px grey text, the page title is 14 px, and the stream, rail and dock share one background.
- **The pictures aren't the hero:**
  - 112 px thumbnails on text-first cards;
  - a native checkbox on every grid tile;
  - native selects, checkboxes and sliders beside custom chips;
  - a doubled 2 px focus outline on the prompt.

### 10.1 Layout

Studio keeps the app's shell: the sidebar, the tokens in `src/styles.css`, Radix primitives, `lucide-react` icons and `clsx`. No new UI library is added. It is rearranged around one idea: **the results are the page, and everything needed to make the next one sits together in one composer at the bottom.**

```
┌ Studio  [Pictures | Video]                  🔍 Search   ☆ Starred   ▦ Grid   ⬇ Get models ─┐
│                                                                                        │
│   results: the card stream (or the grid), wide, with big pictures                      │
│                                                                                        │
│ ┌──────────────────────────────── composer ─────────────────────────────────────────┐  │
│ │  [+ picture]  Describe the picture you want…                                        │  │
│ │  ◉ FLUX.2 klein 4B ▾ · 1:1 ▾ · ×2 ▾ · LoRAs (1) ▾ · ⚙ Settings     ~2 min  [Create] │  │
│ └────────────────────────────────────────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

- **The composer** is one raised card (`surface`, rounded-2xl, soft shadow), styled like the chat composer.
  - Its first row is the prompt, with the added pictures as small thumbnails in front of it.
  - Its second row is a toolbar:
    - the **model chip**: the model's name, with a dot for where it runs, on this computer or the cloud's company;
    - shape and count as compact menus;
    - **LoRAs** with a count, for local models that can take them;
    - **⚙ Settings**, which opens the side panel;
    - and, at the right, the time or price and **Create**.
  - The privacy line ("On this computer: your prompt and pictures stay here") becomes a tooltip on the model chip's dot plus one quiet line under the composer.
- **The side panel** becomes **Settings**: My setups, Advanced, LoRAs in full, and the model's files. It slides in from the right when asked for, and stays open on wide windows (at least 1400 px) if the person leaves it open. It is never needed for a basic run. Shape and count move to the composer.
- **The top bar** holds:
  - the title "Studio" at `text-lg` semibold;
  - the Pictures/Video segmented control;
  - Search, Starred and Grid;
  - **Get models**, a button with the download icon, replacing the ⋯ menu entry.

  The non-clickable "On this computer" chip goes. The day's spend shows only while a spending switch is on (section 9.1).

### 10.2 Switching models

The model chip opens the **model sheet**: a large popover anchored to the chip, about 640×560 px, or a full-height sheet on narrow windows.

- **At the top:** a search field, focused on open, and a segmented filter: *All · On this computer · Cloud*. The kind follows the Pictures/Video tab.
- **Recent:** up to four models as compact tiles across the top. They are not repeated in the lists below.
- **On this computer:** each installed model is a row card with:
  - its name;
  - a plain type label, for example "SDXL · picture", "Wan 2.2 · video" or "FLUX.1 Kontext · picture edit";
  - its size;
  - a fit dot, with the warning wording from section 9.2 on hover;
  - its LoRA count when LoRAs are attached.
- **Cloud:** models are grouped by company. Each row shows its name and either its price or "price unknown" in one muted line. The capability badges are reduced to at most two small icons ("can change a picture", "transparent background"), each with an accessible label. The "Refs 16" style counts go.
- **The footer** has two buttons, **Get more models** (primary) and **Add a model file**.
- **Behaviour:**
  - picking a model closes the sheet;
  - the arrow keys, Enter and Esc work;
  - focus returns to the chip.

  The existing rules for unusable models (no key, no list) stay. They are shown as rows with their fix as a link, not as greyed rows.

### 10.3 Getting models

**Get models** is a **full Studio view**: the composer and side panel are hidden while it's open. It always opens scrolled to the top, and Back returns to the results.

- **Header:**
  - "Get models" at `text-xl` semibold;
  - this computer as quiet chips: graphics card and memory, RAM and free disk;
  - **Add a model file** and **Add a LoRA file**.
- **Search** is a large field at the top: "Search Hugging Face: SDXL, Flux, Wan 2.2, or a repo name". Beside it:
  - filter chips: *Pictures · Video · LoRAs*;
  - a switch: *Fits this computer*.
- **Without a query:**
  - **Recommended for this computer**, from the catalog, as cards in a responsive grid (2–3 columns);
  - **Popular on Hugging Face**, the top downloads for the current kind, read when the view opens;
  - **On this computer**: installed models and LoRAs, with Delete and Repair.
- **Results** are cards in the same grid. Each card shows:
  - the repo's name and author;
  - downloads and likes;
  - when it was updated;
  - a **Studio verdict** badge. The first is a quick hint from names; once the card's files have been read (section 7.3), it becomes exact: "Works in Studio · SDXL", "LoRA for FLUX.1", "Can't run: Lumina 2".
- **Opening a result** shows a **detail panel** that slides over the right half:
  - the repo's summary;
  - the **main file**: one per quantisation or precision, each with size and fit, and **"Best for this computer"** marked on the largest that fits;
  - the **extra files Studio adds**, each marked "already on this computer" where true;
  - the licence and the total;
  - one **Download** button.

  A file Studio can't run shows its reason, with a quiet "Download file anyway".
- **Downloads** show progress on the card and in the detail panel, with **Pause** and **Cancel**, in one size format throughout. A small download indicator joins the top bar, and its popover lists the running downloads. When a model finishes, its card shows **Use it**, which selects it and returns to the results with the composer.
- **The click target:** from the results, using a new model takes at most **4 clicks**: Get models, open a result, Download, Use it. Typing the search is not a click.

### 10.4 Results and polish

- **Run cards** lead with the pictures:
  - thumbnails up to 256 px, at the picture's own shape;
  - the prompt as the title;
  - settings and model as one quiet line;
  - progress as a thin bar with one status line;
  - failures as an inline alert with the message, the next step and their actions.
- **The grid** has larger tiles (about 220 px), with actions on hover. A **Select** mode replaces the checkbox on every tile.
- **Type scale:**
  - view titles at `text-lg` or `text-xl` semibold;
  - section headings at `text-sm` semibold;
  - body text at `text-sm`;
  - meta at `text-xs` muted.

  At least half of each screen's text is at body size.
- **Surfaces:**
  - the page on `bg`;
  - cards and composer on `surface`, with `border` and a soft shadow;
  - one confident accent: the gradient on Create and on primary buttons only.

  Light mode gets a clearer separation between page and cards: a slightly darker page or a stronger card border, within the existing tokens.
- **Controls:** native selects, checkboxes and range inputs in Studio become Radix equivalents styled like the rest of the app: DropdownMenu, a Checkbox or a Switch, and a Slider. Tooltips use Radix Tooltip, already in the `radix-ui` package, instead of native `title` attributes where the text matters.
- **Focus:** one focus ring, `ring-2` in the accent at 40%, replacing the doubled outline.
- **Empty states:** an icon, one line and a primary action, for example "No models on this computer yet" with *Get models*.
- **Accessibility:** both themes are checked, and the 900×650 narrow window must not scroll sideways.

### 10.5 How the look is checked

- The screenshot harness from 2026-11-24 renders the real `App.tsx` in Chromium with Tauri IPC mocked. It is kept in the repo under `e2e/preview/`, out of the app build, so "after" screenshots of the same 26 screens can be compared with the "before" set.
- The owner is shown before/after pairs of the main view, the model sheet, Get models, a search result's detail panel and the grid.

## 11. Fixes along the way

- **The Reverse engineering scheduled job**, failing daily since 2026-11-20:
  - Linux: the sandbox check sees a non-loopback `connect` to `10.1.0.32` port 0, which is probably a route lookup by Java or Node;
  - macOS: `kuna --version` misses its deadline during "ownership preparation". That would also make Kuna unavailable on a slow Mac, so it is a real bug as well as a test failure.

  Both are to be diagnosed and fixed at the cause.
- **The llama-server contract test** `a_broken_image_file_is_classified_and_the_model_loads_without_it` sometimes sees an empty stderr tail. The supervisor reports the exit before reading llama-server's last stderr lines; drain the tail first.
- **Stale docs.**
  - ARCHITECTURE.md still describes 0.1.0's licence gate and "Not supported yet".
  - `docs/dev/0.1.0/follow-ups.md` lists the small items still open.
  - Update the docs for everything 0.2.0 changes.

## 12. Left to the builder

- The exact matching table (version → recipes, name tokens), and the companion-table defaults per version, from the research data.
- The memory-estimate factors per kind.
- How the detection port is laid out across modules.
- Visual details within section 10's decisions.

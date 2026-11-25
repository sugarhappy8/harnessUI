# Studio "before": screenshots and design critique (harnessUI 0.1.2)

Input for `docs/dev/0.2.0/design.md` §10 ("Studio's look, and switching and getting models").
The owner's words: Studio "isn't attractive", and "I just don't really like the layout too much,
especially how you switch or download models".

## Screenshot files

All in `.cache/preview/before/`.
Each is 1280×800 CSS px at deviceScaleFactor 2 (2560×1600), except where noted. Every screen
exists in both themes: `<name>-light.png` and `<name>-dark.png` (52 files).

| # | Files (`-light.png` / `-dark.png`) | What it shows |
| --- | --- | --- |
| a | `a-main-view-light.png`, `a-main-view-dark.png` | Main Studio view: card stream (a failed cloud run, a finished local run, a local run in progress), right panel, prompt dock with a typed prompt |
| b | `b-model-picker-open-light.png`, `b-model-picker-open-dark.png` | The model picker popover open |
| b2 | `b2-after-switching-model-light.png`, `b2-after-switching-model-dark.png` | After setting "How many: 2" and switching FLUX.2 klein → Nano Banana 2.1 (cloud): the "what changed" note, price line |
| c0 | `c0-studio-menu-light.png`, `c0-studio-menu-dark.png` | The top bar's ⋯ menu, where *Get local models* lives |
| c | `c-get-local-models-light.png`, `c-get-local-models-dark.png` | Get local models, scrolled to its top: hardware line, tabs, recommended list |
| c2 | `c2-get-local-models-as-opened-light.png`, `c2-get-local-models-as-opened-dark.png` | Get local models exactly as it opens: its title, Back, hardware line and tabs are scrolled out of view |
| d | `d-hf-search-results-light.png`, `d-hf-search-results-dark.png` | Hugging Face search results ("z-image") |
| d2 | `d2-hf-result-supported-light.png`, `d2-hf-result-supported-dark.png` | A result Studio can run, opened: three quantisation cards with parts and Download |
| d3 | `d3-hf-result-not-supported-light.png`, `d3-hf-result-not-supported-dark.png` | A result Studio cannot run, opened: file select + *Download file* dead end |
| e0 | `e0-local-notice-before-first-download-light.png`, `e0-local-notice-before-first-download-dark.png` | The one-time content notice that interrupts the first download |
| e | `e-download-dialog-light.png`, `e-download-dialog-dark.png` | The download confirmation for Z-Image Turbo |
| e2 | `e2-downloading-row-light.png`, `e2-downloading-row-dark.png` | The model's row after pressing Download (progress, no Pause/Cancel) |
| f | `f-advanced-panel-light.png`, `f-advanced-panel-dark.png` | Right panel with Advanced open |
| f2 | `f2-advanced-panel-full-height-light.png`, `f2-advanced-panel-full-height-dark.png` | The whole right panel (element crop, 288×1406 CSS px) |
| g | `g-gallery-grid-light.png`, `g-gallery-grid-dark.png` | Gallery grid, scrolled to its top |
| g2 | `g2-gallery-grid-as-opened-light.png`, `g2-gallery-grid-as-opened-dark.png` | Grid exactly as it opens: its filters are scrolled out of view |
| h | `h-viewer-light.png`, `h-viewer-dark.png` | The viewer (result 2 of 4) |
| i | `i-settings-studio-light.png`, `i-settings-studio-dark.png` | Settings → Studio (top) |
| i2 | `i2-settings-studio-lower-light.png`, `i2-settings-studio-lower-dark.png` | Settings → Studio (bottom: results, cloud spending, lists) |
| j | `j-main-view-narrow-900x650-light.png`, `j-main-view-narrow-900x650-dark.png` | Main view in a 900×650 window (1800×1300 px) |
| k | `k-video-tab-light.png`, `k-video-tab-dark.png` | Video tab: chips with times, start-frame drop zone |
| z1 | `z1-first-use-empty-light.png`, `z1-first-use-empty-dark.png` | First use: nothing installed, no endpoint, no runs |
| z2 | `z2-first-use-model-picker-light.png`, `z2-first-use-model-picker-dark.png` | First use: the model picker |
| x1 | `x1-chat-screen-for-comparison-light.png`, `x1-chat-screen-for-comparison-dark.png` | The chat screen, for comparison |
| x2 | `x2-chat-model-menu-for-comparison-light.png`, `x2-chat-model-menu-for-comparison-dark.png` | The chat's model menu, for comparison |
| x3 | `x3-models-dialog-for-comparison-light.png`, `x3-models-dialog-for-comparison-dark.png` | The Models dialog (chat models), for comparison |

How they were made: the real `src/App.tsx` (sidebar, Studio, Settings, Models dialog) rendered
by Vite in Chromium, with Tauri's IPC mocked through `@tauri-apps/api/mocks` and fed from the
repo's own Studio fixtures (`src/components/studio/fixtures/models.ts`) plus mock records: an
OpenRouter endpoint with 8 picture and 3 video models, FLUX.2 klein 4B and Wan 2.2 5B installed,
Z-Image Turbo offered, seven gallery runs with generated sample pictures, one local run in
progress, six Hugging Face hits. Every screen was reached by clicks and typing, not by setting
state. The harness is in
`/tmp/claude-0/-home-user/f8f6f1ef-33e4-57b5-aed7-1e790c206f77/scratchpad/studio-preview-harness/`
(see its README) for the "after" set. Caveats: the app's font stack (Segoe UI Variable / SF)
renders as Inter on this Linux box; the cloud catalog is a plausible mock, not a live list;
the source shown is 0.1.2's frontend (the two commits since, design doc and Cargo deps, do not
touch `src/`).

---

## 1. Verdict in one paragraph

Studio is a careful, honest, very complete *form* that was never given a *design*. Every
fact is there (where the prompt goes, what it costs, how long it takes, what licence applies),
but almost all of it is set in 12–14 px grey text of equal weight, inside identical bordered
boxes, in a three-zone layout (stream · settings rail · full-width dock) that is borrowed
from a settings page rather than from a creative tool. The pictures, the one thing people come
for, are 112 px thumbnails at the bottom of text-heavy cards. And the two jobs the owner named
are the weakest: **switching models** happens in a small dropdown in the right rail that opens
a long, badge-cluttered popover, and **getting models** is hidden in a ⋯ overflow menu, opens as
a cramped page squeezed between the settings rail and the prompt dock, puts Hugging Face search
below the fold, and gives no way to pause, cancel or use a model without leaving that page.

## 2. The top problems, ranked

1. **Getting models is buried and cramped.** *Get local models* is an item in the ⋯ menu
   (c0) or a small link that appears inside the picker only while more models fit (b). It opens
   in the middle column only, with the right panel and the prompt dock still on screen
   ("Write a prompt first", Create, Shape chips), so the catalogue gets about 736×570 CSS px of
   a 1280×800 window, roughly 40% of it (c). Two catalogue cards fit per screen.
2. **It opens scrolled to the bottom.** The middle column keeps the card stream's scroll
   position, so Get local models opens with its title, *Back*, hardware line and Pictures/Video
   tabs out of view (c2), and the grid opens with its filters out of view (g2). Cause:
   `StudioView.tsx` only resets `scrollTop` for the cards (`useLayoutEffect … if (el && cards &&
   loaded) el.scrollTop = el.scrollHeight`); nothing resets it for `localView` or `grid`.
3. **Hugging Face search is an afterthought.** It sits under the recommended list (d). Results
   are bare links with "48.2k downloads · labelled apache-2.0": no size, no type, no picture,
   and **no sign of which ones Studio can run** until each is opened one at a time (opening one
   closes the other, so two cannot be compared). An unsupported repo (d3) still gets a gradient
   primary *Download file* button for weights Studio cannot use.
4. **The model switcher is in the wrong place and too heavy.** It is a form field at the top
   of the right rail, the far corner from the prompt box (about 675 px above Create). The popover (b) is a 416 px
   list over the stream, where most rows are two lines because capability badges ("Edit",
   "Refs 16", "Transparent", "GIF") push the price onto a second line; six rows say "price shown
   when picked", the *Recent* group repeats rows from the provider group, and the most visible
   place indicator, the top bar's "On this computer" chip, is a plain `<span>` that cannot be
   clicked (`PlaceMark.tsx`).
5. **No visual hierarchy.** In `src/components/studio` there are 108 uses of `text-xs`, 71 of
   `text-sm`, 5 of `text-base` and nothing larger; the screen title "Studio" is 14 px semibold.
   Prompts, settings lines, statuses, fit words, licence chips and hints all read at the same
   level. The only strong colour is the small gradient Create button, which competes with the
   sidebar's large gradient *New chat* button, irrelevant while in Studio (a).
6. **Results are not the hero.** Run cards are text first: quoted prompt, settings line,
   status line, error lines, then 112 px tiles left-aligned with empty space beside them, then a
   button row (a). The gallery grid is better but carries a native checkbox on every tile (g),
   and both grid and cards still share the screen with the settings rail and the dock.
7. **Download feedback ends at the row.** After Download (e2) the row shows "2.4 GB of 6.7 GB ·
   downloading · 2 min left", with no Pause or Cancel (those are only in Models → Downloads, and
   nothing says so), and the same card says "6.2 GB" a line above (two formatters: `amountGb`
   binary vs `formatBytes` decimal). When it finishes the model is not selected, and if you have
   pressed *Back* meanwhile, Studio does not re-read its local models (only the open Get local
   models view follows downloads, `LocalModels.tsx` → `followDownloads`), so the picker may not
   list it until Studio is shown again.
8. **Unfinished-looking controls.** Native `<select>` (My setups, Sampler, grid's *Any time*),
   native checkboxes (Keep it, Only what fits, grid tiles, viewer's .json option), a native
   range slider with an empty number box (Steps), Chromium's blue search-clear ×, a hard 2 px
   focus outline around the prompt box (the global `:focus-visible` rule in `styles.css` is
   unlayered, so it beats Tailwind's layered `outline-none`; the chat composer has a special
   case, Studio does not), help that exists only as native `title` tooltips (30 `title=` in
   Studio), and model notes (SynthID watermark, zero data retention) visible only on hover.

---

## 3. The flows the owner named

Click counts are from the main view with the mock data above, counting presses only (typing
and scrolling noted separately).

### 3.1 Switching models

| Task | Presses | Notes |
| --- | --- | --- |
| Pick another installed or cloud picture model | **2** (Model field → row) | Rows below the fold need a scroll inside the popover (K3 Edit is below 800 px in b). |
| Compare two OpenRouter picture prices | **4+** | Prices are hidden until a model is picked ("price shown when picked"); pick, read the line under the field, reopen, pick again. |
| Switch Pictures ↔ Video | 1 | The tab silently swaps the model too (each tab keeps its own); the top bar chip and panel change wholesale (k). |
| Apply a saved setup | 2 | A *second* model switcher: the native "My setups" select in the panel (f). |
| Open the picker from the keyboard | not possible | No shortcut; Ctrl+M opens the chat's Models dialog instead. |

What is confusing or hidden:

- **Location.** The chat switches models from the header's top-left (x2), right where the
  eye starts. Studio puts it in a 288 px rail on the right, styled as a form dropdown with a label
  "Model" above it and two grey lines under it. Nothing in the dock (where you type and press
  Create) says which model will run except "On this computer: your prompt and pictures stay here".
- **Noise per row.** Badges are bordered micro-chips in the same grey as the price, the
  same weight as the name. "Refs 16" means "accepts up to 16 reference pictures"; the filter chips
  (Edit · References · Transparent) use the same jargon. Rows have three heights in one list (b).
- **Groups.** "On this computer" uses the place chip as its heading, "Recent" is a bold label,
  "via OpenRouter" is a bold label with a "Cloud" chip on the far right, families are separated by
  hairlines with no names. Four different heading treatments in one popover.
- **What is not there.** No preview images, no speed or quality cue for cloud models, no
  "installed size" for local ones, no way to download from the picker (only "+ Get local models
  (1 more fit this computer)", which leaves the picker for another page).
- **After switching (b2).** The carry-over note ("Nano Banana 2.1 has no how many setting, so it
  was removed") is a good idea, set as a grey box with a tiny ×; the field list below it changes
  completely (chips for 12 shapes collapse behind a "+⌄" chip), and the dock's line changes from
  "about 2 min" to "~ $0.04". The switch works, but nothing animates or highlights what changed.
- **First use (z2).** The picker opens on "No local models yet. FLUX.2 klein 4B fits this
  computer (4.9 GB)" and a *Get local models* button. It names the model but cannot download it.

### 3.2 Finding and downloading models

| Task | Presses | Notes |
| --- | --- | --- |
| Download a recommended model and use it | **7** (⋯ → Get local models → Download → Download (dialog) → *wait* → Back → Model field → row) | **8** the first time (the content notice, e0). The model is not selected when it finishes; the view must stay open to see progress. |
| Same, first use with nothing installed | 5 (Get local models → Download → I understand → Download → Back) | The tab gets the new model automatically only because it had none. |
| Find a model on Hugging Face and use it | **9** + typing + scrolling (⋯ → Get local models → scroll → type, Enter → open repo → pick a quant → Download → Download → Back → Model field → row) | Each unsupported repo opened is 1–2 wasted presses and a network read; nothing in the list tells them apart. |
| Pause or cancel a Studio model download | 3, **outside Studio** (sidebar Models → Downloads → Pause/Cancel) | The Studio row has no controls and no hint of where they are. |
| Delete a local model | 4 (⋯ → Get local models → Delete → confirm) | Video models are under Get local models' own Video tab, which is separate from Studio's. |
| Update cloud model lists | 2 (⋯ → Update model lists) | Also in Settings → Studio → Update now. |

What is confusing, hidden or unfinished:

- **Three model managers.** Picture/video models: Studio's Get local models (c). Chat
  models: the Models dialog (x3), whose foot says "Picture and video models: 2 (14 GB) · Manage
  in Studio". Studio downloads' Pause/Cancel: Models → Downloads. Settings → Studio repeats the
  count and a *Get local models* button (i). Each looks different (gradient Download vs solid
  accent Use, rounded-lg vs rounded-md, binary vs decimal GB).
- **The page is a column, not a place.** Get local models replaces only the stream; the top
  bar drops its tabs and search but keeps the place chip and spending; the page brings its own
  title, *Back* button and its own Pictures/Video tabs, which are independent of Studio's tabs.
  The right rail keeps showing the current model's Shape/How many/Advanced, and the dock keeps
  "Write a prompt first." The page has two jobs on screen at once.
- **Catalogue cards (c)** are all text: name, one-line summary, publisher right-aligned, size,
  licence chip, fit dot + word, time line, buttons. There is no sample picture, no "what it is
  good at" beyond one sentence, and no hierarchy between the installed model (FLUX.2 klein, with
  *Download again* and *Delete*) and the one to get (Z-Image Turbo, the only gradient button).
  "Fits" next to an **amber** dot (Z-Image: verdict yellow, word "fits") reads as a contradiction.
  "Recommended for this computer" is a small accent text at the card's top right.
- **Hugging Face search (d, d2, d3).** The box warns "Searching sends your words to
  huggingface.co." in a muted line under it; results are a plain list; an opened result indents
  under a 1 px tree line and shows one card per quantisation (Q4_K, Q5_K_M, Q8_0) each with the
  full parts list, a fit line, "Time shown after your first run here" and its own Download, with
  nothing marking the sensible default. A LoRA repo appears among models with nothing to say
  what it is. The unsupported repo (d3) says "Individual files can be used in a compatible app.
  Studio has no local generation setup for this repository" under a select of 12–13 GB files.
- **Download dialog (e)** is the best part of the flow: plain-language part names ("makes the
  picture", "reads your prompt", "picture decoder"), sizes, licences, "Pictures you make: free for
  any use, including commercial." Visually it is a text block with right-aligned columns and an
  underlined link; it would carry a picture of the model and a clearer total well.
- **Content notice (e0)** appears in place of the dialog on the first download, so the first
  download is notice → dialog → download: correct to keep (the owner chose to keep notices), but
  it should not look like a blocking error; it is a dense paragraph with a gradient *I understand*.
- **After the download.** No toast, no "Use it now" button on the row, no selection of the new
  model, no progress anywhere in Studio once you leave the page (the sidebar's Studio badge counts
  runs, not downloads).

---

## 4. Screen by screen

**a. Main view.** Three zones: card stream (max-width 768 px), 288 px settings rail, and a
full-width dock under both. The dock's textarea is ~990 px wide and two rows tall, the Create
button sits on a separate row below it, far right, next to "about 2 min"; "+ Add picture" is a
dashed chip above the textarea. The stream is bottom-anchored like chat, so with few runs there
is a blank band at the top (k). Cards: quoted prompt (14 px), settings line (12 px grey:
"FLUX.2 klein 4B · on this computer · 1024×1024 · ×2 · seed 4412 · free"), "took 3 min 51 s" in
14 px at the right, tiles, then *Again* / *Reuse* bordered buttons. The failed card stacks a bold
message, a grey next step, a charge line and three button styles (*Again*, *Reuse*, underlined
*Details*). The progress card is the clearest element on screen (gradient bar, "Picture 1 of 2 ·
step 3 of 4 · 0:23 so far · about 0:34 left", *Stop*). "1 making" floats under the last card in
12 px. The top bar crowds 9 controls into one row: chat-list toggle, title, Pictures|Video,
Search, Starred, Grid, place chip, spending, ⋯. Search/Starred/Grid stay even with zero runs (z1).

**b. Picker.** See §3.1. Also: its search box gets the hard focus ring inside a bordered label
(a double border), and the popover covers the stream rather than the rail it belongs to.

**c, c2. Get local models.** See §3.2. The hardware line ("This computer: Radeon RX 7600, 8 GB
graphics memory · 32 GB memory · 412 GB free") is useful and should be more prominent than a grey
sentence. "Only what fits" is a native checkbox at the far right of the tab row.

**d–d3. Hugging Face.** See §3.2.

**e, e0, e2. Download.** See §3.2. In e2 the row's progress bar is a thin accent bar (no
gradient, unlike the run card's), and the row's size and the progress line disagree (6.2 vs 6.7 GB).

**f, f2. Settings rail.** Six control types in one column: chip radios (Shape, How many,
Scheduler, Memory saving, Run on), native select (My setups, Sampler), text box + dice icon +
native checkbox (Seed), slider + empty number box (Steps), Width × Height boxes (Exact size), and a
bordered button (*See the exact command*). Every field carries a grey "Automatic" at its right,
which reads like a value but marks "unset" (and Memory saving shows "Automatic" twice: as a chip
and as the label). "Save as setup…" is a grey text button that looks disabled. Advanced is a
chevron + 16 px label after a hairline: fine, but it opens a column longer than the window (f2
is 1406 px tall) inside a 288 px rail. Sampler uses a select because it has 7 values while
Scheduler (4 values) uses chips, so similar settings look different.

**g, g2. Grid.** Tiles are square, 168 px, with a native checkbox always visible top-left and a
star on starred ones; video tiles get a "0:08" pill. The grid has its own All/Pictures/Video
segmented control and a native *Any time* select, while the top bar hides Pictures|Video and turns
the Grid toggle accent-coloured, a weak mode signal. No prompt, model or date on hover. The rail and
the dock stay, so the grid gets about 60% of the window.

**h. Viewer.** The most finished screen: dark matte, large picture, details in a right panel
with label/value rows. Weak spots: actions are split between a text-button bar at the bottom
(Star, Again, Edit, Animate, Use as reference, Show in folder, Delete) and bordered buttons in the
panel (Reuse all, Reuse without seed, Send to chat, Save as…, Copy), plus a native checkbox
("Also save the settings as a .json file") and *See the exact request*. Prev/next are small
chevrons around "2 / 4" with no filmstrip. The file path wraps mid-word in monospace.

**i, i2. Settings → Studio.** Consistent with the rest of Settings (plain sections, native radios
and checkboxes, bordered buttons). It duplicates *Get local models* and *Update now*. Fine as is;
it should simply follow whatever control style the redesign adopts.

**j. 900×650.** The sidebar keeps 256 px; the top bar wraps into two rows (place chip, spending
and ⋯ drop to a second line); the stream column is ~400 px, so one and a half cards show and the
status line wraps; the rail shrinks to 240 px; header plus dock take ~45% of the height. Usable,
but it shows that the three-zone layout has no small-window mode (no collapsing rail, no compact
dock).

**k. Video tab.** Resolution and Length chips each carry a time ("480p · about 5 min 30 s"):
informative but long, so Length becomes a vertical stack of five full-width chips. The start-frame
drop zone is a dashed box in the rail, separate from "+ Add picture" in the dock: two places to add
pictures. A video run costing $1.60 shows its price in amber.

**z1, z2. First use.** A single card at the top of an empty canvas: "Make pictures and videos on
this computer: free, and nothing is sent." + "FLUX.2 klein 4B fits this computer (4.9 GB)." +
gradient *Get local models*, and a secondary line for cloud. The rail shows only "Choose a model";
the dock shows a textarea with no Create. The empty state knows exactly which model to offer but
makes you go to a list to get it.

**x1–x3. The rest of the app.** The chat (x1) is calmer and more "designed": a centred reading
column, 15 px message text, a floating composer card with a soft accent glow on focus and the send
button inside it, a header that holds the model menu at top-left. The chat's model menu (x2) is a
compact dropdown: "On this computer", "Endpoints", then *Open a GGUF file…* and *Find and manage
models…*. The Models dialog (x3) uses a modal with tabs, metadata chips, solid accent *Use*
buttons and decimal GB. Studio shares the tokens but none of these patterns.

---

## 5. Visual design

**Hierarchy.** Flat. Type is 12/14 px only (16 px for two titles); weight changes are the only
emphasis, and grey (`--fg-muted`) is used for most secondary text, so 60–70% of each screen is grey
12 px. Nothing marks the primary area: the stream, the rail and the dock have the same background
and are separated by 1 px borders.

**Layout.** Stream + rail + dock means the three things that belong together (model, settings,
prompt, Create) are spread across the screen's right edge and bottom edge, and the results sit in
between. The dock spanning under the rail visually amputates the rail. Every other Studio view
(Get local models, Grid) is forced into the stream's column.

**Spacing and density.** Cards use `p-3.5`, rail `p-3 gap-4`, dock `px-4 py-3 gap-2`; groups are
separated by spacing alone except one hairline before Advanced. Text density is high (every card
line is a "·"-joined sentence) while image density is low (112 px tiles). The picker and catalogue
are sparse (10 rows, 2 cards per screen).

**Colour.** Neutral greys plus the indigo→violet gradient (`--accent` → `--accent-2`) for primary
buttons and the run progress bar; status dots in green/amber/red; accent text for links and
"Recommended". In light mode the page (`#fbfbfd`) and cards (`#ffffff`) barely separate, so the UI
looks washed out; dark mode (`#0e0f13` page, `#191b21` surface) separates better and is the more
attractive of the two. The neutral chrome is right for a picture tool; what is missing is surface
layering and one confident accent area.

**Consistency.** Buttons: across the app there are 16 different `btn-primary` class strings, 25
solid `bg-accent` buttons, and a dozen bordered-button variants mixing `rounded-md`/`rounded-lg`
and `py-1`/`py-1.5`; Studio's `CARD_BUTTON` is rounded-lg, Settings' buttons are rounded-md, the
Models dialog's *Use* is solid accent, Studio's Download is a gradient. Segmented controls: the
Radix Tabs pill (KindTabs) in the top bar and in Get local models, an `aria-pressed` look-alike in
the grid, and text toggles (Starred, Grid) that turn accent. Studio's own class constants are split
between `studio/styles.ts` and local copies (`SECONDARY` is defined twice, in PromptDock and
EmptyStates; `CARD` in RunCard and EmptyStates; `CHIP` in ModelPicker and ParamField).

**Dark theme.** Renders correctly everywhere; native controls follow `color-scheme: dark`
(white-ish checkboxes on tiles in g-dark stand out more than in light). The disabled Create button
(bg-muted on bg) nearly disappears in dark.

---

## 6. Defects found along the way

| Defect | Where | Cause |
| --- | --- | --- |
| Get local models and Grid open scrolled to their bottom; headers hidden | c2, g2 | `StudioView.tsx` resets `main.scrollTop` only for the cards view |
| Hard 2 px focus outline on Studio's prompt and search boxes | a, b, d | `styles.css` `:focus-visible { outline … }` is unlayered and outranks Tailwind's `outline-none` (in `@layer utilities`); only `.composer textarea` is exempted |
| Same model shows 6.2 GB and 6.7 GB on one card | e2 | Row uses `amountGb` (GiB labelled GB), progress uses `formatBytes` (decimal) |
| "Fits" with an amber dot | c, d2 | Fit word "fits" with verdict "yellow" |
| A model finished after leaving Get local models is not re-read by Studio | code | Only `LocalModels.tsx` follows `download-progress`; the chat side does (`refreshModelsAfterDownloads`) |
| Top-bar place chip looks interactive but is not | a | `PlaceMark` is a `<span>` |
| ⋯ button keeps a heavy focus ring after its menu closes | c, c2 | Focus returns to the trigger and `:focus-visible` shows |
| "Write a prompt first." shown permanently under an empty prompt | a, c | Prompt check rendered before any typing (muted, but always there) |

---

## 7. Design tokens and components already in the app

Stay inside these and the redesign will match the chat and Settings.

**Stack.** Tailwind CSS 4 via `@tailwindcss/vite` (no `tailwind.config.*`; tokens are CSS
variables in `src/styles.css`, exposed with `@theme inline`). Radix primitives through the
`radix-ui` umbrella package: AlertDialog (14 imports), DropdownMenu (9), Dialog (6), Tabs (5),
Popover (2), RadioGroup (1). **No shadcn/ui**, no `class-variance-authority`, no `tailwind-merge`;
`clsx` is installed; class strings are hand-written per component. Icons: `lucide-react`
(14–16 px in toolbars, 12 px in chips). Tooltips: native `title` only (no Radix Tooltip yet).

**Colour tokens** (`:root` light / dark under `prefers-color-scheme` and `[data-theme="dark"]`;
Tailwind names `bg-bg`, `text-fg-muted`, `border-border-strong`, …):

| Token | Light | Dark | Use |
| --- | --- | --- | --- |
| `--bg` | `#fbfbfd` | `#0e0f13` | page |
| `--bg-subtle` | `#f3f3f7` | `#14151a` | sidebar, quiet panels |
| `--bg-muted` | `#e9e9f0` | `#20222a` | hover, chips, segmented track |
| `--surface` | `#ffffff` | `#191b21` | cards, menus, dialogs, composer |
| `--fg` | `#17181c` | `#ecedf2` | text |
| `--fg-muted` | `#5d5f6b` | `#9a9cab` | secondary text |
| `--border` | `#e3e3ea` | `#272932` | hairlines |
| `--border-strong` | `#cdced8` | `#383b49` | hover/focus borders, scrollbars |
| `--accent` | `#4f56d9` | `#8b93ff` | links, selection, gradient start |
| `--accent-2` | `#9a4fe0` | `#c084fc` | gradient end |
| `--accent-fg` | `#ffffff` | `#0b0d1f` | text on accent |
| `--danger` | `#c0262d` | `#ff6b70` | |
| `--warning` | `#a15c00` | `#f0b45a` | amber prices, cautions |
| `--success` | `#1c7c3c` | `#5fd38a` | fit dots, licence dots |
| `--focus` | `#4f56d9` | `#8b93ff` | focus outline |

Other variables: `--shadow-color` (`230 25% 20%` / `230 40% 2%`), `--glow` (0.06 / 0.1),
`--shadow-sm|md|lg` → Tailwind `shadow-soft|float|pop`, `--ease-out: cubic-bezier(0.22,1,0.36,1)`,
`--ease-spring: cubic-bezier(0.34,1.4,0.64,1)`. Fonts: `--font-sans` "Segoe UI Variable Text",
"Segoe UI", system-ui, -apple-system, Roboto, "Helvetica Neue", sans-serif; `--font-mono` "Cascadia
Code", "Cascadia Mono", ui-monospace, "SF Mono", Menlo, Consolas, monospace. Body 14 px / 1.5;
`.chat-text` 15 px / 1.65.

**Shared classes in `styles.css`.** `.brand-gradient`, `.brand-text`, `.btn-primary` (gradient,
accent glow, brighten on hover, muted when disabled), `.app-backdrop` (faint accent glow behind the
chat), `.glass` (blurred translucent bar; Studio's top bar uses it), `.card-interactive` (lift and
accent border on hover; not used in Studio), `.composer` (floating card with focus glow),
`.dialog-overlay`, `.enter` / `.enter-fade`, `.progress-indeterminate`, `.pulse`, `.typing-dots`,
`.message-row`. Keyframes: fade-in/out, rise-in, pop-in, dialog-in/out, dot-bounce,
progress-slide, glow-pulse. Global: 140 ms colour transitions, `button:active { scale(.97) }`,
reduced-motion switch-off, thin themed scrollbars.

**Radii and sizes in use.** `rounded-md` 6 px (inputs, small buttons), `rounded-lg` 8 px
(buttons, menus items, tiles), `rounded-xl` 12 px (cards, menus, popovers), `rounded-2xl` 16 px
(dialogs), `rounded-full` (chips, badges, place marks). Type `text-xs` 12, `text-sm` 14,
`text-base` 16, `text-[11px]` in the sidebar.

**Reusable components.** `PlaceMark` (icon + words chip: "On this computer", "Cloud ·
openrouter.ai"), `LocalMark`, `FitDot` (`FitBadge.tsx`, colour dot + word), `LicenceBadge`
(`LocalModelRow.tsx`), `ErrorBanner`, `ConfirmDialog` (`studio/ConfirmDialog.tsx`), `KindTabs`
(Radix Tabs segmented pill), `BrandMark`, `visiblePath` (safe names), `DownloadProgress`,
`ResultTile`. Studio class constants in `studio/styles.ts`: `BOX` (inputs), `SMALL_BUTTON`,
`CARD_BUTTON`, `PRIMARY_BUTTON`; plus local `CARD`/`ROW` (`rounded-xl border bg-surface p-3.5
shadow-soft`), `CHIP` (`rounded-full border px-2 py-0.5 text-xs`, `aria-pressed:` / `aria-checked:`
accent), `TOGGLE`, `TAB_CLASS`, `MENU_ITEM` (`rounded-lg px-2.5 py-1.5 data-[highlighted]:bg-bg-muted`).

---

## 8. What this suggests for §10 (direction, not a spec)

1. **One creation surface.** Put model, the few settings that matter, the prompt and Create
   together in one composer (the chat's floating `.composer` card is the model), and give the rest
   of the screen to results. Advanced settings become a popover or sheet from that composer, not a
   permanent 288 px rail.
2. **The model chip is the switcher.** A prominent chip in the composer (and/or the top bar),
   showing name, place (local/cloud) and the cost or time; it opens a picker built like a
   command palette: search first, local and recent at the top, a clear row per model (name, one
   capability line, price or time on the right, one line high), installed size for local models,
   and a "Get more models…" row at the end that opens the model library in place.
3. **A real model library.** Full-width (or a large modal like Models), no rail or dock behind
   it, opening at its top. Cards with a sample image, what it is good at, size, fit, licence, and
   one primary action that turns into progress with Pause/Cancel and then **Use**. Hugging Face
   search at the top of the library with results that say up front "Works in Studio · FLUX.1 ·
   6.8 GB" or "Not runnable here", a pre-selected sensible quantisation, and the other files behind
   "Other files".
4. **Results as the hero.** Larger tiles (or a masonry grid) in the stream, the prompt and model as
   a caption, actions on hover, one click to the viewer; the grid as the same surface with filters,
   not a separate mode squeezed beside the rail.
5. **Typographic scale and surfaces.** Introduce at least a 20–24 px page title and a 16 px
   section level, reserve 12 px grey for true metadata, and use `--surface` vs `--bg` (and
   `--bg-subtle` for the composer area) to separate zones instead of hairlines.
6. **Replace native controls** with styled equivalents (Radix Select/Checkbox/Slider are available
   in the same `radix-ui` package), fix the focus rule (put the global `:focus-visible` in a layer,
   or exempt Studio's inputs as the composer is), use one size formatter, and reset scroll when the
   middle view changes.

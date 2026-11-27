# Studio preview harness

Renders the **real** harnessUI app (`src/App.tsx`, with the sidebar, Studio, Settings, the
Models dialog) in a plain browser, with Tauri's IPC mocked by `@tauri-apps/api/mocks`
(`mockWindows` + `mockIPC` with `shouldMockEvents`), and takes screenshots of every Studio
screen with Playwright in the light and dark themes. It exists so that each change to Studio's
look can be shown as a before/after pair (design 10.5).

Nothing in `src/` is changed or mocked at module level: `src/lib/ipc.ts` and
`src/components/studio/api.ts` call `invoke()` as in the app, and `mock/ipc.ts` answers.

The harness is not part of the app: `tsconfig.json` (`include: ["src", "vite.config.ts"]`), the
app's `index.html` and `vite build` never reach it, and vitest finds no tests in it. PNGs are
never committed: they go to `.cache/preview/` (ignored).

## Files

| File | What it is |
| --- | --- |
| `vite.config.ts` | Vite with the app's plugins (React, Tailwind 4), **root = the repo**, port 5199 (`PREVIEW_PORT`). |
| `index.html`, `main.tsx` | The page `/e2e/preview/index.html`: the app's own `main.tsx` with the mocks installed first. `?view=studio` opens Studio. |
| `mock/ipc.ts` | The mocked backend: one `case` per Tauri command. Unknown commands answer `null`, are logged once (`[preview] unhandled command ...`) and collected in `window.__preview.unhandled`. |
| `mock/data.ts` | The records, built from `src/components/studio/fixtures/models.ts`: its cloud models, installed models (a generic SDXL model, an untested one, one that needs set-up) and a LoRA; plus an SDXL inpainting model, a second and a third LoRA (one of an unknown model type), Hugging Face hits and repositories (a supported one, an OpenRAIL one, a Lumina 2 one that cannot run, a Wan 2.2 A14B one with 15 quantisations, a LoRA one), set-up plans, finished runs, one local run in progress, chats and a loaded chat model. |
| `mock/pictures.ts` | Sample pictures, drawn on a canvas per output id (PNG full size, JPEG thumbnails). |
| `shoot.mjs` | The Playwright script: each screen from a fresh page, driven by clicks and typing; measures each screen. |
| `pairs.mjs` | One HTML page with each screen's before and after, side by side, in both themes: first the five pairs design 10.5 names for the owner (main view `a`, model sheet `b`, Get models `c`, a result's detail panel `d2`, grid `g`), then every screen. |

Query flags: `?view=studio` (open Studio), `?empty=1` (first use: nothing installed, no
endpoint, no runs), `&fits=0` with it (nothing in the catalog fits this computer either),
`&pictures=1` with it (the picture models stay installed; no video model is installed or fits),
`?cantopen=1` opens Models (`window.dispatchEvent(new CustomEvent("app:open-models", { detail: "browse" }))`, or `installed`, `compare`) on the owner's Inkling 2 repository with its real file list and a made-up kind this version does not know (`mock/chatModels.ts`): a search card with the hint, the repository's page, an Installed row and a Compare column for a model that cannot open yet, beside one that opens, `?notice=0` (the local-model notice never accepted, so the first download shows it).
`?real=1` answers Get models with real backend answers captured from Hugging Face
(`src/components/studio/fixtures/real/`: the search for "qwen image uncensored" and the
repositories saved there), and `&crash=detail` with it takes the parts out of their plans, so a
model's page throws while it is drawn and shows its error notice. `?crash=root` draws a part that
throws at once, to show the whole window's error notice (0.2.1). The page installs the app's
Trusted Types policy as `main.tsx` does; serve the built page with the CSP of
`src-tauri/tauri.conf.json` as a header to check a screen under the app's real rules (the dev
server sends none). `window.__preview` exposes `useStudio`, `useApp`, `state` (the mock's
data) and `emit(event, payload)` to send a backend event such as `studio-progress`.

## Run it

From the repo, with `node_modules` installed:

```bash
npm run preview:serve &                       # http://127.0.0.1:5199/e2e/preview/index.html
npm run preview:shoot                         # -> .cache/preview/after/ (one PNG per screen and theme) and a table
npm run preview:pairs -- .cache/preview/before .cache/preview/after   # -> .cache/preview/pairs.html
pkill -f "e2e/preview/vite.config.ts"
```

`shoot.mjs [--only ids] [--themes light,dark] [--scale 2] [--out DIR] [--base URL] [--strict]`.
The default output is `.cache/preview/after`, at 1280x800 and scale 2.

After each screen the script records the **commands the mock did not answer**, whether the page
scrolls **sideways** (`document.scrollingElement.scrollWidth > innerWidth`), and the **body
share**: the share of visible text characters set at 14 px or more. It prints them as a table.
With `--strict` it exits 1 on an unanswered command, any sideways scroll, or a body share under
0.5 on a Studio screen (every screen but `i`, `i2` and `x1`-`x3`): design 10.4, "at least half
of each screen's text is at body size" and "the 900x650 narrow window must not scroll sideways".

Playwright is not a dependency: `shoot.mjs` loads a global install with its Chromium, or the
path in `PLAYWRIGHT_PATH`. No browser download is needed where Playwright's Chromium is
preinstalled.

To look at a screen by hand, open `http://127.0.0.1:5199/e2e/preview/index.html?view=studio`.

## The before set

The "before" screenshots are 0.1.2 itself: 52 PNGs (26 screens, both themes), captured on
2026-11-24 and described in `docs/dev/0.2.0/studio-critique.md`. They are not committed;
`pairs.mjs` reads them from `.cache/preview/before/`. They are regenerated by running the 0.1.2
harness, committed at `docs/dev/0.2.0/before-harness/` (its mock answers 0.1.2's commands, not
this folder's), in a worktree of `e7dc878` (0.1.2), as its README says, with
`--out .cache/preview/before`.

## Screens taken (`shoot.mjs`)

`a` main view with results and a typed prompt · `b` the model sheet open (the model picker
before 0.2.0) · `b2` after switching to a cloud model (with the "what changed" note) · `c0` the Studio menu · `c` Get
models, the full view (Get local models before 0.2.0; scrolled to its top) · `c2` Get models as
it opens (the same since Task U5: it opens at its top) · `l` Get models at 900x650 · `m` Get
models on its LoRAs chip · `d` Hugging Face results (cards with verdict badges) · `d2` a
supported result opened in its detail panel (Task U6) · `d3` an unsupported result opened ·
`e0` the local-model notice before a first download, over the panel · `e` the review before a
download (the download dialog before Task U6; now Z-Image Turbo's detail panel) · `e2` the card
while downloading (with Pause and Cancel since Task U7) · `p` the top bar's download indicator
open, listing a model downloading, a LoRA downloading and a model ready to use (Task U7) · `e3` the detail panel of a result with 15 quantisations (Wan 2.2 A14B) ·
`e4` the detail panel of an OpenRAIL model with two model types (SDXL) · `e5` a detail panel at
900x650 ·
`f` Advanced open (the side panel, from Settings) · `f2` the whole side panel, docked (tall
1440 px viewport) · `lr` (file `l-loras`) the side panel's LoRAs, opened from the composer's
LoRAs button with two LoRAs added to SDXL, one of them for another model type (Task U8) · `mf`
(file `m-model-files`) the side panel's Files for the untested model (flux-dev-copy, set up as
SDXL), with Z-Image picked as its type: the plan of what that downloads, before it starts · `g` gallery grid (top) · `g2` grid as it opens · `g3` the grid's Select mode with two tiles ticked and its bar of actions (Task U9) · `g4` a tile's actions and prompt, shown as the keyboard reaches it (Task U9) · `h` viewer · `i`, `i2`
Settings -> Studio · `j` main view at 900x650 · `k` Video tab · `o` the SDXL inpainting model
chosen, with its Picture to change and Mask zones · `n` *Add a model file*, from the model
sheet's foot, with the file read · `q` the model sheet at 900x650 (a full-height sheet) ·
`z1`, `z2` first use (empty, the model sheet) · `z3` no model on this computer and none that fits
(`?empty=1&fits=0`): *Get models* (Task U9) · `z4` first use's button, which opens the model that fits in
its detail panel (Task U9) · `z5` the Video tab with picture models installed and no video model
(`?empty=1&pictures=1`): "No video models on this computer yet" (U9 fix round 1) · `x1` chat, `x2` chat model menu, `x3` Models dialog (for comparison).

## Updating it for a redesign

- New or renamed commands: add a `case` in `mock/ipc.ts`; the table's "unanswered commands"
  column names any command a screen called that the mock does not answer.
- New labels or layout: the selectors in `shoot.mjs` use roles and visible names
  (`getByRole("button", { name: "Studio menu" })`, `[data-model-chip] button`, `[data-studio-view] main`);
  change the step for a screen whose control moved, keep the file names so before/after pairs
  line up.
- Fonts: the app's stack names Segoe UI / SF; on Linux fontconfig resolves it to Inter.

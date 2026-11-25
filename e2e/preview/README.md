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
| `mock/data.ts` | The records, built from `src/components/studio/fixtures/models.ts`: its cloud models, installed models (a generic SDXL model, an untested one, one that needs set-up) and a LoRA; plus an SDXL inpainting model, a second LoRA, Hugging Face hits and repositories (a supported one, an OpenRAIL one, a Lumina 2 one that cannot run, a Wan 2.2 A14B one with 15 quantisations, a LoRA one), set-up plans, finished runs, one local run in progress, chats and a loaded chat model. |
| `mock/pictures.ts` | Sample pictures, drawn on a canvas per output id (PNG full size, JPEG thumbnails). |
| `shoot.mjs` | The Playwright script: each screen from a fresh page, driven by clicks and typing; measures each screen. |
| `pairs.mjs` | One HTML page with each screen's before and after, side by side, in both themes. |

Query flags: `?view=studio` (open Studio), `?empty=1` (first use: nothing installed, no
endpoint, no runs), `?notice=0` (the local-model notice never accepted, so the first
download shows it). `window.__preview` exposes `useStudio`, `useApp`, `state` (the mock's
data) and `emit(event, payload)` to send a backend event such as `studio-progress`.

## Run it

From the repo, with `node_modules` installed:

```bash
npm run preview:serve &                       # http://127.0.0.1:5199/e2e/preview/index.html
npm run preview:shoot                         # -> .cache/preview/after/ (52 PNGs) and a table
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
2026-11-24 and described in `docs/dev/0.2.0/studio-critique.md`. They are copied to
`.cache/preview/before/`, where `pairs.mjs` reads them. If they are lost, retake them from a
worktree of `e7dc878` (0.1.2): copy the harness exactly as it was then (the 0.1.2 version,
whose mock answers 0.1.2's commands, not this folder's, which answers Phase S's) into the
worktree and run it there. The tag and the harness are in the 0.2.0 plan notes under
`.superpowers/sdd/plan/` (`preview-harness/`, `studio-before/`).

## Screens taken (`shoot.mjs`)

`a` main view with results and a typed prompt · `b` the model sheet open (the model picker
before 0.2.0) · `b2` after switching to a cloud model (with the "what changed" note) · `c0` the Studio menu · `c` Get local
models (scrolled to its top) · `c2` Get local models as it opens · `d` Hugging Face results ·
`d2` a supported result opened · `d3` an unsupported result opened · `e0` the local-model
notice before a first download · `e` the download dialog · `e2` the row while downloading ·
`f` Advanced open (the side panel, from Settings) · `f2` the whole side panel, docked (tall
1440 px viewport) · `g` gallery grid (top) · `g2` grid as it opens · `h` viewer · `i`, `i2`
Settings -> Studio · `j` main view at 900x650 · `k` Video tab · `o` the SDXL inpainting model
chosen, with its Picture to change and Mask zones · `n` *Add a model file*, from the model
sheet's foot, with the file read · `q` the model sheet at 900x650 (a full-height sheet) ·
`z1`, `z2` first use (empty, the model sheet) · `x1` chat, `x2` chat model menu, `x3` Models dialog (for comparison).

## Updating it for a redesign

- New or renamed commands: add a `case` in `mock/ipc.ts`; the table's "unanswered commands"
  column names any command a screen called that the mock does not answer.
- New labels or layout: the selectors in `shoot.mjs` use roles and visible names
  (`getByRole("button", { name: "Studio menu" })`, `[data-model-chip] button`, `[data-studio-view] main`);
  change the step for a screen whose control moved, keep the file names so before/after pairs
  line up.
- Fonts: the app's stack names Segoe UI / SF; on Linux fontconfig resolves it to Inter.

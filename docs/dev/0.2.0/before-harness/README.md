# Studio preview harness at 0.1.2 (the "before" set)

This is the harness that took the 0.1.2 screenshots `docs/dev/0.2.0/studio-critique.md`
describes, kept as it was: its mock answers 0.1.2's commands. Run it at 0.1.2 (`e7dc878`) to
regenerate the before screenshots, which are not committed (see "Run it" below). The harness
for the redesigned screens is `e2e/preview/`.

Renders the **real** harnessUI app (`src/App.tsx`, with the sidebar, Studio, Settings, the
Models dialog) in a plain browser, with Tauri's IPC mocked by `@tauri-apps/api/mocks`
(`mockWindows` + `mockIPC` with `shouldMockEvents`), and takes screenshots of every Studio
screen with Playwright in the light and dark themes.

Nothing in `src/` is changed or mocked at module level: `src/lib/ipc.ts` and
`src/components/studio/api.ts` call `invoke()` as in the app, and `mock/ipc.ts` answers.
That is why the same harness works for "after" screenshots of a redesign.

## Files

| File | What it is |
| --- | --- |
| `vite.config.ts` | Vite with the app's plugins (React, Tailwind 4), **root = the repo**, port 5199 (`PREVIEW_PORT`). |
| `index.html`, `main.tsx` | The page: the app's own `main.tsx` with the mocks installed first. `?view=studio` opens Studio. |
| `mock/ipc.ts` | The mocked backend: one `case` per Tauri command. Unknown commands answer `null`, are logged once (`[preview] unhandled command …`) and collected in `window.__preview.unhandled`. |
| `mock/data.ts` | The records: the repo's own Studio fixtures (`src/components/studio/fixtures/models.ts`) plus OpenRouter's picture catalog, finished runs, one local run in progress, Hugging Face hits and repos, chats, a loaded chat model. |
| `mock/pictures.ts` | Sample pictures, drawn on a canvas per output id (PNG full size, JPEG thumbnails). |
| `shoot.cjs` | The Playwright script: each screen from a fresh page, driven by clicks and typing. |

Query flags: `?view=studio` (open Studio), `?empty=1` (first use: nothing installed, no
endpoint, no runs), `?notice=0` (the local-model notice never accepted, so the first
download shows it). `window.__preview` exposes `useStudio`, `useApp`, `state` (the mock's
data) and `emit(event, payload)` to send a backend event such as `studio-progress`.

## Run it

The harness must sit **inside a checkout of 0.1.2** as `zz-preview/` (Vite's root is its
parent, and `node_modules` is that checkout's). It is not tracked there: delete it when done.

```bash
git worktree add ../harnessUI-0.1.2 e7dc878          # 0.1.2, beside the repository
cd ../harnessUI-0.1.2 && npm ci
cp -r <repo>/docs/dev/0.2.0/before-harness zz-preview
npx vite --config zz-preview/vite.config.ts &        # serves http://127.0.0.1:5199/zz-preview/
node zz-preview/shoot.cjs --out <repo>/.cache/preview/before   # all screens, light + dark, 1280x800 @2x
# options: --only a,b,c2   --themes dark   --scale 1   --base http://127.0.0.1:5199/zz-preview/index.html
pkill -f "zz-preview/vite.config.ts"; cd <repo> && git worktree remove ../harnessUI-0.1.2   # clean up
```

Playwright: `shoot.cjs` uses `require("playwright")` (global install with its Chromium), or
the path in `PLAYWRIGHT_PATH`. No browser download is needed where Playwright's Chromium is
preinstalled.

To look at a screen by hand, open `http://127.0.0.1:5199/zz-preview/index.html?view=studio`
in any browser.

## Screens taken (`shoot.cjs`)

`a` main view with results and a typed prompt · `b` model picker open · `b2` after switching
to a cloud model (with the "what changed" note) · `c0` the Studio ⋯ menu · `c` Get local
models (scrolled to its top) · `c2` Get local models as it opens · `d` Hugging Face results ·
`d2` a supported result opened · `d3` an unsupported result opened · `e0` the local-model
notice before a first download · `e` the download dialog · `e2` the row while downloading ·
`f` Advanced open · `f2` the whole right panel (tall viewport) · `g` gallery grid (top) · `g2`
grid as it opens · `h` viewer · `i`, `i2` Settings → Studio · `j` main view at 900x650 · `k`
Video tab · `z1`, `z2` first use (empty, picker) · `x1` chat, `x2` chat model menu, `x3`
Models dialog (for comparison).

## Updating it for a redesign

- New or renamed commands: add a `case` in `mock/ipc.ts`; the `problems` list printed at the
  end of `shoot.cjs` names any command a screen called that the mock does not answer.
- New labels or layout: the selectors in `shoot.cjs` use roles and visible names
  (`getByRole("button", { name: "Studio menu" })`, `[data-model-picker]`, `[data-studio-view] main`);
  change the step for a screen whose control moved, keep the file names so before/after pairs
  line up.
- Fonts: the app's stack names Segoe UI / SF; on Linux fontconfig resolves it to Inter.

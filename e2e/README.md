# End-to-end tests

These drive the real app (a debug build) through
[`tauri-driver`](https://crates.io/crates/tauri-driver) and WebKitGTK's WebDriver on
Linux. They use only the Python standard library.

They complement, not replace, the manual checklists in each milestone summary: Linux is
a development platform only, so window behaviour, keychains, GPUs and installers are
tested on Windows and macOS by hand.

## Setup (Linux)

```bash
sudo apt-get install -y webkit2gtk-driver xvfb
```

```bash
cargo install tauri-driver --locked
```

```bash
python3 scripts/fetch_llama.py linux
```

```bash
python3 scripts/fetch_test_model.py
```

## Run

Start the Vite dev server (debug builds load the UI from it), build the app, then run a
scenario under a virtual display:

```bash
npm run dev
```

```bash
cargo build -p app
```

```bash
xvfb-run -a python3 -I e2e/m1_chat.py
```

Environment variables: `APP_BIN` (default `target/debug/app`), `TEST_GGUF`,
`APP_DEV_LLAMA_DIR` (default `.cache/llama-linux`), `E2E_OUT` (screenshots and logs).
Each run uses a fresh temporary home folder, so it never touches your own app data.

## Scenarios

| Script | Covers |
|---|---|
| `m1_chat.py` | First run, loading a GGUF, streaming, regenerate and versions, edit-and-resend, stop, search, rename, delete, an OpenAI-compatible endpoint |
| `m2_models.py` | Hugging Face search, a real download with verification, installed models, a vision model with its projector |
| `m3_agent.py` | A tool-calling turn: Allow once, Allow for this chat then a call under the grant, Deny, Stop with a card open, audit rows |
| `m5_skills.py` | Creating a skill in the editor, the strict validator, the model loading it without a card, disable and delete |
| `m6_browser.py` | The browser tool registered and off by default, the browser found, Settings → Browser |
| `m8_offline.py` | Start, a local chat and every settings page with all proxies pointing at a recorder that refuses everything: no outbound request |

The scenarios with a model use Qwen3 0.6B, which sometimes answers without calling the
tool it was asked to use; those steps ask again up to three times.

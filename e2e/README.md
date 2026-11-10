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

#!/usr/bin/env python3
"""Offline gate (ARCHITECTURE.md 17): the app, with every proxy variable pointing at a
recording proxy that refuses everything, starts, loads a local model, answers a chat,
opens Models → Installed with a model that has a Hugging Face repository, opens every
settings page (Search included), opens Studio (both tabs, Get models and Settings → Studio
again), makes a picture on this computer with the stand-in engine (e2e/fake_sd/sd-cli) from an
installed FLUX.2 klein 4B of tiny files, and then another from a model the catalog does not
list (a tiny SDXL file and a LoRA at strength 0.8, `c_generic_model_with_a_lora`), the
engine's recorded arguments and prompt file checked, without a single outbound request other than Get models'
Popular list, which the proxy refuses.

Loopback is exempt (NO_PROXY): the UI is served from the dev server and llama-server
listens on 127.0.0.1. The app's loopback client never uses a proxy anyway.

    python3 -I e2e/m8_offline.py
"""

import hashlib
import json
import os
import pathlib
import shutil
import socket
import sqlite3
import struct
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "e2e"))
from webdriver import Session, close_settings, settings_tab, studio_tab  # noqa: E402

APP = os.environ.get("APP_BIN", str(ROOT / "target" / "debug" / "app"))
MODEL = os.environ.get("TEST_GGUF", "/home/user/llama-dev/Qwen3-0.6B-Q8_0.gguf")
LLAMA_DIR = os.environ.get("APP_DEV_LLAMA_DIR", str(ROOT / ".cache" / "llama-linux"))
OUT = pathlib.Path(os.environ.get("E2E_OUT", tempfile.mkdtemp(prefix="e2e-m8-")))
IDENTIFIER = "io.github.sugarhappy8.harnessui"
# A repository for the installed model (never asked for: nothing reaches the network).
REPO = "Qwen/Qwen3-0.6B-GGUF"
# The Studio engine: a stand-in that answers as the pinned sd-cli does (e2e/fake_sd/sd-cli).
FAKE_SD_DIR = ROOT / "e2e" / "fake_sd"
# The picture model installed for the local run, as the bundled catalog lists it.
KLEIN = "flux2-klein-4b-q4"
KLEIN_TABLE = ROOT / "crates" / "models" / "tests" / "fixtures" / "gguf" / "klein4b-tensors.txt"
STUDIO_PROMPT = "a lighthouse on a rock at dawn, seen from the sea"
# The imported SDXL file and its LoRA (`install_generic`); the LoRA's name carries a non-ASCII
# letter on purpose, as the engine's `<lora:NAME:W>` tag must.
GENERIC_ID = "local:e2e0000000000000"
GENERIC_NAME = "E2E SDXL file"
LORA_NAME = "e2e style \u00fc"
LORA_REL = f"local/{LORA_NAME}"
GENERIC_PROMPT = "a paper boat on a pond, soft light"
# Where the stand-in engine records its starts (e2e/fake_sd/sd-cli): `$HOME/fake-sd-record`.
RECORD_FOLDER = "fake-sd-record"

# Every tab of the settings dialog, in the order src/components/SettingsDialog.tsx shows them.
SETTINGS_PAGES = [
    "General",
    "Local models",
    "Reverse engineering",
    "Other apps",
    "Endpoints",
    "Studio",
    "Hugging Face",
    "MCP servers",
    "Skills",
    "Git",
    "Browser",
    "Computer use",
    "Search",
    "Permissions",
    "Audit log",
    "Diagnostics",
]

SEEN: list[str] = []


class Recorder(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _refuse(self):
        SEEN.append(f"{self.command} {self.path}")
        self.send_error(403, "offline test")

    do_GET = do_POST = do_PUT = do_HEAD = do_CONNECT = do_OPTIONS = _refuse


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def diffusion_gguf(table: pathlib.Path) -> bytes:
    """A GGUF v3 file with no metadata keys and the tensor table `table` lists (one
    `name d0 d1 …` a line), 32 bytes of data a tensor: the tiny diffusion file the Rust tests
    write (`models::gguf::testing::diffusion_gguf`), which the app identifies by its table."""
    tensors = []
    for line in table.read_text(encoding="utf-8").splitlines():
        if line.strip():
            name, *dims = line.split()
            tensors.append((name.encode(), [int(d) for d in dims]))
    align, data = 32, 32
    out = bytearray(b"GGUF" + struct.pack("<IQQ", 3, len(tensors), 0))
    for i, (name, dims) in enumerate(tensors):
        out += struct.pack("<Q", len(name)) + name + struct.pack("<I", len(dims))
        out += b"".join(struct.pack("<Q", d) for d in dims)
        # GGML_TYPE_I8, and the tensor's offset in the data.
        out += struct.pack("<IQ", 24, i * data)
    out += bytes(-len(out) % align)
    return bytes(out + bytes(data * len(tensors)))


def install_klein(con: sqlite3.Connection, models: pathlib.Path) -> str:
    """Installs FLUX.2 klein 4B as a download would leave it: its catalog entry's parts
    written under `models` (the diffusion part the tiny GGUF of the 4B's tensor table, the
    others a few bytes) and its `gen_models` row, each part's `file` and `size` those of the
    file written. Returns its name."""
    entry = json.loads((ROOT / "catalog" / "models.json").read_text(encoding="utf-8"))["generators"][KLEIN]
    parts, total = [], 0
    for part in entry["parts"]:
        file = models.joinpath(*part["repo"].split("/"), *part["path"].split("/"))
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_bytes(diffusion_gguf(KLEIN_TABLE) if part["role"] == "diffusion" else bytes([7]) * 300)
        size = file.stat().st_size
        total += size
        parts.append({k: part[k] for k in ("role", "repo", "revision", "path", "sha256", "license")}
                     | {"file": str(file), "size": size})
    con.execute(
        "INSERT INTO gen_models (id, catalog_id, name, kind, family, recipe_id, parts_json, size_bytes, "
        "source, added_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'catalog', ?)",
        # The recipe a 0.1.x family runs with is the family's own id (`GenFamily::recipe_id`).
        (KLEIN, KLEIN, entry["name"], entry["kind"], entry["family"], entry["family"], json.dumps(parts),
         total, int(time.time() * 1000)),
    )
    con.commit()
    return entry["name"]


def sdxl_header_file() -> bytes:
    """A safetensors file with only a header (no tensor data) that the app reads as an SDXL
    checkpoint: the five tensors the detection rows 36, 37, 40, 41, 43 and 48 look at
    (`models::sd::detect`). Without the middle-block and output-block names it would read as
    SDXL Vega."""
    tensors = {
        "model.diffusion_model.input_blocks.0.0.weight": [320, 4, 3, 3],
        "model.diffusion_model.middle_block.1.proj_in.weight": [1],
        "model.diffusion_model.output_blocks.3.1.transformer_blocks.1.attn1.to_k.weight": [1],
        "conditioner.embedders.1.model.token_embedding.weight": [1],
        "conditioner.embedders.0.transformer.text_model.embeddings.token_embedding.weight": [49408, 768],
    }
    header = json.dumps({name: {"dtype": "F16", "shape": shape, "data_offsets": [0, 0]}
                         for name, shape in tensors.items()}).encode()
    return struct.pack("<Q", len(header)) + header


def install_generic(con: sqlite3.Connection, models: pathlib.Path) -> str:
    """Installs a model the catalog does not list, as importing a file leaves it: a tiny SDXL
    checkpoint (`sdxl_header_file`) under `models`, its `gen_models` row (recipe
    `sdxl-checkpoint`, family `sdxl`, source `import`, its one `model` part kept where it is),
    and a LoRA `local/e2e style ü` with its file under `<models>/@loras/local/` and its
    `gen_loras` row. Returns the model's name."""
    now = int(time.time() * 1000)
    file = models / "imported" / "e2e-sdxl.safetensors"
    file.parent.mkdir(parents=True, exist_ok=True)
    data = sdxl_header_file()
    file.write_bytes(data)
    part = {"role": "model", "repo": "", "revision": "", "path": file.name, "size": len(data),
            "sha256": hashlib.sha256(data).hexdigest(), "license": "", "local": str(file), "file": str(file)}
    con.execute(
        "INSERT INTO gen_models (id, catalog_id, name, kind, family, recipe_id, parts_json, size_bytes, "
        "source, added_at) VALUES (?, NULL, ?, 'image', 'sdxl', 'sdxl-checkpoint', ?, ?, 'import', ?)",
        (GENERIC_ID, GENERIC_NAME, json.dumps([part]), len(data), now),
    )
    lora = models / "@loras" / "local" / f"{LORA_NAME}.safetensors"
    lora.parent.mkdir(parents=True, exist_ok=True)
    lora_data = json.dumps({"lora_unet_x.lora_down.weight": {"dtype": "F16", "shape": [1], "data_offsets": [0, 0]}}).encode()
    lora_data = struct.pack("<Q", len(lora_data)) + lora_data
    lora.write_bytes(lora_data)
    sha = hashlib.sha256(lora_data).hexdigest()
    con.execute(
        "INSERT INTO gen_loras (id, name, file, rel_name, size_bytes, sha256, base_hint, format, source, "
        "added_at) VALUES (?, ?, ?, ?, ?, ?, 'sdxl', 'kohya', 'import', ?)",
        (f"local:{sha[:16]}", LORA_NAME, str(lora), LORA_REL, len(lora_data), sha, now),
    )
    con.commit()
    return GENERIC_NAME


def studio_part(s, home: pathlib.Path, con_path: pathlib.Path):
    """Studio offline: opened from the sidebar, both tabs, Get models, Settings → Studio
    again; then FLUX.2 klein 4B installed (as a download leaves it), chosen in the model sheet
    under "On this computer", and one picture made by the stand-in engine."""
    s.click_button("Studio")
    s.wait_for(lambda: s.execute("return !!document.querySelector('[data-studio-view]');"), 15, "Studio shown")
    studio_tab(s, "Video")
    studio_tab(s, "Pictures")
    # 0.2.0 (Task U3): Get models is the top bar's own button (the ⋯ menu lost the item). Since
    # Task U5 opening it asks Hugging Face for its Popular list (design 10.3, ARCHITECTURE
    # §13.6), as does each of its filter chips the first time: the only requests this gate
    # expects, each a CONNECT to huggingface.co that the proxy refuses (the list says so).
    asked_before = len(SEEN)
    s.click_button("Get models")
    s.wait_for(lambda: s.execute("return !!document.querySelector('[data-local-models]');"), 15, "Get models")
    s.wait_for(
        lambda: s.execute("return !!document.querySelector('[data-local-models] [aria-label=\"This computer\"]');"),
        15,
        "this computer's chips",
    )
    s.click_button("Video")
    s.click_button("Pictures")
    time.sleep(1)
    s.click_button("Back")
    s.wait_for(lambda: s.execute("return !document.querySelector('[data-local-models]');"), 10, "Back to the cards")
    popular = SEEN[asked_before:]
    assert all(r == "CONNECT huggingface.co:443" for r in popular), popular
    del SEEN[asked_before:]

    con = sqlite3.connect(con_path)
    name = install_klein(con, home / "studio-models")
    con.close()
    # Settings → Studio while Studio is open; closing Settings has Studio read what is saved
    # again, the installed model with it.
    s.click_button("Settings")
    settings_tab(s, "Studio")
    time.sleep(1)
    close_settings(s)

    # The model sheet leads with this computer, and the installed model is there; it is chosen
    # by hand, as on a computer with only a processor its fit word ("Slow here") does not make it
    # the default. It opens from the composer's model chip, which reads "Choose a model" while
    # the tab has none.
    s.click_button("Choose a model")
    first = s.wait_for(
        lambda: s.execute(
            """
            const sheet = document.querySelector('[data-model-sheet]');
            const heading = sheet && sheet.querySelector('section h3');
            return heading ? heading.innerText.trim() : null;
            """
        ),
        10,
        "the model sheet's sections",
    )
    assert first == "On this computer", f"the model sheet's first section is {first!r}"
    s.click_button(name)
    s.wait_text("On this computer: your prompt and pictures stay here", 10)
    box = s.find("textarea[aria-label=Prompt]")
    s.type(box, STUDIO_PROMPT)
    s.wait_for(
        lambda: s.execute(
            """
            const b = [...document.querySelectorAll('[data-studio-view] [data-composer] button')]
              .find(b => b.innerText.trim() === 'Create');
            return !!b && !b.disabled;
            """
        ),
        15,
        "Create enabled",
    )
    s.click_button("Create")
    s.wait_for(
        lambda: s.execute("return document.querySelectorAll('[data-run-card] [data-output-tile]').length;") == 1,
        120,
        "one result tile",
    )
    runs = s.invoke("studio_runs", {"filter": {}, "before": None, "limit": 10})["runs"]
    assert len(runs) == 1, runs
    run = runs[0]
    assert (run["status"], run["runsOn"], run["costUsd"], len(run["outputs"])) == ("done", "local", 0.0, 1), run
    assert pathlib.Path(run["outputs"][0]["path"]).is_file(), run["outputs"][0]


def pointer_click_found(s, script: str, *args, what: str):
    """A real pointer click on the element `script` returns (with `args`)."""

    def find():
        return s.execute(script, *args) or False

    el = s.wait_for(find, 15, what)
    s._pointer_click(find, el)


def c_generic_model_with_a_lora(s, home: pathlib.Path, con_path: pathlib.Path):
    """A model the catalog does not list, with a LoRA, through the stand-in engine: an SDXL file
    (as an import leaves it) and a LoRA are added, the model is chosen in the model sheet, the
    LoRA is attached at strength 0.8 in the side panel's LoRAs section, and Create runs it.
    The engine's recorded start has `-m <the file>`, `--lora-model-dir <models>/@loras`,
    `--steps 20`, `--cfg-scale 5` and no `--diffusion-model`, and its prompt file ends with
    the LoRA's tag."""
    models = home / "studio-models"
    models.mkdir(parents=True, exist_ok=True)
    # The LoRA folder the engine reads is in the models folder as Settings has it.
    s.invoke("set_models_dir", {"path": str(models)})
    con = sqlite3.connect(con_path)
    name = install_generic(con, models)
    con.close()
    s.click_button("Settings")
    settings_tab(s, "Studio")
    time.sleep(1)
    close_settings(s)

    pointer_click_found(
        s, "return document.querySelector('[data-studio-view] [data-composer] [data-model-chip] button');", what="the model chip"
    )
    pointer_click_found(
        s,
        """
        return [...document.querySelectorAll('[data-model-sheet] [data-sheet-pick]')]
          .find(o => o.querySelector('[data-sheet-name]').innerText.trim() === arguments[0]
                     && o.getAttribute('aria-disabled') !== 'true') || null;
        """,
        name,
        what=f"the model {name!r} in the model sheet",
    )
    s.wait_for(lambda: s.execute("return !document.querySelector('[data-model-sheet]');"), 10, "the model sheet closed")

    # The side panel's LoRAs section: Add, the LoRA, then its strength from 1 to 0.8 (the slider's
    # step is 0.05).
    pointer_click_found(
        s, "return document.querySelector('[data-studio-view] [data-composer] [data-settings-button]');", what="the composer's Settings"
    )
    section = "[data-settings-panel] [data-settings-section=loras]"
    pointer_click_found(
        s,
        f"""
        const section = document.querySelector('{section}');
        return section ? [...section.querySelectorAll('button')].find(b => b.innerText.trim() === 'Add') || null : null;
        """,
        what="the LoRAs section's Add",
    )
    s.click_menu_item(LORA_NAME)
    thumb = f'{section} [role=slider][aria-label="Strength of {LORA_NAME}"]'
    s.wait_for(lambda: s.execute("return !!document.querySelector(arguments[0]);", thumb), 10, "the LoRA's strength")
    s.type(s.find(thumb), "\ue012" * 4)
    s.wait_for(
        lambda: s.execute("return document.querySelector(arguments[0]).getAttribute('aria-valuenow');", thumb) == "0.8",
        10,
        "the strength 0.8",
    )
    pointer_click_found(
        s,
        """
        const panel = document.querySelector('[data-settings-panel]');
        const box = panel && panel.closest('[role=dialog], aside');
        return box ? box.querySelector('button[aria-label="Close"]') : null;
        """,
        what="the side panel's Close",
    )
    s.wait_for(lambda: s.execute("return !document.querySelector('[data-settings-panel]');"), 10, "the side panel closed")

    box = s.find("textarea[aria-label=Prompt]")
    s.type(box, "\ue009a\ue000\ue003")
    s.type(box, GENERIC_PROMPT)
    s.wait_for(
        lambda: s.execute(
            """
            const b = [...document.querySelectorAll('[data-studio-view] [data-composer] button')]
              .find(b => b.innerText.trim() === 'Create');
            return !!b && !b.disabled;
            """
        ),
        15,
        "Create enabled",
    )
    s.click_button("Create")
    s.wait_for(
        lambda: s.execute("return document.querySelectorAll('[data-run-card] [data-output-tile]').length;") == 2,
        120,
        "the second result tile",
    )
    runs = s.invoke("studio_runs", {"filter": {}, "before": None, "limit": 10})["runs"]
    run = next(r for r in runs if r["model"] == GENERIC_ID)
    assert (run["status"], run["runsOn"], len(run["outputs"])) == ("done", "local", 1), run

    # The engine's last recorded start is this run's.
    record = home / RECORD_FOLDER
    starts = sorted(int(p.stem.split("-")[1]) for p in record.glob("argv-*.json"))
    n = starts[-1]
    argv = json.loads((record / f"argv-{n}.json").read_text(encoding="utf-8"))
    part_file = str(models / "imported" / "e2e-sdxl.safetensors")

    def after(flag: str) -> str:
        assert flag in argv, (flag, argv)
        return argv[argv.index(flag) + 1]

    assert after("-m") == part_file, argv
    assert after("--lora-model-dir") == str(models / "@loras"), argv
    assert after("--steps") == "20" and after("--cfg-scale") == "5", argv
    assert "--diffusion-model" not in argv, argv
    prompt = (record / f"prompt-{n}.txt").read_text(encoding="utf-8")
    assert prompt.endswith(f" <lora:{LORA_REL}:0.8>") and prompt.startswith(GENERIC_PROMPT), repr(prompt)


def main() -> int:
    home = OUT / "home"
    shutil.rmtree(home, ignore_errors=True)
    home.mkdir(parents=True)
    # The stand-in engine records each start in here (e2e/fake_sd/sd-cli).
    (home / RECORD_FOLDER).mkdir()
    proxy_port = free_port()
    proxy = ThreadingHTTPServer(("127.0.0.1", proxy_port), Recorder)
    threading.Thread(target=proxy.serve_forever, daemon=True).start()
    proxy_url = f"http://127.0.0.1:{proxy_port}"
    env = dict(os.environ)
    env.update(
        HOME=str(home),
        XDG_DATA_HOME=str(home / "data"),
        XDG_CONFIG_HOME=str(home / "config"),
        XDG_CACHE_HOME=str(home / "cache"),
        APP_DEV_LLAMA_DIR=LLAMA_DIR,
        APP_DEV_SD_DIR=str(FAKE_SD_DIR),
        RUST_LOG="info",
    )
    for k in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"]:
        env[k] = proxy_url
    env["NO_PROXY"] = env["no_proxy"] = "localhost,127.0.0.1,::1"
    port = free_port()
    driver = subprocess.Popen(
        ["tauri-driver", "--port", str(port), "--native-driver", shutil.which("WebKitWebDriver") or "WebKitWebDriver"],
        env=env, stdout=open(OUT / "tauri-driver.log", "w"), stderr=subprocess.STDOUT,
    )
    s = None
    try:
        time.sleep(1.5)
        s = Session(f"http://127.0.0.1:{port}", {"tauri:options": {"application": APP}})
        s.wait_text("Welcome to", 60)
        s.click_button("Skip for now")
        s.wait_text("New chat", 20)
        settings = {"ctx": 4096, "backend": None, "device": None}
        s.invoke("set_local_model_settings", {"settings": settings})
        s.invoke("load_local_model", {"path": MODEL, "settings": settings})
        s.click_button("New chat")
        time.sleep(0.5)
        s.click_button("Model for this chat")
        s.click_menu_item(os.path.basename(MODEL))
        s.wait_for(lambda: "still loading" not in s.page_text(), 60, "composer enabled")
        box = s.find("textarea")
        s.type(box, "Say hello in three words. /no_think")
        s.type(box, "")
        time.sleep(1)
        s.wait_for(lambda: s.execute("return document.querySelector('button[aria-label=\"Stop\"]') === null;"), 240, "reply")
        chat_id = s.invoke("list_chats", {"query": None})[0]["id"]
        msgs = s.invoke("get_chat", {"chatId": chat_id})["messages"]
        assert msgs[-1]["role"] == "assistant" and msgs[-1]["text"].strip(), msgs[-1]
        # 0.0.7 (P2 review M2): Models → Installed with a model from Hugging Face asks it for
        # nothing. What it offers as an image file is what was saved when the model was
        # downloaded; a model with none saved (as if downloaded before 0.0.7, here: imported
        # and given a repository) shows "Look for an image file", which asks only when clicked.
        installed = s.invoke("import_model", {"path": MODEL})
        con = sqlite3.connect(home / "data" / IDENTIFIER / "app.db")
        con.execute("UPDATE models SET repo = ? WHERE id = ?", (REPO, installed["id"]))
        con.commit()
        con.close()
        rows = s.invoke("list_installed_models")
        row = next(r for r in rows if r["id"] == installed["id"])
        assert row["repo"] == REPO and row.get("imageFileOffer") is None, row
        s.click_button("Models")
        s.wait_text("Installed", 10)
        s.click_button("Installed")
        s.wait_text("Look for an image file", 15)
        time.sleep(2)
        s.execute("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}));")
        s.wait_for(lambda: "Look for an image file" not in s.page_text(), 10, "Models closed")
        s.click_button("Settings")
        for tab in SETTINGS_PAGES:
            settings_tab(s, tab)
            time.sleep(0.6)
        close_settings(s)
        studio_part(s, home, home / "data" / IDENTIFIER / "app.db")
        c_generic_model_with_a_lora(s, home, home / "data" / IDENTIFIER / "app.db")
        time.sleep(2)
        if SEEN:
            print("OUTBOUND REQUESTS:", *SEEN, sep="\n  ")
            return 1
        print("no outbound requests")
        print("M8 OFFLINE PASS")
        return 0
    finally:
        if s is not None:
            try:
                s.screenshot(str(OUT / "final.png"))
            except Exception:  # noqa: BLE001
                pass
            try:
                s.quit()
            except Exception:  # noqa: BLE001
                pass
        driver.terminate()
        try:
            driver.wait(10)
        except subprocess.TimeoutExpired:
            driver.kill()
        proxy.shutdown()
        print("artifacts in", OUT)


if __name__ == "__main__":
    sys.exit(main())

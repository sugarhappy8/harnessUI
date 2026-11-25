#!/usr/bin/env python3
"""Offline gate (ARCHITECTURE.md 17): the app, with every proxy variable pointing at a
recording proxy that refuses everything, starts, loads a local model, answers a chat,
opens Models → Installed with a model that has a Hugging Face repository, opens every
settings page, opens Studio (both tabs, Get local models and Settings → Studio again) and
makes a picture on this computer with the stand-in engine (e2e/fake_sd/sd-cli) from an
installed FLUX.2 klein 4B of tiny files, without a single outbound request.

Loopback is exempt (NO_PROXY): the UI is served from the dev server and llama-server
listens on 127.0.0.1. The app's loopback client never uses a proxy anyway.

    python3 -I e2e/m8_offline.py
"""

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
from webdriver import Session  # noqa: E402

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
        "INSERT INTO gen_models (id, catalog_id, name, kind, family, parts_json, size_bytes, source, added_at) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, 'catalog', ?)",
        (KLEIN, KLEIN, entry["name"], entry["kind"], entry["family"], json.dumps(parts), total,
         int(time.time() * 1000)),
    )
    con.commit()
    return entry["name"]


def settings_tab(s, tab: str):
    """Opens section `tab` of the open Settings dialog and waits until it shows. The tab is
    found by its text among the dialog's own tabs and given a real pointer click: a button
    elsewhere can carry the same name (the sidebar's Studio button comes first in the page and
    sits under the dialog's overlay, where `click_button` would fall back to a DOM click on
    it)."""

    def find():
        el = s.execute(
            """
            return [...document.querySelectorAll(
              '[role=dialog] [role=tablist][aria-label="Settings sections"] [role=tab]')]
              .find(t => t.innerText.trim() === arguments[0]) || null;
            """,
            tab,
        )
        return el or False

    el = s.wait_for(find, 10, f"settings tab {tab!r}")
    s._pointer_click(find, el)
    # The page must really open, or the gate would pass without loading it.
    s.wait_for(
        lambda: s.execute(
            "return [...document.querySelectorAll('[role=tab][aria-selected=true]')]"
            ".some(t => t.innerText.trim() === arguments[0]);",
            tab,
        ),
        10,
        f"settings page {tab!r}",
    )


def close_settings(s):
    s.execute("document.querySelector('[role=dialog] button[aria-label=\"Close settings\"]').click();")
    s.wait_for(
        lambda: s.execute("return !document.querySelector('[aria-label=\"Settings sections\"]');"),
        10,
        "Settings closed",
    )


def studio_tab(s, label: str):
    """Studio's [Pictures | Video] tab `label` (the top bar's, or Get local models' own), with
    a real pointer click, once it is chosen."""

    def find():
        el = s.execute(
            """
            return [...document.querySelectorAll('[role=tablist][aria-label="Pictures or video"] [role=tab]')]
              .find(t => t.innerText.trim() === arguments[0]) || null;
            """,
            label,
        )
        return el or False

    el = s.wait_for(find, 10, f"Studio tab {label!r}")
    s._pointer_click(find, el)
    s.wait_for(
        lambda: s.execute(
            """
            return [...document.querySelectorAll('[role=tablist][aria-label="Pictures or video"] [role=tab]')]
              .some(t => t.innerText.trim() === arguments[0] && t.getAttribute('aria-selected') === 'true');
            """,
            label,
        ),
        10,
        f"Studio tab {label!r} chosen",
    )


def studio_part(s, home: pathlib.Path, con_path: pathlib.Path):
    """Studio offline: opened from the sidebar, both tabs, Get local models, Settings →
    Studio again; then FLUX.2 klein 4B installed (as a download leaves it), chosen in the
    model sheet under "On this computer", and one picture made by the stand-in engine."""
    s.click_button("Studio")
    s.wait_for(lambda: s.execute("return !!document.querySelector('[data-studio-view]');"), 15, "Studio shown")
    studio_tab(s, "Video")
    studio_tab(s, "Pictures")
    # 0.2.0 (Task U3): Get models is the top bar's own button (the ⋯ menu lost the item).
    s.click_button("Get models")
    s.wait_for(lambda: s.execute("return !!document.querySelector('[data-local-models]');"), 15, "Get local models")
    s.wait_text("This computer:", 15)
    studio_tab(s, "Video")
    studio_tab(s, "Pictures")
    time.sleep(1)
    s.click_button("Back")
    s.wait_for(lambda: s.execute("return !document.querySelector('[data-local-models]');"), 10, "Back to the cards")

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
            const b = [...document.querySelectorAll('[data-composer] button')]
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


def main() -> int:
    home = OUT / "home"
    shutil.rmtree(home, ignore_errors=True)
    home.mkdir(parents=True)
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

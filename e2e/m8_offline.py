#!/usr/bin/env python3
"""Offline gate (ARCHITECTURE.md 17): the app, with every proxy variable pointing at a
recording proxy that refuses everything, starts, loads a local model, answers a chat
and opens every settings page without a single outbound request.

Loopback is exempt (NO_PROXY): the UI is served from the dev server and llama-server
listens on 127.0.0.1. The app's loopback client never uses a proxy anyway.

    python3 -I e2e/m8_offline.py
"""

import os
import pathlib
import shutil
import socket
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

# Every tab of the settings dialog, in the order src/components/SettingsDialog.tsx shows them.
SETTINGS_PAGES = [
    "General",
    "Local models",
    "Other apps",
    "Endpoints",
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
        s.click_button("Settings")
        for tab in SETTINGS_PAGES:
            s.click_button(tab)
            # The page must really open, or the gate would pass without loading it.
            s.wait_for(
                lambda tab=tab: s.execute(
                    "return [...document.querySelectorAll('[role=tab][aria-selected=true]')]"
                    ".some(t => t.innerText.trim() === arguments[0]);",
                    tab,
                ),
                10,
                f"settings page {tab!r}",
            )
            time.sleep(0.6)
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

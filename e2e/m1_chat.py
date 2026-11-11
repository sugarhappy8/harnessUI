#!/usr/bin/env python3
"""M1 end-to-end: drives the real app (debug build) through tauri-driver.

Covers: first-run skip, loading a GGUF into the managed llama-server, a streamed
reply with tokens/s, regenerate and branch switching, edit-and-resend, stop,
history search, rename, delete, and an OpenAI-compatible endpoint (a local fake
server that streams SSE).

Run from the repo root (see e2e/README.md):
    python3 -I e2e/m1_chat.py
"""

import json
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
from webdriver import Session, WebDriverError  # noqa: E402

APP = os.environ.get("APP_BIN", str(ROOT / "target" / "debug" / "app"))
MODEL = os.environ.get("TEST_GGUF", "/home/user/llama-dev/Qwen3-0.6B-Q8_0.gguf")
LLAMA_DIR = os.environ.get("APP_DEV_LLAMA_DIR", str(ROOT / ".cache" / "llama-linux"))
OUT = pathlib.Path(os.environ.get("E2E_OUT", tempfile.mkdtemp(prefix="e2e-")))


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class FakeOpenAI(BaseHTTPRequestHandler):
    """Minimal OpenAI-compatible server: /v1/models and a streamed chat reply."""

    def log_message(self, *a):
        pass

    def do_GET(self):
        if self.path.rstrip("/") == "/v1/models":
            body = json.dumps({"object": "list", "data": [{"id": "fake-model", "object": "model"}]}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        else:
            self.send_error(404)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        req = json.loads(self.rfile.read(length) or b"{}")
        FakeOpenAI.last_request = req
        FakeOpenAI.last_auth = self.headers.get("Authorization")
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        for word in ["Hello ", "from ", "the ", "fake ", "endpoint."]:
            chunk = {"choices": [{"index": 0, "delta": {"content": word}}]}
            self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode())
            self.wfile.flush()
            time.sleep(0.05)
        done = {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 12, "completion_tokens": 5}}
        self.wfile.write(f"data: {json.dumps(done)}\n\ndata: [DONE]\n\n".encode())


# Starts a model load the way the UI does, without waiting for it: the result lands in
# `window[slot]`.
FIRE_LOAD = """
const [slot, path, settings] = arguments;
window[slot] = 'pending';
window.__TAURI_INTERNALS__.invoke('load_local_model', {path, settings})
  .then(() => { window[slot] = {ok: true}; })
  .catch(e => { window[slot] = {ok: false, error: e}; });
"""


def loads_done(s, timeout: float = 180):
    """Waits for the two loads started with FIRE_LOAD and returns their results."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        first = s.execute("return window.__load1")
        second = s.execute("return window.__load2")
        if first != "pending" and second != "pending":
            return first, second
        time.sleep(0.2)
    raise AssertionError("the model loads did not finish")


def step(name):
    print(f"--- {name}", flush=True)


def main() -> int:
    home = OUT / "home"
    shutil.rmtree(home, ignore_errors=True)
    home.mkdir(parents=True)
    env = dict(os.environ)
    env.update(
        HOME=str(home),
        XDG_DATA_HOME=str(home / "data"),
        XDG_CONFIG_HOME=str(home / "config"),
        XDG_CACHE_HOME=str(home / "cache"),
        APP_DEV_LLAMA_DIR=LLAMA_DIR,
        RUST_LOG="info",
    )
    driver_port = free_port()
    driver_log = open(OUT / "tauri-driver.log", "w")
    driver = subprocess.Popen(
        ["tauri-driver", "--port", str(driver_port), "--native-driver", shutil.which("WebKitWebDriver") or "WebKitWebDriver"],
        env=env, stdout=driver_log, stderr=subprocess.STDOUT,
    )
    fake_port = free_port()
    fake = ThreadingHTTPServer(("127.0.0.1", fake_port), FakeOpenAI)
    threading.Thread(target=fake.serve_forever, daemon=True).start()
    s = None
    try:
        time.sleep(1.5)
        s = Session(f"http://127.0.0.1:{driver_port}", {"tauri:options": {"application": APP}})

        step("first run screen, skip")
        s.wait_text("Welcome to", 60)
        s.click_button("Skip for now")
        s.wait_text("New chat", 20)

        step("load the model into llama-server")
        settings = {"ctx": 4096, "backend": None, "device": None}
        s.invoke("set_local_model_settings", {"settings": settings})
        s.invoke("load_local_model", {"path": MODEL, "settings": settings})
        status = s.invoke("runtime_status")
        assert status["state"] == "ready", status
        assert status["nCtx"] == 4096, status
        print("    backend:", status["backend"], "n_ctx:", status["nCtx"])

        # Reported from a real PC: a model picked with no chat open loaded without any
        # sign of it, so it was picked again, and the first load "was cancelled".
        step("the same model picked again while it loads joins that load")
        s.invoke("unload_model")
        s.execute(FIRE_LOAD, "__load1", MODEL, settings)
        time.sleep(0.4)
        s.execute(FIRE_LOAD, "__load2", MODEL, settings)
        first, second = loads_done(s)
        assert first == {"ok": True} and second == {"ok": True}, (first, second)
        assert s.invoke("runtime_status")["state"] == "ready"

        step("another model picked while one loads replaces it quietly")
        other = OUT / "other-model.gguf"
        if other.is_symlink() or other.exists():
            other.unlink()
        other.symlink_to(MODEL)
        s.invoke("unload_model")
        s.execute(FIRE_LOAD, "__load1", MODEL, settings)
        time.sleep(0.4)
        s.execute(FIRE_LOAD, "__load2", str(other), settings)
        first, second = loads_done(s)
        assert first.get("error", {}).get("code") == "load_replaced", first
        assert second == {"ok": True}, second
        status = s.invoke("runtime_status")
        assert status["state"] == "ready" and status["modelName"] == other.name, status
        assert status.get("error") is None, status
        assert "Loading was" not in s.page_text()
        s.invoke("unload_model")

        step("picking a model with no chat open opens a chat and shows it loading")
        header = "return document.querySelector('button[aria-label=\"Model for this chat\"]').textContent"
        assert s.invoke("list_chats", {"query": None}) == []
        assert s.execute(header) == "Choose a model", s.execute(header)
        s.click_button("Model for this chat")
        s.click_menu_item(os.path.basename(MODEL))
        shown_loading = False
        deadline = time.time() + 120
        while time.time() < deadline:
            shown_loading = shown_loading or "Loading model" in s.page_text()
            if s.invoke("runtime_status")["state"] == "ready":
                break
            time.sleep(0.05)
        assert shown_loading, "the header never showed the model loading"
        assert len(s.invoke("list_chats", {"query": None})) == 1
        s.wait_for(lambda: s.execute(header) == os.path.basename(MODEL), 10, "the header names the model")
        s.wait_for(lambda: "still loading" not in s.page_text(), 20, "composer enabled")

        step("send a message and stream the reply")
        box = s.find("textarea")
        s.type(box, "Reply with exactly: the sky is blue /no_think")
        s.type(box, "")
        s.wait_text("tokens/s", 120)
        s.screenshot(str(OUT / "m1-reply.png"))
        chats = s.invoke("list_chats", {"query": None})
        assert len(chats) == 1, chats
        chat_id = chats[0]["id"]
        detail = s.invoke("get_chat", {"chatId": chat_id})
        msgs = detail["messages"]
        assert [m["role"] for m in msgs] == ["user", "assistant"], msgs
        assert msgs[1]["status"] == "complete", msgs[1]
        assert msgs[1]["text"].strip(), msgs[1]
        assert msgs[1]["tokensPerSecond"] and msgs[1]["tokensPerSecond"] > 0, msgs[1]
        assert chats[0]["title"].startswith("Reply with exactly"), chats[0]
        print("    reply:", msgs[1]["text"][:80].replace("\n", " "))

        step("regenerate creates a second version")
        s.click_button("Regenerate")
        s.wait_for(lambda: "2 / 2" in s.page_text() or "2/2" in s.page_text(), 120, "version 2 of 2")
        s.wait_for(lambda: s.invoke("get_chat", {"chatId": chat_id})["messages"][-1]["status"] == "complete", 120, "regenerated reply")
        detail = s.invoke("get_chat", {"chatId": chat_id})
        assert len(detail["messages"][-1]["siblingIds"]) == 2, detail["messages"][-1]
        s.click_button("Previous version")
        s.wait_for(lambda: s.invoke("get_chat", {"chatId": chat_id})["messages"][-1]["id"] == msgs[1]["id"], 10, "first version current")

        step("edit and resend keeps the old branch")
        s.click_button("Edit and resend")
        s.wait_for(lambda: s.execute("return !!document.querySelector('textarea[aria-label=\"Edit message\"]');"), 10, "edit box")
        s.execute(
            """
            const t = document.querySelector('textarea[aria-label="Edit message"]');
            const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
            set.call(t, 'Reply with exactly: grass is green /no_think');
            t.dispatchEvent(new Event('input', {bubbles: true}));
            t.form.requestSubmit();
            """
        )
        s.wait_for(lambda: s.invoke("get_chat", {"chatId": chat_id})["messages"][0]["text"].startswith("Reply with exactly: grass"), 30, "edited message")
        s.wait_for(lambda: s.invoke("get_chat", {"chatId": chat_id})["messages"][-1]["status"] == "complete", 120, "reply to edit")
        detail = s.invoke("get_chat", {"chatId": chat_id})
        assert len(detail["messages"][0]["siblingIds"]) == 2, detail["messages"][0]

        step("stop a long reply")
        # Messages are stored as they complete; the UI finishes the turn a moment later.
        s.wait_for(lambda: s.execute("return document.querySelector('button[aria-label=\"Stop\"]') === null;"), 60, "idle")
        box = s.find("textarea")
        s.type(box, "Tell me a long story about rivers. /no_think")
        s.type(box, "")
        # Press Stop as soon as the first words arrive (a small model's reply can be short).
        clicked = False
        deadline = time.time() + 60
        while not clicked and time.time() < deadline:
            clicked = s.execute(
                "const b = document.querySelector('button[aria-label=\"Stop\"]');"
                "const streaming = document.body.innerText.includes('rivers. /no_think');"
                "if (b && streaming) { b.click(); return true; } return false;"
            )
            if not clicked:
                time.sleep(0.05)
        assert clicked, "the Stop button never appeared"
        s.wait_for(lambda: s.invoke("get_chat", {"chatId": chat_id})["messages"][-1]["status"] in ("stopped", "complete"), 30, "stopped reply")
        last = s.invoke("get_chat", {"chatId": chat_id})["messages"][-1]
        assert last["status"] == "stopped", last
        print("    stopped after", len(last["text"]), "characters")

        step("history search, rename, delete")
        found = s.invoke("list_chats", {"query": "rivers"})
        assert [c["id"] for c in found] == [chat_id], found
        assert s.invoke("list_chats", {"query": "zebra-not-here"}) == []
        s.invoke("rename_chat", {"chatId": chat_id, "title": "Renamed chat"})
        assert s.invoke("list_chats", {"query": None})[0]["title"] == "Renamed chat"

        step("OpenAI-compatible endpoint")
        p = s.invoke("save_provider", {"provider": {"id": None, "name": "Fake", "kind": "generic",
                                                     "baseUrl": f"http://127.0.0.1:{fake_port}/v1",
                                                     "apiKey": "sk-test", "clearApiKey": False}})
        models = s.invoke("test_provider", {"id": p["id"]})
        assert [m["ref"] for m in models] == [f"remote:{p['id']}:fake-model"], models
        chat = s.invoke("create_chat", {"modelRef": models[0]["ref"]})
        s.execute("window.location.reload();")
        time.sleep(2)
        s.wait_for(lambda: "Renamed chat" in s.page_text(), 20, "sidebar after reload")
        s.execute(
            """
            const a = [...document.querySelectorAll('nav[aria-label="Chat history"] button')].find(b => b.innerText.includes('New chat'));
            a && a.click();
            """
        )
        time.sleep(0.5)
        box = s.find("textarea")
        s.type(box, "hello")
        s.type(box, "")
        s.wait_text("Hello from the fake endpoint.", 30)
        assert FakeOpenAI.last_auth == "Bearer sk-test", FakeOpenAI.last_auth
        assert FakeOpenAI.last_request["model"] == "fake-model"
        assert FakeOpenAI.last_request["stream"] is True
        s.invoke("delete_chat", {"chatId": chat["id"]})
        s.invoke("delete_chat", {"chatId": chat_id})
        assert s.invoke("list_chats", {"query": None}) == []

        step("unload")
        s.invoke("unload_model")
        assert s.invoke("runtime_status")["state"] == "stopped"
        s.screenshot(str(OUT / "m1-end.png"))
        print("PASS: M1 end-to-end")
        return 0
    except Exception as e:  # noqa: BLE001
        print("FAIL:", e)
        if s:
            try:
                s.screenshot(str(OUT / "m1-failure.png"))
                print(s.page_text()[:2000])
            except Exception:
                pass
        return 1
    finally:
        if s:
            s.quit()
        driver.terminate()
        fake.shutdown()
        print("artifacts in", OUT)


if __name__ == "__main__":
    sys.exit(main())

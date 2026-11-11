#!/usr/bin/env python3
"""M3 end-to-end: an agent turn with the files tool against the real llama-server.

Covers: files tools on by default in a new chat; an approval card for read_file
(Allow once), "Allow for this chat" and a later call running under that grant, Deny
with the denial reaching the model, Stop while a card is open, tool-call cards in
the chat, and one audit row per call.

The model (Qwen3 0.6B by default) decides whether to call a tool, so the scenario
asks plainly and retries a prompt once if the model answers without calling.

    python3 -I e2e/m3_agent.py
"""

import os
import pathlib
import shutil
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "e2e"))
from webdriver import Session  # noqa: E402

APP = os.environ.get("APP_BIN", str(ROOT / "target" / "debug" / "app"))
MODEL = os.environ.get("TEST_GGUF", "/home/user/llama-dev/Qwen3-0.6B-Q8_0.gguf")
LLAMA_DIR = os.environ.get("APP_DEV_LLAMA_DIR", str(ROOT / ".cache" / "llama-linux"))
OUT = pathlib.Path(os.environ.get("E2E_OUT", tempfile.mkdtemp(prefix="e2e-m3-")))
IDENTIFIER = "io.github.sugarhappy8.harnessui"


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def step(name):
    print(f"--- {name}", flush=True)


def main() -> int:
    home = OUT / "home"
    shutil.rmtree(home, ignore_errors=True)
    home.mkdir(parents=True)
    work = (OUT / "work").resolve()
    shutil.rmtree(work, ignore_errors=True)
    work.mkdir()
    (work / "notes.txt").write_text("The secret word is PELICAN.\n")
    env = dict(os.environ)
    env.update(
        HOME=str(home),
        XDG_DATA_HOME=str(home / "data"),
        XDG_CONFIG_HOME=str(home / "config"),
        XDG_CACHE_HOME=str(home / "cache"),
        APP_DEV_LLAMA_DIR=LLAMA_DIR,
        RUST_LOG="info",
    )
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

        step("approve the work folder (written to the database: the picker is native)")
        db = home / "data" / IDENTIFIER / "app.db"
        assert db.exists(), db
        con = sqlite3.connect(db)
        con.execute("INSERT INTO approved_folders(path, chat_id, created_at) VALUES (?, NULL, ?)", (str(work), int(time.time() * 1000)))
        con.commit()
        con.close()
        folders = s.invoke("list_approved_folders")
        assert any(f["path"] == str(work) for f in folders), folders

        step("load the model")
        settings = {"ctx": 8192, "backend": None, "device": None}
        s.invoke("set_local_model_settings", {"settings": settings})
        s.invoke("load_local_model", {"path": MODEL, "settings": settings})
        status = s.invoke("runtime_status")
        assert status["state"] == "ready", status
        print("    supportsTools:", status["supportsTools"])

        step("new chat; files tools are on by default")
        s.click_button("New chat")
        time.sleep(0.5)
        s.click_button("Model for this chat")
        s.click_menu_item(os.path.basename(MODEL))
        s.wait_for(lambda: "still loading" not in s.page_text(), 60, "composer enabled")
        chat_id = s.invoke("list_chats", {"query": None})[0]["id"]
        tools = s.invoke("list_chat_tools", {"chatId": chat_id})
        on = sorted(t["exposed"] for t in tools if t["enabledInChat"])
        print("    enabled:", on)
        assert "read_file" in on and "run_command" not in on, on
        servers = s.invoke("list_mcp_servers")
        assert {x["name"] for x in servers} >= {"files", "terminal", "git", "skills"}, servers

        def messages():
            return s.invoke("get_chat", {"chatId": chat_id})["messages"]

        def idle():
            return s.execute("return document.querySelector('button[aria-label=\"Stop\"]') === null;")

        def send(text):
            box = s.find("textarea")
            s.type(box, text)
            s.type(box, "\ue007")  # Enter

        def card_shown():
            return "Allow once" in s.page_text()

        def ask_until_card(prompt, attempts=3):
            for i in range(attempts):
                before = len(messages())
                send(prompt)
                s.wait_for(lambda: card_shown() or (idle() and len(messages()) > before + 1), 240, "card or answer")
                if card_shown():
                    return True
                print(f"    attempt {i + 1}: the model answered without a tool call; asking again")
            return False

        step("read_file: approval card, Allow once")
        assert ask_until_card("Use the read_file tool to read notes.txt, then tell me the secret word. /no_think"), "no tool call"
        s.screenshot(str(OUT / "m3-card.png"))
        assert "read_file" in s.page_text()
        time.sleep(0.9)  # the card ignores input for 700 ms
        s.click_button("Allow once")
        s.wait_for(idle, 240, "turn finished")
        msgs = messages()
        roles = [m["role"] for m in msgs]
        print("    roles:", roles)
        assert "tool" in roles, roles
        tool_msg = next(m for m in msgs if m["role"] == "tool")
        assert tool_msg["toolResult"]["status"] == "ok", tool_msg
        assert "PELICAN" in tool_msg["toolResult"]["text"], tool_msg
        call_msg = next(m for m in msgs if m["toolCalls"])
        assert call_msg["toolCalls"][0]["name"] == "read_file", call_msg
        print("    answer:", msgs[-1]["text"][:100].replace("\n", " "))
        s.screenshot(str(OUT / "m3-after-read.png"))
        audit = s.invoke("list_audit", {"filter": {"chatId": chat_id}})
        assert [a["decision"] for a in audit] == ["allow_once"], audit
        assert audit[0]["tool"] == "read_file" and audit[0]["result_status"] == "ok", audit[0]

        step("Allow for this chat, then the same read runs without a card")
        assert ask_until_card("Read notes.txt again with read_file and repeat the secret word. /no_think"), "no tool call"
        time.sleep(0.9)
        s.click_button("Allow for this chat")
        s.wait_for(idle, 240, "turn finished")
        n_cards = 0
        before = len(s.invoke("list_audit", {"filter": {"chatId": chat_id}}))
        send("Use read_file on notes.txt one more time. /no_think")
        deadline = time.time() + 240
        while time.time() < deadline:
            if card_shown():
                n_cards += 1
                break
            if idle() and len(s.invoke("list_audit", {"filter": {"chatId": chat_id}})) > before:
                break
            time.sleep(0.5)
        s.wait_for(idle, 240, "turn finished")
        audit = s.invoke("list_audit", {"filter": {"chatId": chat_id}})
        decisions = [a["decision"] for a in audit]
        print("    decisions:", decisions)
        assert "allow_for_chat" in decisions, decisions
        if len(audit) > before:
            assert n_cards == 0 and "allowed_by_grant" in decisions, decisions

        step("write_file: Deny; nothing is written")
        target = work / "hello.txt"
        assert ask_until_card(f"Use write_file to create the file {target} containing hi. /no_think"), "no tool call"
        time.sleep(0.9)
        s.click_button("Deny")
        s.wait_for(idle, 240, "turn finished")
        assert not (work / "hello.txt").exists()
        audit = s.invoke("list_audit", {"filter": {"chatId": chat_id, "decision": "denied"}})
        assert audit and audit[0]["tool"] == "write_file", audit
        denied = [m for m in messages() if m["role"] == "tool" and m["toolResult"]["status"] == "denied"]
        assert denied, "the denial is in the history"

        step("Stop while a card is open cancels the call")
        assert ask_until_card(f"Use write_file to create the file {work / 'bye.txt'} containing bye. /no_think"), "no tool call"
        s.click_button("Stop")
        s.wait_for(idle, 60, "turn stopped")
        s.wait_for(lambda: not card_shown(), 10, "card gone")
        assert not (work / "bye.txt").exists()
        audit = s.invoke("list_audit", {"filter": {"chatId": chat_id, "decision": "cancelled"}})
        assert audit and audit[0]["tool"] == "write_file", audit
        s.screenshot(str(OUT / "m3-end.png"))

        print("M3 E2E PASS")
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
        print("artifacts in", OUT)


if __name__ == "__main__":
    sys.exit(main())

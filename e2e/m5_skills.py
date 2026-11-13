#!/usr/bin/env python3
"""M5 end-to-end: a skill created in Settings → Skills is listed, offered in the
prompt's skills index, and loaded by the model with load_skill without a card.

The import buttons open native pickers, so this scenario creates the skill through
the editor (the same validator and save path); zip and folder import are covered by
the tools crate's tests.

    python3 -I e2e/m5_skills.py
"""

import os
import pathlib
import shutil
import socket
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
OUT = pathlib.Path(os.environ.get("E2E_OUT", tempfile.mkdtemp(prefix="e2e-m5-")))

SKILL = """---
name: magic-number
description: Knows the team's magic number. Use it when the user asks for the magic number.
license: MIT
---

# Magic number

The magic number is 4242. Answer with it exactly.
"""


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

        step("the validator rejects a bad skill; nothing is saved")
        problems = s.invoke("save_skill", {"folder": "wrong-folder", "text": SKILL, "isNew": True})
        assert problems, "a name that does not match its folder is refused"
        assert s.invoke("list_skills")["skills"] == []

        step("create the skill through the editor")
        s.click_button("Settings")
        s.click_button("Skills")
        s.click_button("New skill")
        s.execute(
            """
            const set = (el, v) => {
              const proto = el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
              Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, v);
              el.dispatchEvent(new Event('input', {bubbles: true}));
            };
            set(document.querySelector('textarea[aria-label="SKILL.md"]'), arguments[0]);
            const folder = [...document.querySelectorAll('input')].find((i) => i.closest('label')?.textContent.includes('Folder name'));
            set(folder, 'magic-number');
            """,
            SKILL,
        )
        s.click_button("Validate")
        s.wait_text("No problems found.", 10)
        s.click_button("Save")
        s.wait_for(lambda: any(x["name"] == "magic-number" for x in s.invoke("list_skills")["skills"]), 10, "skill listed")
        skills = s.invoke("list_skills")["skills"]
        assert skills[0]["enabled"] and not skills[0]["shared"], skills
        s.screenshot(str(OUT / "m5-skills.png"))
        s.execute("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}));")
        time.sleep(0.5)

        step("load the model and ask for the magic number")
        settings = {"ctx": 8192, "backend": None, "device": None}
        s.invoke("set_local_model_settings", {"settings": settings})
        s.invoke("load_local_model", {"path": MODEL, "settings": settings})
        s.click_button("New chat")
        time.sleep(0.5)
        s.click_button("Model for this chat")
        s.click_menu_item(os.path.basename(MODEL))
        s.wait_for(lambda: "still loading" not in s.page_text(), 60, "composer enabled")
        chat_id = s.invoke("list_chats", {"query": None})[0]["id"]
        tools = {t["exposed"]: t for t in s.invoke("list_chat_tools", {"chatId": chat_id})}
        assert tools["load_skill"]["enabledInChat"], tools["load_skill"]
        # The model thinks despite "/no_think" in the message, often for a few thousand
        # tokens; on a busy computer the turn then outlasts the wait below. Thinking off for
        # this chat (the header's switch), so the reply ends quickly.
        s.invoke("set_chat_thinking", {"chatId": chat_id, "on": False})

        def idle():
            return s.execute("return document.querySelector('button[aria-label=\"Stop\"]') === null;")

        loaded = False
        for attempt in range(3):
            box = s.find("textarea")
            s.type(box, "Use the load_skill tool to load the magic-number skill, then tell me the magic number. /no_think")
            s.type(box, "")
            time.sleep(1)
            s.wait_for(idle, 240, "turn finished")
            assert "Allow once" not in s.page_text(), "skills never show a card"
            msgs = s.invoke("get_chat", {"chatId": chat_id})["messages"]
            if any(m["toolResult"] and m["toolResult"]["name"] == "load_skill" for m in msgs):
                loaded = True
                break
            print(f"    attempt {attempt + 1}: no load_skill call; asking again")
        assert loaded, "the model never called load_skill"
        result = next(m for m in msgs if m["toolResult"] and m["toolResult"]["name"] == "load_skill")["toolResult"]
        assert result["status"] == "ok" and "4242" in result["text"], result
        print("    answer:", msgs[-1]["text"][:100].replace("\n", " "))
        assert s.invoke("loaded_skills", {"chatId": chat_id}) == ["magic-number"]
        audit = s.invoke("list_audit", {"filter": {"chatId": chat_id, "tool": "load_skill"}})
        assert audit and all(a["decision"] == "allowed_without_prompt" for a in audit), audit

        step("disable and delete")
        s.invoke("set_skill_enabled", {"name": "magic-number", "enabled": False})
        assert not s.invoke("list_skills")["skills"][0]["enabled"]
        s.invoke("delete_skill", {"name": "magic-number"})
        assert s.invoke("list_skills")["skills"] == []
        s.screenshot(str(OUT / "m5-end.png"))
        print("M5 E2E PASS")
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

#!/usr/bin/env python3
"""Scripted-model end-to-end checks: the real app against a fake OpenAI-compatible
endpoint whose replies (text, thinking, tool calls, errors, dropped streams) are scripted,
so the agent loop, approvals, tool cards, error handling and settings are checked
deterministically, without a model.

Each check records PASS or FAIL and the run goes on; the exit code is 1 if any failed.
`PROBES=name,name` runs only those checks.

    python3 -I e2e/scripted_agent.py
"""

import json
import os
import pathlib
import shutil
import socket
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import traceback
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "e2e"))
from webdriver import Session, WebDriverError  # noqa: E402

APP = os.environ.get("APP_BIN", str(ROOT / "target" / "debug" / "app"))
LLAMA_DIR = os.environ.get("APP_DEV_LLAMA_DIR", str(ROOT / ".cache" / "llama-linux"))
OUT = pathlib.Path(os.environ.get("E2E_OUT", tempfile.mkdtemp(prefix="e2e-scripted-")))
IDENTIFIER = "io.github.sugarhappy8.harnessui"
ONLY = set(filter(None, os.environ.get("PROBES", "").split(",")))

RESULTS = []


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class Fake(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    queue = []  # scripted replies, oldest first
    requests = []
    report_ctx = None
    lock = threading.Lock()

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
        with Fake.lock:
            Fake.requests.append(req)
            item = Fake.queue.pop(0) if Fake.queue else {"content": "ok"}
        if "status" in item:
            body = json.dumps({"error": {"message": item.get("message", "scripted failure")}}).encode()
            self.send_response(item["status"])
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Transfer-Encoding", "chunked")
        self.send_header("Connection", "close")
        self.end_headers()
        self.close_connection = True

        def raw(data: bytes):
            self.wfile.write(f"{len(data):x}\r\n".encode() + data + b"\r\n")
            self.wfile.flush()

        def emit(obj):
            raw(f"data: {json.dumps(obj)}\n\n".encode())

        try:
            if "tool_calls" in item:
                for i, tc in enumerate(item["tool_calls"]):
                    emit({"choices": [{"index": 0, "delta": {"tool_calls": [{
                        "index": i, "id": f"call_{len(Fake.requests)}_{i}", "type": "function",
                        "function": {"name": tc["name"], "arguments": json.dumps(tc["arguments"])}}]}}]})
                emit({"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
                      "usage": {"prompt_tokens": item.get("prompt_tokens", 50), "completion_tokens": 10}})
            else:
                rtext = item.get("reasoning", "")
                for i in range(0, len(rtext), 12):
                    emit({"choices": [{"index": 0, "delta": {"reasoning_content": rtext[i:i + 12]}}]})
                    time.sleep(0.01)
                text = item["content"]
                n = item.get("chunk", 16)
                for i in range(0, len(text), n):
                    emit({"choices": [{"index": 0, "delta": {"content": text[i:i + n]}}]})
                    time.sleep(item.get("delay", 0.01))
                    if item.get("drop_after") is not None and i >= item["drop_after"]:
                        self.connection.shutdown(socket.SHUT_RDWR)
                        return
                if item.get("no_finish"):
                    self.wfile.write(b"0\r\n\r\n")
                    return
                emit({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                      "usage": {"prompt_tokens": item.get("prompt_tokens", 50), "completion_tokens": 10}})
            raw(b"data: [DONE]\n\n")
            self.wfile.write(b"0\r\n\r\n")
        except (BrokenPipeError, ConnectionResetError, OSError):
            pass


HOOK = """
if (!window.__probe) {
  window.__probe = [];
  const push = (kind, msg) => window.__probe.push(kind + ': ' + String(msg).slice(0, 400));
  const ce = console.error.bind(console);
  console.error = (...a) => { push('console.error', a.map(x => (x && x.stack) || (typeof x === 'object' ? JSON.stringify(x) : x)).join(' ')); ce(...a); };
  const cw = console.warn.bind(console);
  console.warn = (...a) => { push('console.warn', a.join(' ')); cw(...a); };
  window.addEventListener('error', e => push('error', e.message + ' @' + e.filename + ':' + e.lineno));
  window.addEventListener('unhandledrejection', e => push('unhandledrejection', (e.reason && (e.reason.stack || JSON.stringify(e.reason))) || e.reason));
  document.addEventListener('securitypolicyviolation', e => push('csp', e.violatedDirective + ' ' + e.blockedURI + ' ' + (e.sample || '')));
}
return true;
"""


class Ctx:
    pass


def probe(name):
    def deco(fn):
        fn.probe_name = name
        return fn
    return deco


def run(fn, c):
    if ONLY and fn.probe_name not in ONLY:
        return
    print(f"--- {fn.probe_name}", flush=True)
    c.s.execute(HOOK)
    c.s.execute("window.__probe.length = 0; return true;")
    Fake.queue.clear()
    notes = []
    try:
        fn(c, notes)
        status = "PASS"
    except Exception as e:  # noqa: BLE001
        status = "FAIL"
        notes.append("".join(traceback.format_exception_only(type(e), e)).strip()[:1500])
        try:
            c.s.screenshot(str(OUT / f"fail-{fn.probe_name}.png"))
            notes.append("page: " + c.s.page_text()[-600:].replace("\n", " | "))
        except Exception:  # noqa: BLE001
            pass
    try:
        console = c.s.execute("return window.__probe ? window.__probe.slice() : ['<hook lost>'];")
    except Exception as e:  # noqa: BLE001
        console = [f"<could not read console: {e}>"]
    RESULTS.append((fn.probe_name, status, notes, console))
    print(f"    {status}", flush=True)
    for n in notes:
        print("    note:", n, flush=True)
    for line in console:
        print("    console:", line, flush=True)
    # Leave dialogs and popovers closed for the next probe.
    try:
        for _ in range(3):
            c.s.execute("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}));")
            time.sleep(0.15)
    except Exception:  # noqa: BLE001
        pass


def idle(s):
    return s.execute("return document.querySelector('button[aria-label=\"Stop\"]') === null;")


def send(s, text):
    box = s.find("textarea")
    s.type(box, text)
    s.type(box, "")


def new_chat(c, tools=None):
    s = c.s
    chat = s.invoke("create_chat", {"modelRef": c.model_ref})
    s.invoke("set_chat_tools", {"chatId": chat["id"], "enabled": c.default_tools if tools is None else tools})
    s.execute("window.location.reload();")
    time.sleep(1.5)
    s.wait_for(lambda: "New chat" in s.page_text(), 20, "sidebar after reload")
    s.execute(HOOK)
    # Open the newest chat (first in the list).
    s.execute(
        """
        const a = [...document.querySelectorAll('nav[aria-label="Chat history"] button')].find(b => b.innerText.includes('New chat'));
        a && a.click();
        """
    )
    time.sleep(0.6)
    return chat["id"]


def messages(s, chat_id):
    return s.invoke("get_chat", {"chatId": chat_id})["messages"]


def wait_idle(s, n=60):
    time.sleep(0.4)
    s.wait_for(lambda: idle(s), n, "turn finished")


def answer_card(s, label, timeout=30):
    s.wait_for(lambda: "Allow once" in s.page_text(), timeout, "approval card")
    time.sleep(0.9)
    s.click_button(label)


# ---------------------------------------------------------------------------------------


@probe("markdown")
def p_markdown(c, notes):
    s = c.s
    chat = new_chat(c, tools=[])
    text = (
        "Entities: A &amp; B &lt;tag&gt; &copy; &notit; &#x41; &nbsp;end.\n\n"
        "```python\ndef f(x):\n    return x + 1  # comment\n```\n\n"
        "```nosuchlang\nplain <b>text</b>\n```\n\n"
        "```\nno language here\n```\n\n"
        "Inline `code` and **bold** and a [link](https://example.com/x) and [bad](javascript:alert(1)).\n\n"
        "<img src=\"https://example.com/pixel.png\" onerror=\"window.__pwned=1\">\n\n"
        "<script>window.__pwned=2</script>\n\n"
        "![remote](https://example.com/remote.png)\n\n"
        "| a | b |\n|---|---|\n| 1 | 2 |\n\n"
        "- [ ] task\n- [x] done\n\n"
        "<details><summary>s</summary>hidden</details>\n"
    )
    Fake.queue.append({"content": text, "chunk": 7})
    send(s, "render this")
    wait_idle(s)
    s.screenshot(str(OUT / "markdown.png"))
    info = s.execute(
        """
        const root = document.querySelector('main') || document.body;
        return {
          text: root.innerText,
          hljs: root.querySelectorAll('pre code [class^="hljs-"], pre code [class*=" hljs-"]').length,
          pre: root.querySelectorAll('pre').length,
          imgs: [...root.querySelectorAll('img')].map(i => i.getAttribute('src')),
          scripts: root.querySelectorAll('script').length,
          links: [...root.querySelectorAll('a')].map(a => a.getAttribute('href')),
          tables: root.querySelectorAll('table').length,
          details: root.querySelectorAll('details').length,
          pwned: window.__pwned || null,
          checkboxes: root.querySelectorAll('input[type=checkbox]').length,
        };
        """
    )
    notes.append(json.dumps({k: v for k, v in info.items() if k != "text"}))
    t = info["text"]
    assert "A & B <tag> ©" in t, t[-500:]
    assert "&amp;" not in t, "entity left undecoded"
    assert info["hljs"] > 0, "no highlighted tokens"
    assert info["pre"] == 3, info["pre"]
    assert info["pwned"] is None, "script ran"
    assert info["scripts"] == 0
    assert not any(src and src.startswith("http") for src in info["imgs"]), info["imgs"]
    assert not any(h and h.lower().startswith("javascript:") for h in info["links"]), info["links"]
    assert info["tables"] == 1
    assert "plain <b>text</b>" in t


@probe("settings-pages")
def p_settings(c, notes):
    s = c.s
    s.click_button("Settings")
    tabs = ["General", "Local models", "Endpoints", "Hugging Face", "MCP servers", "Skills", "Git", "Browser",
            "Computer use", "Permissions", "Audit log", "Diagnostics"]
    for tab in tabs:
        s.click_button(tab)
        time.sleep(0.7)
        text = s.page_text()
        alerts = s.execute("return [...document.querySelectorAll('[role=alert]')].map(e => e.innerText.slice(0, 200));")
        if alerts:
            notes.append(f"{tab}: alerts {alerts}")
        assert "Something went wrong" not in text, tab
    s.screenshot(str(OUT / "settings-diagnostics.png"))
    assert not any("alerts" in n for n in notes), notes


@probe("general-settings")
def p_general(c, notes):
    s = c.s
    st = s.invoke("update_status")
    notes.append("update_status " + json.dumps(st))
    assert st["available"] is None and st["configured"] is False
    hk = s.invoke("stop_hotkey_status")
    notes.append("hotkey " + json.dumps(hk))
    bar = "stop hotkey could not be registered" in s.page_text()
    assert bar == (bool(hk["accelerator"]) and not hk["registered"]), f"warning bar {bar} vs status {hk}"
    info = s.invoke("first_run_info")
    notes.append(f"catalog seq {info.get('catalogSequence')} disabled: {info.get('refreshDisabledReason')}")
    s.click_button("Settings")
    s.click_button("General")
    time.sleep(0.5)
    text = s.page_text()
    assert "Model catalog" in text and "Default system prompt" in text, text[-400:]
    assert "Catalog version" in text
    btn = s.execute("return [...document.querySelectorAll('button')].find(b => b.innerText.includes('Refresh catalog')).disabled;")
    assert btn is True, "refresh should be disabled without a key"
    # Default system prompt through the UI.
    s.execute(
        """
        const el = document.querySelector('section[aria-label="Default system prompt"] textarea');
        Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(el, arguments[0]);
        el.dispatchEvent(new Event('input', {bubbles: true}));
        """,
        "PROBE-SYSTEM-PROMPT-77",
    )
    sec = "section[aria-label=\"Default system prompt\"]"
    s.execute(f"[...document.querySelectorAll('{sec} button')].find(b => /save/i.test(b.innerText)).click();")
    time.sleep(0.6)
    assert s.invoke("get_setting", {"key": "defaultSystemPrompt"}) == "PROBE-SYSTEM-PROMPT-77"
    s.execute("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}));")
    chat = new_chat(c, tools=[])
    Fake.requests.clear()
    send(s, "hi")
    wait_idle(s)
    sysmsg = Fake.requests[-1]["messages"][0]
    assert sysmsg["role"] == "system" and "PROBE-SYSTEM-PROMPT-77" in sysmsg["content"], sysmsg
    s.invoke("set_setting", {"key": "defaultSystemPrompt", "value": None})
    send(s, "again")
    wait_idle(s)
    sysmsg = Fake.requests[-1]["messages"][0]["content"]
    assert "PROBE-SYSTEM-PROMPT-77" not in sysmsg and "helpful assistant" in sysmsg, sysmsg[:200]
    del chat


@probe("remote-context")
def p_remote_ctx(c, notes):
    s = c.s
    for bad in [12, "abc", 99999999, -5, 4096.5]:
        try:
            s.invoke("set_setting", {"key": "remoteContextLength", "value": bad})
            notes.append(f"accepted bad value {bad!r}")
        except WebDriverError:
            pass
    s.invoke("set_setting", {"key": "remoteContextLength", "value": 4096})
    chat = new_chat(c, tools=c.all_tools)
    send(s, "hi")
    wait_idle(s)
    t1 = s.page_text()
    notes.append("4096 with every tool: " + t1[-330:].replace("\n", " | "))
    last = messages(s, chat)[-1]
    notes.append(f"last: role={last['role']} status={last.get('status')} error={json.dumps(last.get('error'))[:300]}")
    s.invoke("set_setting", {"key": "remoteContextLength", "value": 16384})
    send(s, "hi again")
    wait_idle(s)
    t2 = s.page_text()
    import re
    ctx1 = re.findall(r"[\d,.]+\s*(?:/|of)\s*([\d,.]+k?)", t1)
    ctx2 = re.findall(r"[\d,.]+\s*(?:/|of)\s*([\d,.]+k?)", t2)
    notes.append(f"context shown: default {ctx1[:3]} then {ctx2[:3]}")
    s.invoke("set_setting", {"key": "remoteContextLength", "value": 32768})
    assert not any("accepted bad" in n for n in notes), notes
    del chat


@probe("folders")
def p_folders(c, notes):
    s = c.s
    chat = new_chat(c)
    work, other = str(c.work), str(c.other)
    folders = s.invoke("list_approved_folders", {"chatId": chat})
    assert [f["path"] for f in folders] == [work], folders
    try:
        s.invoke("set_chat_working_folder", {"chatId": chat, "path": other})
        raise AssertionError("an unapproved working folder was accepted")
    except WebDriverError as e:
        assert "approved" in str(e), e
    s.invoke("set_chat_working_folder", {"chatId": chat, "path": work})
    assert s.invoke("get_chat", {"chatId": chat})["chat"]["workingFolder"] == work
    # Per-chat folder in the database (the picker is native), then the panel.
    con = sqlite3.connect(c.db)
    con.execute("INSERT INTO approved_folders(path, chat_id, created_at) VALUES (?, ?, ?)", (other, chat, int(time.time() * 1000)))
    con.commit()
    con.close()
    s.click_button("Tools")
    s.wait_text("Folders for this chat", 10)
    time.sleep(0.5)
    s.screenshot(str(OUT / "tools-panel-folders.png"))
    panel = s.page_text()
    assert work in panel and other in panel, panel[-600:]
    radios = s.execute("return [...document.querySelectorAll('input[type=radio]')].map(r => ({v: r.value, c: r.checked, l: r.getAttribute('aria-label') || r.closest('label')?.innerText}));")
    notes.append("radios " + json.dumps(radios))
    assert any(r["c"] for r in radios), "no working folder selected in the panel"
    # Choose the other folder as working folder through the UI.
    s.execute(
        """
        const r = [...document.querySelectorAll('input[type=radio]')].find(r => !r.checked);
        r.click();
        """
    )
    time.sleep(0.6)
    assert s.invoke("get_chat", {"chatId": chat})["chat"]["workingFolder"] == other
    # A relative path now resolves in the working folder.
    Fake.queue.append({"tool_calls": [{"name": "list_dir", "arguments": {"path": "."}}]})
    Fake.queue.append({"content": "listed"})
    s.execute("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}));")
    time.sleep(0.3)
    send(s, "list")
    s.wait_for(lambda: "Allow once" in s.page_text() or idle(s), 30, "card or done")
    card = s.page_text()
    if "Allow once" in card:
        assert other in card, card[-500:]
        time.sleep(0.9)
        s.click_button("Allow once")
    wait_idle(s)
    last_tool = [m for m in messages(s, chat) if m["role"] == "tool"][-1]
    notes.append("list_dir result: " + json.dumps(last_tool)[:300])
    assert "only-in-other.txt" in json.dumps(last_tool), "relative path did not use the chosen working folder"
    # Removing the chat's folder makes the stored working folder stale: the first approved one is used.
    fid = [f for f in s.invoke("list_approved_folders", {"chatId": chat}) if f["path"] == other][0]["id"]
    s.invoke("remove_approved_folder", {"id": fid})
    Fake.queue.append({"tool_calls": [{"name": "list_dir", "arguments": {"path": "."}}]})
    Fake.queue.append({"content": "listed again"})
    send(s, "list again")
    s.wait_for(lambda: "Allow once" in s.page_text() or idle(s), 30, "card or done")
    if "Allow once" in s.page_text():
        time.sleep(0.9)
        s.click_button("Allow once")
    wait_idle(s)
    last_tool = json.dumps([m for m in messages(s, chat) if m["role"] == "tool"][-1])
    assert "notes.txt" in last_tool and "only-in-other" not in last_tool, last_tool[:400]


@probe("skills-unload")
def p_skills(c, notes):
    s = c.s
    skill = "---\nname: probe-skill\ndescription: Probe skill. Use when asked about the probe.\n---\n\nPROBE-SKILL-BODY-991\n"
    problems = s.invoke("save_skill", {"folder": "probe-skill", "text": skill, "isNew": True})
    assert not problems, problems
    chat = new_chat(c)
    Fake.queue.append({"tool_calls": [{"name": "load_skill", "arguments": {"name": "probe-skill"}}]})
    Fake.queue.append({"content": "loaded it"})
    Fake.requests.clear()
    send(s, "load the probe skill")
    wait_idle(s)
    assert "Allow once" not in s.page_text(), "skill load asked for approval (D11)"
    assert s.invoke("loaded_skills", {"chatId": chat}) == ["probe-skill"]
    index = Fake.requests[0]["messages"][0]["content"]
    assert "probe-skill" in index and "PROBE-SKILL-BODY-991" not in index
    Fake.requests.clear()
    send(s, "next")
    wait_idle(s)
    assert "PROBE-SKILL-BODY-991" in Fake.requests[-1]["messages"][0]["content"], "loaded skill text is not in the system message"
    s.click_button("Tools")
    s.wait_text("Skills loaded in this chat", 10)
    s.screenshot(str(OUT / "tools-panel-skills.png"))
    s.click_button("Unload probe-skill")
    time.sleep(0.6)
    assert s.invoke("loaded_skills", {"chatId": chat}) == []
    s.execute("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}));")
    time.sleep(0.3)
    Fake.requests.clear()
    send(s, "after unload")
    wait_idle(s)
    assert "PROBE-SKILL-BODY-991" not in Fake.requests[-1]["messages"][0]["content"], "unloaded skill still in the system message"
    audit = s.invoke("list_audit", {"filter": {"chatId": chat}})
    notes.append("audit decisions: " + json.dumps([(a["tool"], a["decision"]) for a in audit]))
    s.invoke("delete_skill", {"name": "probe-skill"})


@probe("skill-folder-guard")
def p_skill_guard(c, notes):
    s = c.s
    chat = new_chat(c)
    target = str(c.skills_root / "evil" / "SKILL.md")
    Fake.queue.append({"tool_calls": [{"name": "write_file", "arguments": {"path": target, "content": "---\nname: evil\ndescription: x\n---\nhi"}}]})
    Fake.queue.append({"content": "done"})
    send(s, "write a skill")
    time.sleep(1.0)
    s.wait_for(lambda: "Allow once" in s.page_text() or idle(s), 30, "card or done")
    if "Allow once" in s.page_text():
        notes.append("a card was shown for a write into the skills folder")
        time.sleep(0.9)
        s.click_button("Allow once")
    wait_idle(s)
    assert not (c.skills_root / "evil").exists(), "a skill was written by the file tool"
    tool = json.dumps([m for m in messages(s, chat) if m["role"] == "tool"][-1])
    notes.append("result: " + tool[:300])


@probe("terminal-git")
def p_terminal_git(c, notes):
    s = c.s
    chat = new_chat(c, tools=c.all_tools)
    repo = c.work / "repo"
    if not repo.exists():
        repo.mkdir()
        env = dict(os.environ, GIT_CONFIG_GLOBAL="/dev/null", GIT_AUTHOR_NAME="p", GIT_AUTHOR_EMAIL="p@p", GIT_COMMITTER_NAME="p", GIT_COMMITTER_EMAIL="p@p")
        for cmd in [["git", "init", "-q", "-b", "main"], ["git", "commit", "-q", "--allow-empty", "-m", "init"],
                    ["git", "remote", "add", "origin", "https://user:SECRETTOKEN123@example.invalid/r.git"]]:
            subprocess.run(cmd, cwd=repo, check=True, env=env)
    # Terminal: a plain command with a card.
    Fake.queue.append({"tool_calls": [{"name": "run_command", "arguments": {"command": "echo probe-$((40+2))", "cwd": str(c.work)}}]})
    Fake.queue.append({"content": "ran"})
    send(s, "run it")
    answer_card(s, "Allow once")
    wait_idle(s)
    out = json.dumps([m for m in messages(s, chat) if m["role"] == "tool"][-1])
    assert "probe-42" in out, out[:400]
    # A call that does not run says why on its card.
    Fake.queue.append({"tool_calls": [{"name": "git_push", "arguments": {"path": str(repo)}}]})
    Fake.queue.append({"content": "hm"})
    # The message names git: only tools that fit the task are shown to the model (0.0.4).
    send(s, "git push, with a bad argument")
    wait_idle(s)
    bad = [m for m in messages(s, chat) if m["role"] == "tool"][-1]["toolResult"]
    assert bad["status"] == "error" and "Invalid arguments" in bad["text"], bad
    s.execute("[...document.querySelectorAll('button[aria-expanded]')].filter(b => b.innerText.includes('git_push')).pop().click();")
    time.sleep(0.4)
    assert "Invalid arguments for git_push" in s.page_text(), "the card does not show why the call failed"
    s.screenshot(str(OUT / "failed-card.png"))
    # A destructive command always asks and offers no chat-wide grant.
    Fake.queue.append({"tool_calls": [{"name": "run_command", "arguments": {"command": "rm -rf ./nothing-here", "cwd": str(c.work)}}]})
    Fake.queue.append({"content": "ok"})
    send(s, "delete")
    s.wait_for(lambda: "Allow once" in s.page_text(), 30, "card")
    card = s.page_text()
    assert "Allow for this chat" not in card, "a delete offered a chat-wide grant"
    time.sleep(0.9)
    s.click_button("Deny")
    wait_idle(s)
    # git push: the card and everything stored must not show the token.
    Fake.queue.append({"tool_calls": [{"name": "git_push", "arguments": {"repo": str(repo)}}]})
    Fake.queue.append({"content": "pushed?"})
    Fake.requests.clear()
    send(s, "push")
    try:
        s.wait_for(lambda: "Allow once" in s.page_text(), 15, "push card")
    except WebDriverError:
        notes.append("NO CARD; result: " + json.dumps([m for m in messages(s, chat) if m["role"] == "tool"][-1]["toolResult"])[:900])
        raise
    card = s.page_text()
    s.screenshot(str(OUT / "push-card.png"))
    assert "SECRETTOKEN123" not in card, "token on the push card"
    assert "example.invalid" in card, card[-400:]
    assert "Allow for this chat" not in card
    time.sleep(0.9)
    s.click_button("Allow once")
    wait_idle(s, 90)
    dump = json.dumps(messages(s, chat)) + json.dumps(s.invoke("list_audit", {"filter": {"chatId": chat}})) + json.dumps(Fake.requests)
    assert "SECRETTOKEN123" not in dump, "token reached the chat, the audit log or the prompt"
    notes.append("push result: " + json.dumps([m for m in messages(s, chat) if m["role"] == "tool"][-1])[:300])


@probe("errors")
def p_errors(c, notes):
    s = c.s
    chat = new_chat(c, tools=[])
    for item, label in [({"status": 500, "message": "boom"}, "500"), ({"status": 400, "message": "tools are not supported"}, "400"),
                        ({"status": 401, "message": "bad key"}, "401"), ({"content": "partial reply that is cut " * 20, "drop_after": 64}, "dropped"),
                        ({"content": "ends cleanly but never says it finished " * 3, "no_finish": True}, "no-finish")]:
        Fake.queue.clear()
        Fake.queue.append(item)
        before = len(messages(s, chat))
        send(s, f"try {label}")
        wait_idle(s, 90)
        time.sleep(0.5)
        msgs = messages(s, chat)
        last = msgs[-1]
        notes.append(f"{label}: new={[(m['role'], m.get('status'), len(m.get('text') or '')) for m in msgs[before:]]}; error={json.dumps(last.get('error'))[:260]}")
        new = msgs[before:]
        assert [m["role"] for m in new] == ["user", "assistant"], f"{label}: {[(m['role'], m.get('status')) for m in new]}"
        if label in ("500", "400", "401", "dropped"):
            assert last.get("status") == "error", last
            step_text = last["error"]["nextStep"]
            assert "Settings → Endpoints" in step_text, step_text
            assert "model is still running" not in step_text, step_text
        if label == "401":
            assert "API key" in last["error"]["nextStep"], last["error"]
        if label == "dropped":
            assert "partial reply" in (last.get("text") or ""), "the text streamed before the drop was lost"
        if label == "no-finish":
            assert last.get("status") == "complete" and "cut short" in s.page_text(), s.page_text()[-300:]
    s.screenshot(str(OUT / "errors.png"))
    # The chat still works afterwards.
    Fake.queue.clear()
    Fake.queue.append({"content": "recovered fine"})
    send(s, "now work")
    wait_idle(s)
    assert "recovered fine" in s.page_text()


@probe("stop-and-regenerate")
def p_stop(c, notes):
    s = c.s
    chat = new_chat(c, tools=[])
    Fake.queue.append({"content": "slow " * 400, "chunk": 5, "delay": 0.05})
    send(s, "stream slowly")
    s.wait_for(lambda: not idle(s), 20, "streaming")
    time.sleep(1.0)
    s.click_button("Stop")
    wait_idle(s, 20)
    last = messages(s, chat)[-1]
    notes.append(f"after stop: status={last.get('status')}")
    assert last.get("status") == "stopped", last
    Fake.queue.append({"content": "second version"})
    s.click_button("Regenerate")
    wait_idle(s)
    assert "second version" in s.page_text()
    assert "2 / 2" in s.page_text().replace(" ", " "), "no version arrows after regenerate"


@probe("mcp-config")
def p_mcp(c, notes):
    s = c.s
    cfg = {"mcpServers": {"a b": {"command": "true", "env": {"K": "v1"}}, "a_b": {"command": "true", "env": {"K": "v2"}}}}
    try:
        s.invoke("add_mcp_servers", {"json": json.dumps(cfg), "names": ["a b", "a_b"], "moveCredentials": True})
        raise AssertionError("two servers that share secret names were added")
    except WebDriverError as e:
        assert "similar" in str(e), e
    assert not [x for x in s.invoke("list_mcp_servers") if x["name"] in ("a b", "a_b")], "a server was added before the refusal"
    stolen = {"mcpServers": {"thief": {"command": "true", "env": {"X": "${secret:other.env.TOKEN}"}}}}
    for cmd, args in [("preview_mcp_config", {"json": json.dumps(stolen)}),
                      ("add_mcp_servers", {"json": json.dumps(stolen), "names": ["thief"], "moveCredentials": True})]:
        try:
            s.invoke(cmd, args)
            raise AssertionError(f"{cmd} accepted a secret placeholder")
        except WebDriverError:
            pass
    # A real stdio server: python one-liner speaking MCP is too long; a server that exits at once
    # must end in a clear failed state, not hang.
    bad = {"mcpServers": {"exits": {"command": "false"}}}
    s.invoke("add_mcp_servers", {"json": json.dumps(bad), "names": ["exits"], "moveCredentials": True})
    time.sleep(4)
    row = [x for x in s.invoke("list_mcp_servers") if x["name"] == "exits"][0]
    notes.append("exits server: " + json.dumps(row)[:300])
    s.invoke("remove_mcp_server", {"name": "exits"})


@probe("regenerate-after-error")
def p_regen_error(c, notes):
    s = c.s
    chat = new_chat(c, tools=[])
    Fake.queue.append({"status": 500, "message": "boom"})
    send(s, "first")
    wait_idle(s)
    assert messages(s, chat)[-1]["status"] == "error"
    Fake.queue.append({"content": "worked the second time"})
    s.click_button("Regenerate")
    wait_idle(s)
    msgs = messages(s, chat)
    notes.append(str([(m["role"], m.get("status")) for m in msgs]))
    assert msgs[-1]["status"] == "complete" and "worked the second time" in s.page_text()
    assert [m["role"] for m in msgs] == ["user", "assistant"], msgs
    # Edit and resend the user message.
    Fake.queue.append({"content": "answer to the edited one"})
    Fake.requests.clear()
    s.click_button("Edit and resend")
    time.sleep(0.3)
    box = s.execute("return [...document.querySelectorAll('textarea')].find(t => t.value === 'first') || null;")
    assert box, "no edit box with the old text"
    bid = next(iter(box.values()))
    s.type(bid, " edited")
    s.click_button("Send")
    wait_idle(s)
    assert "answer to the edited one" in s.page_text(), s.page_text()[-300:]
    users = [m["content"] for m in Fake.requests[-1]["messages"] if m["role"] == "user"]
    notes.append("user messages sent: " + json.dumps(users)[:200])
    assert len(users) == 1 and "first edited" in json.dumps(users), users


@probe("switch-during-stream")
def p_switch(c, notes):
    s = c.s
    chat = new_chat(c, tools=[])
    Fake.queue.append({"content": "streamword " * 120, "chunk": 11, "delay": 0.04})
    send(s, "long one")
    s.wait_for(lambda: "streamword" in s.page_text(), 20, "streaming")
    s.click_button("New chat")
    time.sleep(0.8)
    t = s.page_text()
    notes.append("other chat while streaming shows stream text: " + str("streamword streamword" in t))
    assert "streamword streamword" not in t, "another chat shows the first chat's stream"
    # The composer in the other chat while a turn runs elsewhere.
    notes.append("stop button visible in other chat: " + str(not idle(s)))
    time.sleep(6)
    # Back to the first chat.
    s.execute(
        """
        const a = [...document.querySelectorAll('nav[aria-label="Chat history"] button')].find(b => b.innerText.includes('long one'));
        a && a.click();
        """
    )
    time.sleep(1.0)
    wait_idle(s, 30)
    last = messages(s, chat)[-1]
    assert last["status"] == "complete" and last["text"].count("streamword") == 120, (last["status"], last["text"].count("streamword"))
    assert s.page_text().count("streamword") >= 120, "the finished reply is not shown after coming back"


@probe("delete-during-stream")
def p_delete(c, notes):
    s = c.s
    chat = new_chat(c, tools=[])
    Fake.queue.append({"content": "doomed " * 200, "chunk": 7, "delay": 0.04})
    send(s, "to be deleted")
    s.wait_for(lambda: "doomed" in s.page_text(), 20, "streaming")
    s.invoke("delete_chat", {"chatId": chat})
    time.sleep(2.5)
    ids = [x["id"] for x in s.invoke("list_chats", {"query": None})]
    assert chat not in ids
    # The app is still usable.
    chat2 = new_chat(c, tools=[])
    Fake.queue.clear()
    Fake.queue.append({"content": "alive after delete"})
    send(s, "hello")
    wait_idle(s, 40)
    assert "alive after delete" in s.page_text(), s.page_text()[-300:]
    del chat2


@probe("reasoning")
def p_reasoning(c, notes):
    s = c.s
    chat = new_chat(c, tools=[])
    Fake.queue.append({"reasoning": "Let me think about this carefully. " * 3, "content": "The answer is 7."})
    send(s, "think")
    wait_idle(s)
    last = messages(s, chat)[-1]
    notes.append("keys: " + ",".join(sorted(last.keys())))
    assert last["text"] == "The answer is 7.", last["text"]
    assert "think about this carefully" in json.dumps(last), "the thinking was not stored"
    assert "The answer is 7." in s.page_text()
    # Thinking is not sent back to the server on the next request.
    Fake.requests.clear()
    Fake.queue.append({"content": "ok"})
    send(s, "next")
    wait_idle(s)
    assert "think about this carefully" not in json.dumps(Fake.requests[-1]), "old thinking was sent back to a remote server"


@probe("text-tool-call-and-grants")
def p_text_call(c, notes):
    s = c.s
    chat = new_chat(c)
    path = str(c.work / "notes.txt")
    # A native call, allowed for the chat.
    Fake.queue.append({"tool_calls": [{"name": "read_file", "arguments": {"path": path}}]})
    Fake.queue.append({"content": "read it"})
    send(s, "read")
    answer_card(s, "Allow for this chat")
    wait_idle(s)
    # The same call now runs under the grant.
    Fake.queue.append({"tool_calls": [{"name": "read_file", "arguments": {"path": path}}]})
    Fake.queue.append({"content": "read it again"})
    send(s, "read again")
    wait_idle(s)
    assert "Allow once" not in s.page_text()
    # A call written as text is recovered and always asks, grant or not.
    # A read written as text may use the grant (13.3 step 4); a write written as text always asks,
    # even after a native write was allowed for the chat.
    wpath = str(c.work / "w.txt")
    Fake.queue.append({"tool_calls": [{"name": "write_file", "arguments": {"path": wpath, "content": "one"}}]})
    Fake.queue.append({"content": "wrote"})
    send(s, "write")
    answer_card(s, "Allow for this chat")
    wait_idle(s)
    Fake.queue.append({"content": '<tool_call>{"name": "write_file", "arguments": {"path": "%s", "content": "two"}}</tool_call>' % wpath})
    Fake.queue.append({"content": "wrote again"})
    send(s, "write as text")
    s.wait_for(lambda: "Allow once" in s.page_text() or idle(s), 30, "card or done")
    card = s.page_text()
    assert "Allow once" in card, "a recovered write ran without asking"
    assert "Allow for this chat" not in card, "a recovered call offered a chat-wide grant"
    assert "<tool_call>" not in card, "tool-call markup shown as the answer"
    time.sleep(0.9)
    s.click_button("Deny")
    wait_idle(s)
    assert open(wpath).read() == "one", "a denied write changed the file"
    # Forget the grant in the Tools panel: the next native call asks again.
    s.click_button("Tools")
    s.wait_text("Forget everything I allowed in this chat", 10)
    s.click_button("Forget everything I allowed in this chat")
    time.sleep(0.5)
    s.execute("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}));")
    time.sleep(0.3)
    Fake.queue.append({"tool_calls": [{"name": "read_file", "arguments": {"path": path}}]})
    Fake.queue.append({"content": "fourth read"})
    send(s, "read once more")
    s.wait_for(lambda: "Allow once" in s.page_text() or idle(s), 30, "card or done")
    assert "Allow once" in s.page_text(), "the grant survived Forget"
    time.sleep(0.9)
    s.click_button("Deny")
    wait_idle(s)
    audit = [(a["tool"], a["decision"]) for a in s.invoke("list_audit", {"filter": {"chatId": chat}})]
    notes.append("audit: " + json.dumps(audit))


@probe("subagent")
def p_subagent(c, notes):
    s = c.s
    assert "run_subagent" in c.all_tools, "the tool is not registered"
    assert "run_subagent" not in c.default_tools, "Subagents are on in a new chat"
    chat = new_chat(c)
    path = str(c.work / "notes.txt")
    # Something earlier in the chat that the helper must never be sent.
    Fake.queue.append({"content": "PARENT-EARLIER-ANSWER"})
    send(s, "PARENT-EARLIER-QUESTION")
    wait_idle(s)
    # Switch the group on in the Tools panel.
    s.click_button("Tools")
    s.wait_text("Subagents", 10)
    panel = s.page_text()
    assert "hand a task to a helper that works in its own context" in panel, panel[-600:]
    on = s.execute(
        """
        const label = [...document.querySelectorAll('section > label')].find(l => l.innerText.trim().startsWith('Subagents'));
        const box = label.querySelector('input');
        const before = box.checked;
        box.click();
        label.scrollIntoView();
        return before;
        """
    )
    assert on is False, "the Subagents group was already on"
    time.sleep(0.6)
    s.screenshot(str(OUT / "tools-panel-subagents.png"))
    tools = {t["exposed"]: t for t in s.invoke("list_chat_tools", {"chatId": chat})}
    assert tools["run_subagent"]["enabledInChat"] and tools["run_subagent"]["server"] == "subagents", tools["run_subagent"]
    s.execute("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}));")
    time.sleep(0.3)

    # 1. The turn hands a task over; the helper reads a file (a card), answers; the turn answers.
    task = "Read the notes file and report the secret word. HELPER-TASK-771"
    Fake.requests.clear()
    Fake.queue.append({"tool_calls": [{"name": "run_subagent", "arguments": {"task": task, "context": f"The file is {path}"}}]})
    # The helper's requests report a prompt of 7,777 tokens: that must never become the chat's.
    Fake.queue.append({"tool_calls": [{"name": "read_file", "arguments": {"path": path}}], "prompt_tokens": 7777})
    Fake.queue.append({"content": "the secret word is heron", "prompt_tokens": 7777})
    Fake.queue.append({"content": "PARENT-FINAL: the helper found the word."})
    send(s, "use a helper")
    s.wait_for(lambda: "Allow once" in s.page_text(), 30, "the helper's approval card")
    card = s.page_text()
    s.screenshot(str(OUT / "subagent-card.png"))
    assert "Subagent: " in card, "the card does not say it comes from a subagent: " + card[-500:]
    assert "Waiting for you" in card, "the run_subagent tool card does not show it is waiting"
    assert not any(n in card for n in ("7,777", "7,787")), "the helper's token count is shown as the chat's"
    assert f"read_file {path}"[:60] in card, "the tool card does not show what the helper is doing"
    time.sleep(0.9)
    s.click_button("Allow once")
    wait_idle(s)
    reqs = list(Fake.requests)
    assert len(reqs) == 4, len(reqs)
    names = lambda r: [t["function"]["name"] for t in r.get("tools", [])]  # noqa: E731
    roles = lambda r: [m["role"] for m in r["messages"]]  # noqa: E731
    assert "run_subagent" in names(reqs[0]) and names(reqs[0])[-1] == "run_subagent", names(reqs[0])
    # The helper's first request: a fresh history, the same system message, no run_subagent.
    first = reqs[1]
    assert roles(first) == ["system", "user"], roles(first)
    assert first["messages"][0]["content"] == reqs[0]["messages"][0]["content"], "the helper has another system message"
    user = json.dumps(first["messages"][1]["content"])
    assert task in user and path in user and "[App note]" in user, user[:400]
    assert "PARENT-EARLIER" not in json.dumps(first) and "use a helper" not in json.dumps(first), "the helper was sent the chat"
    assert "run_subagent" not in names(first) and "read_file" in names(first), names(first)
    assert names(first) == names(reqs[0])[:-1], "the helper's tools are not the chat's minus run_subagent"
    assert first["model"] == reqs[0]["model"]
    # Its second request carries the file; the turn's next request only the answer.
    assert "heron" in json.dumps(reqs[2]["messages"][-1]) and roles(reqs[2])[:2] == ["system", "user"]
    last = reqs[3]["messages"][-1]
    assert last["role"] == "tool" and "the secret word is heron" in json.dumps(last), last
    assert task in json.dumps(reqs[3]["messages"]) and "PARENT-EARLIER-QUESTION" in json.dumps(reqs[3]["messages"])
    assert roles(reqs[3]).count("tool") == 1, "a helper message reached the chat's history"
    # The chat stores the turn's own messages only.
    msgs = messages(s, chat)
    turn = msgs[2:]
    notes.append("stored: " + json.dumps([(m["role"], [tc["name"] for tc in m.get("toolCalls") or []]) for m in turn]))
    assert [m["role"] for m in turn] == ["user", "assistant", "tool", "assistant"], [m["role"] for m in turn]
    assert [tc["name"] for m in turn for tc in (m.get("toolCalls") or [])] == ["run_subagent"]
    result = turn[2]["toolResult"]
    assert result["status"] == "ok" and result["text"] == "the secret word is heron", result
    assert "PARENT-FINAL" in turn[3]["text"]
    # The context numbers of the chat are the turn's own, not the helper's.
    assert "7777" not in json.dumps(msgs), "the helper's token count was stored on a chat message"
    header = s.execute("return document.querySelector('header')?.innerText || document.body.innerText.slice(0, 400);")
    notes.append("header: " + header.replace("\n", " | ")[:160])
    page = s.page_text()
    assert not any(n in page for n in ("7,777", "7777", "7,787", "7787")), "the helper's token count is shown as the chat's: " + header
    # The card shows the answer.
    s.execute("[...document.querySelectorAll('button[aria-expanded]')].filter(b => b.innerText.includes('run_subagent')).pop().click();")
    time.sleep(0.4)
    page = s.page_text()
    s.screenshot(str(OUT / "subagent-done.png"))
    assert "Result | the secret word is heron" in page.replace("\n", " | "), page[-500:]
    # Both calls are in the audit log; the helper's is marked and points at the turn's.
    audit = s.invoke("list_audit", {"filter": {"chatId": chat}})
    notes.append("audit: " + json.dumps([(a["tool"], a["decision"], a["result_status"], a["preview"][:40], a.get("parent_audit_id")) for a in audit]))
    run_row = [a for a in audit if a["tool"] == "run_subagent"]
    read_row = [a for a in audit if a["tool"] == "read_file"]
    assert len(run_row) == 1 and len(read_row) == 1, [a["tool"] for a in audit]
    assert run_row[0]["decision"] == "allowed_without_prompt" and run_row[0]["result_status"] == "ok", run_row[0]
    assert run_row[0]["server"] == "subagents" and "heron" in (run_row[0]["result_summary"] or ""), run_row[0]
    assert read_row[0]["decision"] == "allow_once" and read_row[0]["preview"].startswith("Subagent: "), read_row[0]
    assert read_row[0]["parent_audit_id"] == run_row[0]["id"], (read_row[0], run_row[0]["id"])

    # 2. A helper that tries to start a helper is refused.
    Fake.requests.clear()
    Fake.queue.append({"tool_calls": [{"name": "run_subagent", "arguments": {"task": "Delegate once more."}}]})
    Fake.queue.append({"tool_calls": [{"name": "run_subagent", "arguments": {"task": "NESTED-TASK-552"}}]})
    Fake.queue.append({"content": "I could not delegate, so I did it myself."})
    Fake.queue.append({"content": "PARENT-SECOND-ANSWER"})
    send(s, "let the helper try to delegate")
    wait_idle(s)
    reqs = list(Fake.requests)
    assert len(reqs) == 4, len(reqs)
    refusal = reqs[2]["messages"][-1]
    assert refusal["role"] == "tool" and "No such tool: run_subagent" in json.dumps(refusal), refusal
    assert all("NESTED-TASK-552" not in json.dumps(r["messages"][1]) for r in reqs[1:3]), "a nested helper was started"
    assert "I could not delegate" in json.dumps(reqs[3]["messages"][-1])
    audit = s.invoke("list_audit", {"filter": {"chatId": chat}})
    nested = [a for a in audit if a["tool"] == "run_subagent" and a["preview"].startswith("Subagent: ")]
    assert len(nested) == 1 and nested[0]["decision"] == "refused_by_policy", [(a["tool"], a["decision"], a["preview"][:30]) for a in audit]
    assert len([a for a in audit if a["tool"] == "run_subagent"]) == 3, "a nested helper has its own audit row"

    # 3. Stop while the helper streams ends the whole turn.
    Fake.requests.clear()
    Fake.queue.append({"tool_calls": [{"name": "run_subagent", "arguments": {"task": "Talk for a long time."}}]})
    Fake.queue.append({"content": "helperword " * 400, "chunk": 11, "delay": 0.05})
    send(s, "a slow helper")
    s.wait_for(lambda: "helperword" in s.page_text(), 30, "the helper's text on the tool card")
    time.sleep(0.5)
    started = time.time()
    s.click_button("Stop")
    s.wait_for(lambda: idle(s), 10, "the turn to stop")
    took = time.time() - started
    notes.append(f"stop took {took:.2f}s")
    assert took < 3.0, f"Stop took {took:.1f}s"
    time.sleep(0.6)
    msgs = messages(s, chat)
    assert msgs[-1]["role"] == "assistant" and msgs[-1].get("status") == "stopped", msgs[-1]
    stopped = [m for m in msgs if m["role"] == "tool"][-1]["toolResult"]
    assert stopped["status"] == "cancelled" and "The helper was stopped" in stopped["text"], stopped
    assert "helperword" in stopped["text"], "what the helper had so far was lost"
    before = len(Fake.requests)
    time.sleep(1.0)
    assert len(Fake.requests) == before == 2, f"requests after Stop: {len(Fake.requests)}"
    audit = s.invoke("list_audit", {"filter": {"chatId": chat}})
    newest = [a for a in audit if a["tool"] == "run_subagent"][0]
    assert newest["result_status"] == "cancelled", newest
    # The chat still works.
    Fake.queue.clear()
    Fake.queue.append({"content": "fine after the stopped helper"})
    send(s, "still there?")
    wait_idle(s)
    assert "fine after the stopped helper" in s.page_text()
    assert "PARENT-EARLIER-QUESTION" in json.dumps(Fake.requests[-1]["messages"])


@probe("step-limit")
def p_step_limit(c, notes):
    """Reported from a real PC: a long task stopped at "the step limit" with a red error
    and no way on. The limit is a setting now, and the stop offers Continue."""
    s = c.s
    for bad in (0, 501, "50"):
        try:
            s.invoke("set_setting", {"key": "stepsPerTurn", "value": bad})
            raise AssertionError(f"a step limit of {bad!r} was accepted")
        except WebDriverError as e:
            assert "whole number from 5 to 500" in str(e), e
    s.invoke("set_setting", {"key": "stepsPerTurn", "value": 5})
    try:
        chat = new_chat(c)
        s.invoke("set_chat_working_folder", {"chatId": chat, "path": str(c.work)})
        for i in range(5):
            (c.work / f"step{i}").mkdir(exist_ok=True)
            Fake.queue.append({"tool_calls": [{"name": "list_dir", "arguments": {"path": f"step{i}"}}]})
        Fake.queue.append({"content": "I listed five folders. One is left."})
        Fake.requests.clear()
        send(s, "list every folder")
        deadline = time.time() + 90
        while time.time() < deadline:
            time.sleep(0.4)
            if "Allow once" in s.page_text():
                time.sleep(0.9)
                s.click_button("Allow for this chat")
            elif idle(s):
                break
        # Five model calls, then the wrap-up request in which tools are off.
        assert len(Fake.requests) == 6, len(Fake.requests)
        assert Fake.requests[-1].get("tool_choice") == "none", Fake.requests[-1].get("tool_choice")
        notice = s.execute("const n = document.querySelector('[role=status]'); return n ? n.innerText : null;")
        notes.append("notice: " + str(notice).replace("\n", " | "))
        assert notice and "step limit" in notice and "Continue" in notice, notice
        assert s.execute("return document.querySelector('[role=alert]') === null"), "the stop is shown as an error"
        assert messages(s, chat)[-1]["text"] == "I listed five folders. One is left."
        s.screenshot(str(OUT / "step-limit.png"))
        Fake.queue.append({"content": "The last one is listed too."})
        s.click_button("Continue")
        wait_idle(s)
        msgs = [m for m in messages(s, chat) if m["role"] in ("user", "assistant")]
        assert msgs[-2]["text"] == "Continue where you left off.", msgs[-2]
        assert msgs[-1]["text"] == "The last one is listed too.", msgs[-1]
        assert "step limit" not in s.page_text(), "the notice stayed after Continue"
        # The setting shows in Settings, General.
        s.click_button("Settings")
        s.click_button("General")
        time.sleep(0.5)
        radios = "[...document.querySelectorAll('[aria-label=\"Steps per turn\"] [role=radio]')]"
        chosen = s.execute(f"return {radios}.filter(b => b.getAttribute('aria-checked') === 'true').map(b => b.innerText);")
        assert chosen == ["5"], chosen
        s.execute(f"{radios}.find(b => b.innerText === '100').click();")
        s.wait_for(lambda: s.invoke("get_setting", {"key": "stepsPerTurn"}) == 100, 10, "the choice is saved")
    finally:
        s.invoke("set_setting", {"key": "stepsPerTurn", "value": 50})


@probe("allow-all")
def p_allow_all(c, notes):
    """Owner's request for 0.0.4: "an allow all button so it doesn't ask for approval all the
    time (deleting any type of file or files still needs approval no matter what, but not
    code changes or file editing)"."""
    s = c.s
    files = ["read_file", "list_dir", "search_files", "write_file", "edit_file", "delete_file"]
    chat = new_chat(c, tools=[t for t in files if t in c.all_tools])
    folder = c.work / "allow-all"
    shutil.rmtree(folder, ignore_errors=True)
    folder.mkdir()
    a, b, kept = folder / "a.txt", folder / "b.txt", folder / "kept.txt"
    kept.write_text("keep me\n")

    def call(name, arguments):
        # A tool that is on but not shown this turn is run through call_tool (tool selection).
        offered = [t.get("function", {}).get("name") for t in (Fake.requests[-1].get("tools") or [])] if Fake.requests else []
        if name in offered or "call_tool" not in offered:
            return {"tool_calls": [{"name": name, "arguments": arguments}]}
        return {"tool_calls": [{"name": "call_tool", "arguments": {"name": name, "arguments": arguments}}]}

    def card_or_done():
        s.wait_for(lambda: "Allow once" in s.page_text() or idle(s), 30, "card or done")
        return "Allow once" in s.page_text()

    def no_card_until_done():
        time.sleep(0.4)
        deadline = time.time() + 60
        while time.time() < deadline:
            assert "Allow once" not in s.page_text(), "a card was shown in Allow all"
            if idle(s):
                return
            time.sleep(0.2)
        raise AssertionError("the turn did not finish")

    def click_in_confirmation(label):
        # The confirmation is the open dialog; a card behind it can carry the same label.
        time.sleep(0.9)
        done = s.execute(
            """
            const d = document.querySelector('[role=alertdialog][data-state=open]');
            const b = d && [...d.querySelectorAll('button')].find(x => x.innerText.trim() === arguments[0]);
            if (b) b.click();
            return !!b;
            """,
            label,
        )
        assert done, f"no {label!r} in the confirmation"
        time.sleep(0.4)

    def mode():
        return s.invoke("get_approval_mode", {"chatId": chat})

    def audit():
        return [(r["tool"], r["decision"]) for r in s.invoke("list_audit", {"filter": {"chatId": chat}})]

    try:
        assert s.invoke("get_allow_all_default") is False, "Allow all in every chat is on in a fresh install"
        assert mode() is False, "a new chat is not in Ask mode"
        Fake.queue.append({"content": "ready"})
        send(s, "hello")
        wait_idle(s)

        # 1. Ask first (the default): a write shows a card.
        s.wait_for(lambda: s.execute("return !!document.querySelector('button[aria-label=\"Ask first\"]');"), 10, "the Ask first button")
        Fake.queue.append(call("write_file", {"path": str(a), "content": "one\n"}))
        Fake.queue.append({"content": "not written"})
        send(s, "go on")
        assert card_or_done(), "a write ran without asking in Ask mode"
        card = s.page_text()
        assert "Allow for this chat" in card and "Allow all in this chat" in card, card[-400:]
        time.sleep(0.9)
        s.click_button("Deny")
        wait_idle(s)
        assert not a.exists(), "a denied write created the file"

        # Only the user's own command switches the mode: the generic settings command cannot.
        for key in ("allowAllDefault", "approvalMode", "allowAll"):
            try:
                s.invoke("set_setting", {"key": key, "value": True})
                raise AssertionError(f"set_setting accepted {key}")
            except WebDriverError as e:
                assert "cannot be changed this way" in str(e), e
        assert s.invoke("get_allow_all_default") is False and mode() is False

        # 2. The header button asks first, listing what still asks.
        s.click_button("Ask first")
        s.wait_text("Allow all in this chat?", 10)
        dialog = s.execute(
            "const d = document.querySelector('[role=alertdialog][data-state=open]'); return d ? d.innerText : null;")
        (OUT / "allow-all-dialog.txt").write_text(dialog or "")
        s.screenshot(str(OUT / "allow-all-dialog.png"))
        for needle in ("It still asks before:", "Deleting files or folders", "git push", "Cancel"):
            assert needle in dialog, (needle, dialog)
        assert mode() is False, "opening the confirmation switched the mode"
        click_in_confirmation("Cancel")
        assert mode() is False, "Cancel switched the mode"
        s.click_button("Ask first")
        s.wait_text("Allow all in this chat?", 10)
        click_in_confirmation("Allow all in this chat")
        s.wait_for(mode, 10, "the chat in Allow all")
        s.wait_for(lambda: s.execute("return !!document.querySelector('button[aria-label=\"Allow all\"]');"), 10, "the Allow all button")
        s.screenshot(str(OUT / "allow-all-on.png"))

        # 3. A write and an edit run with no card, and are audited as allowed by the mode.
        Fake.queue.append(call("write_file", {"path": str(a), "content": "one\n"}))
        Fake.queue.append(call("edit_file", {"path": str(a), "old_text": "one", "new_text": "two"}))
        Fake.queue.append({"content": "written and edited"})
        send(s, "go on")
        no_card_until_done()
        assert a.read_text() == "two\n", a.read_text() if a.exists() else "a.txt was not written"
        rows = audit()
        notes.append("audit: " + json.dumps(rows[:4]))
        assert ("write_file", "allowed_by_mode") in rows and ("edit_file", "allowed_by_mode") in rows, rows

        # 4. A delete still shows a card, with Allow once and Deny only.
        Fake.queue.append(call("delete_file", {"path": str(kept)}))
        Fake.queue.append({"content": "kept"})
        send(s, "go on")
        assert card_or_done(), "a delete ran without asking in Allow all"
        card = s.page_text()
        assert "this still asks" in card and "deletes files" in card, card[-500:]
        assert "Allow for this chat" not in card and "Allow all in this chat" not in card, card[-500:]
        s.screenshot(str(OUT / "allow-all-delete-card.png"))
        time.sleep(0.9)
        s.click_button("Deny")
        wait_idle(s)
        assert kept.read_text() == "keep me\n", "a denied delete removed the file"
        assert ("delete_file", "denied") in audit(), audit()

        # The audit log in Settings words it for people.
        s.click_button("Settings")
        s.click_button("Audit log")
        s.wait_text("allowed by Allow all", 10)
        s.execute("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}));")
        time.sleep(0.4)

        # 5. Switching back takes one click, and a write asks again.
        s.click_button("Allow all")
        s.wait_for(lambda: mode() is False, 10, "the chat back in Ask mode")
        assert "Allow all in this chat?" not in s.page_text(), "switching back asked for confirmation"
        Fake.queue.append(call("write_file", {"path": str(b), "content": "three\n"}))
        Fake.queue.append({"content": "written"})
        send(s, "go on")
        assert card_or_done(), "a write ran without asking after switching back"

        # The card's third choice: confirm, and this card is approved and the chat switched.
        time.sleep(0.9)
        s.click_button("Allow all in this chat")
        s.wait_text("Allow all in this chat?", 10)
        assert not b.exists() and mode() is False, "the card's button acted before the confirmation"
        click_in_confirmation("Allow all in this chat")
        wait_idle(s)
        assert b.read_text() == "three\n"
        assert mode() is True
        assert ("write_file", "allow_once") in audit(), audit()

        # "Forget everything I allowed in this chat" ends Allow all in the chat too.
        s.click_button("Tools")
        s.wait_text("Forget everything I allowed in this chat", 10)
        s.click_button("Forget everything I allowed in this chat")
        s.wait_for(lambda: mode() is False, 10, "the chat back in Ask mode after Forget")
        s.wait_for(lambda: s.execute("return !!document.querySelector('button[aria-label=\"Ask first\"]');"), 10,
                   "the Ask first button after Forget")
        s.execute("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}));")
        time.sleep(0.3)

        # "Allow all in every chat" in Settings: asks first, and is a saved setting.
        s.invoke("set_approval_mode", {"chatId": chat, "allowAll": False})
        s.click_button("Settings")
        s.click_button("Permissions")
        s.wait_text("Allow all in every chat", 10)
        s.execute("document.querySelector('input[role=switch]').click();")
        s.wait_text("Allow all in every chat?", 10)
        assert s.invoke("get_allow_all_default") is False
        click_in_confirmation("Allow all in every chat")
        s.wait_for(lambda: s.invoke("get_allow_all_default") is True, 10, "the default switched on")
        assert mode() is True, "an open chat did not follow the default"
        assert s.invoke("get_setting", {"key": "allowAllDefault"}) is True
        s.execute("document.querySelector('input[role=switch]').click();")
        s.wait_for(lambda: s.invoke("get_allow_all_default") is False, 10, "the default switched off")
        assert mode() is False, "switching the default off left a chat in Allow all"
        s.execute("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}));")
    finally:
        s.invoke("set_allow_all_default", {"on": False})
        s.invoke("set_approval_mode", {"chatId": chat, "allowAll": False})


@probe("tool-selection")
def p_tool_selection(c, notes):
    """Adaptive tool selection: a turn shows the model a small set of the tools that are on,
    `find_tools` finds the others and `call_tool` runs them exactly like a direct call (the
    card and the audit row are the real tool's), a tool that is off stays out of reach, and
    the list only grows from turn to turn."""
    s = c.s
    names = lambda r: [t["function"]["name"] for t in r.get("tools", [])]  # noqa: E731
    estimate = lambda r: round(len(json.dumps(r.get("tools", []))) / 3.5)  # noqa: E731

    def listing(chat_id):
        return {t["exposed"]: t for t in s.invoke("list_chat_tools", {"chatId": chat_id})}

    def tool_results(chat_id):
        return [m["toolResult"] for m in messages(s, chat_id) if m["role"] == "tool"]

    def in_order(earlier, later):
        rest = iter(later)
        return all(any(x == y for y in rest) for x in earlier)

    # The catalog covers every built-in tool this app has registered, with examples the
    # tools' own schemas accept (the unit tests can only build some of the servers).
    problems = s.invoke("tool_catalog_problems")
    assert problems == [], "tool catalog: " + json.dumps(problems)
    assert s.invoke("get_tool_selection") == {"auto": True, "max": 14}

    repo = c.work / "selection-repo"
    if not repo.exists():
        repo.mkdir()
        env = dict(os.environ, GIT_CONFIG_GLOBAL="/dev/null", GIT_AUTHOR_NAME="p", GIT_AUTHOR_EMAIL="p@p",
                   GIT_COMMITTER_NAME="p", GIT_COMMITTER_EMAIL="p@p")
        for cmd in [["git", "init", "-q", "-b", "main"], ["git", "commit", "-q", "--allow-empty", "-m", "init"]]:
            subprocess.run(cmd, cwd=repo, check=True, env=env)

    try:
        # Files, terminal and git on; everything else off.
        chat = new_chat(c, tools=[])
        registered = listing(chat)
        wanted = [n for n, t in registered.items() if t["server"] in ("files", "terminal", "git")]
        assert len(wanted) == 14, wanted
        s.invoke("set_chat_tools", {"chatId": chat, "enabled": wanted + ["find_tools", "call_tool"]})
        tools = listing(chat)
        companions = [n for n, t in tools.items() if t["companion"]]
        assert "find_tools" in companions and "call_tool" in companions, companions
        assert all(tools[n]["enabledInChat"] for n in companions), "a companion is off although tools are on"
        on = sorted(n for n, t in tools.items() if t["enabledInChat"] and not t["companion"])
        assert on == sorted(wanted), on
        assert tools["read_file"]["shown"] and not tools["git_status"]["shown"], (tools["read_file"], tools["git_status"])
        core = ["list_dir", "read_file", "search_files", "write_file", "edit_file"]

        # 1. A request about a file: the core and the companions, no git tool.
        Fake.requests.clear()
        Fake.queue.append({"tool_calls": [{"name": "find_tools", "arguments": {"query": "commit"}}]})
        Fake.queue.append({"tool_calls": [{"name": "call_tool", "arguments": {"name": "git_status", "arguments": {"repo": str(repo)}}}]})
        Fake.queue.append({"content": "SELECTION-FIRST-ANSWER"})
        send(s, "Please read the notes file and tell me what it says")
        s.wait_for(lambda: "Allow once" in s.page_text() or idle(s), 30, "the approval card of git_status")
        card = s.page_text()
        s.screenshot(str(OUT / "tool-selection-card.png"))
        first = names(Fake.requests[0])
        notes.append("first request: " + json.dumps(first))
        for name in core + companions:
            assert name in first, f"{name} is not in the first request: {first}"
        assert not any(n.startswith("git_") for n in first), f"a git tool was shown: {first}"
        assert "run_command" not in first and "delete_file" not in first, first
        assert len(first) <= 14, first
        # `find_tools` ran without a card and told the model how to reach git_commit.
        found = json.dumps(Fake.requests[1]["messages"][-1])
        assert Fake.requests[1]["messages"][-1]["role"] == "tool" and "git_commit" in found and "call_tool" in found, found[:600]
        # 2. The card is git_status's own: its name, its server, its summary.
        assert "Allow once" in card, "call_tool ran git_status without a card: " + card[-400:]
        assert "git_status · git" in card, "the card does not name the real tool: " + card[-500:]
        assert "call_tool ·" not in card and "toolfinder" not in card, "the card names call_tool: " + card[-500:]
        assert "selection-repo" in card, card[-500:]
        time.sleep(0.9)
        s.click_button("Allow once")
        wait_idle(s)
        reqs = list(Fake.requests)
        assert len(reqs) == 3, len(reqs)
        assert all(names(r) == first for r in reqs), "the tool list changed within the turn"
        results = tool_results(chat)
        find_text = results[0]["text"]
        notes.append("find_tools commit: " + find_text.replace("\n", " | "))
        (OUT / "find_tools-commit.txt").write_text(find_text)
        assert "git_commit [Git]: " in find_text, find_text
        assert 'Example arguments: {"message": "Fix the date parser"}' in find_text, find_text
        assert 'Run it with call_tool: name "git_commit"' in find_text, find_text
        assert results[1]["status"] == "ok" and "main" in results[1]["text"], results[1]
        audit = s.invoke("list_audit", {"filter": {"chatId": chat}})
        rows = [(a["tool"], a["server"], a["decision"]) for a in audit]
        notes.append("audit: " + json.dumps(rows))
        assert ("git_status", "git", "allow_once") in rows, rows
        assert ("find_tools", "toolfinder", "allowed_without_prompt") in rows, rows
        assert not any(tool == "call_tool" for tool, _, _ in rows), "a call_tool row hides the real tool: " + json.dumps(rows)
        status_row = [a for a in audit if a["tool"] == "git_status"][0]
        assert json.loads(json.dumps(status_row["arguments"])) in ({"repo": str(repo)}, json.dumps({"repo": str(repo)})), status_row
        # Nothing was switched on by finding or calling.
        after = listing(chat)
        assert sorted(n for n, t in after.items() if t["enabledInChat"] and not t["companion"]) == sorted(wanted)

        # 3. The next turn shows what was found and used, added to the same list.
        Fake.requests.clear()
        Fake.queue.append({"content": "SELECTION-SECOND-ANSWER"})
        send(s, "thanks")
        wait_idle(s)
        second = names(Fake.requests[-1])
        notes.append("second request: " + json.dumps(second))
        assert "git_commit" in second and "git_status" in second, f"what was found is not shown: {second}"
        assert set(first) <= set(second) and in_order(first, second), f"the list did not only grow: {first} then {second}"
        assert len(second) <= 14, second
        assert listing(chat)["git_commit"]["shown"], "the Tools panel does not mark a found tool as shown"

        # 4. A tool that is off cannot be reached, and call_tool cannot call itself.
        target = c.work / "selection-nested.txt"
        Fake.queue.append({"tool_calls": [{"name": "call_tool", "arguments": {"name": "browser_navigate", "arguments": {"url": "https://example.com"}}}]})
        Fake.queue.append({"tool_calls": [{"name": "call_tool", "arguments": {"name": "call_tool", "arguments": {
            "name": "write_file", "arguments": {"path": str(target), "content": "x"}}}}]})
        Fake.queue.append({"content": "SELECTION-THIRD-ANSWER"})
        send(s, "try the page anyway")
        deadline = time.time() + 60
        while time.time() < deadline and not idle(s):
            assert "Allow once" not in s.page_text(), "a card was shown for a tool that is off, or for a nested call"
            time.sleep(0.3)
        wait_idle(s)
        refused = tool_results(chat)[-2:]
        notes.append("refusals: " + json.dumps([r["text"][:160] for r in refused]))
        assert refused[0]["status"] == "error" and "switched off" in refused[0]["text"], refused[0]
        assert refused[1]["status"] == "error" and "cannot run call_tool" in refused[1]["text"], refused[1]
        assert not target.exists(), "a nested call_tool wrote a file"
        assert not listing(chat)["browser_navigate"]["enabledInChat"], "a call switched the browser on"
        newest = s.invoke("list_audit", {"filter": {"chatId": chat}})[:2]
        assert all(a["tool"] == "call_tool" and a["decision"] == "refused_by_policy" for a in newest), newest

        # 5. The Tools panel: marks, the companions in one line, the count of tools that are on.
        s.click_button("Tools")
        s.wait_text("Choose tools for each task automatically", 10)
        time.sleep(0.4)
        panel = s.page_text()
        s.screenshot(str(OUT / "tools-panel-selection.png"))
        assert "on request" in panel and "shown" in panel, panel[-900:]
        assert "Always included with the tools above" in panel and "find_tools" in panel, panel[-900:]
        assert "Tools (14)" in panel, "the header does not count the tools that are on"
        # 6. The switch: off shows every tool that is on, and the finder tools are gone.
        s.execute("document.querySelector('input[role=switch]').click();")
        s.wait_for(lambda: s.invoke("get_tool_selection")["auto"] is False, 10, "the setting to be saved")
        time.sleep(0.4)
        assert "on request" not in s.page_text(), "a tool is still marked on request with the selection off"
        s.execute("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}));")
        time.sleep(0.3)
        off = listing(chat)
        assert not off["find_tools"]["enabledInChat"] and not off["call_tool"]["enabledInChat"], "the finder tools are offered with nothing hidden"
        Fake.requests.clear()
        Fake.queue.append({"content": "SELECTION-ALL-ANSWER"})
        send(s, "hello again")
        wait_idle(s)
        everything = names(Fake.requests[-1])
        notes.append("selection off: " + json.dumps(everything))
        for name in wanted:
            assert name in everything, f"{name} is on but was not sent with the selection off"
        assert "find_tools" not in everything and "call_tool" not in everything, everything
        notes.append(f"files+terminal+git: {len(everything)} tools, about {estimate(Fake.requests[-1])} tokens, with the selection off; "
                     f"{len(first)} tools, about {estimate(reqs[0])} tokens, on the first turn with it on")

        # 7. What the default tools of a new chat cost with and without the selection (for the notes).
        defaults = [n for n, t in registered.items()
                    if t["server"] in ("files", "skills", "artifacts", "search", "browser") and t["available"]]
        measured = {}
        for auto in (False, True):
            s.invoke("set_tool_selection", {"auto": auto})
            fresh = new_chat(c, tools=defaults)
            Fake.requests.clear()
            Fake.queue.append({"content": "ok"})
            send(s, "hello")
            wait_idle(s)
            measured[auto] = (len(names(Fake.requests[-1])), estimate(Fake.requests[-1]), len(json.dumps(Fake.requests[-1].get("tools", []))))
            del fresh
        notes.append(f"default tools of a new chat ({len(defaults)} switches): all sent: {measured[False][0]} tools, {measured[False][2]} characters, "
                     f"about {measured[False][1]} tokens; chosen: {measured[True][0]} tools, {measured[True][2]} characters, about {measured[True][1]} tokens")
        assert measured[True][0] < measured[False][0] or measured[False][0] <= 14, measured
    finally:
        s.invoke("set_tool_selection", {"auto": True})


PROBES = [p_markdown, p_settings, p_general, p_remote_ctx, p_folders, p_skills, p_skill_guard, p_terminal_git, p_errors, p_stop, p_mcp,
          p_regen_error, p_switch, p_delete, p_reasoning, p_text_call, p_subagent, p_step_limit, p_allow_all,
          p_tool_selection]


def main() -> int:
    home = OUT / "home"
    shutil.rmtree(home, ignore_errors=True)
    home.mkdir(parents=True)
    c = Ctx()
    c.work = OUT / "work"
    c.other = OUT / "other"
    for d in (c.work, c.other):
        shutil.rmtree(d, ignore_errors=True)
        d.mkdir()
    (c.work / "notes.txt").write_text("the secret word is heron\n")
    (c.other / "only-in-other.txt").write_text("x\n")
    env = dict(os.environ)
    env.update(HOME=str(home), XDG_DATA_HOME=str(home / "data"), XDG_CONFIG_HOME=str(home / "config"),
               XDG_CACHE_HOME=str(home / "cache"), APP_DEV_LLAMA_DIR=LLAMA_DIR, RUST_LOG="info")
    fake_port = free_port()
    fake = ThreadingHTTPServer(("127.0.0.1", fake_port), Fake)
    threading.Thread(target=fake.serve_forever, daemon=True).start()
    port = free_port()
    driver = subprocess.Popen(
        ["tauri-driver", "--port", str(port), "--native-driver", shutil.which("WebKitWebDriver") or "WebKitWebDriver"],
        env=env, stdout=open(OUT / "tauri-driver.log", "w"), stderr=subprocess.STDOUT)
    s = None
    try:
        time.sleep(1.5)
        s = Session(f"http://127.0.0.1:{port}", {"tauri:options": {"application": APP}})
        c.s = s
        s.wait_text("Welcome to", 60)
        s.execute(HOOK)
        s.click_button("Skip for now")
        s.wait_text("New chat", 20)
        c.db = home / "data" / IDENTIFIER / "app.db"
        c.skills_root = home / "data" / IDENTIFIER / "skills"
        con = sqlite3.connect(c.db)
        con.execute("INSERT INTO approved_folders(path, chat_id, created_at) VALUES (?, NULL, ?)", (str(c.work), int(time.time() * 1000)))
        con.commit()
        con.close()
        p = s.invoke("save_provider", {"provider": {"id": None, "name": "Fake", "kind": "generic",
                                                     "baseUrl": f"http://127.0.0.1:{fake_port}/v1", "apiKey": "sk-test", "clearApiKey": False}})
        models = s.invoke("test_provider", {"id": p["id"]})
        c.model_ref = models[0]["ref"]
        first = s.invoke("create_chat", {"modelRef": c.model_ref})
        c.default_tools = [t["exposed"] for t in s.invoke("list_chat_tools", {"chatId": first["id"]}) if t["enabledInChat"]]
        c.all_tools = [t["exposed"] for t in s.invoke("list_chat_tools", {"chatId": first["id"]})]
        print("default tools:", c.default_tools, flush=True)
        # Unknown context on a remote endpoint: the default must be 4,096.
        s.invoke("set_chat_tools", {"chatId": first["id"], "enabled": c.all_tools})
        s.invoke("set_setting", {"key": "remoteContextLength", "value": 32768})
        for fn in PROBES:
            run(fn, c)
    finally:
        if s:
            try:
                s.screenshot(str(OUT / "final.png"))
            except Exception:  # noqa: BLE001
                pass
            s.quit()
        driver.terminate()
        fake.shutdown()
    print("\n===== SUMMARY")
    for name, status, notes, console in RESULTS:
        print(f"{status}  {name}  ({len(console)} console lines)")
    failed = [name for name, status, _, _ in RESULTS if status != "PASS"]
    if failed or not RESULTS:
        print("SCRIPTED AGENT FAIL:", ", ".join(failed) or "nothing ran")
        return 1
    print("SCRIPTED AGENT PASS")
    print(f"artifacts in {OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

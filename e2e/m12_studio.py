#!/usr/bin/env python3
"""0.1.0 Studio in the real app against a mock OpenRouter: a ThreadingHTTPServer on
http://127.0.0.1:<port>/api/v1 that answers as openrouter.ai does, from the recorded lists
(crates/agent/tests/fixtures/media/), with one text model for Test, one picture
(usage.cost 0.048) and one video job that stays pending until the app has restarted
(usage.cost 0.36), as the mock of src-tauri/src/studio/wire_tests.rs (Task D1) answers.

The checks, each PASS or FAIL (the run goes on; the exit code is 1 if any failed):
- Studio opens with "On this computer" first, and the empty state this computer gets
  (*Get models* when a recommended picture model fits here, else cloud as the backup);
- an OpenRouter endpoint at the mock, added in Settings → Endpoints: Test fetches the lists;
- Ctrl+G opens Studio; picking FLUX 3 Image fetches its price; a 1K picture ($0.05) is made
  without a question and one tile appears;
- with *Ask before every video* turned on (every spending guard is off until turned on), a 4 s
  video asks "Make this video?" first, and the card waits;
- the app restarts while the video waits. On Linux the app keeps API keys in memory only, so
  the endpoint has no key after the restart and the card says so, with *Get it anyway*; the
  key is saved again, the job completes at the mock and *Get it anyway* fetches it: one
  `POST /api/v1/videos` in all, its result fetched only after the restart;
- Save as with the settings off, then Delete: the copy is there without settings, the
  result's file is gone;
- Settings → Audit log shows three Studio rows (the picture, the video, and the follow-up of
  *Get it anyway*), none with the prompt;
- with *Daily limit* turned on and set to $1, an 8 s 1080p video (~ $1.12) shows "Daily limit reached: $0.41
  of $1 spent today" in place of Create (the deleted result still counts).

The native save dialog is out of WebDriver's reach: it is answered with xdotool
(`answer_save_dialog`).

    python3 -I e2e/m12_studio.py
"""

import base64
import json
import os
import pathlib
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import traceback
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "e2e"))
from webdriver import Session, WebDriverError  # noqa: E402

APP = os.environ.get("APP_BIN", str(ROOT / "target" / "debug" / "app"))
LLAMA_DIR = os.environ.get("APP_DEV_LLAMA_DIR", str(ROOT / ".cache" / "llama-linux"))
OUT = pathlib.Path(os.environ.get("E2E_OUT", tempfile.mkdtemp(prefix="e2e-m12-")))
FIXTURES = ROOT / "crates" / "agent" / "tests" / "fixtures" / "media"

FLUX = "black-forest-labs/flux-3-image"
ENDPOINT_NAME = "My OpenRouter"
KEY = "sk-or-v1-e2e-test-key-0123456789"
PICTURE_PROMPT = "a red kite over a grey sea, seen from the cliffs"
VIDEO_PROMPT = "waves rolling onto a pebble beach at dusk"
LIMIT_PROMPT = "a slow pan across a misty valley at sunrise"
JOB = "vid_1"
JOB_PATH = f"/api/v1/videos/{JOB}"
CONTENT = f"{JOB_PATH}/content"

RESULTS = []


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def png(width: int, height: int) -> bytes:
    """A PNG of one colour."""

    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    rows = b"".join(b"\x00" + bytes([200, 120, 40]) * width for _ in range(height))
    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")


def mp4() -> bytes:
    """The start of an MP4 file, enough for a type check: an `ftyp` box and an empty `mdat`."""
    return struct.pack(">I", 24) + b"ftypisom" + struct.pack(">I", 0x200) + b"isomiso2" + struct.pack(">I", 8) + b"mdat"


# ---- the mock ---------------------------------------------------------------------------------


class Mock(BaseHTTPRequestHandler):
    """openrouter.ai on this computer. Every request is recorded (its method and path, without
    the query); the video job is pending until `video_ready` is set, then completed. The answers
    that finish a run (the picture, the completed job) take `LATENCY` seconds, as a real
    company's do, so a run never ends before the press that started it has its answer."""

    LATENCY = 1.0

    requests: list[tuple[str, str]] = []
    video_ready = False
    lock = threading.Lock()
    images = (FIXTURES / "openrouter-images-2026-11-17.json").read_bytes()
    videos = (FIXTURES / "openrouter-videos-2026-11-17.json").read_bytes()
    endpoints = json.loads((FIXTURES / "openrouter-image-endpoints-2026-11-17.json").read_text())["answers"]

    def log_message(self, *a):
        pass

    @classmethod
    def count(cls, method: str, path: str) -> int:
        with cls.lock:
            return sum(1 for m, p in cls.requests if m == method and p == path)

    def _reply(self, status: int, body: bytes, kind: str = "application/json"):
        self.send_response(status)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
        self.close_connection = True

    def _json(self, status: int, value):
        self._reply(status, json.dumps(value).encode())

    def _answer(self):
        length = int(self.headers.get("Content-Length") or 0)
        if length:
            self.rfile.read(length)
        url = urlsplit(self.path)
        with Mock.lock:
            Mock.requests.append((self.command, url.path))
        base = f"http://{self.headers.get('Host')}"
        route = (self.command, url.path)
        if route == ("GET", "/api/v1/models"):
            # Test lists the chat models first, and stops there when that fails.
            self._json(200, {"data": [{"id": "test/text-model", "name": "Test: Text Model", "context_length": 8192,
                                       "architecture": {"output_modalities": ["text"]}}]})
        elif route == ("GET", "/api/v1/images/models"):
            self._reply(200, Mock.images)
        elif route == ("GET", "/api/v1/videos/models"):
            self._reply(200, Mock.videos)
        elif route == ("GET", f"/api/v1/images/models/{FLUX}/endpoints"):
            self._json(200, Mock.endpoints[FLUX])
        elif route == ("POST", "/api/v1/images"):
            time.sleep(Mock.LATENCY)
            picture = base64.b64encode(png(8, 8)).decode()
            self._json(200, {"data": [{"b64_json": picture, "media_type": "image/png"}], "usage": {"cost": 0.048}})
        elif route == ("POST", "/api/v1/videos"):
            self._json(202, {"id": JOB, "polling_url": f"{base}{JOB_PATH}", "status": "pending"})
        elif route == ("GET", JOB_PATH):
            if Mock.video_ready:
                time.sleep(Mock.LATENCY)
                self._json(200, {"id": JOB, "status": "completed", "unsigned_urls": [f"{base}{CONTENT}?index=0"],
                                 "usage": {"cost": 0.36}})
            else:
                self._json(200, {"id": JOB, "status": "pending"})
        elif route == ("GET", CONTENT) and url.query == "index=0":
            self._reply(200, mp4(), "video/mp4")
        else:
            self._json(404, {"error": {"message": "Not found", "code": 404}})

    do_GET = do_POST = _answer


# ---- driving the app ----------------------------------------------------------------------------


def app_pids() -> list[str]:
    return subprocess.run(["pgrep", "-f", "-x", APP], capture_output=True, text=True).stdout.split()


def start_app(port: int) -> Session:
    return Session(f"http://127.0.0.1:{port}", {"tauri:options": {"application": APP}})


def visible(s, css: str) -> bool:
    return s.execute(
        "const e = document.querySelector(arguments[0]); return !!e && e.getClientRects().length > 0;", css
    )


def press_ctrl(s, key: str):
    """Ctrl + `key` as real key presses."""
    ctrl = "\ue009"
    s._req("POST", s._s("/actions"), {"actions": [{"type": "key", "id": "keyboard", "actions": [
        {"type": "keyDown", "value": ctrl}, {"type": "keyDown", "value": key},
        {"type": "keyUp", "value": key}, {"type": "keyUp", "value": ctrl}]}]})
    s._req("DELETE", s._s("/actions"))


def type_into(s, css: str, text: str):
    """Replaces what the box `css` holds with `text`, as typed keys."""
    box = s.find(css)
    # Ctrl+A, then the modifiers let go, then Backspace.
    s.type(box, "\ue009a\ue000\ue003")
    s.type(box, text)


def set_select(s, css: str, value: str):
    """Chooses `value` in the select `css` as a person would (React sees the change)."""
    s.execute(
        """
        const [css, value] = arguments;
        const el = document.querySelector(css);
        const set = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set;
        set.call(el, value);
        el.dispatchEvent(new Event('change', {bubbles: true}));
        """,
        css,
        value,
    )


def pointer_click_found(s, script: str, *args, what: str):
    """A real pointer click on the element `script` returns (with `args`)."""

    def find():
        return s.execute(script, *args) or False

    el = s.wait_for(find, 15, what)
    s._pointer_click(find, el)


def settings_tab(s, tab: str):
    """Opens section `tab` of the open Settings dialog, found among the dialog's own tabs and
    given a real pointer click (the sidebar's Studio button carries the same name, under the
    dialog's overlay)."""
    pointer_click_found(
        s,
        """
        return [...document.querySelectorAll(
          '[role=dialog] [role=tablist][aria-label="Settings sections"] [role=tab]')]
          .find(t => t.innerText.trim() === arguments[0]) || null;
        """,
        tab,
        what=f"settings tab {tab!r}",
    )
    s.wait_for(
        lambda: s.execute(
            "return [...document.querySelectorAll('[role=dialog] [role=tab][aria-selected=true]')]"
            ".some(t => t.innerText.trim() === arguments[0]);",
            tab,
        ),
        10,
        f"settings page {tab!r}",
    )


def open_settings(s, tab: str):
    s.click_button("Settings")
    s.wait_for(lambda: s.execute("return !!document.querySelector('[aria-label=\"Settings sections\"]');"), 10,
               "Settings open")
    settings_tab(s, tab)


def close_settings(s):
    s.execute("document.querySelector('[role=dialog] button[aria-label=\"Close settings\"]').click();")
    s.wait_for(lambda: s.execute("return !document.querySelector('[aria-label=\"Settings sections\"]');"), 10,
               "Settings closed")


def set_switch(s, label: str, on: bool):
    """Turns the switch labelled `label` in the open Settings dialog on or off, with a real pointer
    click (the spending guards are all off until turned on)."""
    find = """
        const l = [...document.querySelectorAll('[role=dialog] label')]
          .find(l => l.innerText.trim() === arguments[0]);
        return (l && document.getElementById(l.htmlFor)) || null;
    """
    is_on = """
        const l = [...document.querySelectorAll('[role=dialog] label')]
          .find(l => l.innerText.trim() === arguments[0]);
        const el = l && document.getElementById(l.htmlFor);
        return el ? el.getAttribute('aria-checked') === 'true' : null;
    """
    s.wait_for(lambda: s.execute(is_on, label) is not None, 10, f"the {label!r} switch")
    if s.execute(is_on, label) != on:
        pointer_click_found(s, find, label, what=f"the {label!r} switch")
    s.wait_for(lambda: s.execute(is_on, label) == on, 10, f"the {label!r} switch {'on' if on else 'off'}")


def studio_tab(s, label: str):
    """The top bar's [Pictures | Video] tab `label`, once it is chosen."""
    script = """
        return [...document.querySelectorAll('[data-studio-view] header [role=tablist] [role=tab]')]
          .find(t => t.innerText.trim() === arguments[0]) || null;
    """
    pointer_click_found(s, script, label, what=f"Studio tab {label!r}")
    s.wait_for(lambda: s.execute(
        """
        const t = [...document.querySelectorAll('[data-studio-view] header [role=tablist] [role=tab]')]
          .find(t => t.innerText.trim() === arguments[0]);
        return !!t && t.getAttribute('aria-selected') === 'true';
        """, label), 10, f"Studio tab {label!r} chosen")


# 0.2.0 (Task U3): the composer at the foot of Studio's results holds the prompt, the model chip
# (which opens the model sheet, Task U4), Shape, How many and a video's Length as menus, Settings and
# Create; the side panel (Settings), with the model's other fields, is closed until Settings
# opens it (a slide-over below 1400 px, docked beside the results from there).
MODEL_CHIP = "return document.querySelector('[data-composer] [data-model-chip] button');"


def pick_model(s, name: str):
    """Opens the model sheet from the composer's model chip and chooses the model named `name`
    (its row, or its Recent tile)."""
    pointer_click_found(s, MODEL_CHIP, what="the model chip")
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


def open_studio_settings(s):
    """Studio's side panel, opened from the composer's Settings, once it shows."""
    if not visible(s, "[data-settings-panel]"):
        pointer_click_found(
            s, "return document.querySelector('[data-composer] [data-settings-button]');", what="the composer's Settings"
        )
    s.wait_for(lambda: visible(s, "[data-settings-panel]"), 10, "Studio's side panel")


def close_studio_settings(s):
    """Studio's side panel closed with its own Close button (the slide-over's or the docked one's)."""
    pointer_click_found(
        s,
        """
        const panel = document.querySelector('[data-settings-panel]');
        const box = panel && panel.closest('[role=dialog], aside');
        return box ? box.querySelector('button[aria-label="Close"]') : null;
        """,
        what="the side panel's Close",
    )
    s.wait_for(lambda: s.execute("return !document.querySelector('[data-settings-panel]');"), 10,
               "Studio's side panel closed")


# The chip (a radio) `arguments[1]` of the side panel's field labelled `arguments[0]`, as `chip`.
FIND_CHIP = """
    const [field, label] = arguments;
    const group = [...document.querySelectorAll('[data-settings-panel] [role=radiogroup]')].find(g => {
      const name = document.getElementById(g.getAttribute('aria-labelledby'));
      return name && name.innerText.trim() === field;
    });
    const chip = group ? [...group.querySelectorAll('[role=radio]')].find(r => r.innerText.trim() === label) : null;
"""


def choose_chip(s, field: str, chip: str):
    """In the side panel (opened for it, and closed again so the composer is in reach), the chip
    `chip` of the field labelled `field`, once it is chosen."""
    open_studio_settings(s)
    pointer_click_found(s, FIND_CHIP + "return chip || null;", field, chip, what=f"the {field!r} chip {chip!r}")
    s.wait_for(lambda: s.execute(FIND_CHIP + "return !!chip && chip.getAttribute('aria-checked') === 'true';",
                                 field, chip), 10, f"{chip!r} chosen")
    close_studio_settings(s)


# The button of the composer's menu named `arguments[0]` ("Length: 4 s"), as `trigger`.
FIND_MENU = """
    const field = arguments[0];
    const trigger = [...document.querySelectorAll('[data-composer] [role=toolbar] button')]
      .find(b => (b.getAttribute('aria-label') || '').startsWith(field + ': '));
"""


def menu_value(s, field: str) -> str | None:
    """What the composer's menu `field` shows: "4 s" for the button named "Length: 4 s"."""
    label = s.execute(FIND_MENU + "return trigger ? trigger.getAttribute('aria-label') : null;", field)
    return label[len(field) + 2:] if label else None


def choose_menu(s, field: str, item: str):
    """In the composer's menu `field` (Shape, How many, Length), the item `item`, once chosen."""
    pointer_click_found(s, FIND_MENU + "return trigger || null;", field, what=f"the {field!r} menu")
    pointer_click_found(
        s,
        "return [...document.querySelectorAll('[role=menu] [role=menuitemradio]')]"
        ".find(i => i.innerText.trim() === arguments[0]) || null;",
        item,
        what=f"{item!r} in the {field!r} menu",
    )
    s.wait_for(lambda: menu_value(s, field) == item, 10, f"{item!r} chosen")


def write_prompt(s, text: str):
    type_into(s, "[data-composer] textarea", text)


def dock_text(s) -> str:
    """The composer's words with the privacy line under it (the card's parent holds both)."""
    return s.execute("const d = document.querySelector('[data-composer]'); return d ? d.parentElement.innerText : '';")


def create_button(s):
    return s.execute(
        """
        return [...document.querySelectorAll('[data-composer] button')]
          .find(b => b.innerText.trim() === 'Create') || null;
        """
    )


def press_create(s):
    s.wait_for(lambda: s.execute(
        """
        const b = [...document.querySelectorAll('[data-composer] button')].find(b => b.innerText.trim() === 'Create');
        return !!b && !b.disabled;
        """), 15, "Create enabled")
    pointer_click_found(
        s,
        "return [...document.querySelectorAll('[data-composer] button')].find(b => b.innerText.trim() === 'Create') || null;",
        what="Create",
    )


def cards(s) -> list[dict]:
    """The card stream's cards: each one's text and how many result tiles it shows."""
    return s.execute(
        """
        return [...document.querySelectorAll('[data-run-card]')].map(c => ({
          id: c.getAttribute('data-run-card'), text: c.innerText,
          tiles: c.querySelectorAll('[data-output-tile]').length}));
        """
    )


def card_of(s, run_id: str) -> dict | None:
    return next((c for c in cards(s) if c["id"] == run_id), None)


def runs(s) -> list[dict]:
    return s.invoke("studio_runs", {"filter": {}, "before": None, "limit": 50})["runs"]


def answer_save_dialog(path: pathlib.Path):
    """Answers the native save dialog (GTK's "Save File", which WebDriver cannot reach) as a
    person would: the file's whole path typed into its name box, then Enter. Needs xdotool."""

    def dialog():
        found = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", "^Save File$"],
                               capture_output=True, text=True).stdout.split()
        return found[-1] if found else None

    end = time.time() + 15
    window = dialog()
    while window is None and time.time() < end:
        time.sleep(0.25)
        window = dialog()
    assert window, "no save dialog"
    subprocess.run(["xdotool", "windowfocus", "--sync", window], check=True)
    time.sleep(0.5)
    subprocess.run(["xdotool", "key", "--clearmodifiers", "ctrl+a"], check=True)
    subprocess.run(["xdotool", "type", "--delay", "20", str(path)], check=True)
    subprocess.run(["xdotool", "key", "Return"], check=True)


# ---- the checks -----------------------------------------------------------------------------------


class Ctx:
    pass


def check(name):
    def deco(fn):
        fn.check_name = name
        return fn

    return deco


@check("first-open")
def c_first_open(c, notes):
    """Studio opens with "On this computer" first, and the empty state this computer gets."""
    s = c.s
    s.click_button("Studio")
    s.wait_for(lambda: visible(s, "[data-studio-view]"), 15, "Studio shown")
    local = s.invoke("studio_local_models")
    fits = [e["name"] for e in local["recommended"] if e["kind"] == "image" and e["fit"]["word"] in ("fits", "afterUnload")]
    notes.append(f"this computer: {local['hardware']}; recommended picture models that fit: {fits}")
    pointer_click_found(s, MODEL_CHIP, what="the model chip")
    first = s.wait_for(lambda: s.execute(
        """
        const sheet = document.querySelector('[data-model-sheet]');
        const heading = sheet && sheet.querySelector('section h3');
        return heading ? heading.innerText.trim() : null;
        """), 10, "the model sheet's first section")
    assert first == "On this computer", f"the model sheet's first section: {first!r}"
    s.execute("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}));")
    s.wait_for(lambda: s.execute("return !document.querySelector('[data-model-sheet]');"), 10,
               "the model sheet closed")
    main = s.execute("return document.querySelector('[data-studio-view] main').innerText;")
    if fits:
        assert "Make pictures and videos on this computer: free, and nothing is sent." in main, main
        assert s.execute(
            "return [...document.querySelectorAll('[data-studio-view] main button')]"
            ".some(b => b.innerText.trim() === 'Get models');"
        ), "no Get models button"
    else:
        assert "This computer cannot run local models well." in main, main
        assert "Open Endpoints" in main, main


@check("endpoint")
def c_endpoint(c, notes):
    """An OpenRouter endpoint at the mock, added in Settings → Endpoints; Test fetches the lists."""
    s = c.s
    open_settings(s, "Endpoints")
    s.click_button("Add an endpoint")
    s.wait_for(lambda: s.execute("return !!document.querySelector('form[aria-label=\"New endpoint\"]');"), 10,
               "the new endpoint form")
    set_select(s, "#p-kind", "openrouter")
    s.wait_for(lambda: s.execute("return document.querySelector('#p-name').value;") == "OpenRouter", 10,
               "the OpenRouter choice")
    type_into(s, "#p-name", ENDPOINT_NAME)
    type_into(s, "#p-url", c.base_url)
    type_into(s, "#p-key", KEY)
    s.click_button("Save")
    s.wait_text("Connected · 1 model(s)", 20)
    assert Mock.count("GET", "/api/v1/images/models") == 1, Mock.requests
    assert Mock.count("GET", "/api/v1/videos/models") == 1, Mock.requests
    provider = next(p for p in s.invoke("list_providers") if p["name"] == ENDPOINT_NAME)
    c.provider = provider
    notes.append(f"endpoint {provider['id']} at {provider['baseUrl']}")
    close_settings(s)


@check("picture")
def c_picture(c, notes):
    """Ctrl+G opens Studio; picking FLUX 3 Image fetches its price; a 1K picture ($0.05) is made
    without a question, and one tile appears."""
    s = c.s
    s.click_button("New chat")
    s.wait_for(lambda: not visible(s, "[data-studio-view]"), 10, "the chat shown")
    press_ctrl(s, "g")
    s.wait_for(lambda: visible(s, "[data-studio-view]"), 10, "Studio shown by Ctrl+G")
    studio_tab(s, "Pictures")
    pick_model(s, "FLUX 3 Image")
    s.wait_for(lambda: Mock.count("GET", f"/api/v1/images/models/{FLUX}/endpoints") == 1, 10, "the price fetched")
    choose_chip(s, "Resolution", "1K")
    write_prompt(s, PICTURE_PROMPT)
    s.wait_for(lambda: "~ $0.05" in dock_text(s), 15, "~ $0.05 beside Create")
    assert "Cloud: your prompt and added pictures go to 127.0.0.1" in dock_text(s), dock_text(s)
    press_create(s)
    run = s.wait_for(lambda: next((r for r in runs(s) if r["kind"] == "image"), None), 15, "the picture run")
    time.sleep(0.5)
    assert not s.execute("return !!document.querySelector('[role=alertdialog]');"), "a question was asked"
    s.wait_for(lambda: (card_of(s, run["id"]) or {}).get("tiles") == 1, 30, "one tile on the picture's card")
    assert Mock.count("POST", "/api/v1/images") == 1, Mock.requests


@check("video")
def c_video(c, notes):
    """A 4 s video asks "Make this video?" first, and its card waits."""
    s = c.s
    # Every spending guard is off on a new install: turn the question on first.
    open_settings(s, "Studio")
    set_switch(s, "Ask before every video", True)
    s.wait_for(lambda: s.invoke("studio_get_prefs")["confirmEveryVideo"] is True, 10, "the switch saved")
    close_settings(s)
    studio_tab(s, "Video")
    pick_model(s, "Grok Imagine Video 1.5 Lite")
    # The shortest length of at least 4 s is the one Studio starts with (the composer's Length).
    assert menu_value(s, "Length") == "4 s", menu_value(s, "Length")
    write_prompt(s, VIDEO_PROMPT)
    press_create(s)
    s.wait_for(lambda: s.execute(
        "const d = document.querySelector('[role=alertdialog]'); return d ? d.innerText : null;"), 15,
        "the question")
    question = s.execute("return document.querySelector('[role=alertdialog]').innerText;")
    assert "Make this video?" in question, question
    notes.append("question: " + question.replace("\n", " | "))
    pointer_click_found(
        s,
        "return [...document.querySelectorAll('[role=alertdialog] button')]"
        ".find(b => b.innerText.trim() === 'Make video') || null;",
        what="Make video",
    )
    run = s.wait_for(lambda: next((r for r in runs(s) if r["kind"] == "video"), None), 15, "the video run")
    c.video = run["id"]
    s.wait_for(lambda: Mock.count("GET", JOB_PATH) > 0, 30, "the first check on the job")
    run = next(r for r in runs(s) if r["id"] == c.video)
    assert run["status"] == "waiting", run["status"]
    assert Mock.count("POST", "/api/v1/videos") == 1, Mock.requests


@check("restart")
def c_restart(c, notes):
    """The app restarts while the video waits. The key, kept in memory on Linux, is gone: the card
    says so and offers *Get it anyway*. With the key saved again and the job completed at the
    mock, *Get it anyway* fetches it: one `POST /api/v1/videos` in all, its result fetched only
    after the restart."""
    assert Mock.count("GET", CONTENT) == 0, Mock.requests
    c.s.quit()
    deadline = time.time() + 20
    while app_pids() and time.time() < deadline:
        time.sleep(0.25)
    assert not app_pids(), f"the app is still running: {app_pids()}"
    c.s = s = start_app(c.driver_port)
    s.wait_text("New chat", 60)
    press_ctrl(s, "g")
    s.wait_for(lambda: visible(s, "[data-studio-view]"), 10, "Studio shown by Ctrl+G")
    studio_tab(s, "Video")
    no_key = f"{ENDPOINT_NAME} has no API key."

    def says_no_key():
        card = card_of(s, c.video)
        return card if card and no_key in card["text"] else None

    card = s.wait_for(says_no_key, 30, "the video card saying the key is missing")
    assert "Get it anyway" in card["text"], card["text"]
    assert Mock.count("POST", "/api/v1/videos") == 1 and Mock.count("GET", CONTENT) == 0, Mock.requests
    # The key saved again on the same endpoint (same name, kind and address), so its saved
    # lists stay; closing Settings has Studio read the endpoint again.
    open_settings(s, "Endpoints")
    s.click_button(f"Edit {ENDPOINT_NAME}")
    s.wait_for(lambda: s.execute(f"return !!document.querySelector('form[aria-label=\"Edit {ENDPOINT_NAME}\"]');"),
               10, "the endpoint's form")
    type_into(s, "#p-key", KEY)
    s.click_button("Save")
    s.wait_text("Connected · 1 model(s)", 20)
    assert next(p for p in s.invoke("list_providers") if p["id"] == c.provider["id"])["hasApiKey"]
    close_settings(s)
    Mock.video_ready = True
    pointer_click_found(
        s,
        "return [...document.querySelectorAll('[data-run-card=\"' + arguments[0] + '\"] button')]"
        ".find(b => b.innerText.trim() === 'Get it anyway') || null;",
        c.video,
        what="Get it anyway",
    )
    s.wait_for(lambda: (card_of(s, c.video) or {}).get("tiles") == 1, 60, "the video's tile")
    run = next(r for r in runs(s) if r["id"] == c.video)
    assert run["status"] == "done" and run["outputs"][0]["mime"] == "video/mp4", run
    assert Mock.count("POST", "/api/v1/videos") == 1, Mock.requests
    assert Mock.count("GET", CONTENT) == 1, Mock.requests
    c.video_file = run["outputs"][0]["path"]
    c.video_output = run["outputs"][0]["id"]


@check("save-delete")
def c_save_delete(c, notes):
    """Save as with the settings off, then Delete: the copy is there without settings, and the
    result's file is gone."""
    s = c.s
    assert pathlib.Path(c.video_file).is_file(), c.video_file
    pointer_click_found(
        s, "return document.querySelector('[data-run-card=\"' + arguments[0] + '\"] [data-output-tile] button');",
        c.video, what="the video's tile",
    )
    s.wait_for(lambda: s.execute(
        "return [...document.querySelectorAll('[role=dialog] button')].some(b => b.innerText.trim() === 'Save as…');"),
        10, "the viewer")
    settings_box = s.execute(
        """
        const label = [...document.querySelectorAll('[role=dialog] label')]
          .find(l => l.innerText.includes('Also save the settings'));
        return label.querySelector('input').checked;
        """
    )
    assert settings_box is False, "Also save the settings is ticked"
    folder = OUT / "saved"
    shutil.rmtree(folder, ignore_errors=True)
    folder.mkdir()
    target = folder / "waves.mp4"
    s.click_button("Save as…")
    answer_save_dialog(target)
    s.wait_text(f"Saved to {target}", 10)
    assert target.read_bytes() == mp4(), "the copy differs"
    assert sorted(p.name for p in folder.iterdir()) == ["waves.mp4"], list(folder.iterdir())
    pointer_click_found(
        s,
        "return [...document.querySelectorAll('[role=dialog] button')].find(b => b.innerText.trim() === 'Delete') || null;",
        what="Delete",
    )
    pointer_click_found(
        s,
        "return [...document.querySelectorAll('[role=alertdialog] button')]"
        ".find(b => b.innerText.trim() === 'Delete') || null;",
        what="Delete in the question",
    )
    s.wait_for(lambda: not pathlib.Path(c.video_file).exists(), 10, "the result's file gone")
    s.wait_for(lambda: s.execute("return !document.querySelector('[data-output-tile=\"' + arguments[0] + '\"]');",
                                 c.video_output), 10, "the result's tile gone")
    assert target.is_file(), "the copy went with the result"


@check("audit")
def c_audit(c, notes):
    """Settings → Audit log shows three Studio rows, none with the prompt."""
    s = c.s
    open_settings(s, "Audit log")
    s.wait_for(lambda: s.execute("return document.querySelectorAll('[role=dialog] tbody tr').length;") >= 3, 10,
               "the audit rows")
    rows = s.execute(
        """
        return [...document.querySelectorAll('[role=dialog] tbody tr')].map(r => ({
          cells: [...r.querySelectorAll('td')].map(td => td.innerText.trim()),
          title: (r.querySelector('td[title]') || {}).title || ''}));
        """
    )
    studio = [r for r in rows if len(r["cells"]) > 1 and r["cells"][1] in ("picture", "video")]
    notes.append("rows: " + " / ".join(" | ".join(r["cells"][1:]) for r in studio))
    assert len(studio) == 3, rows
    assert sorted(r["cells"][1] for r in studio) == ["picture", "video", "video"], studio
    assert any(r["title"].startswith("Got the stopped video with ") for r in studio), studio
    for prompt in (PICTURE_PROMPT, VIDEO_PROMPT):
        assert all(prompt not in json.dumps(r) for r in studio), studio
    everything = {"chatId": None, "tool": None, "decision": None, "since": None, "limit": 50}
    saved = [r for r in s.invoke("list_audit", {"filter": everything}) if r["server"] == "studio"]
    ended = sorted((r["tool"], r["preview"].split(" with ")[0], r["result_status"]) for r in saved)
    assert ended == [("picture", "Made a picture", "done"), ("video", "Got the stopped video", "done"),
                     ("video", "Started a video", "failed")], ended
    for prompt in (PICTURE_PROMPT, VIDEO_PROMPT):
        assert all(prompt not in json.dumps(r) for r in saved), saved
    close_settings(s)


@check("daily-limit")
def c_daily_limit(c, notes):
    """With the daily limit turned on at $1, an 8 s 1080p video (~ $1.12) shows "Daily limit reached: $0.41
    of $1 spent today" in place of Create; the deleted result still counts."""
    s = c.s
    open_settings(s, "Studio")
    set_switch(s, "Daily limit", True)  # off until turned on; it comes on at $10
    s.wait_for(lambda: s.execute(
        "return !!document.querySelector('input[aria-label=\"Daily limit in US dollars\"]');"), 10, "the limit's amount box")
    type_into(s, "input[aria-label=\"Daily limit in US dollars\"]", "1")
    s.type(s.find("input[aria-label=\"Daily limit in US dollars\"]"), "\ue007")
    s.wait_for(lambda: s.invoke("studio_get_prefs")["dailyLimitUsd"] == 1, 10, "the limit saved")
    close_settings(s)
    studio_tab(s, "Video")
    choose_chip(s, "Resolution", "1080p")
    choose_menu(s, "Length", "8 s")
    write_prompt(s, LIMIT_PROMPT)
    s.wait_for(lambda: "Daily limit reached: $0.41 of $1 spent today" in dock_text(s), 15, "the daily limit line")
    assert create_button(s) is None, "Create is still there"
    notes.append("dock: " + dock_text(s).replace("\n", " | ")[-300:])
    assert Mock.count("POST", "/api/v1/videos") == 1, Mock.requests


CHECKS = [c_first_open, c_endpoint, c_picture, c_video, c_restart, c_save_delete, c_audit, c_daily_limit]


def run(fn, c):
    print(f"--- {fn.check_name}", flush=True)
    notes = []
    try:
        fn(c, notes)
        status = "PASS"
    except Exception as e:  # noqa: BLE001
        status = "FAIL"
        notes.append("".join(traceback.format_exception_only(type(e), e)).strip()[:1500])
        notes.append("trace: " + " <- ".join(
            f"{f.name}:{f.lineno}" for f in reversed(traceback.extract_tb(e.__traceback__)[-4:])))
        try:
            c.s.screenshot(str(OUT / f"fail-{fn.check_name}.png"))
            notes.append("page: " + c.s.page_text()[-600:].replace("\n", " | "))
        except Exception:  # noqa: BLE001
            pass
    RESULTS.append((fn.check_name, status))
    print(f"    {status}", flush=True)
    for n in notes:
        print("    note:", n, flush=True)


def main() -> int:
    home = OUT / "home"
    shutil.rmtree(home, ignore_errors=True)
    home.mkdir(parents=True)
    mock_port = free_port()
    mock = ThreadingHTTPServer(("127.0.0.1", mock_port), Mock)
    threading.Thread(target=mock.serve_forever, daemon=True).start()
    env = dict(os.environ)
    env.update(HOME=str(home), XDG_DATA_HOME=str(home / "data"), XDG_CONFIG_HOME=str(home / "config"),
               XDG_CACHE_HOME=str(home / "cache"), APP_DEV_LLAMA_DIR=LLAMA_DIR, RUST_LOG="info")
    c = Ctx()
    c.base_url = f"http://127.0.0.1:{mock_port}/api/v1"
    c.driver_port = free_port()
    assert not app_pids(), f"the app is already running: {app_pids()}"
    driver = subprocess.Popen(
        ["tauri-driver", "--port", str(c.driver_port), "--native-driver",
         shutil.which("WebKitWebDriver") or "WebKitWebDriver"],
        env=env, stdout=open(OUT / "tauri-driver.log", "w"), stderr=subprocess.STDOUT)
    c.s = None
    try:
        time.sleep(1.5)
        c.s = start_app(c.driver_port)
        c.s.wait_text("Welcome to", 60)
        c.s.click_button("Skip for now")
        c.s.wait_text("New chat", 20)
        for fn in CHECKS:
            run(fn, c)
    finally:
        if c.s is not None:
            try:
                c.s.screenshot(str(OUT / "final.png"))
            except (WebDriverError, OSError):
                pass
            c.s.quit()
        driver.terminate()
        try:
            driver.wait(10)
        except subprocess.TimeoutExpired:
            driver.kill()
        mock.shutdown()
    print("\n===== SUMMARY")
    for name, status in RESULTS:
        print(f"{status}  {name}")
    print("mock requests:", *(f"{m} {p}" for m, p in Mock.requests), sep="\n  ")
    print(f"artifacts in {OUT}")
    failed = [name for name, status in RESULTS if status != "PASS"]
    if failed or len(RESULTS) != len(CHECKS):
        print("M12 STUDIO FAIL:", ", ".join(failed) or "not every check ran")
        return 1
    print("M12 STUDIO PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())

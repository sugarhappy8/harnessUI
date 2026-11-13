#!/usr/bin/env python3
"""M10 end-to-end (0.0.6 WP1b): one model's load settings, with the real small model.

Drives the backend commands the model settings panel uses, in the real app:
- a fresh install reads Automatic; the 0.0.4 upgrade rule (8,192 is Automatic, any other
  size stays and shows the one-time note on the first start only);
- `model_load_info` for the real file; overrides outside the fixed lists are refused;
  unsaved choices previewed with `model_load_info {path, overrides}` change the fit marks
  and save nothing;
- save a per-model context, Save memory and Keep in memory, load the model with no
  settings: the reported size is that context, and the launched llama-server has `-c`
  with it, `-fa on -ctk q8_0 -ctv q8_0` and `-lm mmap+mlock` (this sandbox's mlock limit
  means it proves the flag reaches the launch, not that memory is locked); without them
  the launch has none of those flags;
- change to Automatic: "Settings changed"; Apply and restart (`reload_local_model` without
  force) uses the new number; again with nothing changed is a no-op (same server process);
  Reload (force) restarts; the thinking limit needs no restart;
- while a real reply streams: Restart when it ends waits with `reloadWaiting` set and
  Cancel ends it with `load_replaced`; a second wait goes ahead when the reply is stopped;
  Stop and restart stops the reply first;
- Unload remembers the model (`unloadedPath`); Load again uses its saved settings; Reset
  to defaults; the 0.0.4 command shape `{ctx, backend, device}` still loads as given, and
  a size outside 512 to 1,048,576 tokens is refused;
- the generic `set_setting` (the one command a tool could ever reach) refuses the defaults
  for all models and a model's own key (D12);
- settings saved while the model loads show "Settings changed" once it is ready;
- removing the loaded model from Installed cancels a change that waits for it, so it is
  not loaded again; removing the unloaded model forgets it (`unloadedPath`);
- the backend check counts as a model check: a model picked meanwhile waits for it (or
  stops it with Stop and restart) and is the one loaded afterwards; with nothing loaded,
  "Unloaded to free memory" is as it was after the check; the backend check itself, asked
  to wait ("Restart when it ends") while the model is busy, waits and stops nothing, and the
  waiting line's Cancel ends it with nothing checked, also when pressed at once (the check
  starts its wait before it probes the backends) while a real reply streams; a reply that
  begins while the check probes the backends (made slow for that step) is waited for before
  the check's first test load, never cut off, and Cancel on that wait leaves the model as it
  is and still gives the report of the devices.

The small model sometimes ends the long reply after a few words; a step that needs the
reply to still stream then tries again with a new reply in a new chat, and a change that
went ahead is accepted only when the reply had ended by itself.

The D3 question in the window itself is WP2's scenario (`model-settings`).

    python3 -I e2e/m10_model_settings.py
"""

import hashlib
import json
import os
import pathlib
import shlex
import shutil
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "e2e"))
from webdriver import Session, WebDriverError  # noqa: E402

APP = os.environ.get("APP_BIN", str(ROOT / "target" / "debug" / "app"))
MODEL = os.environ.get("TEST_GGUF") or str(next(iter(sorted((ROOT / ".cache" / "models").glob("*.gguf"))), ""))
LLAMA_DIR = os.environ.get("APP_DEV_LLAMA_DIR", str(ROOT / ".cache" / "llama-linux"))
OUT = pathlib.Path(os.environ.get("E2E_OUT", tempfile.mkdtemp(prefix="e2e-m10-")))
IDENTIFIER = "io.github.sugarhappy8.harnessui"
# The app runs the pinned llama-server through a small wrapper in this run's folder: while
# SLOW_PROBES exists, its device listing (the backend probe) takes 6 s, as on a computer
# whose graphics drivers start slowly. Only the step that needs a reply to begin while the
# backend check probes creates that file.
WRAPPED_LLAMA = OUT / "llama"
SLOW_PROBES = OUT / "slow-probes"

# Starts a command without waiting for it: the result lands in `window[slot]`.
FIRE = """
const [slot, cmd, args] = arguments;
window[slot] = 'pending';
window.__TAURI_INTERNALS__.invoke(cmd, args)
  .then(v => { window[slot] = {ok: true, value: v === undefined ? null : v}; })
  .catch(e => { window[slot] = {ok: false, error: e}; });
"""

# Sends a chat message the way the composer does, with a channel for the streamed events;
# the characters of reply text streamed so far are counted in `window.__text`.
SEND = """
const [chatId, text, parentId] = arguments;
window.__text = 0;
window.__sent = 'pending';
const id = window.__TAURI_INTERNALS__.transformCallback((raw) => {
  try {
    let m = raw && raw.message;
    if (typeof m === 'string') m = JSON.parse(m);
    if (m && m.type === 'content') window.__text += (m.text || '').length;
  } catch (e) { /* not a turn event */ }
});
window.__TAURI_INTERNALS__.invoke('send_message', {chatId, text, parentId, onEvent: '__CHANNEL__:' + id})
  .then(() => { window.__sent = {ok: true}; })
  .catch(e => { window.__sent = {ok: false, error: e}; });
"""

LONG_REPLY = "Write a story of about 1,500 words about a lighthouse keeper and a long storm. /no_think"

# The chat of the reply started last.
REPLY = {"chat": None}


def wrap_llama_dir() -> pathlib.Path:
    """The folder the app takes llama-server from: wrappers that run the pinned programs."""
    real = pathlib.Path(LLAMA_DIR).resolve()
    WRAPPED_LLAMA.mkdir(parents=True, exist_ok=True)
    SLOW_PROBES.unlink(missing_ok=True)
    for name in ("llama-server", "llama-fit-params"):
        if not (real / name).exists():
            continue
        slow = ""
        if name == "llama-server":
            slow = (
                'for a in "$@"; do\n'
                f'  if [ "$a" = "--list-devices" ] && [ -e {shlex.quote(str(SLOW_PROBES))} ]; then /bin/sleep 6; break; fi\n'
                "done\n"
            )
        script = WRAPPED_LLAMA / name
        script.write_text(f'#!/bin/sh\n{slow}exec {shlex.quote(str(real / name))} "$@"\n')
        script.chmod(0o755)
    return WRAPPED_LLAMA


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def step(name):
    print(f"--- {name}", flush=True)


def error_of(s, cmd, args):
    """Calls `cmd` and returns its error object; fails if it succeeds."""
    s.execute(FIRE, "__refused", cmd, args)
    result = fired(s, "__refused", 120)
    assert not result.get("ok"), f"{cmd} {args} was accepted: {result}"
    return result.get("error")


def server_processes(home: pathlib.Path):
    """The llama-server processes this run's app started: (pid, argument list)."""
    found = []
    for proc in pathlib.Path("/proc").iterdir():
        if not proc.name.isdigit():
            continue
        try:
            args = (proc / "cmdline").read_bytes().split(b"\0")
        except OSError:
            continue
        if not args or not args[0].endswith(b"llama-server"):
            continue
        # One of its parents is this run's app (the only process with this HOME).
        pid, mine = proc.name, False
        for _ in range(6):
            try:
                stat = pathlib.Path(f"/proc/{pid}/stat").read_text()
                pid = stat.rsplit(")", 1)[1].split()[1]
                env = pathlib.Path(f"/proc/{pid}/environ").read_bytes()
            except (OSError, IndexError):
                break
            if f"HOME={home}".encode() in env.split(b"\0"):
                mine = True
                break
        if mine:
            found.append((int(proc.name), [a.decode(errors="replace") for a in args if a]))
    return found


def the_server(home: pathlib.Path):
    servers = server_processes(home)
    assert len(servers) == 1, servers
    return servers[0]


def flag(args, name):
    """The value after `name` in an argument list, or None."""
    return args[args.index(name) + 1] if name in args and args.index(name) + 1 < len(args) else None


def ready(s, timeout=600):
    return s.wait_for(lambda: (st := s.invoke("runtime_status"))["state"] == "ready" and st, timeout, "the model to be ready")


def fired(s, slot, timeout=180):
    return s.wait_for(lambda: (r := s.execute(f"return window.{slot}")) != "pending" and r, timeout, f"{slot} to end")


def busy(s):
    return s.invoke("model_busy")


def pending(s, slot):
    return s.execute(f"return window.{slot}") == "pending"


def settings_key(path):
    """The settings key of one model's own load settings (file name and size)."""
    name, size = os.path.basename(path), os.path.getsize(path)
    return "modelLoad:" + hashlib.sha256(f"local\n{name}\n{size}".encode()).hexdigest()[:16]


def messages(s, chat_id):
    return s.invoke("get_chat", {"chatId": chat_id})["messages"]


def start_reply(s):
    """Sends the long prompt in a new chat (in a chat with short answers the small model
    answers short again) and waits until the reply streams. True while it still streams,
    False when it already ended by itself; a reply that could not start fails."""
    chat = s.invoke("create_chat", {"modelRef": f"local:{MODEL}"})
    # No tools: the reply is text, which a stopped reply keeps.
    s.invoke("set_chat_tools", {"chatId": chat["id"], "enabled": []})
    REPLY["chat"] = chat["id"]
    s.execute(SEND, chat["id"], LONG_REPLY, None)
    s.wait_for(lambda: busy(s)["chatTurns"] == 1 or not pending(s, "__sent"), 120, "the reply to start")
    s.wait_for(lambda: s.execute("return window.__text") > 20 or not pending(s, "__sent"), 300, "the reply to stream")
    if pending(s, "__sent"):
        return True
    sent = fired(s, "__sent")
    assert sent.get("ok"), ("the reply could not start", sent)
    return False


def reply_streams(s):
    """A reply started by `start_reply` is still being written."""
    return pending(s, "__sent") and busy(s)["chatTurns"] == 1


def went_ahead(s, slot):
    """A change that was to wait for the reply went ahead: fine only when the reply had
    ended by itself (status complete), never when the change stopped it."""
    r = fired(s, slot, 600)
    fired(s, "__sent", 300)
    last = messages(s, REPLY["chat"])[-1]
    assert last["role"] == "assistant" and last["status"] == "complete", (
        f"{slot} went ahead while the reply streamed", r, last["status"])
    if r.get("ok"):
        ready(s)


def while_a_reply_streams(s, name, attempt, tries=4):
    """Runs `attempt()` while a real reply streams (the one still streaming from the step
    before, else a new one). When the reply ended by itself before `attempt` could check
    what it needed, `attempt` returns False (after the model is ready again) and it is
    tried again with a new reply."""
    for n in range(1, tries + 1):
        if (reply_streams(s) or start_reply(s)) and attempt():
            return
        fired(s, "__sent", 300)
        print(f"    {name}: the reply ended by itself too soon ({n} of {tries})", flush=True)
    raise AssertionError(f"{name}: the reply ended by itself too soon {tries} times")


CALLS = [0]


def call(s, cmd, args=None, timeout=600):
    """Calls a command that can take longer than a WebDriver script may (loads on a busy
    computer): fires it and waits for its end. Raises when it fails."""
    CALLS[0] += 1
    slot = f"__call{CALLS[0]}"
    s.execute(FIRE, slot, cmd, args or {})
    r = fired(s, slot, timeout)
    if not r.get("ok"):
        raise WebDriverError(f"{cmd} failed: {json.dumps(r.get('error'))[:800]}")
    return r.get("value")


def check_settings_saved_while_loading(s):
    """Settings saved while the model loads mark it "Settings changed" once it is ready
    (only a ready model can be compared, so the save itself could not)."""
    call(s, "unload_model")
    s.execute(FIRE, "__midload", "load_local_model", {"path": MODEL})
    s.wait_for(lambda: s.invoke("runtime_status")["state"] == "starting", 60, "the load to start")
    call(s, "set_model_load_settings", {"path": MODEL, "overrides": {"saveMemory": "on"}})
    r = fired(s, "__midload", 600)
    assert r.get("ok"), r
    st = ready(s)
    info = call(s, "model_load_info", {"path": MODEL})
    assert info["olderSettingsLoaded"] is True, info
    assert st["settingsChanged"] is True, ("Settings changed is not shown for settings saved during the load", st)
    call(s, "reset_model_load_settings", {"path": MODEL})
    assert s.invoke("runtime_status")["settingsChanged"] is False, "the loaded copy matches the settings again"


def check_removing_the_loaded_model(s, home):
    """Removing the loaded model from Installed while a change waits for it ("Restart when
    it ends"): the change is cancelled and the model is not loaded again."""
    installed = call(s, "import_model", {"path": MODEL})

    def remove_while_waiting():
        s.execute(FIRE, "__reload3", "reload_local_model", {"force": True, "whenIdle": True})
        s.wait_for(lambda: s.invoke("runtime_status")["reloadWaiting"] or not pending(s, "__reload3"), 10,
                   "reloadWaiting while the reply streams")
        if not pending(s, "__reload3"):
            went_ahead(s, "__reload3")
            return False
        call(s, "delete_model", {"id": installed["id"], "deleteFiles": False})
        r = fired(s, "__reload3", 120)
        assert r.get("ok") is False and r["error"]["code"] == "load_replaced", (
            "removing the model did not cancel the change that waited", r)
        fired(s, "__sent", 120)
        time.sleep(1.0)
        st = s.invoke("runtime_status")
        assert st["state"] == "stopped" and st["modelPath"] is None and st["reloadWaiting"] is False, (
            "the removed model was loaded again", st)
        assert server_processes(home) == [], server_processes(home)
        return True

    while_a_reply_streams(s, "removing the loaded model", remove_while_waiting)


def check_removing_the_unloaded_model(s):
    """Removing the model the user unloaded forgets it: no "Unloaded to free memory · Load"
    for a model that is no longer installed."""
    call(s, "load_local_model", {"path": MODEL})
    ready(s)
    call(s, "unload_model")
    assert s.invoke("runtime_status")["unloadedPath"] == MODEL
    installed = call(s, "import_model", {"path": MODEL})
    call(s, "delete_model", {"id": installed["id"], "deleteFiles": False})
    st = s.invoke("runtime_status")
    assert st["unloadedPath"] is None, ("the removed model is still offered to load", st)


def check_the_backend_check_keeps_the_unloaded_model(s, home):
    """The backend check (`run_diagnostics`) with nothing loaded leaves nothing loaded and
    "Unloaded to free memory · Load" as it was."""
    call(s, "load_local_model", {"path": MODEL})
    ready(s)
    call(s, "unload_model")
    call(s, "run_diagnostics", {"modelPath": MODEL, "whenIdle": True}, timeout=1200)
    st = s.invoke("runtime_status")
    assert st["state"] == "stopped" and st["unloadedPath"] == MODEL, ("not as it was before the check", st)
    assert server_processes(home) == [], server_processes(home)


def check_a_pick_waits_for_the_backend_check(s):
    """The backend check counts as a model check while it runs (D3): a model picked with
    "Restart when it ends" waits for it and is the one loaded afterwards."""
    second = OUT / "second-model.gguf"
    if not second.exists():
        try:
            os.link(MODEL, second)
        except OSError:
            os.symlink(MODEL, second)
    # Its header is read now, so the pick below reaches the question at once.
    call(s, "model_load_info", {"path": str(second)})
    call(s, "load_local_model", {"path": MODEL})
    ready(s)
    s.execute(FIRE, "__diag", "run_diagnostics", {"modelPath": MODEL, "whenIdle": True})
    s.wait_for(lambda: busy(s)["modelChecks"] == 1 or not pending(s, "__diag"), 300, "the check to start")
    assert pending(s, "__diag") and busy(s)["modelChecks"] == 1, ("the check does not count as a model check", busy(s))
    s.execute(FIRE, "__pick", "load_local_model", {"path": str(second), "settings": None, "whenIdle": True})
    seen = {}

    def waits():
        seen["status"] = s.invoke("runtime_status")
        return seen["status"]["reloadWaiting"] or not pending(s, "__pick") or not pending(s, "__diag")

    s.wait_for(waits, 60, "the pick to wait for the check")
    assert seen["status"]["reloadWaiting"], ("the pick did not wait for the check", seen["status"], busy(s))
    r = fired(s, "__diag", 1200)
    assert r.get("ok"), r
    r = fired(s, "__pick", 600)
    assert r.get("ok"), ("the model picked during the check did not load", r)
    st = ready(s)
    assert st["modelPath"] == str(second) and st["reloadWaiting"] is False, ("the check undid the pick", st)
    assert busy(s)["modelChecks"] == 0


def check_stop_and_restart_stops_the_backend_check(s):
    """A model picked with "Stop and restart" during the backend check stops the check and
    is the one loaded afterwards (the check does not put back what was there before it)."""
    s.execute(FIRE, "__diag2", "run_diagnostics", {"modelPath": MODEL, "whenIdle": True})
    s.wait_for(lambda: busy(s)["modelChecks"] == 1 or not pending(s, "__diag2"), 300, "the check to start")
    assert pending(s, "__diag2") and busy(s)["modelChecks"] == 1, ("the check does not count as a model check", busy(s))
    call(s, "load_local_model", {"path": MODEL, "settings": None, "whenIdle": False})
    r = fired(s, "__diag2", 1200)
    assert r.get("ok"), r
    print("    the check was stopped:", "stopped: what is loaded was changed" in r["value"]["text"])
    time.sleep(1.0)
    st = ready(s)
    assert st["modelPath"] == MODEL and busy(s)["modelChecks"] == 0, ("the check undid the pick", st)


def check_the_backend_check_waits_when_asked(s):
    """The backend check answered "Restart when it ends" (`whenIdle`) while the model is
    busy, here with another backend check, waits for it and stops nothing (D3); the waiting
    line's Cancel ends it with `load_replaced`, and the check that was running goes on."""
    s.execute(FIRE, "__diag3", "run_diagnostics", {"modelPath": MODEL, "whenIdle": True})
    s.wait_for(lambda: busy(s)["modelChecks"] == 1 or not pending(s, "__diag3"), 300, "the check to start")
    assert pending(s, "__diag3") and busy(s)["modelChecks"] == 1, ("the check does not count as a model check", busy(s))
    s.execute(FIRE, "__diag4", "run_diagnostics", {"modelPath": MODEL, "whenIdle": True})
    seen = {}

    def waits():
        seen["status"] = s.invoke("runtime_status")
        return seen["status"]["reloadWaiting"] or not pending(s, "__diag4") or not pending(s, "__diag3")

    s.wait_for(waits, 300, "the second check to wait for the first")
    assert seen["status"]["reloadWaiting"] and pending(s, "__diag3"), ("the second check did not wait", seen["status"], busy(s))
    call(s, "cancel_pending_reload")
    r = fired(s, "__diag4", 60)
    assert not r.get("ok") and r["error"].get("code") == "load_replaced", ("the cancelled wait did not end", r)
    r = fired(s, "__diag3", 1200)
    assert r.get("ok") and "stopped:" not in r["value"]["text"], ("the waiting check stopped the running one", r)
    st = ready(s)
    assert st["modelPath"] == MODEL and st["reloadWaiting"] is False, st
    assert busy(s)["modelChecks"] == 0


def check_cancel_at_once_ends_the_waiting_backend_check(s, home):
    """Cancel pressed right after the backend check was answered "Restart when it ends"
    while a reply streams ends the check at once and replaces nothing: the check starts its
    wait before it probes the backends, so a Cancel at once finds it waiting (before, the
    Cancel landed during the probes, cancelled nothing, and the check then waited and
    replaced the model once the reply ended)."""
    st = ready(s)
    assert st["modelPath"] == MODEL, st
    pid = the_server(home)[0]

    def cancel_at_once():
        nonlocal pid
        s.execute(FIRE, "__diag5", "run_diagnostics", {"modelPath": MODEL, "whenIdle": True})
        s.execute(FIRE, "__cancel5", "cancel_pending_reload", {})
        end = time.time() + 15
        while pending(s, "__diag5") and time.time() < end:
            time.sleep(0.2)
        assert not (pending(s, "__diag5") and reply_streams(s)), (
            "Cancel at once was lost: the backend check still waits for the reply", s.invoke("runtime_status"))
        r = fired(s, "__diag5", 1200)
        if r.get("ok"):
            # The reply ended by itself before the check began to wait: nothing waited, so
            # Cancel had nothing to end, and the check ran (never while the reply streamed).
            fired(s, "__sent", 300)
            last = messages(s, REPLY["chat"])[-1]
            assert last["role"] == "assistant" and last["status"] == "complete", (
                "the check ran while the reply streamed", last["status"])
            ready(s)
            pid = the_server(home)[0]
            return False
        assert r["error"].get("code") == "load_replaced", ("Cancel did not end the check with load_replaced", r)
        assert fired(s, "__cancel5", 30).get("ok"), "cancel_pending_reload failed"
        st = s.invoke("runtime_status")
        assert st["reloadWaiting"] is False, ("still shown as waiting after Cancel", st)
        assert st["modelPath"] == MODEL and the_server(home)[0] == pid, ("the cancelled check replaced the model", st)
        assert busy(s)["modelChecks"] == 0, busy(s)
        return True

    while_a_reply_streams(s, "Cancel at once ends the backend check", cancel_at_once)
    if reply_streams(s):
        s.invoke("stop_generation")
    fired(s, "__sent", 120)
    ready(s)


def check_a_reply_begun_while_the_check_probes_is_waited_for(s, home):
    """The backend check begins with nothing using the model, so it does not wait at first.
    A reply that begins while it probes the backends (made slow here: 6 s) is waited for
    before the check's first test load, with the waiting line on: it is never cut off, and
    the check runs once it has ended. Then the same again, with Cancel on that wait: the
    check gives its report of the devices, with the timed generation not run, and the model
    and its server are as they were. (Before, the first test load stopped the server, the
    reply ended with an error, and nothing waited.)"""
    idle = {"chatTurns": 0, "modelChecks": 0, "otherApps": 0}

    def begin(slot):
        """Starts the check, then a reply while it probes. True once the check waits for
        the reply (the waiting line is on); False when the reply ended by itself first."""
        st = ready(s)
        assert st["modelPath"] == MODEL and busy(s) == idle, (st, busy(s))
        chat = s.invoke("create_chat", {"modelRef": f"local:{MODEL}"})
        s.invoke("set_chat_tools", {"chatId": chat["id"], "enabled": []})
        REPLY["chat"] = chat["id"]
        SLOW_PROBES.touch()
        try:
            s.execute(FIRE, slot, "run_diagnostics", {"modelPath": MODEL, "whenIdle": True})
            s.wait_for(lambda: busy(s)["modelChecks"] == 1 or not pending(s, slot), 30, "the check to start")
            assert pending(s, slot), ("the check ended at once", s.execute(f"return window.{slot}"))
            s.execute(SEND, chat["id"], LONG_REPLY, None)
            s.wait_for(lambda: busy(s)["chatTurns"] == 1 or not pending(s, "__sent"), 30, "the reply to start")
        finally:
            # The probe that runs now has already read it.
            SLOW_PROBES.unlink(missing_ok=True)
        seen = {}

        def settled():
            seen["status"] = s.invoke("runtime_status")
            return seen["status"]["reloadWaiting"] or not pending(s, "__sent") or not pending(s, slot)

        s.wait_for(settled, 120, "the check to wait for the reply")
        if seen["status"]["reloadWaiting"]:
            return True
        # Nothing waited: fine only when the reply had ended by itself, never when the check
        # cut it off.
        fired(s, "__sent", 300)
        last = messages(s, chat["id"])[-1]
        assert last["role"] == "assistant" and last["status"] == "complete", (
            "the backend check cut off a reply that began while it probed", last["status"],
            len(last.get("content") or ""), seen["status"], busy(s))
        fired(s, slot, 1200)
        ready(s)
        return False

    def waits_then_runs():
        pid = the_server(home)[0]
        if not begin("__diag6"):
            return False
        assert pending(s, "__diag6") and reply_streams(s), ("waiting, but not for this reply", busy(s))
        # Waiting, the check has not touched the server: the reply is not cut off (before the
        # fix, the first test load stopped it here and it ended with an error).
        assert the_server(home)[0] == pid, "the check replaced the model while it waited"
        # The small model writes about 1,500 words at a few tokens a second, too long to let
        # it run out; Stop ends the reply, which keeps what it wrote (never an error), the
        # wait then sees the model free, and the check runs its test loads and timing.
        s.invoke("stop_generation")
        fired(s, "__sent", 300)
        last = messages(s, REPLY["chat"])[-1]
        # Not "error": the check did not cut it off (before the fix it ended with an error).
        assert last["role"] == "assistant" and last["status"] != "error", (
            "the reply the check waited for was cut off", last["status"])
        r = fired(s, "__diag6", 1200)
        assert r.get("ok"), r
        text = r["value"]["text"]
        assert "  cpu: prompt" in text and "not run" not in text, ("the check did not run after the wait", text[-600:])
        st = ready(s)
        assert st["modelPath"] == MODEL and st["reloadWaiting"] is False, st
        assert busy(s)["modelChecks"] == 0, busy(s)
        return True

    def cancel_ends_the_wait():
        pid = the_server(home)[0]
        if not begin("__diag7"):
            return False
        call(s, "cancel_pending_reload")
        r = fired(s, "__diag7", 60)
        assert r.get("ok"), ("Cancel on the check's second wait gave no report", r)
        text = r["value"]["text"]
        assert "backend cpu" in text and "not run" in text and "  cpu: prompt" not in text, text[-600:]
        st = s.invoke("runtime_status")
        assert st["reloadWaiting"] is False, ("still shown as waiting after Cancel", st)
        assert st["modelPath"] == MODEL and the_server(home)[0] == pid, ("the cancelled check replaced the model", st)
        assert busy(s)["modelChecks"] == 0, busy(s)
        if reply_streams(s):
            s.invoke("stop_generation")
        fired(s, "__sent", 120)
        last = messages(s, REPLY["chat"])[-1]
        assert last["status"] != "error", ("the reply was cut off", last["status"])
        ready(s)
        return True

    for name, attempt in (
        ("a reply begun while the check probes is waited for", waits_then_runs),
        ("Cancel on that wait leaves the model as it is", cancel_ends_the_wait),
    ):
        for n in range(1, 5):
            if attempt():
                break
            print(f"    {name}: the reply ended by itself too soon ({n} of 4)", flush=True)
        else:
            raise AssertionError(f"{name}: the reply ended by itself too soon 4 times")


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
        APP_DEV_LLAMA_DIR=str(wrap_llama_dir()),
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
        db = home / "data" / IDENTIFIER / "app.db"

        def store_setting(key, value):
            con = sqlite3.connect(db)
            con.execute(
                "INSERT INTO settings(key, value, updated_at) VALUES (?, ?, ?) "
                "ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                (key, json.dumps(value), int(time.time() * 1000)),
            )
            con.commit()
            con.close()

        def stored_setting(key):
            con = sqlite3.connect(db)
            row = con.execute("SELECT value FROM settings WHERE key = ?", (key,)).fetchone()
            con.close()
            return json.loads(row[0]) if row else None

        step("a fresh install: Automatic")
        d = s.invoke("get_local_model_settings")
        assert d["contextAuto"] is True and "upgradeNote" not in d, d
        assert stored_setting("localModel") is None

        step("the 0.0.4 upgrade rule: another size stays, with the note on this start only")
        store_setting("localModel", {"ctx": 16384, "backend": None, "device": None})
        d = s.invoke("get_local_model_settings")
        assert d["contextAuto"] is False and d["ctx"] == 16384 and d.get("upgradeNote") == 16384, d
        assert stored_setting("localModel")["contextAuto"] is False, stored_setting("localModel")
        assert s.invoke("get_local_model_settings").get("upgradeNote") == 16384, "gone within the same start"
        step("the 0.0.4 default of 8,192 is Automatic")
        store_setting("localModel", {"ctx": 8192, "backend": None, "device": None})
        d = s.invoke("get_local_model_settings")
        assert d["contextAuto"] is True and "upgradeNote" not in d, d
        s.invoke("set_local_model_settings", {"settings": {**d, "contextAuto": True}})
        assert stored_setting("localModel")["contextAuto"] is True

        step("model_load_info for the real model")
        info = call(s, "model_load_info", {"path": MODEL})
        print("    trainedCtx:", info["trainedCtx"], "thinking:", info["thinking"], "Automatic:", info["contextOptions"][0],
              "resolved:", info["resolved"]["ctx"], info["resolved"]["ctxChoice"], "fit:", info["fit"])
        assert info["fileName"] == os.path.basename(MODEL) and info["sizeBytes"] == os.path.getsize(MODEL), info
        assert info["trainedCtx"] and info["trainedCtx"] >= 32768, info["trainedCtx"]
        assert info["contextOptions"][0]["choice"] == "auto", info["contextOptions"]
        # Automatic depends on the memory free right now, which other programs on a shared
        # computer change: check that the answer is consistent, whichever it is.
        auto = info["contextOptions"][0]
        if info["tooLarge"]:
            print("    NOTE: too little memory free right now for Automatic's floor; it would load with 4,096")
            assert (auto["tokens"], auto["fit"], info["fit"], info["resolved"]["ctx"]) == (0, "red", "red", 4096), info
        else:
            assert 4096 <= auto["tokens"] <= 32768 and auto["fit"] in ("green", "yellow"), auto
            assert info["resolved"]["ctx"] == auto["tokens"] and info["fit"] == auto["fit"], info
        sizes = [o["choice"]["fixed"] for o in info["contextOptions"][1:]]
        assert sizes[0] == 2048 and sizes[-1] == info["trainedCtx"] and 8192 in sizes, sizes
        assert [g["gpu"] for g in info["gpuOptions"]] == ["all", "fit", "off"], info["gpuOptions"]
        assert info["resolved"]["ctxChoice"] == "auto" and info["resolved"]["ctxSource"] == "automatic", info["resolved"]
        assert info["loaded"] is False, info
        assert all(v == "default" for v in info["overrides"].values()), info["overrides"]

        step("overrides outside the fixed lists are refused")
        for bad in ({"context": {"fixed": 12345}}, {"context": {"fixed": info["trainedCtx"] * 2}}, {"device": {"named": "Vulkan9"}}):
            e = error_of(s, "set_model_load_settings", {"path": MODEL, "overrides": bad})
            assert e.get("code") == "bad_request", (bad, e)
        e = error_of(s, "set_model_load_settings", {"path": MODEL, "overrides": {"gpu": "half"}})
        print("    gpu 'half':", str(e)[:120])
        e = error_of(s, "model_load_info", {"path": str(OUT / "missing.gguf")})
        assert e.get("code") == "model_missing", e
        assert all(v == "default" for v in call(s, "model_load_info", {"path": MODEL})["overrides"].values())

        step("unsaved choices: the fit marks follow them, nothing is saved")
        draft = {"context": "auto", "saveMemory": "on", "gpu": "default", "keepInMemory": "default",
                 "device": "default", "thinkingLimit": "default"}
        preview = call(s, "model_load_info", {"path": MODEL, "overrides": draft})
        assert preview["overrides"]["saveMemory"] == "on" and preview["resolved"]["saveMemory"] is True, preview
        auto_f16, auto_q8 = info["contextOptions"][0]["tokens"], preview["contextOptions"][0]["tokens"]
        print("    Automatic:", auto_f16, "tokens; with Save memory:", auto_q8)
        assert auto_q8 >= auto_f16, (auto_f16, auto_q8)
        fixed = call(s, "model_load_info", {"path": MODEL, "overrides": {**draft, "context": {"fixed": 4096}}})
        assert fixed["resolved"]["ctx"] == 4096 and fixed["resolved"]["ctxSource"] == "modelSetting", fixed["resolved"]
        e = error_of(s, "model_load_info", {"path": MODEL, "overrides": {"context": {"fixed": 12345}}})
        assert e.get("code") == "bad_request", e
        assert all(v == "default" for v in call(s, "model_load_info", {"path": MODEL})["overrides"].values()), "a preview saved"
        con = sqlite3.connect(db)
        saved = con.execute("SELECT key FROM settings WHERE key LIKE 'modelLoad:%'").fetchall()
        con.close()
        assert saved == [], saved

        step("save a per-model context, Save memory and Keep in memory")
        own = {"context": {"fixed": 8192}, "saveMemory": "on", "keepInMemory": "on"}
        info = call(s, "set_model_load_settings", {"path": MODEL, "overrides": own})
        assert info["overrides"]["context"] == {"fixed": 8192} and info["overrides"]["saveMemory"] == "on", info["overrides"]
        r = info["resolved"]
        assert (r["ctx"], r["ctxChoice"], r["ctxSource"], r["saveMemory"], r["keepInMemory"]) == (
            8192, {"fixed": 8192}, "modelSetting", True, True), r
        own_key = settings_key(MODEL)
        assert stored_setting(own_key)["context"] == {"fixed": 8192}, (own_key, stored_setting(own_key))

        step("the generic setting command (the one a tool could reach) refuses model settings")
        for key in ("localModel", own_key):
            before = stored_setting(key)
            e = error_of(s, "set_setting", {"key": key, "value": {
                "ctx": 1048576, "contextAuto": False, "keepInMemory": "on", "context": {"fixed": 1048576}}})
            assert "cannot be changed this way" in json.dumps(e), (key, e)
            assert stored_setting(key) == before, (key, "changed", stored_setting(key))

        step("load with no settings: the model's own")
        call(s, "load_local_model", {"path": MODEL})
        st = ready(s)
        print("    backend:", st["backend"], "nCtx:", st["nCtx"], "ctxSource:", st["ctxSource"])
        assert st["nCtx"] == 8192 and st["ctxSource"] == "modelSetting", st
        pid, args = the_server(home)
        print("    llama-server:", " ".join(a for a in args[1:] if not a.startswith("/")))
        assert flag(args, "-c") == "8192", args
        assert (flag(args, "-fa"), flag(args, "-ctk"), flag(args, "-ctv")) == ("on", "q8_0", "q8_0"), args
        assert flag(args, "-lm") == "mmap+mlock", args
        assert flag(args, "-a") == "harnessui-local", args
        assert st["adjustments"] == [], ("Save memory had to be turned off", st["adjustments"])
        info = call(s, "model_load_info", {"path": MODEL})
        assert info["loaded"] is True and info["olderSettingsLoaded"] is False and info["autoCtx"] is None, info
        assert st["settingsChanged"] is False

        step("picking the loaded model again with the same settings is a no-op")
        call(s, "load_local_model", {"path": MODEL})
        assert the_server(home)[0] == pid

        step("change to Automatic: Settings changed, nothing restarts")
        call(s, "set_model_load_settings", {"path": MODEL, "overrides": {"context": "auto", "saveMemory": "on"}})
        assert the_server(home)[0] == pid
        st = s.invoke("runtime_status")
        assert st["settingsChanged"] is True and st["nCtx"] == 8192, st
        assert call(s, "model_load_info", {"path": MODEL})["olderSettingsLoaded"] is True
        assert the_server(home)[0] == pid

        step("Apply and restart")
        call(s, "reload_local_model", {"force": False, "whenIdle": False})
        st = ready(s)
        info = call(s, "model_load_info", {"path": MODEL})
        print("    Automatic chose", st["nCtx"], "tokens; ctxSource", st["ctxSource"])
        assert st["ctxSource"] == "automatic" and 4096 <= st["nCtx"] <= 32768, st
        assert info["autoCtx"] == st["nCtx"] and info["olderSettingsLoaded"] is False, info
        assert st["settingsChanged"] is False
        new_pid, args = the_server(home)
        assert new_pid != pid and flag(args, "-c") == str(st["nCtx"]), (pid, new_pid, args)
        assert flag(args, "-ctk") == "q8_0" and "-lm" not in args, args
        pid = new_pid

        step("Apply and restart with nothing changed does nothing")
        call(s, "reload_local_model", {"force": False, "whenIdle": False})
        assert the_server(home)[0] == pid

        step("the thinking limit applies to the next reply without a restart")
        call(s, "set_model_load_settings", {"path": MODEL, "overrides": {"context": "auto", "saveMemory": "on", "thinkingLimit": "short"}})
        assert s.invoke("runtime_status")["settingsChanged"] is False
        assert call(s, "model_load_info", {"path": MODEL})["resolved"]["thinkingLimit"] == "short"

        step("Reload")
        call(s, "reload_local_model", {"force": True, "whenIdle": False})
        ready(s)
        new_pid, _ = the_server(home)
        assert new_pid != pid
        pid = new_pid

        step("Restart when it ends: waits while a real reply streams; Cancel")

        def change_went_ahead(slot):
            nonlocal pid
            went_ahead(s, slot)
            pid = the_server(home)[0]
            return False

        def wait_then_cancel():
            s.execute(FIRE, "__reload1", "reload_local_model", {"force": True, "whenIdle": True})
            s.wait_for(lambda: s.invoke("runtime_status")["reloadWaiting"] or not pending(s, "__reload1"), 10,
                       "reloadWaiting while the reply streams")
            time.sleep(1.0)
            if not pending(s, "__reload1"):
                return change_went_ahead("__reload1")
            assert the_server(home)[0] == pid, "the model restarted while the change waited"
            s.invoke("cancel_pending_reload")
            r = fired(s, "__reload1", 10)
            if r.get("ok"):  # the reply ended by itself just before Cancel landed
                return change_went_ahead("__reload1")
            assert r["error"]["code"] == "load_replaced", ("Cancel did not end the wait with load_replaced", r)
            assert s.invoke("runtime_status")["reloadWaiting"] is False, "still shown as waiting after Cancel"
            assert the_server(home)[0] == pid, "Cancel restarted the model"
            return True

        while_a_reply_streams(s, "Restart when it ends, Cancel", wait_then_cancel)

        step("Restart when it ends: goes ahead once the reply has ended")

        def wait_then_stop():
            nonlocal pid
            s.execute(FIRE, "__reload2", "reload_local_model", {"force": True, "whenIdle": True})
            s.wait_for(lambda: s.invoke("runtime_status")["reloadWaiting"] or not pending(s, "__reload2"), 10,
                       "reloadWaiting while the reply streams")
            time.sleep(0.5)
            if not pending(s, "__reload2"):
                return change_went_ahead("__reload2")
            s.invoke("stop_generation")
            r = fired(s, "__reload2", 600)
            assert r.get("ok") is True, ("the waiting reload did not go ahead once the reply ended", r)
            st = ready(s)
            assert st["reloadWaiting"] is False and busy(s)["chatTurns"] == 0, (st, busy(s))
            new_pid = the_server(home)[0]
            assert new_pid != pid, "the model did not restart"
            pid = new_pid
            fired(s, "__sent", 120)
            return True

        while_a_reply_streams(s, "Restart when it ends, goes ahead", wait_then_stop)

        step("Stop and restart: the reply is stopped first and keeps what it wrote")

        def stop_and_restart():
            nonlocal pid
            if busy(s)["chatTurns"] != 1:
                return False
            call(s, "reload_local_model", {"force": True, "whenIdle": False})
            assert busy(s)["chatTurns"] == 0, "Stop and restart did not stop the reply first"
            sent = fired(s, "__sent", 120)
            print("    the stopped reply's call ended with:", sent)
            ready(s)
            new_pid = the_server(home)[0]
            assert new_pid != pid, "the model did not restart"
            pid = new_pid
            last = messages(s, REPLY["chat"])[-1]
            if last["status"] == "complete":  # it ended by itself just before; nothing to stop
                return False
            assert last["role"] == "assistant" and last["text"], ("the stopped reply lost what it wrote", last)
            print("    the stopped reply:", last["status"], len(last["text"]), "characters")
            return True

        while_a_reply_streams(s, "Stop and restart", stop_and_restart)

        step("Unload remembers the model; Load again uses its saved settings")
        call(s, "unload_model")
        st = s.invoke("runtime_status")
        assert st["state"] == "stopped" and st["unloadedPath"] == MODEL, st
        assert server_processes(home) == []
        call(s, "load_local_model", {"path": MODEL})
        st = ready(s)
        assert st["unloadedPath"] is None and st["ctxSource"] == "automatic", st
        pid, _ = the_server(home)

        step("Unload when idle with nothing running is at once")
        call(s, "unload_model", {"whenIdle": True})
        assert s.invoke("runtime_status")["unloadedPath"] == MODEL
        call(s, "load_local_model", {"path": MODEL, "settings": None, "whenIdle": True})
        ready(s)

        step("Reset to defaults")
        info = call(s, "reset_model_load_settings", {"path": MODEL})
        assert all(v == "default" for v in info["overrides"].values()), info["overrides"]
        assert info["resolved"]["saveMemory"] is False and info["olderSettingsLoaded"] is True, info["resolved"]
        assert s.invoke("runtime_status")["settingsChanged"] is True
        call(s, "reload_local_model", {"force": False, "whenIdle": False})
        ready(s)
        args = the_server(home)[1]
        assert not {"-fa", "-ctk", "-ctv", "-lm"} & set(args), ("Save memory or Keep in memory without the setting", args)

        step("settings saved while the model loads: Settings changed once it is ready")
        check_settings_saved_while_loading(s)

        step("removing the loaded model cancels a change that waits for it")
        check_removing_the_loaded_model(s, home)

        step("removing the unloaded model forgets it")
        check_removing_the_unloaded_model(s)

        step("the backend check with nothing loaded keeps \"Unloaded to free memory\"")
        check_the_backend_check_keeps_the_unloaded_model(s, home)

        step("a model picked during the backend check waits for it, then loads")
        check_a_pick_waits_for_the_backend_check(s)

        step("Stop and restart stops the backend check; the model picked is the one loaded")
        check_stop_and_restart_stops_the_backend_check(s)

        step("the backend check asked to wait waits for the model and stops nothing")
        check_the_backend_check_waits_when_asked(s)

        step("Cancel right after Restart when it ends: the backend check ends, the model stays")
        check_cancel_at_once_ends_the_waiting_backend_check(s, home)

        step("a reply begun while the backend check probes is waited for; Cancel on that wait")
        check_a_reply_begun_while_the_check_probes_is_waited_for(s, home)

        step("the 0.0.4 command shape still loads exactly as given")
        old = {"ctx": 4096, "backend": None, "device": None}
        call(s, "load_local_model", {"path": MODEL, "settings": old})
        st = ready(s)
        assert st["nCtx"] == 4096 and st["ctxSource"] == "defaultSetting", st
        assert flag(the_server(home)[1], "-c") == "4096"
        for bad in (0, 100, 4_000_000):
            e = error_of(s, "load_local_model", {"path": MODEL, "settings": {**old, "ctx": bad}})
            assert e.get("code") == "bad_request", (bad, e)
        assert flag(the_server(home)[1], "-c") == "4096", "a refused load changed nothing"
        call(s, "unload_model")
        s.screenshot(str(OUT / "m10-end.png"))
        print("PASS")
        return 0
    except (AssertionError, WebDriverError) as e:
        print("FAIL:", e)
        if s is not None:
            try:
                s.screenshot(str(OUT / "m10-fail.png"))
                print("    status:", json.dumps(s.invoke("runtime_status"))[:600])
            except Exception:  # noqa: BLE001
                pass
        return 1
    finally:
        if s is not None:
            s.quit()
        driver.terminate()


if __name__ == "__main__":
    sys.exit(main())

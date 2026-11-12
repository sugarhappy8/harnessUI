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
  a size outside 512 to 1,048,576 tokens is refused.

The D3 question in the window itself is WP2's scenario (`model-settings`).

    python3 -I e2e/m10_model_settings.py
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
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "e2e"))
from webdriver import Session, WebDriverError  # noqa: E402

APP = os.environ.get("APP_BIN", str(ROOT / "target" / "debug" / "app"))
MODEL = os.environ.get("TEST_GGUF") or next(iter(sorted((ROOT / ".cache" / "models").glob("*.gguf"))), "")
LLAMA_DIR = os.environ.get("APP_DEV_LLAMA_DIR", str(ROOT / ".cache" / "llama-linux"))
OUT = pathlib.Path(os.environ.get("E2E_OUT", tempfile.mkdtemp(prefix="e2e-m10-")))
IDENTIFIER = "io.github.sugarhappy8.harnessui"

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

LONG_REPLY = "Count from 1 to 500, one number per line, and nothing else. /no_think"


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
        chat = s.invoke("create_chat", {"modelRef": f"local:{MODEL}"})
        # No tools: the reply is text, which a stopped reply keeps.
        s.invoke("set_chat_tools", {"chatId": chat["id"], "enabled": []})
        s.execute(SEND, chat["id"], LONG_REPLY, None)
        s.wait_for(lambda: busy(s)["chatTurns"] == 1, 120, "the reply to start")
        s.wait_for(lambda: s.execute("return window.__text") > 20, 300, "the reply to stream")
        s.execute(FIRE, "__reload1", "reload_local_model", {"force": True, "whenIdle": True})
        s.wait_for(lambda: s.invoke("runtime_status")["reloadWaiting"], 10, "reloadWaiting")
        time.sleep(1.0)
        assert s.execute("return window.__reload1") == "pending", "it did not wait"
        assert the_server(home)[0] == pid and busy(s)["chatTurns"] == 1
        s.invoke("cancel_pending_reload")
        r = fired(s, "__reload1", 10)
        assert r.get("ok") is False and r["error"]["code"] == "load_replaced", r
        assert s.invoke("runtime_status")["reloadWaiting"] is False
        assert the_server(home)[0] == pid and busy(s)["chatTurns"] == 1, "the reply was touched"

        step("Restart when it ends: goes ahead once the reply has ended")
        s.execute(FIRE, "__reload2", "reload_local_model", {"force": True, "whenIdle": True})
        s.wait_for(lambda: s.invoke("runtime_status")["reloadWaiting"], 10, "reloadWaiting")
        time.sleep(0.5)
        assert s.execute("return window.__reload2") == "pending"
        s.invoke("stop_generation")
        r = fired(s, "__reload2", 600)
        assert r.get("ok") is True, r
        st = ready(s)
        assert st["reloadWaiting"] is False and busy(s)["chatTurns"] == 0, (st, busy(s))
        new_pid, _ = the_server(home)
        assert new_pid != pid
        pid = new_pid
        fired(s, "__sent", 120)

        step("Stop and restart: the reply is stopped first and keeps what it wrote")
        # A follow-up to the first (stopped) reply, so both are on the chat's path.
        leaf = s.invoke("get_chat", {"chatId": chat["id"]})["messages"][-1]
        assert leaf["role"] == "assistant", leaf
        s.execute(SEND, chat["id"], LONG_REPLY, leaf["id"])
        s.wait_for(lambda: s.execute("return window.__text") > 20, 300, "the second reply to stream")
        assert busy(s)["chatTurns"] == 1
        call(s, "reload_local_model", {"force": True, "whenIdle": False})
        assert busy(s)["chatTurns"] == 0
        sent = fired(s, "__sent", 120)
        print("    the stopped reply's call ended with:", sent)
        st = ready(s)
        new_pid, _ = the_server(home)
        assert new_pid != pid
        pid = new_pid
        replies = [m for m in s.invoke("get_chat", {"chatId": chat["id"]})["messages"] if m["role"] == "assistant"]
        print("    assistant messages:", [(m["status"], len(m["text"] or "")) for m in replies])
        assert len(replies) == 2 and all(m["text"] for m in replies), replies

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

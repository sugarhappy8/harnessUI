#!/usr/bin/env python3
"""M11 end-to-end (0.0.6 item 6, WP10d): Fine control, "Where the model goes" and the usage
line, with the real small model on the processor.

Drives the backend commands and the window, in the real app:
- `model_load_info` carries `fineFacts` (this computer: no graphics device, its threads)
  and the instant picture (`estimate { quick }`, one bar of computer memory, the model's
  units from its tensor table);
- `offload_measure` for an unsaved draft runs llama.cpp's own measurement
  (`llama-fit-params`, from the development folder in this debug build) and answers
  `measured` with the same key as the draft's picture; asked again it answers from the
  cache; nothing is saved; no fit-params process is left;
- values outside the lists are refused by every command that takes them ("This setting
  isn't one of the choices."), and a file that is not a model is refused;
- `usage_now` answers with this computer's threads, the processor (measuring, then a
  percentage) and memory, and no graphics card here;
- in the window: Settings for this model from the picker's gear shows "Where the model
  goes"; opening Fine control says "Worked out for these settings."; Processor threads 2
  and Fast attention Off, Load with these settings: the model becomes ready, the launched
  llama-server has `-t 2 -fa off`, and `runtime_status.placement` says where it went (all on
  the processor);
- the usage menu (its text does not depend on the window width): a "Processor:" row with
  "threads", "Memory: … of … GB in use", no graphics row, "On the processor only.";
- Reset to defaults: the next load has neither flag.

    python3 -I e2e/m11_fine_control.py
"""

import json
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
from webdriver import Session, WebDriverError  # noqa: E402

APP = os.environ.get("APP_BIN", str(ROOT / "target" / "debug" / "app"))
MODEL = os.environ.get("TEST_GGUF") or str(next(iter(sorted((ROOT / ".cache" / "models").glob("*.gguf"))), ""))
LLAMA_DIR = os.environ.get("APP_DEV_LLAMA_DIR", str(ROOT / ".cache" / "llama-linux"))
OUT = pathlib.Path(os.environ.get("E2E_OUT", tempfile.mkdtemp(prefix="e2e-m11-")))
REFUSED = "This setting isn't one of the choices."

# Starts a command without waiting for it: the result lands in `window[slot]`.
FIRE = """
const [slot, cmd, args] = arguments;
window[slot] = 'pending';
window.__TAURI_INTERNALS__.invoke(cmd, args)
  .then(v => { window[slot] = {ok: true, value: v === undefined ? null : v}; })
  .catch(e => { window[slot] = {ok: false, error: e}; });
"""


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def step(name):
    print(f"--- {name}", flush=True)


def fired(s, slot, timeout=180):
    return s.wait_for(lambda: (r := s.execute(f"return window.{slot}")) != "pending" and r, timeout, f"{slot} to end")


CALLS = [0]


def call(s, cmd, args=None, timeout=600):
    """Calls a command that can take longer than a WebDriver script may: fires it and waits
    for its end. Raises when it fails."""
    CALLS[0] += 1
    slot = f"__call{CALLS[0]}"
    s.execute(FIRE, slot, cmd, args or {})
    r = fired(s, slot, timeout)
    if not r.get("ok"):
        raise WebDriverError(f"{cmd} failed: {json.dumps(r.get('error'))[:800]}")
    return r.get("value")


def error_of(s, cmd, args):
    """Calls `cmd` and returns its error object; fails if it succeeds."""
    CALLS[0] += 1
    slot = f"__call{CALLS[0]}"
    s.execute(FIRE, slot, cmd, args)
    r = fired(s, slot, 120)
    assert not r.get("ok"), f"{cmd} {json.dumps(args)[:200]} was accepted: {json.dumps(r)[:300]}"
    return r.get("error")


def processes(home: pathlib.Path, program: bytes):
    """Processes running `program` that this run's app started: (pid, argument list)."""
    found = []
    for proc in pathlib.Path("/proc").iterdir():
        if not proc.name.isdigit():
            continue
        try:
            args = (proc / "cmdline").read_bytes().split(b"\0")
        except OSError:
            continue
        if not args or not args[0].endswith(program):
            continue
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


def the_server(home):
    servers = processes(home, b"llama-server")
    assert len(servers) == 1, servers
    return servers[0]


def flag(args, name):
    """The value after `name` in an argument list, or None."""
    return args[args.index(name) + 1] if name in args and args.index(name) + 1 < len(args) else None


def ready(s, timeout=600):
    return s.wait_for(lambda: (st := s.invoke("runtime_status"))["state"] == "ready" and st, timeout, "the model to be ready")


def main() -> int:
    if not os.path.isfile(MODEL):
        print(f"FAIL: no test model ({MODEL!r}); set TEST_GGUF")
        return 1
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
        # The runtime logs each fit-params run at debug level (counted below).
        RUST_LOG="info,runtime=debug",
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

        def set_select(css, value):
            s.execute(
                """
                const el = document.querySelector(arguments[0]);
                if (!el) throw new Error('no select ' + arguments[0]);
                Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set.call(el, arguments[1]);
                el.dispatchEvent(new Event('change', {bubbles: true}));
                """,
                css, value)
            time.sleep(0.3)

        def set_range(css, value):
            s.execute(
                """
                const el = document.querySelector(arguments[0]);
                if (!el) throw new Error('no slider ' + arguments[0]);
                Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, String(arguments[1]));
                el.dispatchEvent(new Event('input', {bubbles: true}));
                """,
                css, value)
            time.sleep(0.3)

        def escape():
            s.execute("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}));")
            time.sleep(0.5)

        def fit_runs():
            """fit-params runs so far, by the runtime's one log entry per run."""
            logs = list((home / "data").rglob("app.*.log*"))
            return sum(p.read_text(errors="replace").count("llama-fit-params measured a placement") for p in logs)

        def click_css(css, what):
            el = s.wait_for(lambda: s.execute("return document.querySelector(arguments[0]) || false;", css), 15, what)
            try:
                s.click(next(iter(el.values())))
            except WebDriverError:
                s.execute("arguments[0].click();", el)

        step("model_load_info: what Fine control may offer, and the instant picture")
        info = call(s, "model_load_info", {"path": MODEL})
        facts, picture = info["fineFacts"], info["picture"]
        print("    fineFacts:", json.dumps(facts))
        print("    picture:", json.dumps({k: picture[k] for k in ("source", "units", "layersOnGraphics", "mark", "fits")}),
              "computer need:", json.dumps(picture["computer"]["need"]))
        assert facts["graphics"] is False and facts["apple"] is False, facts
        assert facts["threads"] >= 1 and facts["threads"] == os.cpu_count(), (facts, os.cpu_count())
        assert facts["units"] and facts["units"] >= 2 and facts["expertLayers"] is None, facts
        assert facts["graphicsLimitBytes"] is None, facts
        assert picture["source"] == {"kind": "estimate", "why": "quick"}, picture["source"]
        assert picture["graphics"] is None and picture["unified"] is False, picture
        assert picture["units"] == facts["units"] and picture["layersOnGraphics"] == 0, picture
        need = picture["computer"]["need"]
        assert need["model"] > 0.9 * os.path.getsize(MODEL), (need, os.path.getsize(MODEL))
        assert need["context"] > 0 and need["compute"] > 0, need
        assert picture["computer"]["totalBytes"] > 0 and picture["computer"]["freeBytes"] > 0, picture["computer"]
        assert picture["mark"] == info["fit"], (picture["mark"], info["fit"])
        assert info["resolved"]["fine"] == {"gpuLayers": None, "expertsOnCpu": "none", "kvInComputerMemory": False,
                                            "threads": None, "flashAttn": "auto", "fitMarginMib": None}, info["resolved"]["fine"]

        step("offload_measure: llama.cpp's own figures for an unsaved draft")
        draft = {"fine": {"threads": {"count": 2}, "fastAttention": "off"}}
        began = time.time()
        measured = call(s, "offload_measure", {"path": MODEL, "overrides": draft}, timeout=120)
        took = time.time() - began
        print(f"    measured in {took:.2f} s:", json.dumps(measured["source"]), "computer need:",
              json.dumps(measured["computer"]["need"]))
        assert measured["source"] == {"kind": "measured"}, measured["source"]
        preview = call(s, "model_load_info", {"path": MODEL, "overrides": draft})
        assert measured["key"] == preview["picture"]["key"], "the draft's key"
        assert measured["key"] != info["picture"]["key"], "another draft, another key"
        assert preview["resolved"]["fine"]["threads"] == 2 and preview["resolved"]["fine"]["flashAttn"] == "off", preview["resolved"]
        mneed = measured["computer"]["need"]
        assert mneed["model"] > 0.5 * os.path.getsize(MODEL) and mneed["context"] > 0, mneed
        assert measured["graphics"] is None and measured["units"] == facts["units"], measured
        # Asked again: from the cache, no second run (the free figures are read again).
        s.wait_for(lambda: fit_runs() >= 1, 15, "the run's log entry")
        assert fit_runs() == 1, fit_runs()
        again = call(s, "offload_measure", {"path": MODEL, "overrides": draft}, timeout=60)
        for part in ("key", "source", "units", "layersOnGraphics", "mark", "fits"):
            assert again[part] == measured[part], (part, again[part], measured[part])
        assert again["computer"]["need"] == measured["computer"]["need"]
        time.sleep(1.5)
        assert fit_runs() == 1, ("a second run for the same settings", fit_runs())
        assert processes(home, b"llama-fit-params") == [], "a fit-params process was left"
        assert "fine" not in call(s, "model_load_info", {"path": MODEL})["overrides"], "the draft was saved"

        step("values outside the lists are refused")
        for bad in ({"fine": {"threads": {"count": 0}}}, {"fine": {"layers": {"count": 5000}}},
                    {"fine": {"leaveFree": {"mib": 1000}}}, {"fine": {"experts": "all"}},
                    {"fine": {"layers": {"count": facts["units"] + 1}}}):
            for cmd in ("offload_measure", "set_model_load_settings", "model_load_info"):
                e = error_of(s, cmd, {"path": MODEL, "overrides": bad})
                assert e.get("code") == "bad_request" and e.get("message") == REFUSED, (cmd, bad, e)
        e = error_of(s, "offload_measure", {"path": str(OUT / "not-a-model.txt"), "overrides": {}})
        assert e.get("code") == "bad_request", e
        e = error_of(s, "offload_measure", {"path": str(OUT / "missing.gguf"), "overrides": {}})
        assert e.get("code") == "model_missing", e

        step("usage_now: this computer, read here")
        first = s.invoke("usage_now")
        print("    usage_now:", json.dumps(first))
        assert first["threads"] == os.cpu_count(), first
        assert first["graphics"] == {"kind": "notAvailable", "why": "noGraphicsCard"}, first["graphics"]
        assert first["processorPercent"]["kind"] in ("measuring", "value"), first
        mem = first["memory"]
        assert mem["kind"] == "value" and 0 < mem["value"]["usedBytes"] <= mem["value"]["totalBytes"], mem
        time.sleep(1.6)
        second = s.invoke("usage_now")
        assert second["processorPercent"]["kind"] == "value", second
        assert 0 <= second["processorPercent"]["value"] <= 100, second

        step("Settings for this model from the picker's gear: Where the model goes, Fine control")
        # Installed, so the picker lists it (with its gear) without loading it first.
        call(s, "import_model", {"path": MODEL})
        chat = s.invoke("create_chat", {"modelRef": f"local:{MODEL}"})
        s.invoke("set_chat_tools", {"chatId": chat["id"], "enabled": []})
        s.invoke("rename_chat", {"chatId": chat["id"], "title": "Fine control check"})
        s.execute("window.location.reload();")
        time.sleep(1.5)
        s.wait_for(lambda: "Fine control check" in s.page_text(), 20, "the chat in the sidebar")
        s.execute(
            """
            const a = [...document.querySelectorAll('nav[aria-label="Chat history"] button')].find(b => b.innerText.includes('Fine control check'));
            a && a.click();
            """
        )
        s.wait_for(lambda: os.path.basename(MODEL) in s.execute(
            "const p = document.querySelector('main header button[aria-label=\"Model for this chat\"]'); return p ? p.innerText : '';"),
            15, "the local model in the chat's header")
        s.click_button("Model for this chat")
        click_css('[role=menuitem][aria-label^="Settings for this model"]', "the picker's gear")
        s.wait_text("Settings for this model", 10)
        s.wait_text("Where the model goes", 60)
        page = s.page_text()
        assert "Rough estimate from the model file. Open Fine control to work out exact figures." in page, page[-800:]
        click_css("details[data-section=fine] summary", "Fine control")
        s.wait_for(lambda: "Worked out for these settings." in s.page_text(), 60, "Worked out for these settings.")
        s.screenshot(str(OUT / "m11-fine-control.png"))
        set_select("#fine-threads", "count")
        set_range("#fine-threads-count", 2)
        set_select("#fine-fast-attention", "off")
        s.wait_for(lambda: "Worked out for these settings." in s.page_text(), 60, "the figures for the new choices")
        s.click_button("Load with these settings")
        st = ready(s)

        step("the launch carries -t 2 -fa off, and says where the model went")
        args = the_server(home)[1]
        assert flag(args, "-t") == "2" and flag(args, "-fa") == "off", args
        assert flag(args, "--device") == "none", args
        saved = call(s, "model_load_info", {"path": MODEL})
        assert saved["overrides"]["fine"]["threads"] == {"count": 2}, saved["overrides"]
        assert saved["overrides"]["fine"]["fastAttention"] == "off", saved["overrides"]
        st = s.wait_for(lambda: (x := s.invoke("runtime_status"))["placement"] and x, 30, "the placement")
        placement = st["placement"]
        print("    placement:", json.dumps(placement))
        assert placement["graphics"] is None and placement["layersOnGraphics"] == 0, placement
        assert placement["units"] == facts["units"] and not placement["custom"] and not placement["layersAbout"], placement
        assert placement["computer"]["model"] > 0 and placement["computer"]["context"] > 0, placement
        assert st["offloadedLayers"] == 0 and st["totalLayers"] == facts["units"], st
        assert st["settingsChanged"] is False, st

        step("the usage menu: the processor, the memory, where the model went")
        click_css('main header button[aria-label="Computer usage"]', "the usage line")
        menu = s.wait_for(lambda: s.execute(
            "const m = document.querySelector('[role=menu]'); return m ? m.innerText : false;"), 10, "the usage menu")
        print("    menu:", menu.replace("\n", " | "))
        assert "Processor:" in menu and "threads" in menu, menu
        assert "Memory: " in menu and " of " in menu and " GB in use" in menu, menu
        assert "On the processor only." in menu, menu
        assert "Graphics card" not in menu and "Apple graphics" not in menu, menu
        s.screenshot(str(OUT / "m11-usage-menu.png"))
        escape()

        step("Reset to defaults: the next load has neither flag")
        call(s, "reset_model_load_settings", {"path": MODEL})
        assert s.invoke("runtime_status")["settingsChanged"] is True
        call(s, "reload_local_model", {"force": False, "whenIdle": False})
        ready(s)
        args = the_server(home)[1]
        assert "-t" not in args and "-fa" not in args, args
        call(s, "unload_model")
        assert s.invoke("runtime_status")["placement"] is None
        print("PASS")
        return 0
    except (AssertionError, WebDriverError) as e:
        print("FAIL:", e)
        if s is not None:
            try:
                s.screenshot(str(OUT / "m11-fail.png"))
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

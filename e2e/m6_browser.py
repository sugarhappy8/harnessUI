#!/usr/bin/env python3
"""M6 smoke test in the app: the browser tool is registered, off in a new chat, finds a
browser (Playwright's Chromium here), and Settings → Browser saves its options.

The browser's own behaviour (request blocking, held submissions, pop-ups, Stop) is
tested by crates/browser against a real Chromium; pages on this machine are blocked by
design, so this app-level scenario does not browse.

    python3 -I e2e/m6_browser.py
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
OUT = pathlib.Path(os.environ.get("E2E_OUT", tempfile.mkdtemp(prefix="e2e-m6-")))


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def main() -> int:
    home = OUT / "home"
    shutil.rmtree(home, ignore_errors=True)
    home.mkdir(parents=True)
    env = dict(os.environ)
    env.update(HOME=str(home), XDG_DATA_HOME=str(home / "data"), XDG_CONFIG_HOME=str(home / "config"),
               XDG_CACHE_HOME=str(home / "cache"), RUST_LOG="info")
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

        status = s.invoke("browser_status")
        print("    browser:", status["kind"], status["found"], status["unavailable"])
        assert status["found"] or status["unavailable"], status
        assert not status["running"], status

        chat = s.invoke("create_chat", {"modelRef": None})
        tools = {t["exposed"]: t for t in s.invoke("list_chat_tools", {"chatId": chat["id"]})}
        names = sorted(n for n in tools if n.startswith("browser_"))
        print("    tools:", names)
        assert len(names) == 10, names
        assert not any(tools[n]["enabledInChat"] for n in names), "browser starts off in a chat with no model (its context is not known)"
        servers = {x["name"]: x for x in s.invoke("list_mcp_servers")}
        assert servers["browser"]["builtin"], servers["browser"]

        new = dict(status["settings"], allowPrivateNetworks=True, strictSites=["https://bank.example/login", "not a url"])
        s.invoke("set_browser_settings", {"settings": new})
        saved = s.invoke("browser_status")["settings"]
        assert saved["allowPrivateNetworks"] is True, saved
        assert saved["strictSites"] == ["https://bank.example"], saved

        s.click_button("Settings")
        s.click_button("Browser")
        s.wait_text("Strict sites", 10)
        s.screenshot(str(OUT / "m6-browser-settings.png"))
        print("M6 BROWSER SMOKE PASS")
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

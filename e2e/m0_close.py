#!/usr/bin/env python3
"""Closing the main window quits the app, and it starts again afterwards.

The app has two more windows that are never closed by the user (the hidden border and Stop
bar of computer use). If closing the main window left the process running, starting the app
again would only reach that invisible copy and nothing would appear.

Needs `xdotool` (to find the window) and libX11 (to press its close button).

    python3 -I e2e/m0_close.py
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
LLAMA_DIR = os.environ.get("APP_DEV_LLAMA_DIR", str(ROOT / ".cache" / "llama-linux"))
OUT = pathlib.Path(os.environ.get("E2E_OUT", tempfile.mkdtemp(prefix="e2e-m0-")))


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def app_pids() -> list[str]:
    """Processes running the app binary itself (not the driver, not children by name)."""
    out = subprocess.run(["pgrep", "-f", "-x", APP], capture_output=True, text=True).stdout
    return out.split()


def press_close_button(window: int) -> None:
    """Sends the window the message its close button sends (WM_DELETE_WINDOW)."""
    import ctypes
    import ctypes.util

    x = ctypes.CDLL(ctypes.util.find_library("X11") or "libX11.so.6")
    x.XOpenDisplay.restype = ctypes.c_void_p
    x.XInternAtom.restype = ctypes.c_ulong
    x.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]

    class ClientMessage(ctypes.Structure):
        _fields_ = [
            ("type", ctypes.c_int),
            ("serial", ctypes.c_ulong),
            ("send_event", ctypes.c_int),
            ("display", ctypes.c_void_p),
            ("window", ctypes.c_ulong),
            ("message_type", ctypes.c_ulong),
            ("format", ctypes.c_int),
            ("data", ctypes.c_long * 5),
        ]

    class Event(ctypes.Union):
        _fields_ = [("client", ClientMessage), ("pad", ctypes.c_long * 24)]

    display = x.XOpenDisplay(None)
    assert display, "no X display"
    event = Event()
    event.client.type = 33  # ClientMessage
    event.client.send_event = 1
    event.client.display = display
    event.client.window = window
    event.client.message_type = x.XInternAtom(display, b"WM_PROTOCOLS", 0)
    event.client.format = 32
    event.client.data[0] = x.XInternAtom(display, b"WM_DELETE_WINDOW", 0)
    event.client.data[1] = 0  # CurrentTime
    x.XSendEvent.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_long, ctypes.c_void_p]
    x.XSendEvent(display, window, 0, 0, ctypes.byref(event))
    x.XFlush.argtypes = [ctypes.c_void_p]
    x.XFlush(display)
    x.XCloseDisplay.argtypes = [ctypes.c_void_p]
    x.XCloseDisplay(display)


def start(env):
    port = free_port()
    driver = subprocess.Popen(
        ["tauri-driver", "--port", str(port), "--native-driver", shutil.which("WebKitWebDriver") or "WebKitWebDriver"],
        env=env, stdout=open(OUT / "tauri-driver.log", "a"), stderr=subprocess.STDOUT,
    )
    time.sleep(1.5)
    session = Session(f"http://127.0.0.1:{port}", {"tauri:options": {"application": APP}})
    return driver, session


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
    assert not app_pids(), f"the app is already running: {app_pids()}"
    driver, s = start(env)
    try:
        s.wait_text("Welcome to", 60)
        assert len(app_pids()) == 1, app_pids()

        print("--- closing the main window quits the app", flush=True)
        # What the window's close button does: ask the window to close (WM_DELETE_WINDOW).
        # The WebDriver "close window" command is not that: it leaves the app's window up.
        title = s.execute("return document.title;")
        found = subprocess.run(
            ["xdotool", "search", "--name", f"^{title}$"], capture_output=True, text=True, env=env
        ).stdout.split()
        assert found, f"no window titled {title!r}"
        for window in found:
            press_close_button(int(window))
        deadline = time.time() + 15
        while app_pids() and time.time() < deadline:
            time.sleep(0.25)
        assert not app_pids(), f"the app is still running after its window was closed: {app_pids()}"
    finally:
        driver.terminate()
        subprocess.run(["pkill", "-f", "-x", APP])

    print("--- it starts again", flush=True)
    driver, s = start(env)
    try:
        s.wait_text("Welcome to", 60)
        assert len(app_pids()) == 1, app_pids()
        s.quit()
    finally:
        driver.terminate()
        subprocess.run(["pkill", "-f", "-x", APP])
    print("M0 CLOSE PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())

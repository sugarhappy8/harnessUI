#!/usr/bin/env python3
"""M2 end-to-end: first run, Hugging Face search and download (with the image
projector), the installed list with GGUF badges, loading the model with its projector,
and a chat message with an image. Needs network access to huggingface.co.

Run from the repo root (see e2e/README.md):
    xvfb-run -a python3 -I e2e/m2_models.py
"""

import base64
import os
import pathlib
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time
import zlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "e2e"))
from webdriver import Session  # noqa: E402

APP = os.environ.get("APP_BIN", str(ROOT / "target" / "debug" / "app"))
LLAMA_DIR = os.environ.get("APP_DEV_LLAMA_DIR", str(ROOT / ".cache" / "llama-linux"))
OUT = pathlib.Path(os.environ.get("E2E_OUT", tempfile.mkdtemp(prefix="e2e-m2-")))
REPO = os.environ.get("E2E_VISION_REPO", "ggml-org/SmolVLM-256M-Instruct-GGUF")


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def red_square_png(size: int = 64) -> bytes:
    """A solid red PNG, written with the standard library."""
    raw = b"".join(b"\x00" + b"\xff\x00\x00" * size for _ in range(size))

    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw))
        + chunk(b"IEND", b"")
    )


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

        step("first run shows hardware and catalog suggestions")
        s.wait_text("Welcome to", 60)
        info = s.invoke("first_run_info")
        assert info["done"] is False
        assert info["hardware"]["target"] == "cpu", info["hardware"]
        assert info["suggestions"], info
        picked = next((x for x in info["suggestions"] if x["id"] == info["pick"]), None)
        assert info["pick"] is None or (picked and picked["tested"]), "only tested entries may be preselected"
        s.click_button("Skip for now")
        s.wait_text("New chat", 20)
        assert s.invoke("first_run_info")["done"] is True

        step("search Hugging Face from the Models dialog")
        s.click_button("Models")
        s.wait_text("Installed", 10)
        s.click_button("Browse")
        box = s.wait_for(lambda: s.find("input[placeholder^='Search models']"), 10, "search box")
        s.type(box, "SmolVLM-256M-Instruct")
        s.wait_text(REPO, 30)

        step("repository detail: variants with fit verdicts and the projector")
        s.click_button(REPO)
        s.wait_text("Q8_0", 60)
        view = s.invoke("hub_repo", {"repoId": REPO, "ctx": None})
        assert view["architecture"], view
        assert view["architectureSupported"] is True, view
        assert view["projector"] is not None, view
        labels = [v["label"] for v in view["variants"]]
        assert "Q8_0" in labels, labels
        q8 = next(v for v in view["variants"] if v["label"] == "Q8_0")
        assert q8["fit"] and q8["fit"]["verdict"] == "green", q8
        assert q8["hasChecksum"], q8
        print("    ", q8["fit"]["sentence"])

        step("download the Q8_0 file and its projector")
        group = s.invoke("download_start", {"repoId": REPO, "commit": view["commit"], "label": "Q8_0", "withProjector": True})
        s.wait_for(lambda: any(g["id"] == group and g["status"] == "done" for g in s.invoke("list_downloads")), 600, "download done")
        installed = s.invoke("list_installed_models")
        assert len(installed) == 1, installed
        m = installed[0]
        assert m["source"] == "download" and m["repo"] == REPO, m
        assert m["projectorPath"] and os.path.isfile(m["projectorPath"]), m
        assert m["summary"]["architecture"], m
        print("    installed:", m["fileName"], m["sizeBytes"], "bytes; projector:", os.path.basename(m["projectorPath"]))

        step("the Installed tab shows badges; Use loads the model with its projector")
        s.click_button("Installed")
        s.wait_text("vision", 10)
        s.click_button("Use")
        s.wait_for(lambda: s.invoke("runtime_status")["state"] == "ready", 180, "model ready")
        status = s.invoke("runtime_status")
        assert status["vision"] is True, status

        step("send an image from the composer")
        s.click_button("New chat")
        time.sleep(0.5)
        s.wait_for(lambda: "still loading" not in s.page_text(), 30, "composer enabled")
        png = base64.b64encode(red_square_png()).decode()
        s.execute(
            """
            const b64 = arguments[0];
            const bytes = Uint8Array.from(atob(b64), c => c.charCodeAt(0));
            const file = new File([bytes], 'red.png', {type: 'image/png'});
            const dt = new DataTransfer();
            dt.items.add(file);
            // Dropped on the message box; the drop bubbles up to the composer's handler.
            document.querySelector('textarea').dispatchEvent(
              new DragEvent('drop', {dataTransfer: dt, bubbles: true, cancelable: true}));
            """,
            png,
        )
        s.wait_for(lambda: s.execute("return !!document.querySelector('img[alt=\"red.png\"]');"), 10, "thumbnail")
        box = s.find("textarea")
        s.type(box, "What colour is this image? Answer in one word.")
        s.type(box, "")
        chats = s.wait_for(lambda: s.invoke("list_chats", {"query": None}), 20, "chat")
        chat_id = chats[0]["id"]
        def replied():
            msgs = s.invoke("get_chat", {"chatId": chat_id})["messages"]
            return len(msgs) >= 2 and msgs[-1]["status"] == "complete"

        s.wait_for(replied, 180, "reply")
        detail = s.invoke("get_chat", {"chatId": chat_id})
        user, reply = detail["messages"][0], detail["messages"][-1]
        assert len(user["imageIds"]) == 1, user
        src = s.invoke("get_attachment", {"id": user["imageIds"][0]})
        assert src.startswith("data:image/png;base64,"), src[:40]
        s.wait_for(lambda: s.execute("return !!document.querySelector('img[alt=\"Attached image\"]');"), 10, "image in history")
        print("    reply:", reply["text"][:80].replace("\n", " "))
        s.screenshot(str(OUT / "m2-vision.png"))
        assert "red" in reply["text"].lower(), reply["text"]

        step("delete the model and its files")
        s.invoke("delete_model", {"id": m["id"], "deleteFiles": True})
        assert s.invoke("list_installed_models") == []
        assert not os.path.exists(m["path"])
        print("PASS: M2 end-to-end")
        return 0
    except Exception as e:  # noqa: BLE001
        print("FAIL:", e)
        if s:
            try:
                s.screenshot(str(OUT / "m2-failure.png"))
                print(s.page_text()[:2000])
            except Exception:
                pass
        return 1
    finally:
        if s:
            s.quit()
        driver.terminate()
        print("artifacts in", OUT)


if __name__ == "__main__":
    sys.exit(main())

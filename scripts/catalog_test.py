#!/usr/bin/env python3
"""Catalog test (ARCHITECTURE.md 11 and 17): for one catalog entry, download its files
(pinned commit, SHA-256 checked), start the pinned llama-server, and check

1. template shapes: every message shape the agent loop emits renders through the
   model's chat template (`/apply-template`, no inference);
2. a real tool-call round trip: the model calls a tool, gets the result, and answers.

Prints a JSON result to paste into the entry's "tested" field when it passes.

    python3 scripts/catalog_test.py --list
    python3 scripts/catalog_test.py --entry qwen3.5-4b-q4km --server /path/to/llama-server [--device Vulkan0]

Standard library only. Downloads go to ~/<app>-catalog-test/ unless --models-dir is set.
"""

import argparse
import datetime
import hashlib
import json
import os
import pathlib
import platform
import secrets
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
CATALOG = json.loads((ROOT / "catalog" / "models.json").read_text())
LOCK = json.loads((ROOT / "llama.lock.json").read_text())
UA = "catalog-test/1"


def log(msg):
    print(msg, file=sys.stderr, flush=True)


def download(repo: str, commit: str, f: dict, dest_dir: pathlib.Path) -> pathlib.Path:
    dest = dest_dir / repo.replace("/", "__") / f["path"]
    dest.parent.mkdir(parents=True, exist_ok=True)
    if dest.exists() and dest.stat().st_size == f["size"]:
        log(f"  have {f['path']}")
    else:
        url = f"https://huggingface.co/{repo}/resolve/{commit}/{f['path']}"
        part = dest.with_suffix(dest.suffix + ".part")
        have = part.stat().st_size if part.exists() else 0
        req = urllib.request.Request(url, headers={"User-Agent": UA, **({"Range": f"bytes={have}-"} if have else {})})
        with urllib.request.urlopen(req, timeout=120) as resp, open(part, "ab" if have and resp.status == 206 else "wb") as out:
            done = have if resp.status == 206 else 0
            while True:
                chunk = resp.read(1 << 20)
                if not chunk:
                    break
                out.write(chunk)
                done += len(chunk)
                print(f"\r  {f['path']}: {done / 1e9:.2f} / {f['size'] / 1e9:.2f} GB", end="", file=sys.stderr)
        print(file=sys.stderr)
        part.rename(dest)
    h = hashlib.sha256()
    with open(dest, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    if h.hexdigest() != f["sha256"]:
        dest.unlink()
        raise SystemExit(f"checksum mismatch for {f['path']}")
    return dest


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def post(port, key, path, body, timeout=600):
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}{path}",
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json", "Authorization": f"Bearer {key}"},
        method="POST",
    )
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(req, timeout=timeout) as resp:
        return json.loads(resp.read())


WEATHER_TOOL = {
    "type": "function",
    "function": {
        "name": "get_weather",
        "description": "Current weather for a city.",
        "parameters": {"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]},
    },
}

# Every message shape the agent loop can emit (8.2, 8.4): several calls in one message,
# synthetic denied and not-run results, an app note, a summary block, a stopped turn.
SHAPES = [
    {"role": "system", "content": "You are a helpful assistant.\n\n[Summary of earlier messages]\nThe user asked about weather."},
    {"role": "user", "content": "What is the weather in Paris and Rome?"},
    {"role": "assistant", "content": None, "tool_calls": [
        {"id": "call_1", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\": \"Paris\"}"}},
        {"id": "call_2", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\": \"Rome\"}"}},
    ]},
    {"role": "tool", "tool_call_id": "call_1", "content": "Sunny, 21 C"},
    {"role": "tool", "tool_call_id": "call_2", "content": "The user denied this tool call."},
    {"role": "assistant", "content": "Paris is sunny. I could not check Rome."},
    {"role": "user", "content": "[App note: the previous reply was stopped by the user.]\nTry Rome again."},
    {"role": "assistant", "content": None, "tool_calls": [
        {"id": "call_3", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\": \"Rome\"}"}},
    ]},
    {"role": "tool", "tool_call_id": "call_3", "content": "Not run: the step limit was reached."},
    {"role": "assistant", "content": "(stopped)"},
    {"role": "user", "content": "Thanks."},
]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--entry")
    ap.add_argument("--server", help="path to llama-server from the app or scripts/fetch_llama.py")
    ap.add_argument("--device", default=None, help="for example Vulkan0, ROCm0, MTL0; default CPU")
    ap.add_argument("--ctx", type=int, default=8192)
    ap.add_argument("--models-dir", default=str(pathlib.Path.home() / "harnessUI-catalog-test"))
    a = ap.parse_args()
    if a.list or not a.entry:
        for tier, t in sorted(CATALOG["tiers"].items(), key=lambda kv: int(kv[0])):
            for mid in t["models"]:
                m = CATALOG["models"][mid]
                size = sum(f["size"] for f in m["files"]) / 1e9
                print(f"{tier:>3} GB  {mid:28} {m['name']} ({size:.1f} GB) tested={bool(m.get('tested'))}")
        return 0
    entry = CATALOG["models"][a.entry]
    if not a.server or not os.path.isfile(a.server):
        raise SystemExit("--server must point at llama-server")
    log(f"entry {a.entry}: {entry['name']} from {entry['repo']}@{entry['revision'][:10]}")
    paths = [download(entry["repo"], entry["revision"], f, pathlib.Path(a.models_dir)) for f in entry["files"]]
    port, key = free_port(), secrets.token_hex(16)
    args = [a.server, "-m", str(paths[0]), "--host", "127.0.0.1", "--port", str(port), "-c", str(a.ctx),
            "-np", "1", "--jinja", "--no-webui", "--api-key", key]
    if a.device:
        args += ["--device", a.device, "-ngl", "999"]
    else:
        args += ["--device", "none", "-ngl", "0"]
    log("starting llama-server")
    server = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=open(pathlib.Path(a.models_dir) / "server.log", "w"))
    results = {"entry": a.entry, "templateShapes": False, "toolRoundTrip": False}
    try:
        for _ in range(900):
            try:
                urllib.request.build_opener(urllib.request.ProxyHandler({})).open(f"http://127.0.0.1:{port}/health", timeout=2)
                break
            except Exception:
                if server.poll() is not None:
                    raise SystemExit("llama-server exited; see server.log")
                time.sleep(1)
        log("template shapes")
        rendered = post(port, key, "/apply-template", {"messages": SHAPES, "tools": [WEATHER_TOOL]})
        prompt = rendered.get("prompt", "")
        assert "Paris" in prompt and "Rome" in prompt and "denied" in prompt and "step limit" in prompt, prompt[:400]
        results["templateShapes"] = True

        log("tool-call round trip")
        msgs = [{"role": "user", "content": "What is the weather in Paris? Use the tool."}]
        r1 = post(port, key, "/v1/chat/completions", {"messages": msgs, "tools": [WEATHER_TOOL], "tool_choice": "auto",
                                                      "max_tokens": 1024, **entry.get("sampling", {})})
        calls = r1["choices"][0]["message"].get("tool_calls") or []
        assert calls and calls[0]["function"]["name"] == "get_weather", r1["choices"][0]["message"]
        args_obj = json.loads(calls[0]["function"]["arguments"])
        assert "paris" in str(args_obj.get("city", "")).lower(), args_obj
        msgs += [r1["choices"][0]["message"], {"role": "tool", "tool_call_id": calls[0]["id"], "content": "Sunny, 21 C"}]
        r2 = post(port, key, "/v1/chat/completions", {"messages": msgs, "tools": [WEATHER_TOOL], "max_tokens": 1024,
                                                      **entry.get("sampling", {})})
        answer = r2["choices"][0]["message"].get("content") or ""
        assert "21" in answer or "sunny" in answer.lower(), answer
        results["toolRoundTrip"] = True
        log("PASS")
    finally:
        server.terminate()
        try:
            server.wait(10)
        except subprocess.TimeoutExpired:
            server.kill()
    tested = {
        "llamaBuild": LOCK["llama_cpp"]["tag"],
        "date": datetime.date.today().isoformat(),
        "hardware": f"{platform.system()} {platform.machine()} {a.device or 'CPU'}",
    }
    print(json.dumps({**results, "tested": tested}, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())

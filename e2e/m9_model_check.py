#!/usr/bin/env python3
"""M9 end-to-end: the capability check and a benchmark against a real model.

Loads the test model (Qwen3 0.6B by default) in the real app, starts "Check this model"
in Settings, Local models, through its confirmation, and waits for it to end by itself.
What must hold whatever the model manages: the results table lists every setting tried
with a passed or failed mark per probe, the temporary folders are gone, the audit log
has the check's own row and its call rows with no chat, and nothing was written outside
the temporary folders (the approved folder and the home folder are untouched). It does
not assert that probes pass: a small model may fail them; what it passed is printed.

Then it starts a benchmark the same way, lets two tasks finish, ends it with "End the
benchmark", and checks that it stops within a few seconds and that the partial run is
saved, marked as ended early.

    python3 -I e2e/m9_model_check.py
"""

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
from webdriver import Session  # noqa: E402

APP = os.environ.get("APP_BIN", str(ROOT / "target" / "debug" / "app"))
MODEL = os.environ.get("TEST_GGUF", "/home/user/llama-dev/Qwen3-0.6B-Q8_0.gguf")
LLAMA_DIR = os.environ.get("APP_DEV_LLAMA_DIR", str(ROOT / ".cache" / "llama-linux"))
OUT = pathlib.Path(os.environ.get("E2E_OUT", tempfile.mkdtemp(prefix="e2e-m9-")))
IDENTIFIER = "io.github.sugarhappy8.harnessui"
# A tiny model on a CPU is slow: up to four settings times three probes.
CHECK_MINUTES = float(os.environ.get("M9_CHECK_MINUTES", "60"))


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def step(name):
    print(f"--- {name}", flush=True)


def tree(folder: pathlib.Path, skip: tuple[str, ...] = ()) -> dict[str, float]:
    """Every file under `folder` with its modification time, except under `skip`."""
    out = {}
    for p in folder.rglob("*"):
        rel = p.relative_to(folder).as_posix()
        if p.is_file() and not any(rel == s or rel.startswith(s + "/") for s in skip):
            out[rel] = p.stat().st_mtime
    return out


def runs_left(temp: pathlib.Path) -> list[str]:
    return sorted(p.name for p in temp.glob("harness-run-*")) + sorted(p.name for p in temp.glob("harness-verify-*"))


def main() -> int:
    home = OUT / "home"
    shutil.rmtree(home, ignore_errors=True)
    home.mkdir(parents=True)
    work = (OUT / "work").resolve()
    shutil.rmtree(work, ignore_errors=True)
    work.mkdir()
    (work / "notes.txt").write_text("An approved folder the check must not touch.\n")
    temp = pathlib.Path(tempfile.gettempdir())
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
        con = sqlite3.connect(db)
        con.execute("INSERT INTO approved_folders(path, chat_id, created_at) VALUES (?, NULL, ?)", (str(work), int(time.time() * 1000)))
        con.commit()
        con.close()

        step("load the model")
        settings = {"ctx": 8192, "backend": None, "device": None}
        s.invoke("set_local_model_settings", {"settings": settings})
        s.invoke("load_local_model", {"path": MODEL, "settings": settings})
        status = s.invoke("runtime_status")
        assert status["state"] == "ready", status
        print("    backend:", status["backend"], "supportsTools:", status["supportsTools"], "thinking:", status["thinking"])
        model_ref = f"local:{MODEL}"

        temp_before = runs_left(temp)
        work_before = tree(work)
        # The app's own folders change (database, logs, cache); nothing else in the home
        # folder may appear or change.
        own = ("data", "config", "cache", ".cache", ".local", ".config")
        home_before = tree(home, own)

        step("capability check: Settings, Local models, through the confirmation")
        s.click_button("Settings")
        s.click_button("Local models")
        s.wait_text("Tested settings", 10)
        time.sleep(0.5)
        s.click_button("Check this model")
        s.wait_text("Start the check", 10)
        page = s.page_text()
        assert "temporary folder" in page and "Nothing is asked while it runs" in page, page[-600:]
        time.sleep(0.9)
        started = time.time()
        s.click_button("Start the check")
        last = ""

        def check_done():
            nonlocal last
            line = s.execute("const p = document.querySelector('[role=status]'); return p ? p.innerText : '';")
            if line and line != last:
                print(f"    [{int(time.time() - started)} s] {line}", flush=True)
                last = line
            return s.execute("return document.querySelector('table[aria-label=\"Check results\"]') !== null && !document.body.innerText.includes('End the check');")

        s.wait_for(check_done, CHECK_MINUTES * 60, "the check to end")
        print(f"    the check took {int(time.time() - started)} s")
        time.sleep(0.8)
        s.screenshot(str(OUT / "m9-check.png"))
        view = s.invoke("get_model_profile", {"modelRef": model_ref})
        profile = view["profile"]
        assert profile, view
        expected = ["Default", "Thinking off", "Temperature 0.2", "Thinking off, temperature 0.2"] if status["thinking"] else ["Default", "Temperature 0.2"]
        assert [v["label"] for v in profile["results"]] == expected, [v["label"] for v in profile["results"]]
        table = s.execute("return document.querySelector('table[aria-label=\"Check results\"]').innerText;")
        print("    table:\n      " + table.replace("\n", "\n      "))
        rows = [r for r in table.splitlines()[1:] if r.strip()]
        assert len(rows) == len(expected), rows
        for row in rows:
            marks = row.split("\t")[1:4]
            assert all(m in ("passed", "failed") for m in marks), row
        print("    chosen:", profile["chosen"], "defaults:", profile["defaults"])
        for v in profile["results"]:
            for p in v["probes"]:
                print(f"    {v['label']:<32} {p['probe']:<14} {'PASS' if p['passed'] else 'fail'}  "
                      f"calls={p['arrival']} failures={p['toolFailures']} failedEdits={p['failedEdits']} "
                      f"blocked={p['blocked']} steps={p['steps']} status={p['status']} {p['timeMs'] // 1000} s: {p['note']}")

        step("after the check: folders and audit")
        assert runs_left(temp) == temp_before, (temp_before, runs_left(temp))
        assert tree(work) == work_before, "the approved folder changed"
        home_after = tree(home, own)
        assert home_after == home_before, sorted(set(home_after) ^ set(home_before))
        audit = s.invoke("list_audit", {"filter": {"limit": 1000}})
        run_rows = [r for r in audit if r["server"] == "evals" and r["tool"] == "capability_check"]
        assert len(run_rows) == 1 and run_rows[0]["result_status"] == "ok", run_rows
        calls = [r for r in audit if r.get("parent_audit_id") == run_rows[0]["id"]]
        assert all(r["chat_id"] is None for r in calls + run_rows)
        assert all(r["preview"].startswith("Model check: ") for r in calls)
        print(f"    audit: 1 run row, {len(calls)} call rows; decisions:",
              sorted({(r['tool'], r['decision']) for r in calls}))
        blocked = [r for r in calls if r["decision"] == "refused_by_policy" and r["result_status"] == "refused"]
        for r in blocked:
            print("    blocked:", r["tool"], (r["result_summary"] or "")[:160])

        step("benchmark: Settings, Diagnostics, through the confirmation; ended after two tasks")
        s.click_button("Diagnostics")
        s.wait_text("Coding benchmarks", 10)
        time.sleep(0.5)
        s.click_button("Run the benchmark")
        s.wait_text("Start the benchmark", 10)
        time.sleep(0.9)
        started = time.time()
        s.click_button("Start the benchmark")
        last = ""

        def third_task():
            nonlocal last
            line = s.execute("const p = document.querySelector('[role=status]'); return p ? p.innerText : '';")
            if line and line != last:
                print(f"    [{int(time.time() - started)} s] {line}", flush=True)
                last = line
            return "(3 of " in line

        s.wait_for(third_task, 40 * 60, "the third task to start")
        time.sleep(5)
        s.click_button("End the benchmark")
        ended = time.time()
        s.wait_for(lambda: "End the benchmark" not in s.page_text(), 30, "the benchmark to stop")
        took = time.time() - ended
        print(f"    stopped {took:.1f} s after End the benchmark")
        assert took < 5, took
        time.sleep(0.8)
        s.screenshot(str(OUT / "m9-bench.png"))
        runs = s.invoke("list_bench_runs")
        assert len(runs) == 1 and runs[0]["cancelled"], runs
        run = s.invoke("get_bench_run", {"id": runs[0]["id"]})
        statuses = [(t["name"], t["status"]) for t in run["tasks"]]
        print("    partial run saved:", statuses)
        assert len(run["tasks"]) == 3 and run["tasks"][-1]["status"] == "stopped", statuses
        for t in run["tasks"]:
            print(f"    {t['name']:<24} {t['status']:<8} brokenEdits={t['brokenEdits']} failures={t['toolFailures']} "
                  f"blocked={t['blocked']} steps={t['steps']} calls={t['arrival']} turn={t['runStatus']} {t['timeMs'] // 1000} s: {t['note'][:160]}")
        assert runs_left(temp) == temp_before, (temp_before, runs_left(temp))
        assert tree(work) == work_before, "the approved folder changed"
        assert tree(home, own) == home_before
        audit = s.invoke("list_audit", {"filter": {"limit": 2000}})
        bench_rows = [r for r in audit if r["server"] == "evals" and r["tool"] == "benchmark"]
        assert len(bench_rows) == 1 and bench_rows[0]["result_status"] == "cancelled", bench_rows
        bench_calls = [r for r in audit if r.get("parent_audit_id") == bench_rows[0]["id"]]
        print(f"    audit: 1 run row (cancelled), {len(bench_calls)} call rows")
        for r in bench_calls:
            if r["decision"] == "refused_by_policy" and r["result_status"] == "refused":
                print("    blocked:", r["tool"], str(r["arguments"])[:120], "->", (r["result_summary"] or "")[:160])
        print("M9 E2E PASS")
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

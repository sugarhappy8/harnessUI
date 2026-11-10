"""A tiny W3C WebDriver client (standard library only) for driving the app through
tauri-driver. See e2e/README.md."""

import http.client
import json
import time
import urllib.error
import urllib.request


class WebDriverError(Exception):
    pass


class Session:
    def __init__(self, base: str, capabilities: dict):
        self.base = base.rstrip("/")
        r = self._req("POST", "/session", {"capabilities": {"alwaysMatch": capabilities}})
        self.id = r["sessionId"]

    def _req(self, method: str, path: str, body=None):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(self.base + path, data=data, method=method)
        req.add_header("Content-Type", "application/json")
        for attempt in range(3):
            try:
                with urllib.request.urlopen(req, timeout=600) as resp:
                    payload = json.loads(resp.read().decode() or "{}")
                return payload.get("value", payload)
            except urllib.error.HTTPError as e:
                raise WebDriverError(f"{method} {path}: {e.code} {e.read().decode()[:500]}") from None
            except (ConnectionError, http.client.RemoteDisconnected, urllib.error.URLError) as e:
                # WebKitWebDriver occasionally drops a connection; the session survives.
                if attempt == 2:
                    raise WebDriverError(f"{method} {path}: {e}") from None
                time.sleep(1.0)
        raise WebDriverError("unreachable")

    def _s(self, path: str) -> str:
        return f"/session/{self.id}{path}"

    def quit(self):
        try:
            self._req("DELETE", self._s(""))
        except Exception:
            pass

    def execute(self, script: str, *args):
        return self._req("POST", self._s("/execute/sync"), {"script": script, "args": list(args)})

    def execute_async(self, script: str, *args):
        return self._req("POST", self._s("/execute/async"), {"script": script, "args": list(args)})

    def invoke(self, command: str, args: dict | None = None):
        """Calls a Tauri command through the webview's IPC, as the UI does."""
        result = self.execute_async(
            """
            const [cmd, args, done] = arguments;
            window.__TAURI_INTERNALS__.invoke(cmd, args)
              .then(v => done({ok: true, value: v}))
              .catch(e => done({ok: false, error: e}));
            """,
            command,
            args or {},
        )
        if not result.get("ok"):
            raise WebDriverError(f"{command} failed: {json.dumps(result.get('error'))[:800]}")
        return result.get("value")

    def find(self, css: str):
        r = self._req("POST", self._s("/element"), {"using": "css selector", "value": css})
        return next(iter(r.values()))

    def find_all(self, css: str):
        r = self._req("POST", self._s("/elements"), {"using": "css selector", "value": css})
        return [next(iter(e.values())) for e in r]

    def click(self, element_id: str):
        self._req("POST", self._s(f"/element/{element_id}/click"), {})

    def type(self, element_id: str, text: str):
        self._req("POST", self._s(f"/element/{element_id}/value"), {"text": text})

    def text(self, element_id: str) -> str:
        return self._req("GET", self._s(f"/element/{element_id}/text"))

    def page_text(self) -> str:
        return self.execute("return document.body.innerText;")

    def screenshot(self, path: str):
        import base64

        png = self._req("GET", self._s("/screenshot"))
        with open(path, "wb") as f:
            f.write(base64.b64decode(png))

    def wait_for(self, predicate, timeout: float = 60, what: str = "condition"):
        end = time.time() + timeout
        last = None
        while time.time() < end:
            try:
                last = predicate()
                if last:
                    return last
            except WebDriverError:
                pass
            time.sleep(0.25)
        raise WebDriverError(f"timed out waiting for {what} (last: {str(last)[:200]})")

    def wait_text(self, needle: str, timeout: float = 60):
        return self.wait_for(lambda: needle in self.page_text(), timeout, f"text {needle!r}")

    def click_button(self, label: str, timeout: float = 20):
        """Clicks (with a real pointer click) the first visible button or menu item whose
        text contains `label` or whose aria-label equals it."""

        def find():
            el = self.execute(
                """
                const label = arguments[0];
                const visible = e => !!(e.offsetWidth || e.offsetHeight || e.getClientRects().length);
                return [...document.querySelectorAll('button,[role=menuitem],[role=radio],[role=option]')]
                  .find(b => visible(b) && ((b.innerText || '').includes(label) || (b.getAttribute('aria-label') || '') === label)) || null;
                """,
                label,
            )
            return el or False

        el = self.wait_for(find, timeout, f"button {label!r}")
        element_id = next(iter(el.values()))
        try:
            self.click(element_id)
        except WebDriverError:
            # Covered by an overlay or off screen: fall back to a DOM click.
            self.execute("arguments[0].click();", el)

import { useCallback, useEffect, useState } from "react";
import { AlertTriangle } from "lucide-react";
import * as ipc from "../lib/ipc";
import type { BrowserSettings as Settings, BrowserStatus, ErrorInfo } from "../lib/types";
import { toErrorInfo } from "../store/app";
import { ErrorBanner } from "./ErrorBanner";

/** Settings → Browser: which browser the agent uses and how far it may go (9.3). */
export function BrowserSettings() {
  const [status, setStatus] = useState<BrowserStatus | null>(null);
  const [strict, setStrict] = useState("");
  const [error, setError] = useState<ErrorInfo | null>(null);

  const refresh = useCallback(() => {
    void ipc
      .browserStatus()
      .then((s) => {
        setStatus(s);
        setStrict(s.settings.strictSites.join("\n"));
      })
      .catch((e) => setError(toErrorInfo(e)));
  }, []);
  useEffect(refresh, [refresh]);

  async function save(next: Settings) {
    try {
      await ipc.setBrowserSettings(next);
      refresh();
    } catch (e) {
      setError(toErrorInfo(e));
    }
  }

  if (!status) return error ? <ErrorBanner error={error} onDismiss={() => setError(null)} /> : null;
  const s = status.settings;
  return (
    <div className="space-y-4 text-sm">
      <h2 className="text-lg font-medium">Browser</h2>
      <p className="text-fg-muted">
        The browser tool drives its own Chrome or Edge window with a separate profile in the app&apos;s data folder: it
        never sees your own browser&apos;s cookies, logins or history. Form submissions and leaving a site always ask.
      </p>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <section className="space-y-1">
        <h3 className="font-medium">Browser used</h3>
        {status.found ? (
          <p className="font-mono text-xs break-all">
            {status.kind}: {status.found}
          </p>
        ) : (
          <p className="text-warning">{status.unavailable}</p>
        )}
        <div className="flex gap-2">
          <button
            type="button"
            onClick={() =>
              void ipc
                .chooseBrowserPath(false)
                .then(refresh)
                .catch((e) => setError(toErrorInfo(e)))
            }
            className="rounded-md border border-border px-3 py-1 hover:bg-bg-muted"
          >
            Choose a browser…
          </button>
          {s.path && (
            <button
              type="button"
              onClick={() =>
                void ipc
                  .chooseBrowserPath(true)
                  .then(refresh)
                  .catch((e) => setError(toErrorInfo(e)))
              }
              className="px-3 py-1 text-fg-muted underline"
            >
              Use the installed one
            </button>
          )}
        </div>
      </section>
      <label className="flex items-start gap-2">
        <input
          type="checkbox"
          className="mt-1"
          checked={s.allowPrivateNetworks}
          onChange={(e) => void save({ ...s, allowPrivateNetworks: e.target.checked })}
        />
        <span>
          Allow pages on private networks (your router, a NAS, an intranet). This computer&apos;s own addresses stay
          blocked, including the local model server.
        </span>
      </label>
      <label className="flex items-start gap-2">
        <input
          type="checkbox"
          className="mt-1"
          checked={s.loopbackPort}
          onChange={(e) => void save({ ...s, loopbackPort: e.target.checked })}
        />
        <span>
          Connect through a local port instead of a pipe, for sites that refuse automated browsers.
          {s.loopbackPort && (
            <span className="mt-1 flex items-start gap-1 text-xs text-warning">
              <AlertTriangle size={12} className="mt-0.5 shrink-0" aria-hidden />
              While the agent&apos;s browser runs, any program on this computer can connect to that port and control it.
            </span>
          )}
        </span>
      </label>
      <section className="space-y-1">
        <label className="font-medium" htmlFor="strict-sites">
          Strict sites
        </label>
        <p className="text-xs text-fg-muted">
          On these sites, data a page sends in the background (not only form submissions) also asks first. One address
          per line, e.g. https://bank.example.
        </p>
        <textarea
          id="strict-sites"
          value={strict}
          onChange={(e) => setStrict(e.target.value)}
          rows={3}
          className="w-full rounded-md border border-border bg-bg p-2 font-mono text-xs"
        />
        <button
          type="button"
          onClick={() => void save({ ...s, strictSites: strict.split(/\s+/).filter(Boolean) })}
          className="rounded-md border border-border px-3 py-1 hover:bg-bg-muted"
        >
          Save strict sites
        </button>
      </section>
    </div>
  );
}

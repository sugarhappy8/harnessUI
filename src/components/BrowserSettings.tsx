import { useCallback, useEffect, useState } from "react";
import { AlertTriangle, Download, Trash2, X } from "lucide-react";
import * as ipc from "../lib/ipc";
import type { AgentBrowserProgress, BrowserSettings as Settings, BrowserStatus, ErrorInfo } from "../lib/types";
import { formatBytes } from "../lib/format";
import { toErrorInfo } from "../store/app";
import { ErrorBanner } from "./ErrorBanner";

/** "about 200 MB": the download size to the nearest 10 MB. */
function roughSize(bytes: number): string {
  return `about ${Math.round(bytes / 1e7) * 10} MB`;
}

/**
 * Settings → Browser: which browser the agent uses and how far it may go (9.3). On a Mac
 * with no browser the agent can drive, it also offers a one-click download; whether to
 * offer it is the backend's decision (`own.offered`), not this page's.
 */
export function BrowserSettings() {
  const [status, setStatus] = useState<BrowserStatus | null>(null);
  const [strict, setStrict] = useState("");
  const [error, setError] = useState<ErrorInfo | null>(null);
  const [progress, setProgress] = useState<AgentBrowserProgress | null>(null);

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

  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    void ipc
      .onAgentBrowserProgress((p) => {
        if (!alive) return;
        if (p.finished) {
          setProgress(null);
          if (p.error) setError(p.error);
          refresh();
        } else {
          setProgress(p);
        }
      })
      .then((u) => {
        if (alive) unlisten = u;
        else u();
      });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, [refresh]);

  async function downloadOwn() {
    try {
      setError(null);
      await ipc.downloadAgentBrowser();
      refresh();
    } catch (e) {
      setError(toErrorInfo(e));
    }
  }

  async function removeOwn(question: string) {
    if (!window.confirm(question)) return;
    try {
      await ipc.removeAgentBrowser();
      refresh();
    } catch (e) {
      setError(toErrorInfo(e));
    }
  }

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
  const own = status.own;
  const downloading = own.state === "downloading" || progress !== null;
  // Offered only when nothing else can be used; an installed or half-downloaded one always
  // shows, so it can be removed.
  const showOwn = own.state !== "notInstalled" || (own.offered && !status.found && !s.path);
  const pct = progress && progress.total > 0 ? Math.min(100, (progress.done / progress.total) * 100) : 0;
  return (
    <div className="space-y-4 text-sm">
      <h2 className="text-lg font-medium">Browser</h2>
      <p className="text-fg-muted">
        The browser tool drives a browser window of its own (Chrome, Edge, Brave, Vivaldi or Chromium) with a separate
        profile in the app&apos;s data folder: it never sees your own browser&apos;s cookies, logins or history. Form
        submissions and leaving a site always ask.
      </p>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <section className="space-y-1">
        <h3 className="font-medium">Browser used</h3>
        {status.found ? (
          <>
            <p>{status.kind}</p>
            <p className="font-mono text-xs break-all text-fg-muted">{status.found}</p>
          </>
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
      {showOwn && (
        <section className="space-y-2 rounded-md border border-border p-3" aria-labelledby="own-browser-title">
          <h3 id="own-browser-title" className="font-medium">
            A browser for the agent
          </h3>
          {own.state === "installed" ? (
            <>
              <p>Installed (version {own.version}).</p>
              <p className="text-xs text-fg-muted">
                {own.inUse
                  ? "The agent uses this browser."
                  : status.found
                    ? `Not in use: ${status.kind} is used instead.`
                    : "Not in use."}
              </p>
            </>
          ) : (
            <p>
              No browser the agent can use was found on this Mac. Safari cannot be driven by the agent. You can
              download one here.
            </p>
          )}
          <p className="text-xs text-fg-muted">
            It is Google&apos;s &ldquo;Chrome for Testing&rdquo; build, used only by the agent with its own profile. It
            does not update itself. The alternative that stays up to date is to install Google Chrome or Microsoft
            Edge: the agent then uses that instead.
          </p>
          {own.state === "installed" && own.offered && own.version !== own.pinnedVersion && (
            <p className="text-xs text-fg-muted">
              This version of the app comes with a newer build (version {own.pinnedVersion}). To get it, remove this
              one and download again.
            </p>
          )}
          {own.state === "installed" ? (
            <button
              type="button"
              onClick={() => void removeOwn("Remove the agent's browser? You can download it again later.")}
              className="flex items-center gap-1 rounded-md border border-border px-3 py-1 hover:bg-bg-muted"
            >
              <Trash2 size={13} aria-hidden /> Remove
            </button>
          ) : downloading ? (
            <div>
              <div
                className="h-1.5 overflow-hidden rounded bg-bg-muted"
                role="progressbar"
                aria-label="Browser download"
                aria-valuenow={Math.round(pct)}
                aria-valuemin={0}
                aria-valuemax={100}
              >
                <div className="h-full bg-accent" style={{ width: `${pct}%` }} />
              </div>
              <div className="mt-1 flex items-center justify-between text-xs text-fg-muted">
                <span>
                  {progress?.unpacking
                    ? "Checking and unpacking…"
                    : `${formatBytes(progress?.done ?? own.bytesDone)} of ${formatBytes(own.downloadBytes)}`}
                </span>
                {!progress?.unpacking && (
                  <button
                    type="button"
                    onClick={() => void ipc.cancelAgentBrowserDownload().catch((e) => setError(toErrorInfo(e)))}
                    className="flex items-center gap-1 hover:text-danger"
                  >
                    <X size={13} aria-hidden /> Stop
                  </button>
                )}
              </div>
            </div>
          ) : (
            <div className="flex flex-wrap items-center gap-3">
              {own.offered && (
                <button
                  type="button"
                  onClick={() => void downloadOwn()}
                  className="flex items-center gap-1 rounded-md bg-accent px-3 py-1 text-accent-fg"
                >
                  <Download size={13} aria-hidden />
                  {own.state === "partial"
                    ? `Continue the download (${formatBytes(own.bytesDone)} of ${formatBytes(own.downloadBytes)} done)`
                    : `Download a browser for the agent (${roughSize(own.downloadBytes)})`}
                </button>
              )}
              {own.state === "partial" && (
                <button
                  type="button"
                  onClick={() => void removeOwn("Delete the part that is already downloaded?")}
                  className="text-xs text-fg-muted underline"
                >
                  Delete the partial download
                </button>
              )}
            </div>
          )}
        </section>
      )}
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

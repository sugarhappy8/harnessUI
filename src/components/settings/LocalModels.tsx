import { useCallback, useEffect, useState } from "react";
import { RefreshCw } from "lucide-react";
import * as ipc from "../../lib/ipc";
import type { BackendInfo, ErrorInfo, LocalModelSettings } from "../../lib/types";
import { toErrorInfo } from "../../store/app";
import { ErrorBanner } from "../ErrorBanner";
import { BackendDownloads } from "../BackendDownloads";

/**
 * Settings → Local models: the backend, the context size and the detected backends
 * (moved here from SettingsDialog.tsx in 0.0.5; WP2 adds Defaults for all models).
 */
export function LocalModels() {
  const [backends, setBackends] = useState<BackendInfo[] | null>(null);
  const [settings, setSettings] = useState<LocalModelSettings | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);

  const load = useCallback(async (reprobe: boolean) => {
    setBusy(true);
    try {
      setBackends(await ipc.listBackends(reprobe));
      setSettings(await ipc.getLocalModelSettings());
    } catch (err) {
      setError(toErrorInfo(err));
    } finally {
      setBusy(false);
    }
  }, []);
  const reprobe = useCallback(() => void load(true), [load]);
  useEffect(() => {
    let alive = true;
    Promise.all([ipc.listBackends(false), ipc.getLocalModelSettings()])
      .then(([b, s]) => {
        if (alive) {
          setBackends(b);
          setSettings(s);
        }
      })
      .catch((err) => alive && setError(toErrorInfo(err)));
    return () => {
      alive = false;
    };
  }, []);

  async function save(next: LocalModelSettings) {
    setSettings(next);
    try {
      await ipc.setLocalModelSettings(next);
    } catch (err) {
      setError(toErrorInfo(err));
    }
  }

  return (
    <div className="space-y-5">
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      {settings && (
        <div className="grid grid-cols-[10rem_1fr] items-center gap-3 text-sm">
          <label htmlFor="backend">Backend</label>
          <select
            id="backend"
            value={settings.backend ?? "auto"}
            onChange={(e) =>
              void save({ ...settings, backend: e.target.value === "auto" ? null : e.target.value })
            }
            className="rounded-md border border-border bg-bg px-2 py-1"
          >
            <option value="auto">Auto (recommended)</option>
            {(backends ?? []).map((b) => (
              <option key={b.kind} value={b.kind} disabled={!b.available}>
                {b.kind}
                {b.available ? "" : " (unavailable)"}
              </option>
            ))}
          </select>
          <label htmlFor="ctx">Context size</label>
          <select
            id="ctx"
            value={settings.ctx}
            onChange={(e) => void save({ ...settings, ctx: Number(e.target.value) })}
            className="rounded-md border border-border bg-bg px-2 py-1"
          >
            {[2048, 4096, 8192, 16384, 32768, 65536].map((n) => (
              <option key={n} value={n}>
                {n.toLocaleString()} tokens
              </option>
            ))}
          </select>
        </div>
      )}
      <p className="text-xs text-fg-muted">
        Changes apply the next time a model loads. A larger context needs more memory.
      </p>
      <section>
        <div className="mb-2 flex items-center justify-between">
          <h3 className="text-sm font-medium">Detected backends</h3>
          <button
            type="button"
            onClick={() => void load(true)}
            disabled={busy}
            className="flex items-center gap-1 text-xs text-fg-muted hover:text-fg disabled:opacity-40"
          >
            <RefreshCw size={12} aria-hidden className={busy ? "animate-spin" : ""} /> Check again
          </button>
        </div>
        <ul className="space-y-2 text-sm">
          {(backends ?? []).map((b) => (
            <li key={b.kind} className="rounded-md border border-border px-3 py-2">
              <div className="flex justify-between">
                <span className="font-medium">{b.kind}</span>
                <span className={b.available ? "text-success" : "text-fg-muted"}>
                  {b.available ? "available" : "unavailable"}
                </span>
              </div>
              {b.devices.map((d) => (
                <p key={d.name} className="text-xs text-fg-muted">
                  {d.name}: {d.description}
                  {d.totalMib > 0 ? ` · ${(d.totalMib / 1024).toFixed(1)} GB` : ""}
                </p>
              ))}
              {b.error && <p className="text-xs text-fg-muted">{b.error}</p>}
            </li>
          ))}
          {backends?.length === 0 && (
            <li className="text-fg-muted">No llama.cpp runtime was found in this build.</li>
          )}
        </ul>
      </section>
      <BackendDownloads onChanged={reprobe} />
    </div>
  );
}

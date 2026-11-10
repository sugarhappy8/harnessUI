import { useCallback, useEffect, useState } from "react";
import { AlertTriangle, CheckCircle2 } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import * as ipc from "../lib/ipc";
import type { ComputerStatus, Convention, ErrorInfo } from "../lib/types";
import { toErrorInfo, useApp } from "../store/app";
import { ErrorBanner } from "./ErrorBanner";

const CONVENTIONS: { value: Convention; label: string; models: string }[] = [
  { value: "pixels", label: "Pixels of the image the model saw", models: "Qwen2.5-VL family: UI-TARS-1.5, Fara-7B, Holo1.5" },
  { value: "grid1000", label: "0–1000 grid, [x, y]", models: "Qwen3-VL, Qwen3.5, Holo2 and later" },
  { value: "box1000", label: "0–1000 boxes, [x1, y1, x2, y2]", models: "Models that answer with boxes; the centre is used" },
  { value: "box1000Yx", label: "0–1000 boxes, [y1, x1, y2, x2]", models: "Gemma's box_2d order (unverified)" },
];

/** Settings → Computer use (9.3): permissions on macOS and each model's coordinate convention. */
export function ComputerSettings() {
  const { modelOptions, detail } = useApp();
  const [model, setModel] = useState<string | null>(detail?.chat.modelRef ?? modelOptions[0]?.ref ?? null);
  const [status, setStatus] = useState<ComputerStatus | null>(null);
  const [convention, setConvention] = useState<Convention>("pixels");
  const [confirmed, setConfirmed] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const isMac = /Mac/i.test(navigator.platform);

  const refresh = useCallback(() => {
    void ipc
      .computerStatus(model)
      .then((s) => {
        setStatus(s);
        setConvention(s.profile?.profile.convention ?? "pixels");
        setConfirmed(s.profile?.confirmed ?? false);
      })
      .catch((e) => setError(toErrorInfo(e)));
  }, [model]);
  useEffect(refresh, [refresh]);

  async function save() {
    if (!model) return;
    try {
      await ipc.setComputerProfile(model, convention, confirmed);
      refresh();
    } catch (e) {
      setError(toErrorInfo(e));
    }
  }

  if (!status) return error ? <ErrorBanner error={error} onDismiss={() => setError(null)} /> : null;
  return (
    <div className="space-y-4 text-sm">
      <h2 className="text-lg font-medium">Computer use</h2>
      <p className="text-fg-muted">
        Lets a model that can see images take screenshots and use the mouse and keyboard. Every kind of input to an app
        asks first; input to this app is always refused; a red border and a Stop button stay on screen while it acts,
        and the stop hotkey releases every key and button at once.
      </p>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      {!status.available && (
        <p className="flex items-start gap-1 text-warning">
          <AlertTriangle size={14} className="mt-0.5 shrink-0" aria-hidden />
          Computer use is not available on this system.
        </p>
      )}
      {isMac && (
        <section className="space-y-2">
          <h3 className="font-medium">macOS permissions</h3>
          {[
            { ok: status.screenRecording, name: "Screen Recording", url: status.screenRecordingUrl },
            { ok: status.accessibility, name: "Accessibility", url: status.accessibilityUrl },
          ].map((p) => (
            <div key={p.name} className="flex items-center gap-2">
              {p.ok ? (
                <CheckCircle2 size={14} className="text-success" aria-hidden />
              ) : (
                <AlertTriangle size={14} className="text-warning" aria-hidden />
              )}
              <span>
                {p.name}: {p.ok ? "allowed" : "not allowed"}
              </span>
              {!p.ok && (
                <button type="button" className="text-accent underline" onClick={() => void openUrl(p.url)}>
                  Open System Settings
                </button>
              )}
            </div>
          ))}
          <p className="text-xs text-fg-muted">
            In System Settings → Privacy &amp; Security, turn on harnissUI under Screen Recording and Accessibility, then
            come back here (macOS may ask you to restart the app). Unsigned builds lose these permissions on every update.
          </p>
          <button type="button" className="rounded-md border border-border px-3 py-1" onClick={refresh}>
            Check again
          </button>
        </section>
      )}
      <section className="space-y-2">
        <h3 className="font-medium">How the model writes positions</h3>
        <label className="block">
          Model
          <select
            value={model ?? ""}
            onChange={(e) => setModel(e.target.value || null)}
            className="mt-1 block w-full rounded-md border border-border bg-bg px-2 py-1"
          >
            {modelOptions.map((o) => (
              <option key={o.ref} value={o.ref}>
                {o.label}
              </option>
            ))}
          </select>
        </label>
        <div className="space-y-1" role="radiogroup" aria-label="Coordinate convention">
          {CONVENTIONS.map((c) => (
            <label key={c.value} className="flex items-start gap-2">
              <input
                type="radio"
                className="mt-1"
                name="convention"
                checked={convention === c.value}
                onChange={() => setConvention(c.value)}
              />
              <span>
                {c.label}
                <span className="block text-xs text-fg-muted">{c.models}</span>
              </span>
            </label>
          ))}
        </div>
        <label className="flex items-start gap-2">
          <input type="checkbox" className="mt-1" checked={confirmed} onChange={(e) => setConfirmed(e.target.checked)} />
          <span>
            I checked this convention against the model&apos;s documentation, or watched it click a known target
            correctly. Computer use stays off for this model until this is ticked.
          </span>
        </label>
        <button
          type="button"
          disabled={!model}
          onClick={() => void save()}
          className="rounded-md bg-accent px-3 py-1 text-accent-fg disabled:opacity-40"
        >
          Save
        </button>
      </section>
    </div>
  );
}

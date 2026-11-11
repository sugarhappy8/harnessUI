import { useEffect, useState } from "react";
import { AlertTriangle, CheckCircle2 } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import * as ipc from "../lib/ipc";
import type { ComputerStatus, Convention, ErrorInfo } from "../lib/types";
import { APP_NAME } from "../lib/app";
import { toErrorInfo, useApp } from "../store/app";
import { ErrorBanner } from "./ErrorBanner";

const CONVENTIONS: { value: Convention; label: string; models: string }[] = [
  { value: "pixels", label: "Pixels of the image the model saw", models: "Qwen2.5-VL family: UI-TARS-1.5, Fara-7B, Holo1.5" },
  { value: "grid1000", label: "0–1000 grid, [x, y]", models: "Qwen3-VL, Qwen3.5, Holo2 and later" },
  { value: "box1000", label: "0–1000 boxes, [x1, y1, x2, y2]", models: "Models that answer with boxes; the centre is used" },
  { value: "box1000Yx", label: "0–1000 boxes, [y1, x1, y2, x2]", models: "Gemma family (y before x)" },
];

/** "Pixels of the image…" → "pixels of the image…", to follow "Automatic: ". */
function afterColon(label: string): string {
  return label.charAt(0).toLowerCase() + label.slice(1);
}

/**
 * Settings → Computer use (9.3): permissions on macOS, and how each model writes screen
 * positions. That is picked from the model's name, so there is nothing to set up; the user
 * can choose another convention for a model here, and the choice is saved at once.
 */
export function ComputerSettings() {
  const { modelOptions, detail } = useApp();
  const [model, setModel] = useState<string | null>(detail?.chat.modelRef ?? modelOptions[0]?.ref ?? null);
  const [status, setStatus] = useState<ComputerStatus | null>(null);
  // Bumped to read the status again (after a change, or "Check again").
  const [reads, setReads] = useState(0);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const isMac = /Mac/i.test(navigator.platform);

  useEffect(() => {
    // An answer for a model the user has since moved away from is dropped.
    let alive = true;
    ipc
      .computerStatus(model)
      .then((s) => alive && setStatus(s))
      .catch((e) => alive && setError(toErrorInfo(e)));
    return () => {
      alive = false;
    };
  }, [model, reads]);
  const refresh = () => setReads((n) => n + 1);

  function openSettingsPane(url: string) {
    openUrl(url).catch((e) => {
      const info = toErrorInfo(e);
      // The opener plugin rejects with a plain string; say what failed and what to do instead.
      setError(
        info.code === "unexpected"
          ? {
              ...info,
              message: "System Settings could not be opened.",
              nextStep: "Open System Settings yourself and follow the steps below.",
            }
          : info,
      );
    });
  }

  /** Saves the convention for the model; `null` goes back to Automatic. */
  async function choose(convention: Convention | null) {
    if (!model) return;
    try {
      await ipc.setComputerProfile(model, convention);
    } catch (e) {
      setError(toErrorInfo(e));
    }
    refresh();
  }

  if (!status) return error ? <ErrorBanner error={error} onDismiss={() => setError(null)} /> : null;
  const profile = status.profile;
  const autoLabel = CONVENTIONS.find((c) => c.value === profile?.autoConvention)?.label ?? "";
  return (
    <div className="space-y-4 text-sm">
      <h2 className="text-lg font-medium">Computer use</h2>
      <p className="text-fg-muted">
        Lets a model that can see images take screenshots and use the mouse and keyboard. It is off until you switch on
        Computer under Tools in a chat. Every kind of input to an app asks first; input to this app is always refused; a
        red border and a Stop button stay on screen while it acts, and the stop hotkey releases every key and button at
        once.
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
                <button type="button" className="text-accent underline" onClick={() => openSettingsPane(p.url)}>
                  Open System Settings
                </button>
              )}
            </div>
          ))}
          <p className="text-xs text-fg-muted">
            In System Settings → Privacy &amp; Security, turn on {APP_NAME} under Screen Recording and Accessibility, then
            come back here (macOS may ask you to restart the app). Unsigned builds lose these permissions on every update.
          </p>
          <button type="button" className="rounded-md border border-border px-3 py-1" onClick={refresh}>
            Check again
          </button>
        </section>
      )}
      <section className="space-y-2">
        <h3 className="font-medium">How the model writes positions</h3>
        <p className="text-fg-muted">
          Models write screen positions in different ways. The app picks the way from the model&apos;s name, so there is
          nothing to set up. If clicks land in the wrong place, choose another convention here.
        </p>
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
        {profile ? (
          <div className="space-y-1" role="radiogroup" aria-label="Coordinate convention">
            <label className="flex items-start gap-2">
              <input
                type="radio"
                className="mt-1"
                name="convention"
                checked={profile.automatic}
                onChange={() => void choose(null)}
              />
              <span>
                Automatic: {afterColon(autoLabel)}
                <span className="block text-xs text-fg-muted">
                  {profile.autoGuess ? "Best guess for an unknown model." : "Picked from the model's name."}
                </span>
              </span>
            </label>
            {CONVENTIONS.map((c) => (
              <label key={c.value} className="flex items-start gap-2">
                <input
                  type="radio"
                  className="mt-1"
                  name="convention"
                  checked={!profile.automatic && profile.convention === c.value}
                  onChange={() => void choose(c.value)}
                />
                <span>
                  {c.label}
                  <span className="block text-xs text-fg-muted">{c.models}</span>
                </span>
              </label>
            ))}
          </div>
        ) : (
          <p className="text-fg-muted">No model yet. Download or connect one first.</p>
        )}
      </section>
    </div>
  );
}

import { useCallback, useEffect, useState } from "react";
import { RefreshCw } from "lucide-react";
import * as ipc from "../../lib/ipc";
import type { BackendInfo, ErrorInfo, GpuUse, LocalModelSettings, ThinkingLimit } from "../../lib/types";
import {
  backendWords,
  deviceLabel,
  gpuHelp,
  gpuLabel,
  GPU_USE_WORDS,
  HELP,
  isAppleComputer,
  THINKING_WORDS,
  tokensWithWords,
  upgradeNoteWords,
} from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { ErrorBanner } from "../ErrorBanner";
import { BackendDownloads } from "../BackendDownloads";
import { Field, type Option } from "../ModelSettings";

/** Fixed sizes offered for "How much it can remember" (besides Automatic). */
const SIZES = [2048, 4096, 8192, 16_384, 32_768, 65_536];

/** The backend Automatic tries first among graphics backends (runtime_manager `auto_choice`). */
const AUTO_ORDER = ["metal", "vulkan"];

/**
 * Settings → Local models: the defaults for all models (0.0.5 D4; the tab keeps its
 * label), the backends found on this computer and the optional backend downloads. A model
 * uses these unless its own settings (the gear next to it) say otherwise.
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

  async function save(patch: Partial<LocalModelSettings>) {
    if (!settings) return;
    // The upgrade note is shown once; any change hides it (and it is never stored).
    const next: LocalModelSettings = { ...settings, ...patch };
    delete next.upgradeNote;
    setSettings(next);
    try {
      await ipc.setLocalModelSettings(next);
    } catch (err) {
      setError(toErrorInfo(err));
    }
  }

  const apple = isAppleComputer() || (backends ?? []).some((b) => b.kind === "metal" && b.available);
  // "Which graphics card": the devices of the chosen backend, or of the one Automatic uses.
  const usable = (backends ?? []).filter((b) => b.available && b.kind !== "cpu");
  const deviceBackend =
    settings?.backend != null
      ? usable.find((b) => b.kind === settings.backend)
      : AUTO_ORDER.map((k) => usable.find((b) => b.kind === k)).find((b) => b !== undefined);
  const devices = deviceBackend?.devices ?? [];

  const backendOptions: Option[] = [
    { value: "auto", label: "Automatic (recommended)" },
    ...(backends ?? []).map((b) => ({
      value: b.kind,
      label: `${backendWords(b.kind)}${b.available ? "" : " (not available)"}`,
      disabled: !b.available,
    })),
  ];
  if (settings?.backend && !backendOptions.some((o) => o.value === settings.backend)) {
    backendOptions.push({ value: settings.backend, label: `${backendWords(settings.backend)} (not available)` });
  }
  const contextOptions: Option[] = [
    { value: "auto", label: "Automatic (recommended)" },
    ...SIZES.map((n) => ({ value: String(n), label: tokensWithWords(n) })),
  ];
  if (settings && !SIZES.includes(settings.ctx)) {
    contextOptions.push({ value: String(settings.ctx), label: tokensWithWords(settings.ctx) });
  }
  const deviceOptions: Option[] = [
    { value: "auto", label: "Automatic (recommended)" },
    ...devices.map((d) => ({ value: d.name, label: deviceLabel({ ...d, recommended: false }) })),
  ];
  if (settings?.device && !devices.some((d) => d.name === settings.device)) {
    deviceOptions.push({ value: settings.device, label: "A graphics card that is not connected now" });
  }
  const onOff: Option[] = [
    { value: "off", label: "Off" },
    { value: "on", label: "On" },
  ];

  return (
    <div className="space-y-5">
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <section aria-label="Defaults for all models" className="space-y-4">
        <div>
          <h3 className="text-sm font-medium">Defaults for all models</h3>
          <p className="text-xs text-fg-muted">
            Every model uses these unless you change them in its own settings (the gear next to a model). A model
            that is loaded keeps its settings until it restarts.
          </p>
        </div>
        {settings && (
          <>
            <Field
              id="defaults-backend"
              label="Run models on"
              value={settings.backend ?? "auto"}
              options={backendOptions}
              onChange={(v) => void save({ backend: v === "auto" ? null : v })}
            />
            <Field
              id="defaults-context"
              label="How much it can remember"
              value={settings.contextAuto ? "auto" : String(settings.ctx)}
              options={contextOptions}
              onChange={(v) => void save(v === "auto" ? { contextAuto: true } : { contextAuto: false, ctx: Number(v) })}
              help={
                <>
                  <p>{HELP.context}</p>
                  {settings.upgradeNote !== undefined && (
                    <p className="text-fg" role="note">
                      {upgradeNoteWords(settings.upgradeNote)}
                    </p>
                  )}
                </>
              }
            />
            <Field
              id="defaults-gpu"
              label={gpuLabel(apple)}
              value={settings.gpu}
              options={(["all", "fit", "off"] as const).map((g) => ({ value: g, label: GPU_USE_WORDS[g] }))}
              onChange={(v) => void save({ gpu: v as GpuUse })}
              help={<p>{gpuHelp(apple)}</p>}
            />
            <Field
              id="defaults-save-memory"
              label="Save memory"
              value={settings.saveMemory ? "on" : "off"}
              options={onOff}
              onChange={(v) => void save({ saveMemory: v === "on" })}
              help={<p>{HELP.saveMemory}</p>}
            />
            <Field
              id="defaults-keep-in-memory"
              label="Keep the model in memory"
              value={settings.keepInMemory ? "on" : "off"}
              options={onOff}
              onChange={(v) => void save({ keepInMemory: v === "on" })}
              help={<p>{HELP.keepInMemory}</p>}
            />
            <details className="rounded-md border border-border px-3 py-2">
              <summary className="cursor-pointer text-sm font-medium">More options</summary>
              <div className="mt-3 space-y-4">
                {devices.length > 1 && (
                  <Field
                    id="defaults-device"
                    label="Which graphics card"
                    value={settings.device ?? "auto"}
                    options={deviceOptions}
                    onChange={(v) => void save({ device: v === "auto" ? null : v })}
                  />
                )}
                <Field
                  id="defaults-thinking"
                  label="How long it may think"
                  value={settings.thinkingLimit}
                  options={(["noLimit", "short", "medium", "long"] as const).map((t) => ({ value: t, label: THINKING_WORDS[t] }))}
                  onChange={(v) => void save({ thinkingLimit: v as ThinkingLimit })}
                  help={<p>{HELP.thinking} Only for models that think before answering.</p>}
                />
              </div>
            </details>
          </>
        )}
      </section>
      <section>
        <div className="mb-2 flex items-center justify-between">
          <h3 className="text-sm font-medium">Found on this computer</h3>
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
                <span className="font-medium">{backendWords(b.kind)}</span>
                <span className={b.available ? "text-success" : "text-fg-muted"}>
                  {b.available ? "Can be used" : "Not available"}
                </span>
              </div>
              {b.devices.map((d) => (
                <p key={d.name} className="text-xs text-fg-muted">
                  {deviceLabel({ ...d, recommended: false })}
                </p>
              ))}
              {b.error && <p className="text-xs text-fg-muted">{b.error}</p>}
            </li>
          ))}
          {backends?.length === 0 && (
            <li className="text-fg-muted">This build has no program to run models. Reinstall the app.</li>
          )}
        </ul>
      </section>
      <BackendDownloads onChanged={reprobe} />
    </div>
  );
}

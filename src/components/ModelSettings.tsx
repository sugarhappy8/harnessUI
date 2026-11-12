import { useEffect, useState, type ReactNode } from "react";
import { AlertDialog, Dialog } from "radix-ui";
import { X } from "lucide-react";
import * as ipc from "../lib/ipc";
import type {
  ContextChoice,
  DeviceChoice,
  ErrorInfo,
  GpuChoice,
  ModelLoadInfo,
  ModelLoadSettings,
  ThinkingChoice,
  Toggle,
} from "../lib/types";
import {
  automaticResult,
  busyQuestion,
  deviceLabel,
  FIT_WORDS,
  gpuHelp,
  gpuLabel,
  GPU_USE_WORDS,
  HELP,
  isAppleComputer,
  THINKING_WORDS,
  TOO_LARGE_LINE,
  tokensWithWords,
  upgradeNoteWords,
} from "../lib/words";
import { toErrorInfo, useApp } from "../store/app";
import {
  answerQuestion,
  reloadModel,
  registerQuestionHost,
  updateQuestionText,
  useQuestion,
} from "../store/modelChange";
import { ErrorBanner } from "./ErrorBanner";

// ---- Shared controls (also used by Settings → Local models) ------------------------------

export interface Option {
  value: string;
  label: string;
  disabled?: boolean;
}

/** A setting: label, a choice from a fixed list, and its help line. */
export function Field(props: {
  id: string;
  label: string;
  value: string;
  options: Option[];
  onChange: (value: string) => void;
  help?: ReactNode;
  disabled?: boolean;
}) {
  return (
    <div className="space-y-1">
      <label htmlFor={props.id} className="block text-sm font-medium">
        {props.label}
      </label>
      <select
        id={props.id}
        value={props.value}
        disabled={props.disabled}
        onChange={(e) => props.onChange(e.target.value)}
        className="w-full rounded-md border border-border bg-bg px-2 py-1 text-sm disabled:opacity-60"
      >
        {props.options.map((o) => (
          <option key={o.value} value={o.value} disabled={o.disabled}>
            {o.label}
          </option>
        ))}
      </select>
      {props.help && <div className="space-y-1 text-xs text-fg-muted">{props.help}</div>}
    </div>
  );
}

// Choices as <select> values. Every value comes from a fixed list; nothing is free text.

function contextValue(c: ContextChoice): string {
  return typeof c === "string" ? c : `fixed:${c.fixed}`;
}

function contextChoice(v: string): ContextChoice {
  if (v === "default" || v === "auto") return v;
  return { fixed: Number(v.slice("fixed:".length)) };
}

function deviceValue(d: DeviceChoice): string {
  return typeof d === "string" ? d : `named:${d.named}`;
}

function deviceChoice(v: string): DeviceChoice {
  if (v === "default" || v === "auto") return v;
  return { named: v.slice("named:".length) };
}

function sameAs(words: string): string {
  return `Same as default (${words})`;
}

function onOff(on: boolean): string {
  return on ? "On" : "Off";
}

function same(a: ModelLoadSettings, b: ModelLoadSettings): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

const ALL_DEFAULT: ModelLoadSettings = {
  context: "default",
  gpu: "default",
  saveMemory: "default",
  keepInMemory: "default",
  device: "default",
  thinkingLimit: "default",
};

/** Whether the defaults for all models say Automatic for this model. */
function defaultIsAuto(info: ModelLoadInfo): boolean {
  return info.defaults.contextAuto || (info.overrides.context === "default" && info.resolved.ctxChoice === "auto");
}

// ---- The panel ----------------------------------------------------------------------------

/**
 * Settings for one model file (D1, D4): each setting starts as "Same as default (…)", with
 * its help line and a fit mark per choice. Save keeps them for every later load; Apply and
 * restart (the loaded model) or Load with these settings (another model) uses them now.
 */
export function ModelSettings(props: { path: string; onClose: () => void; onSaved?: (info: ModelLoadInfo) => void }) {
  const { path, onClose, onSaved } = props;
  const [info, setInfo] = useState<ModelLoadInfo | null>(null);
  const [draft, setDraft] = useState<ModelLoadSettings | null>(null);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const [working, setWorking] = useState(false);
  const [saved, setSaved] = useState(false);
  const runtime = useApp((s) => s.runtime);
  const apple = isAppleComputer();

  useEffect(() => {
    let alive = true;
    ipc
      .modelLoadInfo(path)
      .then((i) => {
        if (!alive) return;
        setInfo(i);
        setDraft(i.overrides);
      })
      .catch((err) => alive && setError(toErrorInfo(err)));
    return () => {
      alive = false;
    };
  }, [path]);

  const name = info?.fileName ?? path.split(/[\\/]/).pop() ?? path;
  const loaded = !!info && (info.loaded || (runtime?.state === "ready" && runtime.modelPath === path));
  const dirty = !!info && !!draft && !same(draft, info.overrides);

  function change(patch: Partial<ModelLoadSettings>) {
    setSaved(false);
    setDraft((d) => (d ? { ...d, ...patch } : d));
  }

  function took(next: ModelLoadInfo) {
    setInfo(next);
    setDraft(next.overrides);
    onSaved?.(next);
  }

  /** Saves the draft when it changed; resolves with the model's info, or null after an error. */
  async function save(): Promise<ModelLoadInfo | null> {
    if (!info || !draft) return null;
    if (!dirty) return info;
    try {
      const next = await ipc.setModelLoadSettings(path, draft);
      took(next);
      return next;
    } catch (err) {
      setError(toErrorInfo(err));
      return null;
    }
  }

  async function run(action: () => Promise<void>) {
    setWorking(true);
    setError(null);
    try {
      await action();
    } catch (err) {
      setError(toErrorInfo(err));
    } finally {
      setWorking(false);
    }
  }

  const onSave = () =>
    run(async () => {
      if (await save()) setSaved(true);
    });

  const onReset = () =>
    run(async () => {
      took(await ipc.resetModelLoadSettings(path));
      setSaved(false);
    });

  const onApply = () =>
    run(async () => {
      if (!(await save())) return;
      const state = useApp.getState();
      // The question (when the model is busy) shows over the panel; the panel closes once
      // it is answered, and stays open after Cancel.
      void reloadModel(state.runtime, false, onClose).catch((err) => state.showError(toErrorInfo(err)));
    });

  const onLoad = () =>
    run(async () => {
      if (!(await save())) return;
      // pickModel asks first when the loaded model is busy, and closes the panel once the
      // chat has the model.
      await useApp.getState().pickModel(`local:${path}`, { onChatSet: onClose });
    });

  return (
    <Dialog.Root open onOpenChange={(o) => !o && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="dialog-overlay fixed inset-0" />
        <Dialog.Content className="fixed top-1/2 left-1/2 flex max-h-[88vh] w-[34rem] max-w-[92vw] -translate-x-1/2 -translate-y-1/2 flex-col rounded-2xl border border-border bg-surface shadow-pop">
          <div className="flex items-start justify-between gap-3 border-b border-border px-5 py-3">
            <div className="min-w-0">
              <Dialog.Title className="font-semibold">Settings for this model</Dialog.Title>
              <Dialog.Description className="truncate text-xs text-fg-muted" title={path}>
                {name}
              </Dialog.Description>
            </div>
            <Dialog.Close aria-label="Close" className="mt-0.5 text-fg-muted hover:text-fg">
              <X size={16} aria-hidden />
            </Dialog.Close>
          </div>
          <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-5 py-4">
            {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
            {!info && !error && <p className="text-sm text-fg-muted">Reading the model file…</p>}
            {info && draft && <Controls info={info} draft={draft} apple={apple} onChange={change} />}
          </div>
          {info && draft && (
            <div className="space-y-2 border-t border-border px-5 py-3">
              {loaded && info.olderSettingsLoaded && (
                <p className="text-xs text-warning">Settings changed. Restart the model to use them.</p>
              )}
              {saved && !dirty && (
                <p className="text-xs text-fg-muted" role="status">
                  Saved. {loaded ? "They are used from the next restart." : "They are used every time this model loads."}
                </p>
              )}
              <div className="flex flex-wrap items-center gap-2">
                <button
                  type="button"
                  onClick={() => void onReset()}
                  disabled={working || (same(info.overrides, ALL_DEFAULT) && same(draft, ALL_DEFAULT))}
                  className="text-xs text-fg-muted underline hover:text-fg disabled:no-underline disabled:opacity-40"
                >
                  Reset to defaults
                </button>
                <div className="flex-1" />
                <button
                  type="button"
                  onClick={() => void onSave()}
                  disabled={working || !dirty}
                  className="rounded-md border border-border px-3 py-1.5 text-sm hover:bg-bg-muted disabled:opacity-40"
                >
                  Save
                </button>
                {loaded ? (
                  <button
                    type="button"
                    onClick={() => void onApply()}
                    disabled={working || (!dirty && !info.olderSettingsLoaded)}
                    title={!dirty && !info.olderSettingsLoaded ? "The model already uses these settings." : undefined}
                    className="btn-primary rounded-md px-3 py-1.5 text-sm font-medium disabled:opacity-40"
                  >
                    Apply and restart
                  </button>
                ) : (
                  <button
                    type="button"
                    onClick={() => void onLoad()}
                    disabled={working}
                    className="btn-primary rounded-md px-3 py-1.5 text-sm font-medium disabled:opacity-40"
                  >
                    Load with these settings
                  </button>
                )}
              </div>
            </div>
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/**
 * The panel's settings, in plain words, each "Same as default (…)" until changed.
 *
 * TODO(0.0.6): the fit marks are those of the saved settings (`model_load_info`); a change
 * to another setting in the draft (Save memory, the graphics card) updates them only after
 * Save. A preview command that checks the fit of unsaved choices would let them follow the
 * draft.
 */
function Controls(props: {
  info: ModelLoadInfo;
  draft: ModelLoadSettings;
  apple: boolean;
  onChange: (patch: Partial<ModelLoadSettings>) => void;
}) {
  const { info, draft, apple, onChange } = props;
  const d = info.defaults;

  // How much it can remember.
  const autoDefault = defaultIsAuto(info);
  const contextOptions: Option[] = [
    { value: "default", label: sameAs(autoDefault ? "Automatic" : tokensWithWords(d.ctx)) },
    ...info.contextOptions.map((o) =>
      o.choice === "auto"
        ? {
            value: "auto",
            label: o.tokens > 0 ? `Automatic: ${tokensWithWords(o.tokens)} · ${FIT_WORDS[o.fit]}` : "Automatic: too large for this computer",
          }
        : { value: `fixed:${o.choice.fixed}`, label: `${tokensWithWords(o.choice.fixed)} · ${FIT_WORDS[o.fit]}` },
    ),
  ];
  const contextNow = contextValue(draft.context);
  if (!contextOptions.some((o) => o.value === contextNow) && typeof draft.context === "object") {
    contextOptions.push({ value: contextNow, label: tokensWithWords(draft.context.fixed) });
  }
  if (!contextOptions.some((o) => o.value === "auto")) contextOptions.splice(1, 0, { value: "auto", label: "Automatic" });
  const usesAuto = draft.context === "auto" || (draft.context === "default" && autoDefault);
  const autoOption = info.contextOptions.find((o) => o.choice === "auto");
  const autoTokens = info.loaded && info.autoCtx ? info.autoCtx : (autoOption?.tokens ?? 0);

  // Use the graphics card: only when there is one.
  const showGpu = info.devices.length > 0;
  const gpuOptions: Option[] = [
    { value: "default", label: sameAs(GPU_USE_WORDS[d.gpu]) },
    ...info.gpuOptions.map((o) => ({ value: o.gpu, label: `${GPU_USE_WORDS[o.gpu]} · ${FIT_WORDS[o.fit]}` })),
  ];

  const toggle = (on: boolean): Option[] => [
    { value: "default", label: sameAs(onOff(on)) },
    { value: "on", label: "On" },
    { value: "off", label: "Off" },
  ];

  // More options: which graphics card (more than one), how long it may think (thinking models).
  const showDevice = info.devices.length > 1;
  const defaultDevice =
    d.device === null
      ? "Automatic"
      : (() => {
          const found = info.devices.find((x) => x.name === d.device);
          return found ? deviceLabel({ ...found, recommended: false }) : "a graphics card that is not connected now";
        })();
  const deviceOptions: Option[] = [
    { value: "default", label: sameAs(defaultDevice) },
    { value: "auto", label: "Automatic" },
    ...info.devices.map((x) => ({ value: `named:${x.name}`, label: deviceLabel(x) })),
  ];
  const deviceNow = deviceValue(draft.device);
  if (!deviceOptions.some((o) => o.value === deviceNow)) {
    deviceOptions.push({ value: deviceNow, label: "A graphics card that is not connected now" });
  }
  const thinkingOptions: Option[] = [
    { value: "default", label: sameAs(d.thinkingLimit === "noLimit" ? "no limit" : THINKING_WORDS[d.thinkingLimit]) },
    ...(["noLimit", "short", "medium", "long"] as const).map((t) => ({ value: t, label: THINKING_WORDS[t] })),
  ];
  const more = showDevice || info.thinking;

  return (
    <div className="space-y-4">
      {info.tooLarge && (
        <p className="rounded-md border border-danger/40 bg-danger/5 px-3 py-2 text-sm text-danger" role="alert">
          {TOO_LARGE_LINE}
        </p>
      )}
      <Field
        id="model-context"
        label="How much it can remember"
        value={contextNow}
        options={contextOptions}
        onChange={(v) => onChange({ context: contextChoice(v) })}
        help={
          <>
            <p>{HELP.context}</p>
            {usesAuto && !info.tooLarge && autoTokens > 0 && <p className="text-fg">{automaticResult(autoTokens)}</p>}
            {info.trainedCtx !== null && <p>The most this model can remember is {tokensWithWords(info.trainedCtx)}.</p>}
            {d.upgradeNote !== undefined && draft.context === "default" && <p className="text-fg">{upgradeNoteWords(d.upgradeNote)}</p>}
          </>
        }
      />
      {showGpu && (
        <Field
          id="model-gpu"
          label={gpuLabel(apple)}
          value={draft.gpu}
          options={gpuOptions}
          onChange={(v) => onChange({ gpu: v as GpuChoice })}
          help={<p>{gpuHelp(apple)}</p>}
        />
      )}
      <Field
        id="model-save-memory"
        label="Save memory"
        value={draft.saveMemory}
        options={toggle(d.saveMemory)}
        onChange={(v) => onChange({ saveMemory: v as Toggle })}
        help={<p>{HELP.saveMemory}</p>}
      />
      <Field
        id="model-keep-in-memory"
        label="Keep the model in memory"
        value={draft.keepInMemory}
        options={toggle(d.keepInMemory)}
        onChange={(v) => onChange({ keepInMemory: v as Toggle })}
        help={<p>{HELP.keepInMemory}</p>}
      />
      {more && (
        <details className="rounded-md border border-border px-3 py-2">
          <summary className="cursor-pointer text-sm font-medium">More options</summary>
          <div className="mt-3 space-y-4">
            {showDevice && (
              <Field
                id="model-device"
                label="Which graphics card"
                value={deviceNow}
                options={deviceOptions}
                onChange={(v) => onChange({ device: deviceChoice(v) })}
              />
            )}
            {info.thinking && (
              <Field
                id="model-thinking"
                label="How long it may think"
                value={draft.thinkingLimit}
                options={thinkingOptions}
                onChange={(v) => onChange({ thinkingLimit: v as ThinkingChoice })}
                help={<p>{HELP.thinking}</p>}
              />
            )}
          </div>
        </details>
      )}
    </div>
  );
}

// ---- Hosts ----------------------------------------------------------------------------------

const OPEN_EVENT = "app:open-model-settings";

/** Opens the settings panel of a model file (the chat's model list, the runtime indicator, a chat error). */
export function openModelSettings(path: string): void {
  window.dispatchEvent(new CustomEvent(OPEN_EVENT, { detail: path }));
}

/** Shows the panel asked for with `openModelSettings`. Mounted once, with the chat. */
export function ModelSettingsHost() {
  const [path, setPath] = useState<string | null>(null);
  useEffect(() => {
    const onOpen = (event: Event) => {
      const detail: unknown = (event as CustomEvent<unknown>).detail;
      if (typeof detail === "string" && detail.length > 0) setPath(detail);
    };
    window.addEventListener(OPEN_EVENT, onOpen);
    return () => window.removeEventListener(OPEN_EVENT, onOpen);
  }, []);
  return path ? <ModelSettings key={path} path={path} onClose={() => setPath(null)} /> : null;
}

/**
 * Shows the questions of `store/modelChange` (D3, Too large). Mounted once with the chat,
 * and on the first-run screen. While the D3 question is open it re-asks what uses the
 * model every 1.5 seconds, so the question names what is running now.
 */
export function QuestionHost() {
  const question = useQuestion((s) => s.question);
  useEffect(() => registerQuestionHost(), []);
  const id = question?.id ?? null;
  const watch = question?.busy ?? false;
  useEffect(() => {
    if (id === null || !watch) return;
    const timer = window.setInterval(() => {
      if (document.visibilityState !== "visible") return;
      ipc
        .modelBusy()
        .then((busy) => {
          const text = busyQuestion(busy);
          if (text) updateQuestionText(id, text);
        })
        .catch(() => undefined);
    }, 1500);
    return () => window.clearInterval(timer);
  }, [id, watch]);
  if (!question) return null;
  const last = question.choices.length - 1;
  return (
    <AlertDialog.Root open onOpenChange={(o) => !o && answerQuestion(question.id, null)}>
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="dialog-overlay fixed inset-0" />
        <AlertDialog.Content
          aria-describedby={undefined}
          className="fixed top-1/2 left-1/2 w-[28rem] max-w-[92vw] -translate-x-1/2 -translate-y-1/2 rounded-xl border border-border bg-surface p-5 shadow-pop"
        >
          <AlertDialog.Title className="mb-4 text-sm font-medium">{question.text}</AlertDialog.Title>
          <div className="flex flex-wrap justify-end gap-2">
            <AlertDialog.Cancel className="rounded-md border border-border px-3 py-1.5 text-sm hover:bg-bg-muted">
              {question.cancel}
            </AlertDialog.Cancel>
            {question.choices.map((c, i) => (
              <button
                key={c.value}
                type="button"
                onClick={() => answerQuestion(question.id, c.value)}
                className={
                  "rounded-md px-3 py-1.5 text-sm " +
                  (c.tone === "danger"
                    ? "bg-danger text-white"
                    : c.tone === "primary" || i === last
                      ? "btn-primary font-medium"
                      : "border border-border hover:bg-bg-muted")
                }
              >
                {c.label}
              </button>
            ))}
          </div>
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}

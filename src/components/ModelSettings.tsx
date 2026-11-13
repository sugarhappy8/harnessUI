import { useEffect, useState, type ReactNode } from "react";
import { AlertDialog, Dialog } from "radix-ui";
import { X } from "lucide-react";
import * as ipc from "../lib/ipc";
import type {
  ContextChoice,
  ContextSize,
  DeviceChoice,
  ErrorInfo,
  FineControl as FineSettings,
  GpuChoice,
  GpuUse,
  ModelLoadInfo,
  ModelLoadSettings,
  OffloadPreview,
  ThinkingChoice,
  Toggle,
  Verdict,
} from "../lib/types";
import {
  automaticResult,
  busyQuestion,
  customPlacementWords,
  CUSTOM_PLACEMENT_HELP,
  deviceLabel,
  FIT_WORDS,
  gpuHelp,
  gpuLabel,
  GPU_USE_WORDS,
  HELP,
  isAppleComputer,
  pictureWarning,
  THINKING_WORDS,
  TOO_LARGE_LINE,
  tokensWithWords,
  upgradeNoteWords,
  wontFitQuestion,
} from "../lib/words";
import { toErrorInfo, useApp } from "../store/app";
import {
  answerQuestion,
  ask,
  askTooLarge,
  reloadModel,
  registerQuestionHost,
  updateQuestionText,
  useQuestion,
} from "../store/modelChange";
import { ErrorBanner } from "./ErrorBanner";
import { FINE_AUTO, FineControl, fineIsAuto, hasCustomPlacement } from "./FineControl";
import { OffloadPicture } from "./OffloadPicture";

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

/** The choices without a Fine control that is all Automatic (absent means Automatic, item 6). */
function tidy(s: ModelLoadSettings): ModelLoadSettings {
  if (s.fine === undefined || !fineIsAuto(s.fine)) return s;
  const next = { ...s };
  delete next.fine;
  return next;
}

/** JSON with the keys of every object in order, so two equal choices always compare equal. */
function canonical(value: unknown): string {
  if (value === null || typeof value !== "object") return JSON.stringify(value) ?? "null";
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  const entries = Object.entries(value as Record<string, unknown>)
    .filter(([, v]) => v !== undefined)
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  return `{${entries.map(([k, v]) => `${JSON.stringify(k)}:${canonical(v)}`).join(",")}}`;
}

/** Whether two sets of choices are the same; Fine control absent and all Automatic are the same. */
function same(a: ModelLoadSettings, b: ModelLoadSettings): boolean {
  return canonical(tidy(a)) === canonical(tidy(b));
}

function sameSize(a: ContextSize, b: ContextSize): boolean {
  return typeof a === "string" || typeof b === "string" ? a === b : a.fixed === b.fixed;
}

/** What an on/off choice of one model comes to with the defaults for all models. */
function toggleOn(t: Toggle, fallback: boolean): boolean {
  return t === "default" ? fallback : t === "on";
}

/** What "Use the graphics card" comes to with the defaults for all models. */
function gpuOf(g: GpuChoice, fallback: GpuUse): GpuUse {
  return g === "default" ? fallback : g;
}

/** Whether a setting the model is launched with differs (How long it may think is not one). */
function launchChanged(a: ModelLoadSettings, b: ModelLoadSettings): boolean {
  return !same({ ...a, thinkingLimit: "default" }, { ...b, thinkingLimit: "default" });
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
  // What the panel is doing while a command runs (they can take a while on a busy computer).
  const [working, setWorking] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  // The facts for the unsaved choices (`draftKey`), so the fit marks follow them.
  const [preview, setPreview] = useState<{ key: string; info: ModelLoadInfo } | null>(null);
  // Item 6: Fine control is open (exact figures are worked out only then), and llama.cpp's
  // measurement for the choices `key` (`picture` null when it could not be asked).
  const [fineOpen, setFineOpen] = useState(false);
  const [measured, setMeasured] = useState<{ key: string; picture: OffloadPreview | null } | null>(null);
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
  const draftKey = draft ? JSON.stringify(draft) : null;
  const savedKey = info ? JSON.stringify(info.overrides) : null;

  // Unsaved choices: ask for their fit (nothing is saved), a moment after the last change.
  // An answer for choices changed since is dropped; without an answer the saved marks stay,
  // and Save says what is wrong with the choices, if anything.
  useEffect(() => {
    if (draftKey === null || draftKey === savedKey) return;
    const choices = JSON.parse(draftKey) as ModelLoadSettings;
    let alive = true;
    const timer = window.setTimeout(() => {
      void (async () => {
        try {
          const next = await ipc.previewModelLoadSettings(path, choices);
          if (alive) setPreview({ key: draftKey, info: next });
        } catch {
          // The saved marks stay.
        }
      })();
    }, 150);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [path, draftKey, savedKey]);
  // What the controls show: the facts for the choices on screen when known, else the saved ones.
  const previewNow = info && dirty && preview && preview.key === draftKey ? preview.info : null;
  const facts = previewNow ?? info;

  // Item 6: while Fine control is open, llama.cpp works out the exact figures for the choices
  // on screen, a moment after the last change. An answer is used only while the choices it
  // was asked for are still on screen; a newer request replaces an older one.
  const canMeasure = !!info?.picture && !!info.fineFacts;
  useEffect(() => {
    if (!fineOpen || !canMeasure || draftKey === null) return;
    const choices = JSON.parse(draftKey) as ModelLoadSettings;
    let alive = true;
    const timer = window.setTimeout(() => {
      ipc.offloadMeasure(path, choices).then(
        (picture) => alive && setMeasured({ key: draftKey, picture }),
        () => alive && setMeasured({ key: draftKey, picture: null }),
      );
    }, 600);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [path, fineOpen, canMeasure, draftKey]);
  // The backend keys both answers by the draft they answer (`OffloadPreview.key`): when the
  // instant estimate on screen is for these choices, a measurement with another key is old.
  const estimateNow = previewNow ?? (!dirty ? info : null);
  const measuredNow =
    measured &&
    measured.key === draftKey &&
    (!measured.picture || !estimateNow?.picture || measured.picture.key === estimateNow.picture.key)
      ? measured
      : null;
  const measuring = fineOpen && canMeasure && !measuredNow;
  // The latest answer for the choices on screen: the measurement, else the instant estimate
  // (which says why exact figures could not be worked out when the request failed).
  const estimate: OffloadPreview | null = facts?.picture ?? null;
  const picture: OffloadPreview | null =
    measuredNow?.picture ??
    (measuredNow && estimate ? { ...estimate, source: { kind: "estimate", why: "failed" } } : estimate);
  const measuredMark: Verdict | null = measuredNow?.picture?.source.kind === "measured" ? measuredNow.picture.mark : null;
  // Whether loading with the choices on screen would launch the model differently from the
  // copy that is loaded (the backend's own rule, `olderSettingsLoaded`): only then does
  // Apply and restart have something to restart. How long it may think is a per-request
  // limit; a change to it alone, or to a choice that comes to the same, restarts nothing.
  const needsRestart = !info
    ? false
    : !dirty
      ? info.olderSettingsLoaded
      : previewNow
        ? previewNow.olderSettingsLoaded
        : info.olderSettingsLoaded || (!!draft && launchChanged(draft, info.overrides));

  function change(patch: Partial<ModelLoadSettings>) {
    setSaved(false);
    setDraft((d) => (d ? tidy({ ...d, ...patch }) : d));
  }

  /**
   * The facts for the choices on screen: the saved ones, or the draft's answer. When the
   * draft's answer has not come yet (or could not be had), it is asked for now, so a quick
   * Load never goes by the saved choices' answer. Throws when it cannot be had.
   */
  async function factsForLoad(): Promise<ModelLoadInfo | null> {
    if (!info || !draft || !dirty) return info;
    if (previewNow) return previewNow;
    setWorking("Working out whether it fits…");
    try {
      return await ipc.previewModelLoadSettings(path, draft);
    } finally {
      setWorking(null);
    }
  }

  /**
   * Before a load with these choices: the latest answer may say it probably won't fit
   * (item 6, 6.2): the measurement when there is one for the choices on screen, else the
   * estimate. Too large or not at all: WP2's question; not on the graphics card: the same
   * dialog with its own question (Cancel has the focus). Resolves true to go on; when the
   * answer cannot be had, nothing loads and the panel says why.
   */
  async function loadAnyway(): Promise<boolean> {
    let now: ModelLoadInfo | null;
    try {
      now = await factsForLoad();
    } catch (err) {
      setError(toErrorInfo(err));
      return false;
    }
    const exact =
      measured && measured.key === draftKey && measured.picture && (!now?.picture || measured.picture.key === now.picture.key)
        ? measured.picture
        : null;
    const answer = exact ?? now?.picture ?? null;
    if (now?.tooLarge || answer?.fits === "wontFitAtAll") return askTooLarge();
    if (answer?.fits === "wontFitGraphics") {
      const choice = await ask({
        text: wontFitQuestion(now?.fineFacts?.apple ?? apple),
        cancel: "Cancel",
        choices: [{ label: "Load anyway", value: "load", tone: "primary" }],
        busy: false,
      });
      return choice === "load";
    }
    return true;
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

  async function run(doing: string, action: () => Promise<void>) {
    setWorking(doing);
    setError(null);
    try {
      await action();
    } catch (err) {
      setError(toErrorInfo(err));
    } finally {
      setWorking(null);
    }
  }

  const onSave = () =>
    run("Saving…", async () => {
      if (await save()) setSaved(true);
    });

  const onReset = () =>
    run("Going back to the defaults…", async () => {
      took(await ipc.resetModelLoadSettings(path));
      setSaved(false);
    });

  const onApply = async () => {
    if (needsRestart && !(await loadAnyway())) return;
    await run(dirty ? "Saving…" : "Restarting the model…", async () => {
      const next = await save();
      if (!next) return;
      // The backend says whether the loaded copy now differs from these settings; when it
      // does not, a restart would change nothing (and must not say it did).
      if (!next.olderSettingsLoaded) {
        setSaved(true);
        return;
      }
      const state = useApp.getState();
      // The question (when the model is busy) shows over the panel; the panel closes once
      // it is answered, and stays open after Cancel.
      void reloadModel(state.runtime, false, onClose).catch((err) => state.showError(toErrorInfo(err)));
    });
  };

  const onLoad = async () => {
    if (!(await loadAnyway())) return;
    await run(dirty ? "Saving…" : "Loading the model…", async () => {
      if (!(await save())) return;
      // pickModel asks first when the loaded model is busy, and closes the panel once the
      // chat has the model.
      await useApp.getState().pickModel(`local:${path}`, { onChatSet: onClose });
    });
  };

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
            {facts && draft && (
              <Controls
                info={facts}
                draft={draft}
                dirty={dirty}
                apple={apple}
                disabled={working !== null}
                onChange={change}
                picture={picture}
                measuring={measuring}
                measuredMark={measuredMark}
                fineOpen={fineOpen}
                onFineToggle={setFineOpen}
              />
            )}
          </div>
          {info && draft && (
            <div className="space-y-2 border-t border-border px-5 py-3">
              {loaded && info.olderSettingsLoaded && (
                <p className="text-xs text-warning">Settings changed. Restart the model to use them.</p>
              )}
              {working !== null && (
                <p className="text-xs text-fg-muted" role="status">
                  {working}
                </p>
              )}
              {working === null && saved && !dirty && (
                <p className="text-xs text-fg-muted" role="status">
                  Saved.{" "}
                  {!loaded
                    ? "They are used every time this model loads."
                    : info.olderSettingsLoaded
                      ? "They are used from the next restart."
                      : "They are used from the next reply."}
                </p>
              )}
              <div className="flex flex-wrap items-center gap-2">
                <button
                  type="button"
                  onClick={() => void onReset()}
                  disabled={working !== null || (same(info.overrides, ALL_DEFAULT) && same(draft, ALL_DEFAULT))}
                  className="text-xs text-fg-muted underline hover:text-fg disabled:no-underline disabled:opacity-40"
                >
                  Reset to defaults
                </button>
                <div className="flex-1" />
                <button
                  type="button"
                  onClick={() => void onSave()}
                  disabled={working !== null || !dirty}
                  className="rounded-md border border-border px-3 py-1.5 text-sm hover:bg-bg-muted disabled:opacity-40"
                >
                  Save
                </button>
                {loaded ? (
                  <button
                    type="button"
                    onClick={() => void onApply()}
                    disabled={working !== null || !needsRestart}
                    title={
                      needsRestart
                        ? undefined
                        : dirty
                          ? "This change needs no restart. Save keeps it."
                          : "The model already uses these settings."
                    }
                    className="btn-primary rounded-md px-3 py-1.5 text-sm font-medium disabled:opacity-40"
                  >
                    Apply and restart
                  </button>
                ) : (
                  <button
                    type="button"
                    onClick={() => void onLoad()}
                    disabled={working !== null}
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
 * The panel's settings, in plain words, each "Same as default (…)" until changed. `info`
 * holds the facts for the choices on screen (`model_load_info` with the unsaved choices
 * once it answered), so the fit marks, the too-large line and what Automatic would pick
 * follow a change before it is saved.
 */
function Controls(props: {
  info: ModelLoadInfo;
  draft: ModelLoadSettings;
  /** The choices on screen are not saved yet. */
  dirty: boolean;
  apple: boolean;
  /** While a command runs, nothing can be changed. */
  disabled: boolean;
  onChange: (patch: Partial<ModelLoadSettings>) => void;
  /** Item 6: the latest picture for the choices on screen (the measurement when there is one). */
  picture: OffloadPreview | null;
  /** Exact figures are being worked out for the choices on screen. */
  measuring: boolean;
  /** The measurement's mark for the selected choices, which replaces their estimated marks. */
  measuredMark: Verdict | null;
  fineOpen: boolean;
  onFineToggle: (open: boolean) => void;
}) {
  const { info, draft, dirty, apple, disabled, onChange, picture, measuring, measuredMark, fineOpen, onFineToggle } = props;
  const d = info.defaults;
  // A backend without item 6 sends no `fineFacts`; Fine control then stays hidden.
  const fineFacts = info.fineFacts ?? null;
  const appleGraphics = fineFacts?.apple ?? apple;
  const fine: FineSettings = draft.fine ?? FINE_AUTO;
  const gpuNow = gpuOf(draft.gpu, d.gpu);
  const gpuOff = gpuNow === "off";
  const saveMemoryOn = toggleOn(draft.saveMemory, d.saveMemory);
  // The measurement's mark replaces the estimated marks of the selected choices (4.1).
  const markOf = (fit: Verdict, selected: boolean): Verdict => (selected && measuredMark !== null ? measuredMark : fit);

  // How much it can remember.
  const autoDefault = defaultIsAuto(info);
  const contextOptions: Option[] = [
    { value: "default", label: sameAs(autoDefault ? "Automatic" : tokensWithWords(d.ctx)) },
    ...info.contextOptions.map((o) => {
      const fit = markOf(o.fit, sameSize(o.choice, info.resolved.ctxChoice));
      return o.choice === "auto"
        ? {
            value: "auto",
            label: o.tokens > 0 ? `Automatic: ${tokensWithWords(o.tokens)} · ${FIT_WORDS[fit]}` : "Automatic: too large for this computer",
          }
        : { value: `fixed:${o.choice.fixed}`, label: `${tokensWithWords(o.choice.fixed)} · ${FIT_WORDS[fit]}` };
    }),
  ];
  const contextNow = contextValue(draft.context);
  if (!contextOptions.some((o) => o.value === contextNow) && typeof draft.context === "object") {
    contextOptions.push({ value: contextNow, label: tokensWithWords(draft.context.fixed) });
  }
  if (!contextOptions.some((o) => o.value === "auto")) contextOptions.splice(1, 0, { value: "auto", label: "Automatic" });
  const usesAuto = draft.context === "auto" || (draft.context === "default" && autoDefault);
  const autoOption = info.contextOptions.find((o) => o.choice === "auto");
  // What Automatic chose for the loaded copy while it uses the settings on screen (D2b),
  // else what it would choose with them now.
  const autoTokens =
    !dirty && info.loaded && !info.olderSettingsLoaded && info.autoCtx ? info.autoCtx : (autoOption?.tokens ?? 0);

  // Use the graphics card: only when there is one. A placement chosen in Fine control is
  // one more, selected option ("Custom: …"), unless the graphics card is not used at all
  // (item 6, 6.3); choosing any other option goes back to the plain placement.
  const showGpu = info.devices.length > 0;
  const custom = hasCustomPlacement(draft.fine) && !gpuOff;
  const gpuOptions: Option[] = [
    { value: "default", label: sameAs(GPU_USE_WORDS[d.gpu]) },
    ...info.gpuOptions.map((o) => ({
      value: o.gpu,
      label: `${GPU_USE_WORDS[o.gpu]} · ${FIT_WORDS[markOf(o.fit, !custom && o.gpu === info.resolved.gpu)]}`,
    })),
  ];
  if (custom) {
    gpuOptions.push({
      value: "custom",
      label: customPlacementWords(typeof fine.layers === "object" ? fine.layers.count : null, fineFacts?.units ?? null),
      disabled: true,
    });
  }
  const pickGpu = (v: string) =>
    onChange(
      hasCustomPlacement(draft.fine)
        ? { gpu: v as GpuChoice, fine: { ...fine, layers: "auto", experts: "auto" } }
        : { gpu: v as GpuChoice },
    );
  // The one warning line (4.1): too large first, then the picture's own warning.
  const warning = info.tooLarge
    ? TOO_LARGE_LINE
    : picture
      ? pictureWarning(picture, { apple: appleGraphics, saveMemory: saveMemoryOn })
      : null;

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
      {warning && (
        <p
          className={
            "rounded-md border px-3 py-2 text-sm " +
            (info.tooLarge || picture?.fits === "wontFitAtAll"
              ? "border-danger/40 bg-danger/5 text-danger"
              : "border-warning/40 bg-warning/5 text-warning")
          }
          role="alert"
        >
          {warning}
        </p>
      )}
      <Field
        id="model-context"
        label="How much it can remember"
        value={contextNow}
        options={contextOptions}
        disabled={disabled}
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
          value={custom ? "custom" : draft.gpu}
          options={gpuOptions}
          disabled={disabled}
          onChange={pickGpu}
          help={<p>{custom ? CUSTOM_PLACEMENT_HELP : gpuHelp(apple)}</p>}
        />
      )}
      <Field
        id="model-save-memory"
        label="Save memory"
        value={draft.saveMemory}
        options={toggle(d.saveMemory)}
        disabled={disabled}
        onChange={(v) => onChange({ saveMemory: v as Toggle })}
        help={<p>{HELP.saveMemory}</p>}
      />
      <Field
        id="model-keep-in-memory"
        label="Keep the model in memory"
        value={draft.keepInMemory}
        options={toggle(d.keepInMemory)}
        disabled={disabled}
        onChange={(v) => onChange({ keepInMemory: v as Toggle })}
        help={<p>{HELP.keepInMemory}</p>}
      />
      {picture && (
        <OffloadPicture
          picture={picture}
          measuring={measuring}
          fineOpen={fineOpen}
          apple={appleGraphics}
          mac={apple || appleGraphics}
        />
      )}
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
                disabled={disabled}
                onChange={(v) => onChange({ device: deviceChoice(v) })}
              />
            )}
            {info.thinking && (
              <Field
                id="model-thinking"
                label="How long it may think"
                value={draft.thinkingLimit}
                options={thinkingOptions}
                disabled={disabled}
                onChange={(v) => onChange({ thinkingLimit: v as ThinkingChoice })}
                help={<p>{HELP.thinking}</p>}
              />
            )}
          </div>
        </details>
      )}
      {fineFacts && (
        <FineControl
          facts={fineFacts}
          fine={fine}
          gpuOff={gpuOff}
          gpu={gpuNow}
          saveMemory={saveMemoryOn}
          picture={picture}
          open={fineOpen}
          onToggle={onFineToggle}
          disabled={disabled}
          onChange={(next) => onChange({ fine: next })}
        />
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

import type { ReactNode } from "react";
import type {
  ExpertsChoice,
  FastAttention,
  FineControl as FineSettings,
  FineFacts,
  GpuUse,
  KvPlace,
  LayersChoice,
  LeaveFree,
  OffloadPreview,
  ThreadsChoice,
} from "../lib/types";
import {
  expertLayersWords,
  expertsAfterLayersNote,
  fineChoiceWords,
  fineHelp,
  fineLabels,
  fineNotes,
  fineSummary,
  layersOfWords,
  leaveFreeWords,
  threadsOfWords,
} from "../lib/words";

/** Every Fine control setting at Automatic: what the app does without Fine control. */
export const FINE_AUTO: FineSettings = {
  layers: "auto",
  experts: "auto",
  kv: "auto",
  threads: "auto",
  fastAttention: "auto",
  leaveFree: "auto",
};

/** Whether every Fine control setting is Automatic (absent means Automatic). */
export function fineIsAuto(fine: FineSettings | undefined): boolean {
  if (!fine) return true;
  return (Object.keys(FINE_AUTO) as (keyof FineSettings)[]).every((k) => fine[k] === "auto");
}

/** A placement chosen in Fine control (layers or expert parts): it replaces the plain choice (I6-2). */
export function hasCustomPlacement(fine: FineSettings | undefined): boolean {
  return !!fine && (fine.layers !== "auto" || fine.experts !== "auto");
}

const LEAVE_FREE_MIB = [512, 2048, 4096] as const;

function clamp(n: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, Math.round(n)));
}

/** One Fine control setting: label, a choice from a fixed list, a slider when it takes a number, a note and the help line. */
function Setting(props: {
  id: string;
  label: string;
  value: string;
  options: { value: string; label: string }[];
  onChange: (value: string) => void;
  disabled: boolean;
  slider?: ReactNode;
  note?: string | null;
  help: string;
}) {
  return (
    <div className="space-y-1">
      <label htmlFor={props.id} className={`block text-sm font-medium ${props.disabled ? "text-fg-muted" : ""}`}>
        {props.label}
      </label>
      <select
        id={props.id}
        data-fine
        value={props.value}
        disabled={props.disabled}
        onChange={(e) => props.onChange(e.target.value)}
        className="w-full rounded-md border border-border bg-bg px-2 py-1 text-sm disabled:opacity-60"
      >
        {props.options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
      {props.slider}
      <div className="space-y-1 text-xs text-fg-muted">
        {props.note && <p className="text-fg">{props.note}</p>}
        <p>{props.help}</p>
      </div>
    </div>
  );
}

/** A native slider with its number in words beside it ("30 of 41 layers"); the arrow keys step it. */
function Slider(props: {
  id: string;
  label: string;
  value: number;
  min: number;
  max: number;
  words: string;
  disabled: boolean;
  onChange: (n: number) => void;
}) {
  return (
    <div className="flex items-center gap-3">
      <input
        id={props.id}
        type="range"
        min={props.min}
        max={props.max}
        step={1}
        value={clamp(props.value, props.min, props.max)}
        disabled={props.disabled}
        aria-label={props.label}
        aria-valuetext={props.words}
        onChange={(e) => props.onChange(clamp(Number(e.target.value), props.min, props.max))}
        className="min-w-0 flex-1 accent-[var(--accent)] disabled:opacity-60"
      />
      <span className="shrink-0 text-xs tabular-nums" data-slider-words>
        {props.words}
      </span>
    </div>
  );
}

/**
 * Fine control (item 6, section 3): a closed section in Settings for this model with the
 * finer offloading settings, each a typed value from a fixed list or range. Hidden behind
 * the plain settings, which stay the default; a setting that does nothing with the other
 * choices is greyed out with a visible note. Nothing here is a tool action: it is the same
 * in every approval mode and no tool or model output can change it (I6-17).
 */
export function FineControl(props: {
  facts: FineFacts;
  fine: FineSettings;
  /** The graphics choice resolves to Don't use it (set for this model or in the defaults). */
  gpuOff: boolean;
  /** The resolved graphics choice. */
  gpu: GpuUse;
  /** Save memory resolves to on (it needs fast attention). */
  saveMemory: boolean;
  /** The picture for the choices on screen (the summary's layer count, the sliders' start). */
  picture: OffloadPreview | null;
  open: boolean;
  onToggle: (open: boolean) => void;
  disabled: boolean;
  onChange: (fine: FineSettings) => void;
}) {
  const { facts, fine, gpuOff, gpu, saveMemory, picture, open, onToggle, disabled, onChange } = props;
  const apple = facts.apple;
  const labels = fineLabels(apple);
  const help = fineHelp(apple, facts.graphicsLimitBytes);
  const words = fineChoiceWords(apple);
  const notes = fineNotes(apple);
  const set = (patch: Partial<FineSettings>) => onChange({ ...fine, ...patch });
  const custom = fine.layers !== "auto" || fine.experts !== "auto";
  const changed = !fineIsAuto(fine);

  const units = facts.units;
  const showLayers = facts.graphics && units !== null;
  const showExperts = facts.graphics && facts.expertLayers !== null;
  const layerCount = typeof fine.layers === "object" ? fine.layers.count : null;

  // Where a slider starts: what the app would do now (layers), half the experts or threads.
  const startLayers = () =>
    units === null ? 0 : picture && picture.layersOnGraphics > 0 ? clamp(picture.layersOnGraphics, 0, units) : units;
  const expertLayers = facts.expertLayers ?? 0;
  const startExperts = () => Math.max(1, Math.floor(expertLayers / 2));
  const threads = Math.max(1, facts.threads);
  const startThreads = () => Math.max(1, Math.floor(threads / 2));

  // 1. Layers on the graphics card.
  const layersValue = layerCount === null ? "auto" : "count";
  const layersSetting = showLayers && units !== null && (
    <Setting
      id="fine-layers"
      label={labels.layers}
      value={layersValue}
      options={[
        { value: "auto", label: words.layersAuto },
        { value: "count", label: words.choose },
      ]}
      disabled={disabled || gpuOff}
      onChange={(v) => set({ layers: (v === "auto" ? "auto" : { count: startLayers() }) as LayersChoice })}
      slider={
        layerCount !== null && (
          <Slider
            id="fine-layers-count"
            label={`${labels.layers}: how many`}
            value={layerCount}
            min={0}
            max={units}
            words={layersOfWords(clamp(layerCount, 0, units), units)}
            disabled={disabled || gpuOff}
            onChange={(n) => set({ layers: { count: n } })}
          />
        )
      }
      note={gpuOff ? notes.gpuOff : null}
      help={help.layers}
    />
  );

  // 2. Expert parts on the processor.
  const expertsCount = typeof fine.experts === "object" ? fine.experts.firstLayers : null;
  const noLayers = layerCount !== null && layerCount <= 0;
  const expertsNote = gpuOff
    ? notes.gpuOff
    : noLayers
      ? notes.expertsNoLayers
      : layerCount !== null && units !== null && layerCount < units
        ? expertsAfterLayersNote(layerCount, units)
        : null;
  const expertsSetting = showExperts && (
    <Setting
      id="fine-experts"
      label={labels.experts}
      value={fine.experts === "auto" ? "auto" : fine.experts === "all" ? "all" : "some"}
      options={[
        { value: "auto", label: words.automatic },
        { value: "all", label: words.expertsAll },
        { value: "some", label: words.expertsSome },
      ]}
      disabled={disabled || gpuOff || noLayers}
      onChange={(v) =>
        set({ experts: (v === "auto" ? "auto" : v === "all" ? "all" : { firstLayers: startExperts() }) as ExpertsChoice })
      }
      slider={
        expertsCount !== null && (
          <Slider
            id="fine-experts-count"
            label={`${labels.experts}: how many`}
            value={expertsCount}
            min={1}
            max={Math.max(1, expertLayers)}
            words={expertLayersWords(clamp(expertsCount, 1, Math.max(1, expertLayers)), expertLayers)}
            disabled={disabled || gpuOff || noLayers}
            onChange={(n) => set({ experts: { firstLayers: n } })}
          />
        )
      }
      note={expertsNote}
      help={help.experts}
    />
  );

  // 3. Conversation memory on the graphics card.
  const kvSetting = facts.graphics && (
    <Setting
      id="fine-kv"
      label={labels.kv}
      value={fine.kv}
      options={[
        { value: "auto", label: words.kvAuto },
        { value: "computerMemory", label: words.kvComputer },
      ]}
      disabled={disabled || gpuOff}
      onChange={(v) => set({ kv: v as KvPlace })}
      note={gpuOff ? notes.gpuOff : null}
      help={help.kv}
    />
  );

  // 4. Processor threads.
  const threadCount = typeof fine.threads === "object" ? fine.threads.count : null;
  const threadsSetting = (
    <Setting
      id="fine-threads"
      label={labels.threads}
      value={threadCount === null ? "auto" : "count"}
      options={[
        { value: "auto", label: words.automatic },
        { value: "count", label: words.choose },
      ]}
      disabled={disabled}
      onChange={(v) => set({ threads: (v === "auto" ? "auto" : { count: startThreads() }) as ThreadsChoice })}
      slider={
        threadCount !== null && (
          <Slider
            id="fine-threads-count"
            label={`${labels.threads}: how many`}
            value={threadCount}
            min={1}
            max={threads}
            words={threadsOfWords(clamp(threadCount, 1, threads), threads)}
            disabled={disabled}
            onChange={(n) => set({ threads: { count: n } })}
          />
        )
      }
      help={help.threads}
    />
  );

  // 5. Fast attention (Save memory needs it on).
  const fastSetting = (
    <Setting
      id="fine-fast-attention"
      label={labels.fastAttention}
      value={saveMemory ? "saveMemory" : fine.fastAttention}
      options={
        saveMemory
          ? [{ value: "saveMemory", label: words.onForSaveMemory }]
          : [
              { value: "auto", label: words.automatic },
              { value: "on", label: words.on },
              { value: "off", label: words.off },
            ]
      }
      disabled={disabled || saveMemory}
      onChange={(v) => set({ fastAttention: v as FastAttention })}
      help={help.fastAttention}
    />
  );

  // 6. Leave free on the graphics card: only As much as fits uses it.
  const leaveFreeNote = gpuOff ? notes.gpuOff : custom ? notes.leaveFreeCustom : gpu === "all" ? notes.leaveFreeAll : null;
  const leaveFreeSetting = facts.graphics && (
    <Setting
      id="fine-leave-free"
      label={labels.leaveFree}
      value={fine.leaveFree === "auto" ? "auto" : String(fine.leaveFree.mib)}
      options={[
        { value: "auto", label: words.leaveFreeAuto },
        ...LEAVE_FREE_MIB.map((mib) => ({ value: String(mib), label: leaveFreeWords(mib) })),
      ]}
      disabled={disabled || leaveFreeNote !== null}
      onChange={(v) =>
        set({ leaveFree: (v === "auto" ? "auto" : { mib: Number(v) as (typeof LEAVE_FREE_MIB)[number] }) as LeaveFree })
      }
      note={leaveFreeNote}
      help={help.leaveFree}
    />
  );

  return (
    <details
      className="rounded-md border border-border px-3 py-2"
      data-section="fine"
      open={open}
      onToggle={(e) => onToggle(e.currentTarget.open)}
    >
      <summary className="cursor-pointer text-sm font-medium">{fineSummary(picture, apple, changed)}</summary>
      <div className="mt-3 space-y-4">
        {layersSetting}
        {expertsSetting}
        {kvSetting}
        {threadsSetting}
        {fastSetting}
        {leaveFreeSetting}
        <button
          type="button"
          onClick={() => onChange({ ...FINE_AUTO })}
          disabled={disabled || !changed}
          className="text-xs text-fg-muted underline hover:text-fg disabled:no-underline disabled:opacity-40"
        >
          {words.reset}
        </button>
      </div>
    </details>
  );
}

import { useId, useState, type ReactNode } from "react";
import { DropdownMenu } from "radix-ui";
import { Check, ChevronDown, CircleHelp, Plus, RotateCcw } from "lucide-react";
import { STUDIO_FIELDS } from "../../lib/words";
import type { Values } from "./canonical";
import type { Choice, Layer, MediaModel, ParamKind, ParamSpec, SizeRules } from "./types";

// One of Studio's controls (0.1.0 Task B5; design 3, "Widgets"): every field is drawn from the
// model's own `ParamSpec`, so there is no code per company. Shape and How many are chips
// (Shape's first five values, the rest under More shapes; How many 1, 2 and 4, more in a number
// box up to the model's maximum), a short list is a set of radios, a long one a select, a range
// a slider with a number box, a boolean a switch, a seed a number box, a custom size width and
// height boxes checked as they are typed, a text a text box. An unset field reads "Automatic
// (company default)" ("Automatic" on this computer) and is not sent; a set one offers its
// reset. Added pictures (RefTray.tsx) and Extra fields (Advanced, Task B6) are drawn elsewhere.
// Also: which fields show (what each requires is met) and the values that no longer apply.

/** How a field is drawn. */
export type Widget = "chips" | "segmented" | "select" | "slider" | "switch" | "seed" | "drop" | "size" | "text" | "json";

/** The most values a list shows as radios; a longer list is a select. */
const SEGMENTED_MAX = 5;
/** How many shapes show as chips; the rest are under More shapes. */
const SHAPES_SHOWN = 5;
/** How many's chips (those within the model's range); the simple layer stops at 4. */
const COUNT_CHIPS = [1, 2, 4];

/** The widget for `spec`: Shape and How many by their role, whatever their size, then by kind. */
export function widgetFor(spec: ParamSpec): Widget {
  const kind = spec.kind;
  if (spec.canonical === "shape" && kind.type === "enum") return "chips";
  if (spec.canonical === "count" && kind.type === "range") return "chips";
  switch (kind.type) {
    case "enum":
      return kind.values.length <= SEGMENTED_MAX ? "segmented" : "select";
    case "range":
      return "slider";
    case "boolean":
      return "switch";
    case "seed":
      return "seed";
    case "images":
      return "drop";
    case "size":
      return "size";
    case "text":
      return "text";
    case "extra":
      return "json";
  }
}

function isSet(value: unknown): boolean {
  return value !== undefined && value !== null;
}

/** Whether what `spec` requires is met: the field it names has one of the values listed, an
 *  unset field counting as its company default (the backend's `unmet` rule). */
function applies(params: ParamSpec[], spec: ParamSpec, values: Values): boolean {
  const requires = spec.requires;
  if (!requires) return true;
  const set = values[requires.key];
  const current = isSet(set) ? set : (params.find((p) => p.key === requires.key)?.companyDefault ?? null);
  return current !== null && requires.values.some((v) => v === current);
}

/** The fields of `layer`, in the model's order, whose requirements `values` meets. */
export function visibleFields(params: ParamSpec[], layer: Layer, values: Values): ParamSpec[] {
  return params.filter((p) => p.layer === layer && applies(params, p, values));
}

/** `values` without each value whose field requires what `values` no longer has, again until
 *  nothing more goes (a field may require one that went). The same object when nothing goes. */
export function dropUnmet(model: MediaModel, values: Values): Values {
  let now = values;
  for (;;) {
    const gone = new Set(
      Object.keys(now).filter((key) => {
        const spec = model.params.find((p) => p.key === key);
        return spec !== undefined && !applies(model.params, spec, now);
      }),
    );
    if (gone.size === 0) return now;
    now = Object.fromEntries(Object.entries(now).filter(([key]) => !gone.has(key)));
  }
}

// ---- the field ------------------------------------------------------------------------------

const CHIP =
  "rounded-full border border-border px-2 py-0.5 text-xs whitespace-nowrap hover:border-border-strong aria-checked:border-accent aria-checked:text-accent aria-expanded:border-border-strong";
const BOX =
  "rounded-md border border-border bg-bg px-2 py-1 text-sm outline-none focus:border-border-strong disabled:opacity-60";

/**
 * One field: its label (with its help as a tooltip), "Automatic" while it is unset or its reset
 * once set, and its control. `local` says the model runs on this computer, where there is no
 * company default to name. `timeOf` gives a chip its time here ("2 s · about 3 min"). Pictures
 * (`drop`) and Extra fields (`json`) are drawn by the tray and by Advanced, so nothing here.
 */
export function ParamField(props: {
  spec: ParamSpec;
  value: unknown;
  onChange(v: unknown): void;
  onReset(): void;
  local?: boolean;
  timeOf?: (value: unknown) => string | null;
}) {
  const { spec, value, onChange, onReset } = props;
  const labelId = useId();
  const widget = widgetFor(spec);
  if (widget === "drop" || widget === "json") return null;
  const set = isSet(value);
  const automatic = props.local ? STUDIO_FIELDS.automaticHere : STUDIO_FIELDS.automatic;
  // A select names Automatic as its own first choice, and an unset seed's box says Random
  // (Studio picks one for each run).
  const namesUnset = widget === "select" || widget === "seed" || (spec.kind.type === "size" && spec.kind.values.length > 0);

  return (
    <div role="group" aria-labelledby={labelId} data-param={spec.key} className="flex flex-col gap-1.5">
      <div className="flex min-h-5 items-center gap-1.5">
        <span id={labelId} className="text-xs font-medium text-fg-muted">
          {spec.label}
        </span>
        {spec.help && (
          <span title={spec.help} className="text-fg-muted">
            <CircleHelp size={12} aria-hidden />
            <span className="sr-only">{spec.help}</span>
          </span>
        )}
        {set ? (
          <button
            type="button"
            aria-label={STUDIO_FIELDS.reset(spec.label)}
            title={STUDIO_FIELDS.reset(spec.label)}
            onClick={onReset}
            className="ml-auto rounded p-0.5 text-fg-muted hover:bg-bg-muted hover:text-fg"
          >
            <RotateCcw size={12} aria-hidden />
          </button>
        ) : (
          !namesUnset && <span className="ml-auto text-xs text-fg-muted">{automatic}</span>
        )}
      </div>
      <Control
        spec={spec}
        widget={widget}
        labelId={labelId}
        value={value}
        automatic={automatic}
        onChange={onChange}
        onReset={onReset}
        timeOf={props.timeOf}
      />
    </div>
  );
}

/** The control `widget` draws for the field's kind. */
function Control(props: {
  spec: ParamSpec;
  widget: Widget;
  labelId: string;
  value: unknown;
  automatic: string;
  onChange(v: unknown): void;
  onReset(): void;
  timeOf?: (value: unknown) => string | null;
}) {
  const { spec, widget, labelId, value, onChange, onReset } = props;
  const kind = spec.kind;
  const number = typeof value === "number" ? value : null;
  const orReset = (n: number | null) => (n === null ? onReset() : onChange(n));
  if (widget === "chips" && kind.type === "enum") {
    return <ShapeChips labelId={labelId} choices={kind.values} value={value} onChange={onChange} />;
  }
  if (widget === "chips" && kind.type === "range") {
    return <CountChips labelId={labelId} kind={kind} value={number} onChange={orReset} />;
  }
  if (widget === "segmented" && kind.type === "enum") {
    return <Radios labelId={labelId} choices={kind.values} value={value} onChange={onChange} timeOf={props.timeOf} />;
  }
  if (widget === "select" && kind.type === "enum") {
    return (
      <ChoiceSelect
        labelId={labelId}
        choices={kind.values}
        value={value}
        automatic={props.automatic}
        onChange={onChange}
        onReset={onReset}
      />
    );
  }
  if (widget === "slider" && kind.type === "range") {
    return <Slider labelId={labelId} kind={kind} value={number} companyDefault={spec.companyDefault} onChange={orReset} />;
  }
  if (widget === "switch") {
    return (
      <input
        type="checkbox"
        role="switch"
        aria-labelledby={labelId}
        checked={isSet(value) ? value === true : spec.companyDefault === true}
        onChange={(e) => onChange(e.target.checked)}
        className="self-start accent-[var(--accent)]"
      />
    );
  }
  if (widget === "seed" && kind.type === "seed") {
    return (
      <NumberBox
        labelledBy={labelId}
        value={number}
        min={0}
        max={kind.max}
        step={1}
        placeholder={STUDIO_FIELDS.random}
        onCommit={orReset}
        className={`${BOX} w-36`}
      />
    );
  }
  if (widget === "size" && kind.type === "size") {
    return (
      <SizeField
        labelId={labelId}
        rules={kind.rules}
        listed={kind.values}
        value={typeof value === "string" ? value : null}
        automatic={props.automatic}
        onChange={onChange}
        onReset={onReset}
      />
    );
  }
  if (widget === "text" && kind.type === "text") {
    return (
      <TextBox
        labelId={labelId}
        maxChars={kind.maxChars}
        value={typeof value === "string" ? value : ""}
        onChange={onChange}
        onReset={onReset}
      />
    );
  }
  return null;
}

/** A chip's words: the value's name, then its time here when it has one. */
function chipWords(choice: Choice, timeOf?: (value: unknown) => string | null): string {
  const time = timeOf?.(choice.value) ?? null;
  return time ? STUDIO_FIELDS.withTime(choice.label, time) : choice.label;
}

/** A list as radios; `children` follow them in the same row (Shape's More shapes). */
function Radios(props: {
  labelId: string;
  choices: Choice[];
  value: unknown;
  onChange(v: unknown): void;
  timeOf?: (value: unknown) => string | null;
  children?: ReactNode;
}) {
  return (
    <div role="radiogroup" aria-labelledby={props.labelId} className="flex flex-wrap gap-1">
      {props.choices.map((choice) => (
        <button
          key={String(choice.value)}
          type="button"
          role="radio"
          aria-checked={choice.value === props.value}
          onClick={() => props.onChange(choice.value)}
          className={CHIP}
        >
          {chipWords(choice, props.timeOf)}
        </button>
      ))}
      {props.children}
    </div>
  );
}

/** Shape: the first five shapes as chips, the rest under More shapes, which names the shape
 *  chosen there. */
function ShapeChips(props: { labelId: string; choices: Choice[]; value: unknown; onChange(v: unknown): void }) {
  const shown = props.choices.slice(0, SHAPES_SHOWN);
  const rest = props.choices.slice(SHAPES_SHOWN);
  const chosen = rest.findIndex((c) => c.value === props.value);
  return (
    <Radios labelId={props.labelId} choices={shown} value={props.value} onChange={props.onChange}>
      {rest.length > 0 && (
        <DropdownMenu.Root>
          <DropdownMenu.Trigger asChild>
            <button
              type="button"
              aria-label={STUDIO_FIELDS.moreShapes}
              title={STUDIO_FIELDS.moreShapes}
              className={`${CHIP} flex items-center gap-0.5 ${chosen >= 0 ? "border-accent text-accent" : ""}`}
            >
              {chosen >= 0 ? rest[chosen]?.label : <Plus size={12} aria-hidden />}
              <ChevronDown size={12} aria-hidden />
            </button>
          </DropdownMenu.Trigger>
          <DropdownMenu.Portal>
            <DropdownMenu.Content
              align="start"
              sideOffset={4}
              collisionPadding={8}
              className="z-50 min-w-28 rounded-xl border border-border bg-surface p-1 text-sm shadow-pop"
            >
              <DropdownMenu.RadioGroup
                value={chosen >= 0 ? String(chosen) : ""}
                onValueChange={(i) => {
                  const choice = rest[Number(i)];
                  if (choice) props.onChange(choice.value);
                }}
              >
                {rest.map((choice, i) => (
                  <DropdownMenu.RadioItem
                    key={String(choice.value)}
                    value={String(i)}
                    className="flex cursor-default items-center gap-2 rounded-lg px-2 py-1 outline-none data-[highlighted]:bg-bg-muted"
                  >
                    <span className="w-3.5">
                      <DropdownMenu.ItemIndicator>
                        <Check size={14} aria-hidden className="text-accent" />
                      </DropdownMenu.ItemIndicator>
                    </span>
                    {choice.label}
                  </DropdownMenu.RadioItem>
                ))}
              </DropdownMenu.RadioGroup>
            </DropdownMenu.Content>
          </DropdownMenu.Portal>
        </DropdownMenu.Root>
      )}
    </Radios>
  );
}

/** Whole numbers from `min` to `max` on `step`: whether any is not one of `chips`. */
function moreThan(chips: number[], kind: { min: number; max: number; step?: number | null }): boolean {
  const step = kind.step && kind.step > 0 ? kind.step : 1;
  for (let n = kind.min; n <= kind.max; n += step) if (!chips.includes(n)) return true;
  return false;
}

/** How many: 1, 2 and 4 (those within the range) as chips, and More, a number box up to the
 *  model's maximum (open whenever the number set is not a chip). */
function CountChips(props: {
  labelId: string;
  kind: Extract<ParamKind, { type: "range" }>;
  value: number | null;
  onChange(n: number | null): void;
}) {
  const { kind, value } = props;
  const chips = COUNT_CHIPS.filter((n) => n >= kind.min && n <= kind.max);
  const [more, setMore] = useState(false);
  const open = more || (value !== null && !chips.includes(value));
  const choices = chips.map((n) => ({ value: n, label: String(n) }));
  return (
    <div className="flex flex-col gap-1.5">
      <Radios labelId={props.labelId} choices={choices} value={value} onChange={(v) => props.onChange(v as number)}>
        {moreThan(chips, kind) && (
          <button
            type="button"
            aria-label={STUDIO_FIELDS.more}
            title={STUDIO_FIELDS.more}
            aria-expanded={open}
            onClick={() => setMore(!open)}
            className={CHIP}
          >
            <Plus size={12} aria-hidden />
          </button>
        )}
      </Radios>
      {open && (
        <NumberBox
          labelledBy={props.labelId}
          value={value}
          min={kind.min}
          max={kind.max}
          step={kind.step}
          onCommit={props.onChange}
          className={`${BOX} w-20`}
        />
      )}
    </div>
  );
}

/** A long list: Automatic first, then the model's values. */
function ChoiceSelect(props: {
  labelId: string;
  choices: Choice[];
  value: unknown;
  automatic: string;
  onChange(v: unknown): void;
  onReset(): void;
}) {
  const index = props.choices.findIndex((c) => c.value === props.value);
  return (
    <select
      aria-labelledby={props.labelId}
      value={index < 0 ? "" : String(index)}
      onChange={(e) => {
        const choice = props.choices[Number(e.target.value)];
        if (e.target.value === "" || !choice) props.onReset();
        else props.onChange(choice.value);
      }}
      className={BOX}
    >
      <option value="">{props.automatic}</option>
      {props.choices.map((choice, i) => (
        <option key={String(choice.value)} value={String(i)}>
          {choice.label}
        </option>
      ))}
    </select>
  );
}

/** `n` within `min`..`max`, on the steps from `min` when there are steps. */
function fitNumber(n: number, min: number, max: number, step?: number | null): number {
  const clamped = Math.min(max, Math.max(min, n));
  if (!step) return clamped;
  const stepped = min + Math.round((clamped - min) / step) * step;
  return Math.min(max, Number(stepped.toPrecision(12)));
}

/** A range: a slider (at the company's default, else the lowest, while unset) and a number
 *  box, with the field's unit. */
function Slider(props: {
  labelId: string;
  kind: Extract<ParamKind, { type: "range" }>;
  value: number | null;
  companyDefault: unknown;
  onChange(n: number | null): void;
}) {
  const { kind, value } = props;
  const resting = typeof props.companyDefault === "number" ? props.companyDefault : kind.min;
  return (
    <div className="flex items-center gap-2">
      <input
        type="range"
        aria-labelledby={props.labelId}
        min={kind.min}
        max={kind.max}
        step={kind.step ?? "any"}
        value={value ?? resting}
        onChange={(e) => props.onChange(fitNumber(Number(e.target.value), kind.min, kind.max, kind.step))}
        className="min-w-0 flex-1 accent-[var(--accent)]"
      />
      <NumberBox
        labelledBy={props.labelId}
        value={value}
        min={kind.min}
        max={kind.max}
        step={kind.step}
        onCommit={props.onChange}
        className={`${BOX} w-16`}
      />
      {kind.unit && <span className="text-xs text-fg-muted">{kind.unit}</span>}
    </div>
  );
}

/** A number typed, taken when it is done (Enter, or leaving the box), moved into the field's
 *  range; emptied, the field is unset again. It follows a value changed elsewhere. */
function NumberBox(props: {
  labelledBy?: string;
  label?: string;
  value: number | null;
  min: number;
  max: number;
  step?: number | null;
  placeholder?: string;
  onCommit(n: number | null): void;
  className: string;
}) {
  const { value } = props;
  const words = (n: number | null) => (n === null ? "" : String(n));
  const [text, setText] = useState(words(value));
  const [shown, setShown] = useState(value);
  if (shown !== value) {
    setShown(value);
    setText(words(value));
  }
  const commit = () => {
    const typed = text.trim();
    if (typed === "") {
      if (value !== null) props.onCommit(null);
      return;
    }
    const n = Number(typed);
    if (!Number.isFinite(n)) {
      setText(words(value));
      return;
    }
    const fitted = fitNumber(n, props.min, props.max, props.step);
    setText(words(fitted));
    if (fitted !== value) props.onCommit(fitted);
  };
  return (
    <input
      type="number"
      inputMode="numeric"
      aria-labelledby={props.labelledBy}
      aria-label={props.label}
      min={props.min}
      max={props.max}
      step={props.step ?? "any"}
      placeholder={props.placeholder}
      value={text}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          commit();
        }
      }}
      className={props.className}
    />
  );
}

/** A ratio as a shape: 0.25 → "1:4", 3 → "3:1". */
function ratioWords(ratio: number): string {
  const side = Number((ratio >= 1 ? ratio : 1 / ratio).toFixed(2)).toString();
  return ratio >= 1 ? `${side}:1` : `1:${side}`;
}

/** Why `width` × `height` does not meet `rules` (the checks of Rust's `check_size`, in its
 *  order), or `null` when it does. */
function sizeProblem(width: string, height: string, rules: SizeRules): string | null {
  const words = STUDIO_FIELDS.size;
  const side = (text: string) => (/^\d+$/.test(text.trim()) ? Number(text.trim()) : 0);
  const w = side(width);
  const h = side(height);
  if (w <= 0 || h <= 0) return words.both;
  if (rules.multipleOf > 0 && (w % rules.multipleOf !== 0 || h % rules.multipleOf !== 0)) {
    return words.multipleOf(rules.multipleOf);
  }
  const ratio = w / h;
  if (ratio < rules.minRatio - 1e-9 || ratio > rules.maxRatio + 1e-9) {
    return words.ratio(ratioWords(rules.minRatio), ratioWords(rules.maxRatio));
  }
  if (Math.max(w, h) > rules.maxSide) return words.longestSide(rules.maxSide);
  if (w * h < rules.minPixels) return words.tooSmall;
  if (w * h > rules.maxPixels) return words.tooLarge;
  return null;
}

/** A `WxH` size as its two sides' words, or empty ones. */
function sidesOf(value: string | null): [string, string] {
  const match = value ? /^(\d+)x(\d+)$/.exec(value) : null;
  return match ? [match[1] ?? "", match[2] ?? ""] : ["", ""];
}

/** A size: the model's listed sizes in a select (Automatic first, then Custom size) when it
 *  lists any, and width and height boxes for a custom one, checked as they are typed; a size
 *  is sent only once it fits. */
function SizeField(props: {
  labelId: string;
  rules: SizeRules;
  listed: Choice[];
  value: string | null;
  automatic: string;
  onChange(v: unknown): void;
  onReset(): void;
}) {
  const { listed, value } = props;
  const index = listed.findIndex((c) => c.value === value);
  const isCustom = (v: string | null) => listed.length === 0 || (v !== null && !listed.some((c) => c.value === v));
  const [custom, setCustom] = useState(isCustom(value));
  // A size changed elsewhere (a reset, another model) picks its own place in the list.
  const [shown, setShown] = useState(value);
  if (shown !== value) {
    setShown(value);
    if (value === null || index >= 0) setCustom(isCustom(value));
  }
  return (
    <div className="flex flex-col gap-1.5">
      {listed.length > 0 && (
        <select
          aria-labelledby={props.labelId}
          value={custom ? "custom" : index < 0 ? "" : String(index)}
          onChange={(e) => {
            const picked = e.target.value;
            setCustom(picked === "custom");
            const choice = listed[Number(picked)];
            if (picked === "") props.onReset();
            else if (picked !== "custom" && choice) props.onChange(choice.value);
          }}
          className={BOX}
        >
          <option value="">{props.automatic}</option>
          {listed.map((choice, i) => (
            <option key={String(choice.value)} value={String(i)}>
              {choice.label}
            </option>
          ))}
          <option value="custom">{STUDIO_FIELDS.customSize}</option>
        </select>
      )}
      {custom && <SizeBoxes rules={props.rules} value={index < 0 ? value : null} onChange={props.onChange} onReset={props.onReset} />}
    </div>
  );
}

/** Width and height, checked as they are typed: a size that fits is sent, emptied boxes unset
 *  the field, and anything else says what to change. */
function SizeBoxes(props: { rules: SizeRules; value: string | null; onChange(v: unknown): void; onReset(): void }) {
  const { value } = props;
  const [sides, setSides] = useState(sidesOf(value));
  const [shown, setShown] = useState(value);
  // A size sent, or changed elsewhere (a reset, another model): the boxes show it. A size
  // typed that does not fit yet changes nothing, so it stays in the boxes.
  if (shown !== value) {
    setShown(value);
    setSides(sidesOf(value));
  }
  const [width, height] = sides;
  const empty = width.trim() === "" && height.trim() === "";
  const problem = empty ? null : sizeProblem(width, height, props.rules);

  const change = (w: string, h: string) => {
    setSides([w, h]);
    if (w.trim() === "" && h.trim() === "") {
      if (value !== null) props.onReset();
      return;
    }
    if (sizeProblem(w, h, props.rules) !== null) return;
    const size = `${Number(w)}x${Number(h)}`;
    if (size !== value) props.onChange(size);
  };
  const box = `${BOX} w-20`;
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center gap-1.5 text-sm">
        <input
          type="number"
          inputMode="numeric"
          aria-label={STUDIO_FIELDS.width}
          placeholder={STUDIO_FIELDS.width}
          min={1}
          value={width}
          onChange={(e) => change(e.target.value, height)}
          className={box}
        />
        <span aria-hidden className="text-fg-muted">
          ×
        </span>
        <input
          type="number"
          inputMode="numeric"
          aria-label={STUDIO_FIELDS.height}
          placeholder={STUDIO_FIELDS.height}
          min={1}
          value={height}
          onChange={(e) => change(width, e.target.value)}
          className={box}
        />
      </div>
      {problem && <p className="text-xs text-warning">{problem}</p>}
    </div>
  );
}

/** A text field, kept within the model's limit; emptied, it is unset. */
function TextBox(props: { labelId: string; maxChars: number; value: string; onChange(v: unknown): void; onReset(): void }) {
  const length = Array.from(props.value).length;
  return (
    <div className="flex flex-col gap-1">
      <textarea
        aria-labelledby={props.labelId}
        value={props.value}
        rows={2}
        onChange={(e) => {
          const text = Array.from(e.target.value).slice(0, props.maxChars).join("");
          if (text === "") props.onReset();
          else props.onChange(text);
        }}
        className={`${BOX} resize-y`}
      />
      <span className="self-end text-xs text-fg-muted">{STUDIO_FIELDS.charsLeft(props.maxChars - length)}</span>
    </div>
  );
}

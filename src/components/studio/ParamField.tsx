import { useEffect, useEffectEvent, useId, useState } from "react";
import { RadioGroup } from "radix-ui";
import { CircleHelp, Dices, RotateCcw } from "lucide-react";
import { SETTING_WORDS, STUDIO_FIELDS } from "../../lib/words";
import { CheckboxField } from "../ui/CheckboxField";
import { SelectField } from "../ui/SelectField";
import { SliderField } from "../ui/SliderField";
import { SwitchField } from "../ui/SwitchField";
import { Tip } from "../ui/Tip";
import { BOX, CHOICE_CHIP, ICON_BUTTON } from "./styles";
import type { Choice, ParamKind, ParamSpec, SizeRules } from "./types";

// One of Studio's controls (0.1.0 Task B5; design 3, "Widgets"; the app's styled Radix controls
// since 0.2.0 Task U8, critique §2.8): every field is drawn from the model's own `ParamSpec`, so
// there is no code per company. A short list is a row of chips (a radio group), a long one a
// select, a range a slider with its value beside its name, a boolean a switch, a seed a number
// box (under Advanced with its dice and *Keep it*, Task B6), a custom size width and height boxes
// checked as they are typed, a text a text box. An unset field is not sent: it shows the
// model's own default where the model names one, else "Automatic (company default)"
// ("Automatic" on this computer) inside the control, as its first chip, its placeholder or its
// value (critique §4.f: never a second label beside it); a set one offers its reset. Help is a
// tooltip on a help button. Shape and How many are the composer's menus (ComposerMenus.tsx) and
// never drawn here; added pictures (the composer), Extra fields (AdvancedDrawer.tsx) and LoRAs
// (LorasSection.tsx) are drawn elsewhere.
// Which fields show, and the values that no longer apply, are worked out in canonical.ts.

/** How a field is drawn. */
export type Widget = "segmented" | "select" | "slider" | "switch" | "seed" | "drop" | "size" | "text" | "json" | "loras";

/** The most values a list shows as chips; a longer list is a select. */
const SEGMENTED_MAX = 5;

/** The value Automatic takes in a list: the model's choices take their index, so none is it. */
const AUTOMATIC = "automatic";

/** The widget for `spec`, by its kind. */
export function widgetFor(spec: ParamSpec): Widget {
  const kind = spec.kind;
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
    case "loras":
      return "loras";
  }
}

function isSet(value: unknown): boolean {
  return value !== undefined && value !== null;
}

/** Whether Automatic is one of the field's choices: the model names no default for it, and
 *  Studio starts it at no value of its own (a video's length), as the composer's menus do. */
function takesAutomatic(spec: ParamSpec): boolean {
  return !isSet(spec.companyDefault) && !isSet(spec.initial);
}

/** A number as a range shows it, with the field's unit ("80%", "500 ms", "7.5"). */
function numberWords(n: number, unit: string | null | undefined): string {
  return unit ? SETTING_WORDS.withUnit(String(n), unit) : String(n);
}

// ---- the field ------------------------------------------------------------------------------

/**
 * One field: its name (with its help as a tooltip), its reset once it is set, and its control.
 * `local` says the model runs on this computer, where there is no company default to name.
 * `timeOf` gives a chip its time here ("2 s · about 3 min"). `seed` gives a seed its dice (a new
 * seed) and its lock (*Keep it*). Pictures (`drop`), Extra fields (`json`) and LoRAs are drawn
 * by the composer, by Advanced and by the LoRAs section, so nothing here.
 */
export function ParamField(props: {
  spec: ParamSpec;
  value: unknown;
  onChange(v: unknown): void;
  onReset(): void;
  local?: boolean;
  timeOf?: (value: unknown) => string | null;
  seed?: SeedControls;
  /** What the field holds that cannot be sent (a custom size that does not fit), or `null`:
   *  told as it changes, and `null` once the field is gone. */
  onInvalid?: (problem: string | null) => void;
}) {
  const { spec, value, onChange, onReset } = props;
  const labelId = useId();
  const helpId = useId();
  const widget = widgetFor(spec);
  if (widget === "drop" || widget === "json" || widget === "loras") return null;
  const set = isSet(value);
  const automatic = props.local ? STUDIO_FIELDS.automaticHere : STUDIO_FIELDS.automatic;
  const reset = set && (
    <Tip content={STUDIO_FIELDS.reset(spec.label)}>
      <button type="button" aria-label={STUDIO_FIELDS.reset(spec.label)} onClick={onReset} className={ICON_BUTTON}>
        <RotateCcw size={14} aria-hidden />
      </button>
    </Tip>
  );

  if (widget === "switch") {
    // A switch always shows a state (the model's default while unset): its help goes under it.
    return (
      <div role="group" aria-label={spec.label} data-param={spec.key} className="flex items-start gap-1">
        <div className="min-w-0 flex-1">
          <SwitchField
            label={spec.label}
            description={spec.help ?? undefined}
            checked={set ? value === true : spec.companyDefault === true}
            onChange={onChange}
          />
        </div>
        {reset}
      </div>
    );
  }

  const readout = spec.kind.type === "range" ? rangeWords(spec.kind, value, spec.companyDefault, automatic) : null;
  return (
    <div
      role="group"
      aria-labelledby={labelId}
      aria-describedby={spec.help ? helpId : undefined}
      data-param={spec.key}
      className="flex flex-col gap-2"
    >
      <div className="flex min-h-7 items-center gap-1">
        <span id={labelId} className="text-sm font-medium">
          {spec.label}
        </span>
        {spec.help && <HelpButton id={helpId} label={spec.label} help={spec.help} />}
        <span className="ml-auto flex items-center gap-1">
          {readout !== null && <span className="text-sm text-fg-muted tabular-nums">{readout}</span>}
          {reset}
        </span>
      </div>
      <Control
        spec={spec}
        widget={widget}
        labelId={labelId}
        value={value}
        automatic={automatic}
        readout={readout}
        onChange={onChange}
        onReset={onReset}
        timeOf={props.timeOf}
        seed={props.seed}
        onInvalid={props.onInvalid}
      />
    </div>
  );
}

/** A field's help: a button whose tooltip holds it (on hover and keyboard focus), and the same
 *  words for a screen reader, which describe the field (`id`). */
function HelpButton(props: { id: string; label: string; help: string }) {
  return (
    <>
      <Tip content={props.help}>
        <button type="button" aria-label={STUDIO_FIELDS.about(props.label)} className={`${ICON_BUTTON} size-6`}>
          <CircleHelp size={14} aria-hidden />
        </button>
      </Tip>
      <span id={props.id} className="sr-only">
        {props.help}
      </span>
    </>
  );
}

/** A range's value as its row says it: the value set, else the model's default, else
 *  Automatic. */
function rangeWords(
  kind: Extract<ParamKind, { type: "range" }>,
  value: unknown,
  companyDefault: unknown,
  automatic: string,
): string {
  if (typeof value === "number") return numberWords(value, kind.unit);
  if (typeof companyDefault === "number") return numberWords(companyDefault, kind.unit);
  return automatic;
}

/** A seed's dice and lock. */
export interface SeedControls {
  /** The seed stays as it is for the next run. */
  locked: boolean;
  /** The dice: a new random seed. */
  onNew(): void;
  onLock(locked: boolean): void;
}

/** The control `widget` draws for the field's kind. */
function Control(props: {
  spec: ParamSpec;
  widget: Widget;
  labelId: string;
  value: unknown;
  automatic: string;
  readout: string | null;
  onChange(v: unknown): void;
  onReset(): void;
  timeOf?: (value: unknown) => string | null;
  seed?: SeedControls;
  onInvalid?: (problem: string | null) => void;
}) {
  const { spec, widget, labelId, value, onChange, onReset } = props;
  const kind = spec.kind;
  const number = typeof value === "number" ? value : null;
  const orReset = (n: number | null) => (n === null ? onReset() : onChange(n));
  if (widget === "segmented" && kind.type === "enum") {
    return (
      <Chips
        labelId={labelId}
        spec={spec}
        choices={kind.values}
        value={value}
        automatic={props.automatic}
        onChange={onChange}
        onReset={onReset}
        timeOf={props.timeOf}
      />
    );
  }
  if (widget === "select" && kind.type === "enum") {
    return (
      <ChoiceSelect
        labelId={labelId}
        spec={spec}
        choices={kind.values}
        value={value}
        automatic={props.automatic}
        onChange={onChange}
        onReset={onReset}
      />
    );
  }
  if (widget === "slider" && kind.type === "range") {
    const resting = number ?? (typeof spec.companyDefault === "number" ? spec.companyDefault : kind.min);
    return (
      <SliderField
        label={spec.label}
        hideLabel
        min={kind.min}
        max={kind.max}
        step={kind.step && kind.step > 0 ? kind.step : (kind.max - kind.min) / 100}
        value={resting}
        onChange={(n) => orReset(fitNumber(n, kind.min, kind.max, kind.step))}
        format={(n) => (number === null ? (props.readout ?? String(n)) : numberWords(n, kind.unit))}
      />
    );
  }
  if (widget === "seed" && kind.type === "seed") {
    const seed = props.seed;
    return (
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <div className="flex items-center gap-1.5">
          <NumberBox
            labelledBy={labelId}
            value={number}
            min={0}
            max={kind.max}
            step={1}
            placeholder={STUDIO_FIELDS.random}
            onCommit={orReset}
            className={`${BOX} w-36 tabular-nums`}
          />
          {seed && (
            <Tip content={STUDIO_FIELDS.newSeed}>
              <button
                type="button"
                aria-label={STUDIO_FIELDS.newSeed}
                onClick={seed.onNew}
                className={`${ICON_BUTTON} size-8 border border-border bg-surface`}
              >
                <Dices size={15} aria-hidden />
              </button>
            </Tip>
          )}
        </div>
        {seed && <CheckboxField label={STUDIO_FIELDS.keepIt} checked={seed.locked} onChange={seed.onLock} />}
      </div>
    );
  }
  if (widget === "size" && kind.type === "size") {
    return (
      <SizeField
        labelId={labelId}
        label={spec.label}
        rules={kind.rules}
        listed={kind.values}
        value={typeof value === "string" ? value : null}
        automatic={props.automatic}
        onChange={onChange}
        onReset={onReset}
        onInvalid={props.onInvalid}
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

/** A short list as chips, one chosen (a radio group: the arrow keys move along it): Automatic
 *  first when the field takes it, chosen while unset, and choosing it resets the field; else
 *  the model's default chosen while unset. */
function Chips(props: {
  labelId: string;
  spec: ParamSpec;
  choices: Choice[];
  value: unknown;
  automatic: string;
  onChange(v: unknown): void;
  onReset(): void;
  timeOf?: (value: unknown) => string | null;
}) {
  const { choices, value } = props;
  const automatic = takesAutomatic(props.spec);
  const shown = isSet(value) ? value : props.spec.companyDefault;
  const index = choices.findIndex((c) => c.value === shown);
  const current = index >= 0 ? String(index) : automatic && !isSet(value) ? AUTOMATIC : "";
  return (
    <RadioGroup.Root
      aria-labelledby={props.labelId}
      value={current}
      onValueChange={(v) => {
        const choice = choices[Number(v)];
        if (v === AUTOMATIC) props.onReset();
        else if (choice) props.onChange(choice.value);
      }}
      className="flex flex-wrap gap-1.5"
    >
      {automatic && (
        <RadioGroup.Item value={AUTOMATIC} className={CHOICE_CHIP}>
          {props.automatic}
        </RadioGroup.Item>
      )}
      {choices.map((choice, i) => (
        <RadioGroup.Item key={String(choice.value)} value={String(i)} className={CHOICE_CHIP}>
          {chipWords(choice, props.timeOf)}
        </RadioGroup.Item>
      ))}
    </RadioGroup.Root>
  );
}

/** A long list: the value set, else the model's default, else Automatic as its placeholder.
 *  Once set, Automatic heads the list (when the field takes it) and puts the field back. */
function ChoiceSelect(props: {
  labelId: string;
  spec: ParamSpec;
  choices: Choice[];
  value: unknown;
  automatic: string;
  onChange(v: unknown): void;
  onReset(): void;
}) {
  const { choices, value } = props;
  const shown = isSet(value) ? value : props.spec.companyDefault;
  const index = choices.findIndex((c) => c.value === shown);
  const options = [
    ...(isSet(value) && takesAutomatic(props.spec) ? [{ value: AUTOMATIC, label: props.automatic }] : []),
    ...choices.map((choice, i) => ({ value: String(i), label: choice.label })),
  ];
  return (
    <SelectField
      label={props.spec.label}
      labelledBy={props.labelId}
      value={index < 0 ? null : String(index)}
      placeholder={props.automatic}
      options={options}
      onChange={(v) => {
        const choice = choices[Number(v)];
        if (v === AUTOMATIC) props.onReset();
        else if (choice) props.onChange(choice.value);
      }}
    />
  );
}

/** `n` within `min`..`max`, on the steps from `min` when there are steps. */
function fitNumber(n: number, min: number, max: number, step?: number | null): number {
  const clamped = Math.min(max, Math.max(min, n));
  if (!step) return clamped;
  const stepped = min + Math.round((clamped - min) / step) * step;
  return Math.min(max, Number(stepped.toPrecision(12)));
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

/** The value Custom size takes in the size select. */
const CUSTOM = "custom";

/** A size: the model's listed sizes in a select (Automatic its placeholder, then the sizes and
 *  Custom size; Automatic heads the list once a size or Custom size is chosen) when it lists
 *  any, and width and height boxes for a custom one, checked as they are typed; a size is sent
 *  only once it fits. */
function SizeField(props: {
  labelId: string;
  label: string;
  rules: SizeRules;
  listed: Choice[];
  value: string | null;
  automatic: string;
  onChange(v: unknown): void;
  onReset(): void;
  onInvalid?: (problem: string | null) => void;
}) {
  const { listed, value } = props;
  const index = listed.findIndex((c) => c.value === value);
  // The one thing kept here: Custom size chosen (or the boxes emptied) with no size typed yet.
  // It is forgotten when a listed size is set and when the field changes to another model's.
  const [chosen, setChosen] = useState(false);
  const field = `${JSON.stringify(props.rules)} ${listed.map((c) => String(c.value)).join(" ")}`;
  const [shownField, setShownField] = useState(field);
  if (shownField !== field) {
    setShownField(field);
    setChosen(false);
  }
  if (chosen && index >= 0) setChosen(false);
  // Everything else follows the size set and the field, whoever set it (a carry, a setup, the
  // store): a size not in the list is a custom one, and a field that lists none has only boxes.
  const custom = listed.length === 0 || (value !== null ? index < 0 : chosen);
  const options = [
    ...(value !== null || chosen ? [{ value: AUTOMATIC, label: props.automatic }] : []),
    ...listed.map((choice, i) => ({ value: String(i), label: choice.label })),
    { value: CUSTOM, label: STUDIO_FIELDS.customSize },
  ];
  return (
    <div className="flex flex-col gap-2">
      {listed.length > 0 && (
        <SelectField
          label={props.label}
          labelledBy={props.labelId}
          value={custom ? CUSTOM : index < 0 ? null : String(index)}
          placeholder={props.automatic}
          options={options}
          onChange={(picked) => {
            const choice = listed[Number(picked)];
            setChosen(picked === CUSTOM);
            // Custom size starts with empty boxes: a listed size set goes, as nothing is
            // sent until a custom one fits.
            if (picked === AUTOMATIC || (picked === CUSTOM && value !== null)) props.onReset();
            else if (picked !== CUSTOM && choice) props.onChange(choice.value);
          }}
        />
      )}
      {custom && (
        <SizeBoxes
          rules={props.rules}
          value={value}
          onChange={props.onChange}
          onInvalid={props.onInvalid}
          onReset={() => {
            // Boxes emptied to type another size stay open.
            setChosen(true);
            props.onReset();
          }}
        />
      )}
    </div>
  );
}

/** Width and height, checked as they are typed: a size that fits is sent, emptied boxes unset
 *  the field, and anything else says what to change, and is told (`onInvalid`) so Create waits
 *  while the boxes show a size other than the one that would be sent. */
function SizeBoxes(props: {
  rules: SizeRules;
  value: string | null;
  onChange(v: unknown): void;
  onReset(): void;
  onInvalid?: (problem: string | null) => void;
}) {
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
  const onInvalid = useEffectEvent((p: string | null) => props.onInvalid?.(p));
  useEffect(() => {
    onInvalid(problem);
  }, [problem]);
  // Gone (a listed size picked, another model): nothing it held holds Create back.
  useEffect(() => () => onInvalid(null), []);

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
  const box = `${BOX} w-24 tabular-nums`;
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center gap-2 text-sm">
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

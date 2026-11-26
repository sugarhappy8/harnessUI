import { ChevronDown, Images, RectangleHorizontal, RectangleVertical, Square, Timer } from "lucide-react";
import { STUDIO_COMPOSER, STUDIO_FIELDS } from "../../lib/words";
import { draftModel, useStudio, type StudioTab } from "../../store/studio";
import { MenuButton, MenuRadio } from "../ui/MenuButton";
import { COMPOSER_CHIP } from "./styles";
import type { Canonical, Choice, MediaModel, ParamSpec } from "./types";

// Shape, How many and a video's Length in the composer's toolbar (0.2.0 Task U3, design 10.1:
// Settings "is never needed for a basic run"): each a compact menu of radio items whose button
// names the value ("Shape: 1:1", "How many: 2", "Length: 5 s"). A field left unset shows the
// company's default when the model names one, else Automatic, which is then the menu's first
// item and puts the field back to unset (unless Studio starts the field at a value of its own);
// a value set that the menu does not list is named as it is. A model without the field has no menu. Length lists at most 16 of the model's lengths
// (every second up to 10 s, then wider steps), always with its default and its longest.

/** The value a menu's radio items carry for an unset field. */
const UNSET = "";

/** The model's field for `canonical`, when it has one. */
function fieldOf(model: MediaModel | null, canonical: Canonical): ParamSpec | null {
  return model?.params.find((p) => p.canonical === canonical) ?? null;
}

/** The radio value of `value`, `UNSET` for none. */
function radioValue(value: unknown): string {
  return value === undefined || value === null ? UNSET : String(value);
}

/** The value `tab`'s draft holds for field `spec`, if any. */
function useFieldValue(tab: StudioTab, spec: ParamSpec | null): unknown {
  return useStudio((s) => (spec ? s.drafts[tab].values[spec.key] : undefined));
}

/** One field's menu state for `tab`: what it shows (`label`: the choice's name, the value as it
 *  is when the menu does not list it, else Automatic), its radio items, and how a pick is set. */
function useFieldMenu(tab: StudioTab, spec: ParamSpec | null, choices: Choice[]) {
  const value = useFieldValue(tab, spec);
  const here = useStudio((s) => s.drafts[tab].target === "thisComputer");
  const setValue = useStudio((s) => s.setValue);
  const resetValue = useStudio((s) => s.resetValue);
  const shown = radioValue(value ?? spec?.companyDefault);
  const options = [
    // Without a default the model names or a value Studio starts with, the company (or Studio
    // here) picks: Automatic. A field Studio starts at a value (a video's length) lists values.
    ...(spec && radioValue(spec.companyDefault) === UNSET && radioValue(spec.initial) === UNSET
      ? [{ value: UNSET, label: here ? STUDIO_FIELDS.automaticHere : STUDIO_FIELDS.automatic }]
      : []),
    ...choices.map((c) => ({ value: String(c.value), label: c.label })),
  ];
  const pick = (v: string) => {
    if (!spec) return;
    if (v === UNSET) resetValue(tab, spec.key);
    else setValue(tab, spec.key, choices.find((c) => String(c.value) === v)?.value ?? v);
  };
  const chosen = choices.find((c) => String(c.value) === shown);
  const label = chosen?.label ?? (shown === UNSET ? STUDIO_FIELDS.automaticHere : shown);
  return { shown, chosen, label, options, pick };
}

/** The icon for shape `value` ("16:9" is wide, "9:16" tall); a square for anything else. */
function ShapeIcon(props: { value: string }) {
  const sides = /^(\d+):(\d+)$/.exec(props.value);
  const w = Number(sides?.[1]);
  const h = Number(sides?.[2]);
  const Icon = sides && w > h ? RectangleHorizontal : sides && h > w ? RectangleVertical : Square;
  return <Icon size={14} aria-hidden className="shrink-0 text-fg-muted" />;
}

/** Shape: the model's shapes as radio items; nothing for a model without a list of shapes. */
export function ShapeMenu(props: { tab: StudioTab }) {
  const model = useStudio((s) => draftModel(s, props.tab));
  const spec = fieldOf(model, "shape");
  const choices = spec?.kind.type === "enum" ? spec.kind.values : [];
  const menu = useFieldMenu(props.tab, spec, choices);
  if (!spec || choices.length === 0) return null;
  return (
    <MenuButton
      label={STUDIO_COMPOSER.menuValue(STUDIO_COMPOSER.shape, menu.label)}
      className={COMPOSER_CHIP}
      trigger={
        <>
          <ShapeIcon value={menu.shown} />
          <span className="tabular-nums">{menu.label}</span>
          <ChevronDown size={14} aria-hidden className="shrink-0 text-fg-muted" />
        </>
      }
    >
      <MenuRadio value={menu.shown} options={menu.options} onChange={menu.pick} />
    </MenuButton>
  );
}

/** The most numbers How many lists. */
const COUNT_MAX_ITEMS = 16;

/** How many: each number the model's range takes, on its steps; nothing for a model without. */
export function CountMenu(props: { tab: StudioTab }) {
  const model = useStudio((s) => draftModel(s, props.tab));
  const spec = fieldOf(model, "count");
  const choices: Choice[] = [];
  if (spec?.kind.type === "range") {
    const step = spec.kind.step && spec.kind.step > 0 ? spec.kind.step : 1;
    for (let n = Math.max(1, Math.ceil(spec.kind.min)); n <= spec.kind.max && choices.length < COUNT_MAX_ITEMS; n += step) {
      choices.push({ value: n, label: String(n) });
    }
  }
  const menu = useFieldMenu(props.tab, spec, choices);
  if (!spec || choices.length === 0) return null;
  return (
    <MenuButton
      label={STUDIO_COMPOSER.menuValue(STUDIO_COMPOSER.count, menu.label)}
      className={COMPOSER_CHIP}
      trigger={
        <>
          <Images size={14} aria-hidden className="shrink-0 text-fg-muted" />
          <span className="tabular-nums">{menu.shown === UNSET ? menu.label : STUDIO_COMPOSER.times(menu.label)}</span>
          <ChevronDown size={14} aria-hidden className="shrink-0 text-fg-muted" />
        </>
      }
    >
      <MenuRadio value={menu.shown} options={menu.options} onChange={menu.pick} />
    </MenuButton>
  );
}

/** The most lengths the Length menu lists. */
const LENGTH_MAX_ITEMS = 16;

/** The lengths a person picks, in seconds: every second up to 10, then wider steps. */
const LENGTH_LADDER: ReadonlySet<number> = new Set([
  1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12, 15, 20, 25, 30, 40, 45, 60, 75, 90, 120, 150, 180, 240, 300, 360, 480, 600,
]);

/** At most `max` of `pool`, in its order, spread evenly from its first to its last. */
function spread<T>(pool: T[], max: number): T[] {
  if (pool.length <= max) return pool;
  if (max <= 0) return [];
  if (max === 1) return pool.slice(0, 1);
  return [...new Set(Array.from({ length: max }, (_, i) => pool[Math.round((i * (pool.length - 1)) / (max - 1))]!))];
}

/**
 * The lengths the Length menu lists of `choices` (a model's Length field): all of them up to
 * `LENGTH_MAX_ITEMS`; else the longest, each of `keep` that is listed (its default, the value
 * set), then the lengths of `LENGTH_LADDER` the model lists, the short ones first, the longer
 * ones spread over what room is left. In the model's own order.
 */
export function lengthChoices(choices: Choice[], keep: unknown[]): Choice[] {
  if (choices.length <= LENGTH_MAX_ITEMS) return choices;
  const seconds = (c: Choice) => Number(c.value);
  const longest = choices.reduce((a, c) => (seconds(c) > seconds(a) ? c : a));
  const kept = new Set(keep.filter((k) => k !== undefined && k !== null).map(String));
  const required = choices.filter((c) => c === longest || kept.has(String(c.value)));
  const ladder = choices.filter((c) => !required.includes(c) && LENGTH_LADDER.has(seconds(c)));
  const short = spread(
    ladder.filter((c) => seconds(c) <= 10),
    LENGTH_MAX_ITEMS - required.length,
  );
  const long = spread(
    ladder.filter((c) => seconds(c) > 10),
    LENGTH_MAX_ITEMS - required.length - short.length,
  );
  const listed = new Set([...required, ...short, ...long]);
  return choices.filter((c) => listed.has(c));
}

/** Length: a video's lengths, at most 16 (`lengthChoices`); nothing for a model without. */
export function LengthMenu(props: { tab: StudioTab }) {
  const model = useStudio((s) => draftModel(s, props.tab));
  const spec = fieldOf(model, "length");
  const value = useFieldValue(props.tab, spec);
  const all = spec?.kind.type === "enum" ? spec.kind.values : [];
  const choices = spec ? lengthChoices(all, [spec.initial, spec.companyDefault, value]) : [];
  const menu = useFieldMenu(props.tab, spec, choices);
  if (!spec || choices.length === 0) return null;
  return (
    <MenuButton
      label={STUDIO_COMPOSER.menuValue(STUDIO_COMPOSER.length, menu.label)}
      className={COMPOSER_CHIP}
      trigger={
        <>
          <Timer size={14} aria-hidden className="shrink-0 text-fg-muted" />
          <span className="tabular-nums">{menu.label}</span>
          <ChevronDown size={14} aria-hidden className="shrink-0 text-fg-muted" />
        </>
      }
    >
      <MenuRadio value={menu.shown} options={menu.options} onChange={menu.pick} />
    </MenuButton>
  );
}

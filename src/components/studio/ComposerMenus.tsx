import { ChevronDown, Images, RectangleHorizontal, RectangleVertical, Square } from "lucide-react";
import { STUDIO_COMPOSER, STUDIO_FIELDS } from "../../lib/words";
import { draftModel, useStudio, type StudioTab } from "../../store/studio";
import { MenuButton, MenuRadio } from "../ui/MenuButton";
import { COMPOSER_CHIP } from "./styles";
import type { Canonical, Choice, MediaModel, ParamSpec } from "./types";

// Shape and How many in the composer's toolbar (0.2.0 Task U3, design 10.1): each a compact menu
// of radio items whose button names the value ("Shape: 1:1", "How many: 2"). A field left
// unset shows the company's default when the model names one, else Automatic, which is then
// the menu's first item and puts the field back to unset. A model without the field has no
// menu.

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

/** One field's menu state for `tab`: what it shows, its radio items, and how a pick is set. */
function useFieldMenu(tab: StudioTab, spec: ParamSpec | null, choices: Choice[]) {
  const value = useStudio((s) => (spec ? s.drafts[tab].values[spec.key] : undefined));
  const here = useStudio((s) => s.drafts[tab].target === "thisComputer");
  const setValue = useStudio((s) => s.setValue);
  const resetValue = useStudio((s) => s.resetValue);
  const shown = radioValue(value ?? spec?.companyDefault);
  const options = [
    // Without a default the model names, the company (or Studio here) picks: Automatic.
    ...(spec && radioValue(spec.companyDefault) === UNSET
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
  return { shown, chosen, options, pick };
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
  const label = menu.chosen?.label ?? STUDIO_FIELDS.automaticHere;
  return (
    <MenuButton
      label={STUDIO_COMPOSER.menuValue(STUDIO_COMPOSER.shape, label)}
      className={COMPOSER_CHIP}
      trigger={
        <>
          <ShapeIcon value={menu.shown} />
          <span className="tabular-nums">{label}</span>
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
  const label = menu.chosen?.label ?? STUDIO_FIELDS.automaticHere;
  return (
    <MenuButton
      label={STUDIO_COMPOSER.menuValue(STUDIO_COMPOSER.count, label)}
      className={COMPOSER_CHIP}
      trigger={
        <>
          <Images size={14} aria-hidden className="shrink-0 text-fg-muted" />
          <span className="tabular-nums">{menu.chosen ? STUDIO_COMPOSER.times(label) : label}</span>
          <ChevronDown size={14} aria-hidden className="shrink-0 text-fg-muted" />
        </>
      }
    >
      <MenuRadio value={menu.shown} options={menu.options} onChange={menu.pick} />
    </MenuButton>
  );
}

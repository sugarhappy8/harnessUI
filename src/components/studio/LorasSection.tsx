import { useId } from "react";
import { DropdownMenu } from "radix-ui";
import { AlertTriangle, Download, FilePlus2, Layers, Plus, X } from "lucide-react";
import { FILE_LABEL, IMPORT, LORAS, STUDIO_CARD, STUDIO_PANEL, typeName } from "../../lib/words";
import { useStudio, type StudioTab } from "../../store/studio";
import { MenuButton, MenuItem } from "../ui/MenuButton";
import { SliderField } from "../ui/SliderField";
import { SwitchField } from "../ui/SwitchField";
import { Tip } from "../ui/Tip";
import { ICON_BUTTON, PANEL_BUTTON, PANEL_CARD, PANEL_SECTION } from "./styles";
import type { InstalledLocal, Lora, LoraUse, ParamKind } from "./types";

// The LoRAs section of Studio's side panel (0.2.0 Task U8, design 8 and 10.1). *Add* lists the
// installed LoRAs not attached yet (`lorasFor`): those made for the model's type first, then
// those whose type is not known under "LoRA for an unknown model type" (not judged: ruling
// R6-lora-none), then the others, each marked "may not fit this model", a warning and never a
// refusal. A LoRA whose file is gone is greyed out there with its reason. Each attached LoRA is
// a card: its name and what it is for, a strength slider within the model's own bounds (its
// `loras` field), the High-noise part switch on a model with a high-noise part (Wan 2.2 A14B),
// and Remove; one whose file went missing since says so ("a removed LoRA") and keeps only
// Remove. Under them, *Add a LoRA file* and *Get LoRAs* (Get models on its LoRAs filter). The
// composer's LoRAs button counts them and opens the panel here.

type LorasField = Extract<ParamKind, { type: "loras" }>;

/** While the draft attaches none. */
const NONE: LoraUse[] = [];

/** A strength as its slider says it: "1.00", "0.85", "-0.25". */
function strengthWords(weight: number): string {
  return weight.toFixed(2);
}

/** What a LoRA is for: "LoRA for SDXL", or "LoRA for an unknown model type". */
function madeFor(lora: Lora): string {
  return FILE_LABEL.lora(lora.base === null ? null : typeName(lora.base, null));
}

/** Whether `lora` may not fit `model`: both their types are known, and they differ. */
function mayNotFit(lora: Lora, model: InstalledLocal): boolean {
  return lora.base !== null && model.loraBase !== null && lora.base !== model.loraBase;
}

/** The LoRAs section of `model`, a model on this computer that takes LoRAs, in `tab`'s draft. */
export function LorasSection(p: { tab: StudioTab; model: InstalledLocal }) {
  const { tab, model } = p;
  const headingId = useId();
  const spec = model.model.params.find((x) => x.kind.type === "loras");
  const field = spec?.kind.type === "loras" ? spec.kind : null;
  const uses = useStudio((s) => {
    const held = spec ? s.drafts[tab].values[spec.key] : undefined;
    return Array.isArray(held) ? (held as LoraUse[]) : NONE;
  });
  // `lorasFor` reads them: followed here, so the section shows a LoRA added or gone.
  const loras = useStudio((s) => s.loras);
  const lorasFor = useStudio((s) => s.lorasFor);
  const attachLora = useStudio((s) => s.attachLora);
  const openImport = useStudio((s) => s.openImport);
  const showLocalModels = useStudio((s) => s.showLocalModels);
  const setSearchFilter = useStudio((s) => s.setSearchFilter);
  if (!field) return <></>;

  const attached = new Set(uses.map((u) => u.id));
  // Each group without the LoRAs attached already, those whose file is gone last.
  const offered = (list: Lora[]) =>
    list.filter((l) => !attached.has(l.id)).sort((a, b) => Number(a.missing) - Number(b.missing));
  const groups = lorasFor(model);
  const fits = offered(groups.fits);
  const unknown = offered(groups.unknown);
  const others = offered(groups.others);
  const canAdd = uses.length < field.max && fits.length + unknown.length + others.length > 0;
  const getLoras = () => {
    showLocalModels(true);
    setSearchFilter("loras");
  };

  return (
    <section aria-labelledby={headingId} data-settings-section="loras" className={PANEL_SECTION}>
      <div className="flex min-h-7 items-center gap-2">
        <h3 id={headingId} className="text-section">
          {STUDIO_PANEL.loras}
        </h3>
        {uses.length > 0 && (
          <span className="rounded-full bg-accent/10 px-2 py-px text-xs font-medium text-accent tabular-nums">
            {uses.length}
          </span>
        )}
        {canAdd && (
          <div className="ml-auto">
            <MenuButton
              label={LORAS.add}
              align="end"
              className={`${PANEL_BUTTON} py-1`}
              contentClassName="w-72 max-h-[min(26rem,var(--radix-dropdown-menu-content-available-height))] overflow-y-auto"
              trigger={
                <>
                  <Plus size={14} aria-hidden className="text-fg-muted" />
                  {LORAS.add}
                </>
              }
            >
              {fits.length > 0 && model.loraBase !== null && (
                <DropdownMenu.Label className={MENU_HEADING}>
                  {FILE_LABEL.lora(typeName(model.loraBase, null))}
                </DropdownMenu.Label>
              )}
              {fits.map((l) => (
                <AddItem key={l.id} lora={l} warn={false} onAdd={attachLora} />
              ))}
              {unknown.length > 0 && (
                <>
                  {fits.length > 0 && <DropdownMenu.Separator className={MENU_SEPARATOR} />}
                  <DropdownMenu.Label className={MENU_HEADING}>{LORAS.unknownType}</DropdownMenu.Label>
                  {unknown.map((l) => (
                    <AddItem key={l.id} lora={l} warn={false} onAdd={attachLora} />
                  ))}
                </>
              )}
              {others.length > 0 && fits.length + unknown.length > 0 && (
                <DropdownMenu.Separator className={MENU_SEPARATOR} />
              )}
              {others.map((l) => (
                <AddItem key={l.id} lora={l} warn onAdd={attachLora} />
              ))}
            </MenuButton>
          </div>
        )}
      </div>
      {loras.length === 0 && <p className="text-sm text-fg-muted">{LORAS.none}</p>}
      {uses.length > 0 && (
        <ul className="flex flex-col gap-2">
          {uses.map((use) => {
            const lora = loras.find((l) => l.id === use.id) ?? null;
            return (
              <li key={use.id}>
                <LoraRow
                  use={use}
                  lora={lora}
                  field={field}
                  highNoise={model.highNoise && field.highNoise}
                  warn={lora !== null && mayNotFit(lora, model)}
                />
              </li>
            );
          })}
        </ul>
      )}
      <div className="flex flex-wrap gap-2">
        <button type="button" onClick={() => openImport("lora")} className={PANEL_BUTTON}>
          <FilePlus2 size={14} aria-hidden className="text-fg-muted" />
          {IMPORT.addLora}
        </button>
        <button type="button" onClick={getLoras} className={PANEL_BUTTON}>
          <Download size={14} aria-hidden className="text-accent" />
          {LORAS.getLoras}
        </button>
      </div>
    </section>
  );
}

/** A heading inside *Add*'s menu. */
const MENU_HEADING = "px-2.5 pt-2 pb-1 text-xs font-medium text-fg-muted";

/** A line between *Add*'s groups. */
const MENU_SEPARATOR = "-mx-1 my-1 h-px bg-border";

/** One LoRA in *Add*'s menu: its name, and "may not fit this model" (`warn`), or, greyed out,
 *  why its file cannot be used. */
function AddItem(props: { lora: Lora; warn: boolean; onAdd(id: string): void }) {
  const { lora } = props;
  return (
    <MenuItem disabled={lora.missing} onSelect={() => props.onAdd(lora.id)}>
      <Layers size={15} aria-hidden className="mt-0.5 shrink-0 self-start text-fg-muted" />
      <span className="flex min-w-0 flex-col">
        <span data-lora-name className="truncate">
          {lora.name}
        </span>
        {lora.missing ? (
          <span className="text-xs">{LORAS.missing}</span>
        ) : (
          props.warn && <span className="text-xs text-warning">{LORAS.mayNotFit}</span>
        )}
      </span>
    </MenuItem>
  );
}

/**
 * One attached LoRA (`lora`: its row, `null` once it was deleted): its name and what it is for,
 * or "may not fit this model" (`warn`); its strength slider, from the model's `weightMin` to
 * `weightMax` by `weightStep`; the High-noise part switch where the model has one
 * (`highNoise`); and Remove. A LoRA whose file is gone says so, and keeps only Remove.
 */
export function LoraRow(props: { use: LoraUse; lora: Lora | null; field: LorasField; highNoise: boolean; warn: boolean }) {
  const { use, lora, field } = props;
  const setLoraWeight = useStudio((s) => s.setLoraWeight);
  const setLoraHighNoise = useStudio((s) => s.setLoraHighNoise);
  const detachLora = useStudio((s) => s.detachLora);
  const name = lora?.name ?? STUDIO_CARD.loraGone;
  const gone = lora === null || lora.missing;
  return (
    <div role="group" aria-label={name} className={`${PANEL_CARD} flex flex-col gap-3 p-3`}>
      <div className="flex items-start gap-2.5">
        <span
          aria-hidden
          className={`grid size-8 shrink-0 place-items-center rounded-lg ${gone ? "bg-warning/10 text-warning" : "bg-accent/10 text-accent"}`}
        >
          {gone ? <AlertTriangle size={15} /> : <Layers size={15} />}
        </span>
        <div className="flex min-w-0 flex-1 flex-col gap-0.5">
          <p className="truncate text-sm font-medium">
            <bdi>{name}</bdi>
          </p>
          {lora === null || lora.missing ? (
            <div className="flex flex-wrap items-center gap-x-1.5 gap-y-1 text-xs text-warning">
              {lora !== null && (
                <span className="rounded-full bg-warning/10 px-1.5 py-px font-medium">{STUDIO_CARD.loraGone}</span>
              )}
              <span>{LORAS.missing}</span>
            </div>
          ) : props.warn ? (
            <p className="text-xs text-warning">{LORAS.mayNotFit}</p>
          ) : (
            <p className="truncate text-xs text-fg-muted">{madeFor(lora)}</p>
          )}
        </div>
        <Tip content={LORAS.remove}>
          <button
            type="button"
            aria-label={LORAS.removeOf(name)}
            onClick={() => detachLora(use.id)}
            className={`${ICON_BUTTON} -mt-0.5 -mr-1`}
          >
            <X size={15} aria-hidden />
          </button>
        </Tip>
      </div>
      {!gone && (
        <>
          <SliderField
            label={LORAS.strength}
            name={LORAS.strengthOf(name)}
            min={field.weightMin}
            max={field.weightMax}
            step={field.weightStep}
            value={use.weight}
            onChange={(w) => setLoraWeight(use.id, w)}
            format={strengthWords}
          />
          {props.highNoise && (
            <SwitchField
              label={LORAS.highNoise}
              checked={use.highNoise}
              onChange={(on) => setLoraHighNoise(use.id, on)}
            />
          )}
        </>
      )}
    </div>
  );
}

import { formatBytes } from "../../lib/format";
import {
  KIND_WORD,
  LOCAL_FIT_WORDS,
  LOCAL_MARK,
  LOCAL_WARNING,
  MODEL_SHEET,
  RECIPE_TYPE_NAME,
  STUDIO_PRICE,
  STUDIO_TIME,
  typeName,
} from "../../lib/words";
import {
  isAutoModel,
  makesKind,
  priceNotAsked,
  RECENT_RUNS,
  recentChoices,
  sameTarget,
  type SheetFilter,
  type StudioState,
  type StudioTab,
} from "../../store/studio";
import { formatUsd } from "./cost";
import { etaText } from "./eta";
import type {
  Canonical,
  Company,
  InstalledLocal,
  LocalModels,
  MediaModel,
  ModelChoice,
  RunTarget,
  StudioEndpoint,
} from "./types";

// What the model sheet lists (0.2.0 Task U4, design 10.2), worked out apart from how it is
// drawn (ModelSheet.tsx): Recent, up to four models from the newest runs that are still offered,
// never repeated below; On this computer, every installed model of the tab's kind where this
// computer can make it (one that needs setting up, or that this version cannot read, included);
// then each cloud endpoint's models, an endpoint without a key or without its lists saved as one
// row with its fix. The search and the All · On this computer · Cloud filter apply to all three.
// Also the words a row shows: its type, size, fit, price and what it can do. `pickerGroups`,
// `priceHint` and `localHint` are 0.1.0's picker's (Task B3), moved here.

/** What the filter chips select: models that take added pictures (Edit, References), a
 *  start or end frame, that make sound, or a transparent background. */
export type PickerFilter = "edit" | "references" | "startFrame" | "endFrame" | "sound" | "transparent";

/** One group of the picker's list. */
export interface PickerGroup {
  place: "local" | "cloud";
  /** The endpoint of a family's group; `null` for On this computer and Recent. */
  endpoint: StudioEndpoint | null;
  /** The family (`openai/gpt-image`), or the group's own name: "On this computer", "Recent". */
  family: string;
  models: MediaModel[];
  /** Recent only: the endpoint each of `models` ran at, in the same order. */
  via?: StudioEndpoint[];
}

// ---- what a model offers --------------------------------------------------------------------

/** How many reference pictures the model takes (0 when none). */
function referencesMax(model: MediaModel): number {
  let max = 0;
  for (const p of model.params) {
    if (p.canonical === "references" && p.kind.type === "images") max = Math.max(max, p.kind.max);
  }
  return max;
}

function hasField(model: MediaModel, canonical: Canonical): boolean {
  return model.params.some((p) => p.canonical === canonical);
}

/** Whether the model's list field for `canonical` offers `value`. */
function offers(model: MediaModel, canonical: Canonical, value: string): boolean {
  return model.params.some(
    (p) => p.canonical === canonical && p.kind.type === "enum" && p.kind.values.some((c) => c.value === value),
  );
}

function hasFeature(model: MediaModel, filter: PickerFilter): boolean {
  switch (filter) {
    case "edit":
    case "references":
      return referencesMax(model) >= 1;
    case "startFrame":
    case "endFrame":
    case "sound":
      return hasField(model, filter);
    case "transparent":
      return offers(model, "background", "transparent");
  }
}

/** Whether the model's name or id holds the search text, in any case. */
function matches(model: MediaModel, query: string): boolean {
  const text = query.trim().toLowerCase();
  return text === "" || model.displayName.toLowerCase().includes(text) || model.id.toLowerCase().includes(text);
}

/** The installed models of `kind` this computer can make (video needs a usable graphics card
 *  or an Apple-silicon Mac), those this version cannot read included. */
export function installedHere(local: LocalModels, kind: StudioTab): InstalledLocal[] {
  return kind === "image" || !local.videoOnProcessor ? local.installed.filter((m) => m.kind === kind) : [];
}

/**
 * The picker's groups for `kind`, each model passing the search and every filter: "On this
 * computer" first whenever local models are read (the installed models of the kind, possibly
 * none), then "Recent" (the cloud models of `recent`, in its order, each at the endpoint it
 * names, when any are still listed there), then each endpoint's models by family, in the order
 * of the endpoints and of their lists. A recent model stays in its family too. Models Studio
 * cannot use are listed (the picker greys them out); "Auto" models never are.
 */
export function pickerGroups(
  local: LocalModels | null,
  endpoints: StudioEndpoint[],
  kind: StudioTab,
  query: string,
  filters: PickerFilter[],
  recent: ModelChoice[],
): PickerGroup[] {
  const wanted = (m: MediaModel) =>
    m.kind === kind && !isAutoModel(m) && matches(m, query) && filters.every((f) => hasFeature(m, f));
  const groups: PickerGroup[] = [];
  if (local) {
    const models = installedHere(local, kind).map((m) => m.model).filter(wanted);
    groups.push({ place: "local", endpoint: null, family: LOCAL_MARK.name, models });
  }
  const recentRows = recent.flatMap(({ target, model: id }) => {
    if (target === "thisComputer") return [];
    const endpoint = endpoints.find((e) => e.providerId === target.endpoint);
    const model = endpoint?.models.find((m) => m.id === id && wanted(m));
    return endpoint && model ? [{ endpoint, model }] : [];
  });
  if (recentRows.length > 0) {
    groups.push({
      place: "cloud",
      endpoint: null,
      family: MODEL_SHEET.recent,
      models: recentRows.map((r) => r.model),
      via: recentRows.map((r) => r.endpoint),
    });
  }
  for (const endpoint of endpoints) {
    const families = new Map<string, MediaModel[]>();
    for (const m of endpoint.models.filter(wanted)) families.set(m.family, [...(families.get(m.family) ?? []), m]);
    for (const [family, models] of families) groups.push({ place: "cloud", endpoint, family, models });
  }
  return groups;
}

// ---- hints ----------------------------------------------------------------------------------

/** A cloud model's price in words: "from $0.04 a picture", "$0.10–$0.40 a second", "price shown
 *  when picked" (an OpenRouter picture not asked for yet), or "price unknown". */
export function priceHint(model: MediaModel, company: Company): string {
  if (priceNotAsked(model, company)) return STUDIO_PRICE.whenPicked;
  const { unit, low, high } = model.price;
  if (unit === "free") return STUDIO_PRICE.free;
  if (low === null || high === null) return STUDIO_PRICE.unknown;
  if (unit === "image") {
    return low === high ? STUDIO_PRICE.perPicture(formatUsd(low)) : STUDIO_PRICE.fromPerPicture(formatUsd(low));
  }
  if (unit === "second") {
    return low === high
      ? STUDIO_PRICE.perSecond(formatUsd(low))
      : STUDIO_PRICE.rangePerSecond(formatUsd(low), formatUsd(high));
  }
  return STUDIO_PRICE.unknown;
}

/** "for a 2 s clip": the model's starting clip length, as its Length field names it. */
function clipWords(model: MediaModel): string {
  const field = model.params.find((p) => p.canonical === "length" && p.layer === "simple");
  const length = field?.initial;
  if (!field || length === null || length === undefined) return STUDIO_TIME.aClip;
  const label = field.kind.type === "enum" ? field.kind.values.find((c) => c.value === length)?.label : undefined;
  if (label) return STUDIO_TIME.perClip(label);
  return typeof length === "number" ? STUDIO_TIME.perClip(STUDIO_TIME.seconds(length)) : STUDIO_TIME.aClip;
}

/** How long a local model takes here: "about 25 s a picture", "about 3 min for a 2 s clip"
 *  (its starting length), "(first guess)" until measured; "time shown after your first run
 *  here" without an estimate. */
export function localHint(model: InstalledLocal, kind: StudioTab): string {
  const per = kind === "image" ? STUDIO_TIME.perPicture : clipWords(model.model);
  return etaText(model.fit.eta, per) ?? STUDIO_TIME.notYet;
}

// ---- the sheet ------------------------------------------------------------------------------

/** One model the sheet lists, where it runs. */
export interface SheetModel {
  /** Unique in the sheet: where it runs and which model. */
  key: string;
  target: RunTarget;
  model: MediaModel;
  /** The installed model, on this computer; `null` for a cloud model. */
  installed: InstalledLocal | null;
  /** The endpoint of a cloud model; `null` on this computer. */
  endpoint: StudioEndpoint | null;
  /** The tab's draft is on it. */
  selected: boolean;
  /** How many LoRAs the draft attaches to it (only the draft's own model has any). */
  loras: number;
}

/** A cloud endpoint's models, or (`fix`) the one row that says what it lacks: a key, or its
 *  lists, which are fetched only when asked. */
export interface SheetCloudGroup {
  /** Its name, as the composer's chip names where a cloud model runs ("OpenRouter"). */
  company: string;
  endpoint: StudioEndpoint;
  fix: "noKey" | "getList" | null;
  rows: SheetModel[];
}

/** What the sheet lists, in its three parts. */
export interface SheetSections {
  recent: SheetModel[];
  local: SheetModel[];
  cloud: SheetCloudGroup[];
}

/** The most Recent tiles. */
const RECENT_TILES = 4;

/** Whether `endpoint` is a cloud endpoint whose company makes `tab`'s kind and whose lists are
 *  not saved yet (the composer's cloud backup counts it, `cloudBackup`). */
export function listsNotSaved(endpoint: StudioEndpoint, tab: StudioTab): boolean {
  return endpoint.cloud && endpoint.fetchedAt === null && makesKind(endpoint, tab);
}

/** How many LoRAs the draft's `values` attach to `model`. */
function lorasOf(model: MediaModel, values: Record<string, unknown>): number {
  const field = model.params.find((p) => p.kind.type === "loras");
  const held = field ? values[field.key] : undefined;
  return Array.isArray(held) ? held.length : 0;
}

/**
 * The sheet's models for `tab`, each passing the search `query` and `filter`:
 * - `local`: the installed models of the kind where this computer can make it, newest first,
 *   those it cannot use included (each row says why);
 * - `cloud`: each endpoint whose company makes the kind, in their order: its models in its
 *   lists' order, or, without a key or before its lists are saved, no models and its `fix`;
 * - `recent`: at most four models from the newest runs, each where it ran, still offered there
 *   and usable, in the runs' order. They are left out of `local` and `cloud`.
 */
export function sheetSections(
  s: Pick<StudioState, "local" | "endpoints" | "recentRuns" | "drafts">,
  tab: StudioTab,
  query: string,
  filter: SheetFilter,
): SheetSections {
  const draft = s.drafts[tab];
  const groups = pickerGroups(
    filter === "cloud" ? null : s.local,
    filter === "local" ? [] : s.endpoints,
    tab,
    query,
    [],
    [],
  );
  const row = (target: RunTarget, model: MediaModel, installed: InstalledLocal | null, endpoint: StudioEndpoint | null) => {
    const selected = draft.target !== null && draft.model === model.id && sameTarget(draft.target, target);
    return {
      key: `${endpoint ? `endpoint ${endpoint.providerId}` : "here"} ${model.id}`,
      target,
      model,
      installed,
      endpoint,
      selected,
      loras: selected && installed ? lorasOf(model, draft.values) : 0,
    };
  };

  const installed = s.local ? installedHere(s.local, tab) : [];
  const local = (groups.find((g) => g.place === "local")?.models ?? []).flatMap((model) => {
    const here = installed.find((m) => m.model === model);
    return here ? [row("thisComputer", model, here, null)] : [];
  });
  const cloud: SheetCloudGroup[] = [];
  if (filter !== "local") {
    for (const endpoint of s.endpoints) {
      if (!makesKind(endpoint, tab)) continue;
      const fix = !endpoint.hasKey ? "noKey" : listsNotSaved(endpoint, tab) ? "getList" : null;
      const target: RunTarget = { endpoint: endpoint.providerId };
      const rows = fix
        ? []
        : groups.filter((g) => g.endpoint === endpoint).flatMap((g) => g.models.map((m) => row(target, m, null, endpoint)));
      if (fix !== null || rows.length > 0) cloud.push({ company: endpoint.name, endpoint, fix, rows });
    }
  }

  const offered = [...local, ...cloud.flatMap((g) => g.rows)];
  const recent: SheetModel[] = [];
  for (const choice of recentChoices(s.recentRuns, tab, RECENT_RUNS)) {
    if (recent.length === RECENT_TILES) break;
    const found = offered.find(
      (r) => r.model.id === choice.model && sameTarget(r.target, choice.target) && r.model.usable === null,
    );
    if (found) recent.push(found);
  }
  const inRecent = new Set(recent.map((r) => r.key));
  return {
    recent,
    local: local.filter((r) => !inRecent.has(r.key)),
    cloud: cloud
      .map((g) => ({ ...g, rows: g.rows.filter((r) => !inRecent.has(r.key)) }))
      .filter((g) => g.fix !== null || g.rows.length > 0),
  };
}

// ---- what a row says ------------------------------------------------------------------------

/** What a cloud model can do, as at most two icons: change a picture, a transparent background. */
export function capabilities(model: MediaModel): ("canEdit" | "transparent")[] {
  const can: ("canEdit" | "transparent")[] = [];
  if (referencesMax(model) >= 1) can.push("canEdit");
  if (offers(model, "background", "transparent")) can.push("transparent");
  return can;
}

/** A cloud model's price in one muted line: its price, or "price unknown" (an OpenRouter
 *  picture's price is asked once it is picked). */
export function priceLine(model: MediaModel, company: Company): string {
  return priceNotAsked(model, company) ? MODEL_SHEET.priceUnknown : priceHint(model, company);
}

/** An installed model's type in plain words: "SDXL · picture", "FLUX.1 Kontext · picture edit";
 *  its recipe's name when its version is not known; `null` before it is set up. */
export function typeLabel(m: InstalledLocal): string | null {
  if (m.recipe === null) return null;
  const type = m.version !== null ? typeName(m.version, m.recipe.id) : (RECIPE_TYPE_NAME[m.recipe.id] ?? m.recipe.name);
  return `${type} · ${KIND_WORD[m.recipe.kind] ?? KIND_WORD.picture}`;
}

/** An installed model's size: "6.9 GB" (decimal, as Hugging Face shows it). */
export function sizeWords(m: InstalledLocal): string {
  return formatBytes(m.sizeBytes);
}

/** Whether an installed model may not run well here: it may not fit, or it is a video made on
 *  the processor. A warning only: it can still be picked and run. */
export function warns(m: InstalledLocal): boolean {
  return m.fit.word === "notHere" || m.fit.videoOnProcessor;
}

/** The words of an installed model's fit dot: the warning when it may not fit ("May not fit
 *  this computer: needs 6 GB of graphics memory. It may fail or be very slow.") or makes a
 *  video on the processor; else its fit word and its time here ("Fits · about 2 min a
 *  picture"). */
export function fitWords(m: InstalledLocal, tab: StudioTab): string {
  if (m.fit.word === "notHere") return m.fit.reason ? LOCAL_WARNING.mayNotFit(m.fit.reason) : LOCAL_FIT_WORDS.notHere;
  if (m.fit.videoOnProcessor) return LOCAL_WARNING.videoOnProcessor;
  return `${LOCAL_FIT_WORDS[m.fit.word] ?? LOCAL_FIT_WORDS.fits} · ${localHint(m, tab)}`;
}

import { useId, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { Popover } from "radix-ui";
import { Check, ChevronDown, Cloud, Search } from "lucide-react";
import {
  cloudChipWords,
  LOCAL_FIT_WORDS,
  LOCAL_MARK,
  STUDIO_ACTIONS,
  STUDIO_PICKER,
  STUDIO_PRICE,
  STUDIO_TIME,
} from "../../lib/words";
import type { ErrorInfo } from "../../lib/types";
import { toErrorInfo } from "../../store/app";
import {
  FITTING,
  fittingEntry,
  isAutoModel,
  makesKind,
  priceNotAsked,
  recentChoices,
  useStudio,
  type Draft,
  type StudioTab,
} from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import { FitDot } from "../FitBadge";
import { PlaceMark } from "../PlaceMark";
import { formatUsd } from "./cost";
import { etaText } from "./eta";
import { LocalMark } from "./LocalMark";
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

// Studio's model picker (0.1.0 Task B3; design 2 and local addendum Wireframe L1): a popover
// with a search box and filter chips, then the models in groups, local first: "On this
// computer" (installed models of the tab's kind, with their fit and time here, free and
// private), then the recent cloud models (from the newest runs whatever the gallery's filter,
// each at the endpoint it ran at), then each endpoint, by family, with the Cloud mark,
// where the prompt goes and each model's price hint. A model Studio cannot use, or one of an
// endpoint without a key, is greyed out with the reason. Never an "Auto" model, which would
// hide where the prompt goes. Picking an OpenRouter picture model asks for its price once
// (`chooseModel`); a model restored when Studio opens is asked only on *Get the price*. An
// endpoint of the tab's kind whose lists are not saved yet has a *Get the list* row in place of
// its models. The prompt dock's *Choose a cloud model* opens it from the store (`pickerOpen`),
// on its first cloud group (Task B7).

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

const FILTER_WORDS: Record<PickerFilter, string> = STUDIO_PICKER.filter;

/** The filter chips each tab offers. */
const CHIPS: Record<StudioTab, readonly PickerFilter[]> = {
  image: ["edit", "references", "transparent"],
  video: ["startFrame", "endFrame", "sound"],
};

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

/** What the model takes or makes, as badges beside its name. */
function badgesFor(model: MediaModel): string[] {
  const words = STUDIO_PICKER.badge;
  const refs = referencesMax(model);
  const badges: string[] = [];
  if (refs >= 1) badges.push(words.edit);
  if (refs > 1) badges.push(words.refs(refs));
  if (hasField(model, "startFrame")) badges.push(words.startFrame);
  if (hasField(model, "endFrame")) badges.push(words.endFrame);
  if (hasField(model, "sound")) badges.push(words.sound);
  if (offers(model, "background", "transparent")) badges.push(words.transparent);
  if (offers(model, "format", "gif")) badges.push(words.gif);
  if (offers(model, "format", "svg")) badges.push(words.svg);
  return badges;
}

/** Whether the model's name or id holds the search text, in any case. */
function matches(model: MediaModel, query: string): boolean {
  const text = query.trim().toLowerCase();
  return text === "" || model.displayName.toLowerCase().includes(text) || model.id.toLowerCase().includes(text);
}

/** The installed models of `kind` this computer can make (video needs a usable graphics card
 *  or an Apple-silicon Mac), those this version cannot read included. */
function installedHere(local: LocalModels, kind: StudioTab): InstalledLocal[] {
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
      family: STUDIO_PICKER.recent,
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

// ---- the picker -----------------------------------------------------------------------------

/** The draft's model as listed, with where it runs; `null` when it is not listed (any more). */
type Chosen =
  | { model: MediaModel; installed: InstalledLocal; endpoint: null }
  | { model: MediaModel; installed: null; endpoint: StudioEndpoint };

function chosenOf(draft: Draft, local: LocalModels | null, endpoints: StudioEndpoint[]): Chosen | null {
  const { target, model } = draft;
  if (target === null || model === null) return null;
  if (target === "thisComputer") {
    const installed = local?.installed.find((m) => m.model.id === model);
    return installed ? { model: installed.model, installed, endpoint: null } : null;
  }
  const endpoint = endpoints.find((e) => e.providerId === target.endpoint);
  const listed = endpoint?.models.find((m) => m.id === model);
  return endpoint && listed ? { model: listed, installed: null, endpoint } : null;
}

function targetKey(target: RunTarget): string {
  return target === "thisComputer" ? "here" : `endpoint ${target.endpoint}`;
}

/** Where the chosen model runs and which it is, as one key. */
function chosenKey(chosen: Chosen | null): string {
  if (!chosen) return "none";
  return `${targetKey(chosen.endpoint ? { endpoint: chosen.endpoint.providerId } : "thisComputer")} ${chosen.model.id}`;
}

const CHIP =
  "rounded-full border border-border px-2 py-0.5 text-xs hover:border-border-strong aria-pressed:border-accent aria-pressed:text-accent";
const LINK_BUTTON = "rounded-lg px-2.5 py-1.5 text-left text-sm text-accent hover:bg-bg-muted";

/** The Model field: the chosen model, its price or time, and the picker it opens. */
export function ModelPicker(props: { tab: StudioTab }) {
  const { tab } = props;
  const draft = useStudio((s) => s.drafts[tab]);
  const endpoints = useStudio((s) => s.endpoints);
  const local = useStudio((s) => s.local);
  const recentRuns = useStudio((s) => s.recentRuns);
  const chooseModel = useStudio((s) => s.chooseModel);
  const showLocalModels = useStudio((s) => s.showLocalModels);
  const pickerOpen = useStudio((s) => s.pickerOpen);
  const openPicker = useStudio((s) => s.openPicker);
  // Opened from its trigger here, or from elsewhere through the store.
  const [openedHere, setOpenedHere] = useState(false);
  const open = openedHere || pickerOpen !== null;
  const [query, setQuery] = useState("");
  const [filters, setFilters] = useState<PickerFilter[]>([]);
  // Opened from elsewhere, it starts with every model shown, so its cloud groups are there.
  const [seenRequest, setSeenRequest] = useState(pickerOpen);
  if (pickerOpen !== seenRequest) {
    setSeenRequest(pickerOpen);
    if (pickerOpen !== null) {
      setQuery("");
      setFilters([]);
    }
  }
  const searchRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const labelId = useId();
  const nameId = useId();

  const chips = CHIPS[tab];
  const active = filters.filter((f) => chips.includes(f));
  const cloudRuns = recentRuns.filter((r) => r.runsOn === "cloud");
  const groups = pickerGroups(local, endpoints, tab, query, active, recentChoices(cloudRuns, tab));
  const chosen = chosenOf(draft, local, endpoints);
  const localGroup = groups.find((g) => g.place === "local");
  const recentGroup = groups.find((g) => g.place === "cloud" && g.endpoint === null);
  const searching = query.trim() !== "" || active.length > 0;
  const nothing = searching && groups.every((g) => g.models.length === 0);

  const isSelected = (target: RunTarget, model: MediaModel) =>
    draft.target !== null && targetKey(draft.target) === targetKey(target) && draft.model === model.id;
  const setOpen = (o: boolean) => {
    setOpenedHere(o);
    if (!o) openPicker(null);
  };
  const choose = (target: RunTarget, model: MediaModel) => {
    chooseModel(tab, target, model.id);
    setOpen(false);
  };
  const getLocal = () => {
    showLocalModels(true);
    setOpen(false);
  };
  const toggle = (filter: PickerFilter) =>
    setFilters((now) => (now.includes(filter) ? now.filter((f) => f !== filter) : [...now, filter]));

  // Up and Down move through the models, and from the first one back to the search box.
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const options = Array.from(e.currentTarget.querySelectorAll<HTMLElement>('[role="option"]'));
    const at = options.findIndex((o) => o === document.activeElement);
    if (e.key === "ArrowDown") (options[at + 1] ?? options[at])?.focus();
    else if (at <= 0) searchRef.current?.focus();
    else options[at - 1]?.focus();
  };

  return (
    <div data-model-picker className="flex flex-col gap-1">
      <span id={labelId} className="text-xs font-medium text-fg-muted">
        {STUDIO_PICKER.label}
      </span>
      <Popover.Root
        open={open}
        onOpenChange={(o) => {
          if (o) setQuery("");
          setOpen(o);
        }}
      >
        <Popover.Trigger asChild>
          <button
            type="button"
            aria-labelledby={`${labelId} ${nameId}`}
            className="flex w-full items-center gap-2 rounded-lg border border-border bg-bg px-2.5 py-1.5 text-left text-sm hover:border-border-strong"
          >
            <span id={nameId} className="min-w-0 flex-1 truncate">
              {chosen?.model.displayName ?? STUDIO_PICKER.choose}
            </span>
            <ChevronDown size={14} aria-hidden className="shrink-0 text-fg-muted" />
          </button>
        </Popover.Trigger>
        <Popover.Portal>
          <Popover.Content
            align="start"
            sideOffset={4}
            collisionPadding={8}
            aria-label={STUDIO_PICKER.choose}
            onOpenAutoFocus={(e) => {
              e.preventDefault();
              // Asked for its cloud models: the first cloud model Studio can use, in the first
              // cloud group that has one, else the first endpoint's *Get the list* (else the
              // search box, as always).
              const list = pickerOpen === "cloud" ? listRef.current : null;
              const cloud =
                list?.querySelector<HTMLElement>(
                  '[data-picker-place="cloud"] [role="option"]:not([aria-disabled="true"])',
                ) ?? list?.querySelector<HTMLElement>("[data-picker-get-list] button");
              (cloud ?? searchRef.current)?.focus();
            }}
            onKeyDown={onKeyDown}
            className="z-50 flex max-h-[var(--radix-popover-content-available-height)] w-[26rem] max-w-[calc(100vw-1rem)] flex-col rounded-xl border border-border bg-surface shadow-pop"
          >
            <div className="flex flex-col gap-2 border-b border-border p-2">
              <label className="flex items-center gap-1.5 rounded-lg border border-border bg-bg px-2 py-1 text-sm focus-within:border-border-strong">
                <Search size={14} aria-hidden className="text-fg-muted" />
                <input
                  ref={searchRef}
                  type="search"
                  aria-label={STUDIO_PICKER.search}
                  placeholder={STUDIO_PICKER.searchPlaceholder}
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  className="min-w-0 flex-1 bg-transparent outline-none placeholder:text-fg-muted"
                />
              </label>
              <div role="group" aria-label={STUDIO_PICKER.filters} className="flex flex-wrap gap-1">
                {chips.map((f) => (
                  <button key={f} type="button" aria-pressed={active.includes(f)} onClick={() => toggle(f)} className={CHIP}>
                    {FILTER_WORDS[f]}
                  </button>
                ))}
              </div>
            </div>
            <div ref={listRef} className="min-h-0 overflow-y-auto p-1.5">
              {local && localGroup && (
                <LocalSection
                  tab={tab}
                  local={local}
                  group={localGroup}
                  isSelected={isSelected}
                  onChoose={choose}
                  onGetLocal={getLocal}
                />
              )}
              {recentGroup && (
                <section data-picker-place="cloud" className="pt-2">
                  <h3 className="px-2 pb-1 text-xs font-semibold text-fg-muted">{STUDIO_PICKER.recent}</h3>
                  <div role="listbox" aria-label={STUDIO_PICKER.recent}>
                    {recentGroup.models.map((model, i) => {
                      const endpoint = recentGroup.via?.[i];
                      return endpoint ? (
                        <CloudRow
                          key={`${endpoint.providerId} ${model.id}`}
                          endpoint={endpoint}
                          model={model}
                          inRecent
                          selected={isSelected({ endpoint: endpoint.providerId }, model)}
                          onChoose={() => choose({ endpoint: endpoint.providerId }, model)}
                        />
                      ) : null;
                    })}
                  </div>
                </section>
              )}
              {endpoints.map((endpoint) => {
                const families = groups.filter((g) => g.endpoint === endpoint);
                if (families.length > 0) {
                  return (
                    <EndpointSection
                      key={endpoint.providerId}
                      endpoint={endpoint}
                      families={families}
                      isSelected={isSelected}
                      onChoose={choose}
                    />
                  );
                }
                return listsNotSaved(endpoint, tab) ? (
                  <GetListRow key={endpoint.providerId} endpoint={endpoint} />
                ) : null;
              })}
              {nothing && <p className="px-2 py-3 text-sm text-fg-muted">{STUDIO_PICKER.nothingMatches}</p>}
            </div>
          </Popover.Content>
        </Popover.Portal>
      </Popover.Root>
      {/* Its state (a request on its way) belongs to the model it was for. */}
      <ChosenLine key={chosenKey(chosen)} tab={tab} chosen={chosen} />
    </div>
  );
}

/** "On this computer": the installed models (rows with the fit word and the time here), then
 *  *+ Get local models* while more fit, or what to do with none yet. Left out while a search
 *  or filter hides every installed model. */
function LocalSection(props: {
  tab: StudioTab;
  local: LocalModels;
  group: PickerGroup;
  isSelected: (target: RunTarget, model: MediaModel) => boolean;
  onChoose: (target: RunTarget, model: MediaModel) => void;
  onGetLocal: () => void;
}) {
  const { tab, local, group } = props;
  const installed = installedHere(local, tab);
  const usable = installed.filter((m) => m.model.usable === null);
  if (group.models.length === 0 && usable.length > 0) return null;
  const entry = fittingEntry(tab, local);
  const fitsHere = entry !== null || usable.some((m) => FITTING.has(m.fit.word));
  // Nothing at all fits this computer (no recommended model left to get, none installed), or
  // the Video tab where it cannot make video: local needs other hardware. Otherwise only this
  // kind has none that fits.
  const nothingFits =
    (tab === "video" && local.videoOnProcessor) ||
    (local.moreFit === 0 && !local.installed.some((m) => m.model.usable === null && FITTING.has(m.fit.word)));

  let after: ReactNode = (
    <p className="px-2.5 py-1.5 text-xs text-fg-muted">
      {nothingFits ? STUDIO_PICKER.noLocalHere : STUDIO_PICKER.noKindHere[tab]}
    </p>
  );
  if (usable.length > 0 && local.moreFit > 0) {
    after = (
      <button type="button" className={LINK_BUTTON} onClick={props.onGetLocal}>
        {STUDIO_PICKER.getMoreLocal(local.moreFit)}
      </button>
    );
  } else if (usable.length > 0 && fitsHere) {
    after = null;
  } else if (usable.length === 0 && entry) {
    after = (
      <div className="flex flex-wrap items-center gap-2 px-2.5 py-1.5">
        <p className="text-sm">{STUDIO_PICKER.noLocalYet(entry.name, entry.sizeBytes)}</p>
        <button
          type="button"
          className="rounded-lg border border-border bg-bg px-2.5 py-1 text-sm hover:border-border-strong hover:bg-bg-muted"
          onClick={props.onGetLocal}
        >
          {STUDIO_ACTIONS.getLocalModels}
        </button>
      </div>
    );
  }

  return (
    <section className="pt-1">
      <div className="flex items-center justify-between gap-2 px-2 pb-1">
        <LocalMark />
        <span className="text-xs text-fg-muted">{STUDIO_PICKER.localTag}</span>
      </div>
      {group.models.length > 0 && (
        <div role="listbox" aria-label={LOCAL_MARK.name}>
          {group.models.map((model) => {
            const here = installed.find((m) => m.model === model);
            const usableHere = model.usable === null;
            return (
              <OptionRow
                key={model.id}
                model={model}
                selected={props.isSelected("thisComputer", model)}
                disabled={!usableHere}
                reason={model.usable}
                tip={model.notes}
                onChoose={() => props.onChoose("thisComputer", model)}
              >
                {here && usableHere && (
                  <>
                    <FitDot verdict={here.fit.verdict} word={LOCAL_FIT_WORDS[here.fit.word]} />
                    <span>{localHint(here, tab)}</span>
                  </>
                )}
              </OptionRow>
            );
          })}
        </div>
      )}
      {after}
    </section>
  );
}

/** An endpoint's models by family, under where the prompt goes and the Cloud mark. */
function EndpointSection(props: {
  endpoint: StudioEndpoint;
  families: PickerGroup[];
  isSelected: (target: RunTarget, model: MediaModel) => boolean;
  onChoose: (target: RunTarget, model: MediaModel) => void;
}) {
  const { endpoint } = props;
  const target: RunTarget = { endpoint: endpoint.providerId };
  const title = STUDIO_PICKER.goesTo(endpoint.host);
  return (
    <section data-picker-place="cloud" className="pt-2">
      <div className="flex items-center gap-2 px-2 pb-1">
        <h3 className="text-xs font-semibold text-fg-muted">{STUDIO_PICKER.via(endpoint.name)}</h3>
        <PlaceMark mark="cloud" icon={Cloud} label={cloudChipWords("")} title={title} className="ml-auto" />
      </div>
      {!endpoint.hasKey && <p className="px-2 pb-1 text-xs text-fg-muted">{STUDIO_PICKER.noKey}</p>}
      <div role="listbox" aria-label={STUDIO_PICKER.via(endpoint.name)}>
        {props.families.map((group) => (
          <div key={group.family} role="group" className="border-t border-border py-0.5 first:border-t-0">
            {group.models.map((model) => (
              <CloudRow
                key={model.id}
                endpoint={endpoint}
                model={model}
                inRecent={false}
                selected={props.isSelected(target, model)}
                onChoose={() => props.onChoose(target, model)}
              />
            ))}
          </div>
        ))}
      </div>
    </section>
  );
}

/** Whether `endpoint` is a cloud endpoint whose company makes `tab`'s kind and whose lists are
 *  not saved yet (the dock's cloud backup counts it, `cloudBackup`). */
function listsNotSaved(endpoint: StudioEndpoint, tab: StudioTab): boolean {
  return endpoint.cloud && endpoint.fetchedAt === null && makesKind(endpoint, tab);
}

/** In place of the models of an endpoint whose lists are not saved yet: *Get the list* fetches
 *  them (lists are fetched only when asked), and its models then take the row's place; a
 *  failure says why under it. */
function GetListRow(props: { endpoint: StudioEndpoint }) {
  const refreshModels = useStudio((s) => s.refreshModels);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const { endpoint } = props;
  const getList = () => {
    setBusy(true);
    setError(null);
    refreshModels(endpoint.providerId)
      .catch((e: unknown) => setError(toErrorInfo(e)))
      .finally(() => setBusy(false));
  };
  return (
    <section data-picker-place="cloud" data-picker-get-list className="flex flex-col gap-1 pt-2">
      <button type="button" disabled={busy} className={`${LINK_BUTTON} disabled:opacity-60`} onClick={getList}>
        {STUDIO_PICKER.getList(endpoint.name)}
      </button>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
    </section>
  );
}

/** A cloud model's row with its price hint; greyed out when Studio cannot use it or its
 *  endpoint has no key. In Recent (`inRecent`) the row names its endpoint ("via OpenRouter")
 *  and says when it has no key, which an endpoint's group says once otherwise. */
function CloudRow(props: {
  endpoint: StudioEndpoint;
  model: MediaModel;
  inRecent: boolean;
  selected: boolean;
  onChoose: () => void;
}) {
  const { endpoint, model } = props;
  const noKey = !endpoint.hasKey;
  const reason = model.usable ?? (noKey && props.inRecent ? STUDIO_PICKER.noKey : null);
  return (
    <OptionRow
      model={model}
      selected={props.selected}
      disabled={model.usable !== null || noKey}
      reason={reason}
      tip={noKey ? [STUDIO_PICKER.noKey, ...model.notes] : model.notes}
      onChoose={props.onChoose}
    >
      {props.inRecent && <span>{STUDIO_PICKER.via(endpoint.name)}</span>}
      <span>{priceHint(model, endpoint.company)}</span>
    </OptionRow>
  );
}

/** One model: its name and badges, then `children` (its price, or its fit and time), the
 *  reason under it when it is greyed out, and its notes in the tooltip. */
function OptionRow(props: {
  model: MediaModel;
  selected: boolean;
  disabled: boolean;
  reason: string | null;
  tip: string[];
  onChoose: () => void;
  children: ReactNode;
}) {
  const choose = () => {
    if (!props.disabled) props.onChoose();
  };
  return (
    <div
      role="option"
      tabIndex={-1}
      aria-selected={props.selected}
      aria-disabled={props.disabled || undefined}
      title={props.tip.length > 0 ? props.tip.join("\n") : undefined}
      onClick={choose}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          choose();
        }
      }}
      className={
        "flex cursor-default items-start gap-1.5 rounded-lg px-2 py-1.5 text-sm outline-none focus:bg-bg-muted " +
        (props.disabled ? "text-fg-muted" : "hover:bg-bg-muted")
      }
    >
      <Check size={14} aria-hidden className={`mt-0.5 shrink-0 text-accent ${props.selected ? "" : "invisible"}`} />
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5">
          <span className="min-w-0 truncate">{props.model.displayName}</span>
          {badgesFor(props.model).map((badge) => (
            <span key={badge} className="rounded border border-border px-1 text-xs text-fg-muted">
              {badge}
            </span>
          ))}
          <div className="ml-auto flex items-center gap-2 text-xs text-fg-muted">{props.children}</div>
        </div>
        {props.reason && <p className="mt-0.5 text-xs text-fg-muted">{props.reason}</p>}
      </div>
    </div>
  );
}

/** Under the Model field: the chosen model's price hint, with *Get the price* while an
 *  OpenRouter picture's price was never asked for (and why the last request failed, kept by
 *  the store for the tab's model), or a local model's time here. */
function ChosenLine(props: { tab: StudioTab; chosen: Chosen | null }) {
  const fetchPrice = useStudio((s) => s.fetchPrice);
  const error = useStudio((s) => s.priceError[props.tab]);
  const dismissPriceError = useStudio((s) => s.dismissPriceError);
  const [busy, setBusy] = useState(false);
  const { chosen } = props;
  if (!chosen) return null;
  if (chosen.installed) return <p className="text-xs text-fg-muted">{localHint(chosen.installed, props.tab)}</p>;
  const { model, endpoint } = chosen;

  const getPrice = () => {
    setBusy(true);
    // A failure goes to the store's `priceError`.
    void fetchPrice(props.tab).finally(() => setBusy(false));
  };

  return (
    <>
      <p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-fg-muted">
        <span>{priceHint(model, endpoint.company)}</span>
        {priceNotAsked(model, endpoint.company) && (
          <button
            type="button"
            disabled={busy}
            onClick={getPrice}
            className="rounded-md border border-border bg-bg px-1.5 py-0.5 text-fg hover:border-border-strong hover:bg-bg-muted disabled:opacity-60"
          >
            {STUDIO_PICKER.getPrice}
          </button>
        )}
      </p>
      {error && <ErrorBanner error={error} onDismiss={() => dismissPriceError(props.tab)} />}
    </>
  );
}

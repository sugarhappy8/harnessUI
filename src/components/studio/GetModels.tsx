import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { ArrowLeft, FilePlus2, Gpu, HardDrive, Layers, MemoryStick, Monitor, Sparkles, TrendingUp, type LucideIcon } from "lucide-react";
import type { ErrorInfo } from "../../lib/types";
import { formatBytes } from "../../lib/format";
import { capacityGb, GET_MODELS, LOCAL_MODELS } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { useStudio } from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import { FOCUS_RING } from "../ui/styles";
import { EngineLine } from "./EngineLine";
import { CatalogCard, InstalledCard, LoraCard, ResultCard, useTokenSaved } from "./ModelCard";
import { ModelDetail } from "./ModelDetail";
import { ModelSearch } from "./ModelSearch";
import { LocalNotice } from "./LocalNotice";
import { MODEL_GRID, SECONDARY_BUTTON } from "./styles";
import type {
  HardwareLine,
  LocalEntry,
  LocalModels,
  LocalRepo,
  LocalSearchHit,
  MediaKind,
  SearchFilter,
} from "./types";

// Get models (0.1.0 Task B13; since 0.2.0 Task U5, design 10.3): Studio's full view for finding
// and downloading models, in place of the results, the composer and the side panel, opened at
// its top. The header says what this computer has, as quiet chips (so the fit and time can be
// checked), with the Studio engine's line, *Add a model file*, *Add a LoRA file* and Back. Then
// one large Hugging Face search (ModelSearch.tsx) with Pictures · Video · LoRAs (starting from
// the tab) and *Fits this computer* (off by default, ruling P35). Without a query it shows
// *Recommended for this computer* (Studio's list, the models that may not fit included while
// the switch is off: a warning, never a refusal, design 9.2), *Popular on Hugging Face* (the
// most downloaded of the filter's kind, asked once each time the view opens) and *On this
// computer* (installed models and LoRAs, with Repair and Delete); with one, what Hugging Face
// found. All are cards in one grid of two or three columns (ModelCard.tsx). A result, or a
// catalog model's *Download*, opens the detail panel (ModelDetail.tsx, Task U6): the review
// before a download, with one Download (the content notice comes first while it was never
// accepted); the footer's *What that means* shows the notice again, read only. A card and the
// panel show how far a model got, with Pause and Cancel, and *Use it* once it is done (Task U7):
// Studio follows the app's downloads whatever it shows (`followStudioDownloads`), so a model
// done after Back is listed too.

/** How many of the Popular list show before *Show all*: two rows of three. */
const POPULAR_SHOWN = 6;

/** A filter's Popular list while the view is open: what Hugging Face answered, or why it could
 *  not be read. No entry: not asked yet, or its request is on its way ("Searching…"); one is
 *  asked as soon as the list is shown without an entry. */
type Popular = { hits: LocalSearchHit[] } | { error: ErrorInfo };

/** `popular` without `filter`'s entry: its list is asked for again once it is shown. */
function forget(popular: Partial<Record<SearchFilter, Popular>>, filter: SearchFilter) {
  const next = { ...popular };
  delete next[filter];
  return next;
}

/** The kind of model each filter is for; LoRAs are for models of either kind. */
const KIND_OF: Record<SearchFilter, MediaKind | null> = { pictures: "image", video: "video", loras: null };

/** This computer as the header's chips: the graphics card and its memory (on shared memory,
 *  the computer's memory with it, never the graphics working set; without one, "Processor
 *  only"), the computer's memory, and the models folder's free disk. A figure not known is left
 *  out. Memory is a capacity, counted as the computer counts it and as the fit's reason says
 *  it ("8 GB graphics memory", "needs 6 GB of graphics memory"): `capacityGb`. The free disk is
 *  what a download fills, so it is in the cards' size format (Task U7): `formatBytes`. */
export function hardwareChips(h: HardwareLine): { icon: LucideIcon; text: string }[] {
  const chips: { icon: LucideIcon; text: string }[] = [];
  if (h.sharedMemory) {
    chips.push({ icon: Gpu, text: h.ramBytes === null ? h.device : GET_MODELS.shared(h.device, capacityGb(h.ramBytes)) });
  } else {
    chips.push(
      h.graphicsBytes === null
        ? { icon: Monitor, text: GET_MODELS.processorOnly }
        : { icon: Gpu, text: GET_MODELS.gpu(h.device, capacityGb(h.graphicsBytes)) },
    );
    if (h.ramBytes !== null) chips.push({ icon: MemoryStick, text: GET_MODELS.ram(capacityGb(h.ramBytes)) });
  }
  if (h.freeDiskBytes !== null) chips.push({ icon: HardDrive, text: GET_MODELS.freeDisk(formatBytes(h.freeDiskBytes)) });
  return chips;
}

/** The catalog models shown for `kind`: Studio's list, then (while *Fits this computer* is off)
 *  those that may not fit this computer, which keep Download. Its picks come first. */
export function catalogShown(local: LocalModels, kind: MediaKind, fitsOnly: boolean): LocalEntry[] {
  const fitting = local.recommended.filter((e) => e.kind === kind && !(fitsOnly && e.fit.word === "notHere"));
  const picks = [...fitting.filter((e) => e.recommended), ...fitting.filter((e) => !e.recommended)];
  return fitsOnly ? picks : [...picks, ...local.notHere.filter((e) => e.kind === kind)];
}

/** Whether a result shows: with *Fits this computer* on, a result opened this session with no
 *  file for this computer (no `best`) is hidden; one not opened yet stays. */
export function resultShown(hit: LocalSearchHit, repos: Record<string, LocalRepo>, fitsOnly: boolean): boolean {
  const opened = repos[hit.repo];
  return !fitsOnly || opened === undefined || opened.files.some((f) => f.best);
}

/** Get models, Studio's full view, with Back to the results. */
export function GetModelsView() {
  const local = useStudio((s) => s.local);
  const filter = useStudio((s) => s.searchFilter);
  const setSearchFilter = useStudio((s) => s.setSearchFilter);
  const fitsOnly = useStudio((s) => s.fitsOnly);
  const setFitsOnly = useStudio((s) => s.setFitsOnly);
  const found = useStudio((s) => s.localSearch);
  const repos = useStudio((s) => s.repos);
  const search = useStudio((s) => s.search);
  const openDetail = useStudio((s) => s.openDetail);
  const noticeOpen = useStudio((s) => s.noticeOpen);
  const setNoticeOpen = useStudio((s) => s.setNoticeOpen);
  const titleId = useId();
  // The words asked for ("" without a query) and what asking them gave.
  const [query, setQuery] = useState("");
  const [queryError, setQueryError] = useState<ErrorInfo | null>(null);
  // The Popular list of each filter, as asked while the view is open, and the filters whose
  // request is on its way (so one is never asked twice at once).
  const [popular, setPopular] = useState<Partial<Record<SearchFilter, Popular>>>({});
  const [allPopular, setAllPopular] = useState(false);
  const asking = useRef(new Set<SearchFilter>());
  const [error, setError] = useState<ErrorInfo | null>(null);
  const fail = (e: unknown) => setError(toErrorInfo(e));
  const kind = KIND_OF[filter];
  const gated = local !== null && [...local.recommended, ...local.notHere].some((e) => e.gate.mode !== "none" && !e.installed);
  const tokenSaved = useTokenSaved(gated);

  // The Popular list of the filter, once per filter while the view is open (each opening mounts
  // it). Each filter keeps its own list or failure. An answer the store dropped (a search was
  // asked after it), a failure dismissed and *Try again* forget the entry, which asks again as
  // soon as the list is shown: it always ends in its list, its failure with *Try again*, or a
  // request on its way.
  useEffect(() => {
    if (query !== "" || popular[filter] !== undefined || asking.current.has(filter)) return;
    const f = filter;
    asking.current.add(f);
    search("", f).then(
      () => {
        asking.current.delete(f);
        const now = useStudio.getState().localSearch;
        if (now !== null && now.text === "" && now.filter === f) setPopular((p) => ({ ...p, [f]: { hits: now.hits } }));
        else setPopular((p) => forget(p, f));
      },
      (e: unknown) => {
        asking.current.delete(f);
        setPopular((p) => ({ ...p, [f]: { error: toErrorInfo(e) } }));
      },
    );
  }, [filter, query, popular, search]);

  function ask(text: string, f: SearchFilter) {
    setQueryError(null);
    search(text, f).catch((e: unknown) => setQueryError(toErrorInfo(e)));
  }

  function onSearch(text: string) {
    setQuery(text);
    if (text === "") setQueryError(null);
    else ask(text, filter);
  }

  function onFilter(f: SearchFilter) {
    setSearchFilter(f);
    setAllPopular(false);
    if (query !== "") ask(query, f);
  }

  /** *Try again*, or the failure dismissed: the list is asked for again. */
  function retryPopular() {
    setPopular((p) => forget(p, filter));
  }

  /** Opens result `repo` in the detail panel: what was read of it this session shows at once,
   *  and it is read again at its latest commit (a failure is said in the panel). */
  function open(repo: string) {
    void openDetail({ repo });
  }

  const results = query !== "" && found !== null && found.text === query && found.filter === filter ? found.hits : null;
  const shownPopular = popular[filter];
  const popularHits = shownPopular && "hits" in shownPopular ? shownPopular.hits.filter((h) => resultShown(h, repos, fitsOnly)) : null;

  return (
    <section
      data-local-models
      data-get-models
      aria-labelledby={titleId}
      className="mx-auto flex w-full max-w-5xl flex-col gap-6 px-6 pt-5 pb-10 max-sm:px-4"
    >
      <GetModelsHeader titleId={titleId} local={local} />
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <ModelSearch filter={filter} onFilter={onFilter} fitsOnly={fitsOnly} onFitsOnly={setFitsOnly} onSearch={onSearch} />
      {query !== "" ? (
        <Section title={GET_MODELS.results} icon={TrendingUp}>
          {queryError && <ErrorBanner error={queryError} onDismiss={() => setQueryError(null)} />}
          {!queryError && results === null && <Quiet status>{GET_MODELS.searching}</Quiet>}
          {results !== null && <Results hits={results.filter((h) => resultShown(h, repos, fitsOnly))} empty={GET_MODELS.noResults[filter]} onOpen={open} />}
        </Section>
      ) : (
        <>
          {local && kind && (
            <Section title={GET_MODELS.recommended} icon={Sparkles} note={kind === "video" ? GET_MODELS.videoNote : undefined}>
              <Catalog entries={catalogShown(local, kind, fitsOnly)} kind={kind} tokenSaved={tokenSaved} />
            </Section>
          )}
          <Section
            title={GET_MODELS.popular}
            icon={TrendingUp}
            action={
              popularHits !== null &&
              popularHits.length > POPULAR_SHOWN && (
                <button
                  type="button"
                  aria-expanded={allPopular}
                  className={`text-sm text-accent hover:underline ${FOCUS_RING}`}
                  onClick={() => setAllPopular((all) => !all)}
                >
                  {allPopular ? GET_MODELS.showFewer : GET_MODELS.showAll(popularHits.length)}
                </button>
              )
            }
          >
            {shownPopular && "error" in shownPopular ? (
              <div className="flex flex-col items-start gap-2">
                <ErrorBanner error={shownPopular.error} onDismiss={retryPopular} />
                <button type="button" className={`${SECONDARY_BUTTON} ${FOCUS_RING}`} onClick={retryPopular}>
                  {GET_MODELS.retry}
                </button>
              </div>
            ) : popularHits === null ? (
              <Quiet status>{GET_MODELS.searching}</Quiet>
            ) : (
              <Results
                hits={allPopular ? popularHits : popularHits.slice(0, POPULAR_SHOWN)}
                empty={GET_MODELS.noResults[filter]}
                onOpen={open}
              />
            )}
          </Section>
          {local && <OnThisComputer local={local} kind={kind} onError={fail} />}
        </>
      )}
      {local && (
        <footer className="flex flex-col gap-2 border-t border-border pt-4">
          <p className="text-sm text-fg-muted">
            {LOCAL_MODELS.notice}{" "}
            <button type="button" className={`underline hover:text-fg ${FOCUS_RING}`} onClick={() => setNoticeOpen("read")}>
              {LOCAL_MODELS.whatThatMeans}
            </button>
          </p>
        </footer>
      )}
      {/* The note read again from the footer; before a first download the panel shows it. */}
      <LocalNotice open={noticeOpen === "read"} readOnly onAccepted={() => undefined} onClose={() => setNoticeOpen(null)} />
      <ModelDetail />
    </section>
  );
}

/** The header: Back, "Get models", what this computer has, the engine's line, *Add a model
 *  file* and *Add a LoRA file*. Back puts focus on the top bar's *Get models*. */
export function GetModelsHeader(props: { titleId: string; local: LocalModels | null }) {
  const { local } = props;
  const showLocalModels = useStudio((s) => s.showLocalModels);
  const openImport = useStudio((s) => s.openImport);
  const back = () => {
    document.querySelector<HTMLElement>("[data-get-models-button]")?.focus();
    showLocalModels(false);
  };
  return (
    // Not a <header>: the top bar is Studio's one banner.
    <div data-get-models-header className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <button
          type="button"
          onClick={back}
          className={`-ml-2 inline-flex h-8 items-center gap-1.5 rounded-lg px-2 text-sm text-fg-muted hover:bg-bg-muted hover:text-fg ${FOCUS_RING}`}
        >
          <ArrowLeft size={16} aria-hidden />
          {GET_MODELS.back}
        </button>
        <div className="ml-auto flex flex-wrap items-center gap-2">
          <button type="button" onClick={() => openImport("model")} className={`${SECONDARY_BUTTON} inline-flex items-center gap-1.5 ${FOCUS_RING}`}>
            <FilePlus2 size={15} aria-hidden className="text-fg-muted" />
            {GET_MODELS.addModel}
          </button>
          <button type="button" onClick={() => openImport("lora")} className={`${SECONDARY_BUTTON} inline-flex items-center gap-1.5 ${FOCUS_RING}`}>
            <Layers size={15} aria-hidden className="text-fg-muted" />
            {GET_MODELS.addLora}
          </button>
        </div>
      </div>
      <div className="flex flex-col gap-1">
        <h2 id={props.titleId} className="text-view-title">
          {GET_MODELS.title}
        </h2>
        <p className="text-sm text-fg-muted">{GET_MODELS.subtitle}</p>
      </div>
      {local && (
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
          <HardwareChips hardware={local.hardware} />
          <EngineLine engine={local.engine} hasModel={local.installed.length > 0} removable compact />
        </div>
      )}
    </div>
  );
}

/** This computer as quiet chips: graphics card and memory, the computer's memory, free disk. */
export function HardwareChips(props: { hardware: HardwareLine }) {
  return (
    <ul aria-label={GET_MODELS.thisComputer} className="flex flex-wrap gap-1.5">
      {hardwareChips(props.hardware).map(({ icon: Icon, text }) => (
        <li
          key={text}
          className="inline-flex items-center gap-1.5 rounded-full border border-border bg-surface px-2.5 py-0.5 text-sm text-fg-muted"
        >
          <Icon size={14} aria-hidden className="shrink-0" />
          {text}
        </li>
      ))}
    </ul>
  );
}

/** One of the view's sections: an icon heading, an optional quiet note and action, then its
 *  cards. */
function Section(props: { title: string; icon: LucideIcon; note?: string; action?: ReactNode; children: ReactNode }) {
  const id = useId();
  const Icon = props.icon;
  return (
    <section aria-labelledby={id} className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <h3 id={id} className="flex items-center gap-2 text-section">
          <span className="grid size-6 place-items-center rounded-lg bg-accent/10 text-accent" aria-hidden>
            <Icon size={14} />
          </span>
          {props.title}
        </h3>
        {props.note && <p className="text-meta">{props.note}</p>}
        {props.action && <div className="ml-auto">{props.action}</div>}
      </div>
      {props.children}
    </section>
  );
}

/** A quiet line in a section: what is on its way, or that there is nothing. */
function Quiet(props: { children: ReactNode; status?: boolean }) {
  return (
    <p className="text-sm text-fg-muted" role={props.status ? "status" : undefined}>
      {props.children}
    </p>
  );
}

/** Catalog models as cards, or that none of the kind is left. */
function Catalog(props: { entries: LocalEntry[]; kind: MediaKind; tokenSaved: boolean | null }) {
  if (props.entries.length === 0) return <Quiet>{GET_MODELS.noneFit[props.kind]}</Quiet>;
  return (
    <div className={MODEL_GRID}>
      {props.entries.map((entry) => (
        <CatalogCard key={entry.id} entry={entry} tokenSaved={props.tokenSaved} />
      ))}
    </div>
  );
}

/** Hugging Face results as cards, or `empty`. */
function Results(props: { hits: LocalSearchHit[]; empty: string; onOpen(repo: string): void }) {
  if (props.hits.length === 0) return <Quiet>{props.empty}</Quiet>;
  return (
    <div className={MODEL_GRID}>
      {props.hits.map((hit) => (
        <ResultCard key={hit.repo} hit={hit} onOpen={() => props.onOpen(hit.repo)} />
      ))}
    </div>
  );
}

/** The models of the filter's kind on this computer (none under LoRAs), then the LoRAs. */
function OnThisComputer(props: { local: LocalModels; kind: MediaKind | null; onError(e: unknown): void }) {
  const { local, kind } = props;
  const models = kind === null ? [] : local.installed.filter((m) => m.kind === kind);
  const empty = models.length === 0 && local.loras.length === 0;
  return (
    <Section title={GET_MODELS.here} icon={HardDrive}>
      {empty ? (
        <Quiet>{kind === null ? GET_MODELS.noLorasHere : GET_MODELS.nothingHere}</Quiet>
      ) : (
        <div className={MODEL_GRID}>
          {models.map((row) => (
            <InstalledCard key={row.id} row={row} onError={props.onError} />
          ))}
          {local.loras.map((lora) => (
            <LoraCard key={lora.id} lora={lora} onError={props.onError} />
          ))}
        </div>
      )}
    </Section>
  );
}

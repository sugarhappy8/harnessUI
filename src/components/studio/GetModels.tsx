import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { ArrowLeft, FilePlus2, Gpu, HardDrive, Layers, MemoryStick, Monitor, Sparkles, TrendingUp, type LucideIcon } from "lucide-react";
import * as ipc from "../../lib/ipc";
import type { ErrorInfo } from "../../lib/types";
import { amountGb, GET_MODELS, LOCAL_MODELS } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { followDownloads } from "../../store/downloads";
import { useStudio } from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import { visiblePath } from "../FolderChip";
import { SlideOver } from "../ui/SlideOver";
import { FOCUS_RING } from "../ui/styles";
import { DownloadLocalModel } from "./DownloadLocalModel";
import { EngineLine } from "./EngineLine";
import { CatalogCard, InstalledCard, LoraCard, ResultCard } from "./ModelCard";
import { ModelSearch } from "./ModelSearch";
import type { DownloadOffer } from "./localOffers";
import { LocalNotice } from "./LocalNotice";
import { RepoResult } from "./LocalSearch";
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
// found. All are cards in one grid of two or three columns (ModelCard.tsx). A result opens in a
// slide-over (LocalSearch.tsx's RepoResult, until Task U6's detail panel). *Download* sets the
// store's `offer`, which shows the download confirmation (DownloadLocalModel.tsx), or the
// content notice first while it was never accepted (LocalNotice.tsx); the footer's *What that
// means* shows the notice again, read only. While it is open it follows the app's downloads, so
// a card shows how far its model got, and once one is done the local models are read again.

/** How many of the Popular list show before *Show all*: two rows of three. */
const POPULAR_SHOWN = 6;

/** The kind of model each filter is for; LoRAs are for models of either kind. */
const KIND_OF: Record<SearchFilter, MediaKind | null> = { pictures: "image", video: "video", loras: null };

/** This computer as the header's chips: the graphics card and its memory (on shared memory,
 *  the computer's memory with it, never the graphics working set; without one, "Processor
 *  only"), the computer's memory, and the models folder's free disk. A figure not known is left
 *  out. */
export function hardwareChips(h: HardwareLine): { icon: LucideIcon; text: string }[] {
  const chips: { icon: LucideIcon; text: string }[] = [];
  if (h.sharedMemory) {
    chips.push({ icon: Gpu, text: h.ramBytes === null ? h.device : GET_MODELS.shared(h.device, amountGb(h.ramBytes)) });
  } else {
    chips.push(
      h.graphicsBytes === null
        ? { icon: Monitor, text: GET_MODELS.processorOnly }
        : { icon: Gpu, text: GET_MODELS.gpu(h.device, amountGb(h.graphicsBytes)) },
    );
    if (h.ramBytes !== null) chips.push({ icon: MemoryStick, text: GET_MODELS.ram(amountGb(h.ramBytes)) });
  }
  if (h.freeDiskBytes !== null) chips.push({ icon: HardDrive, text: GET_MODELS.freeDisk(amountGb(h.freeDiskBytes)) });
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

/** Whether a Hugging Face token is saved, read while a gated catalog model is listed (again
 *  when Settings closes, where it is saved); `null` while not known or not needed. */
function useTokenSaved(needed: boolean): boolean | null {
  const [saved, setSaved] = useState<boolean | null>(null);
  useEffect(() => {
    if (!needed || !ipc.isTauri()) return;
    let alive = true;
    const read = () =>
      ipc.hubTokenStatus().then(
        (t) => alive && setSaved(t.saved),
        (e: unknown) => console.warn("Studio: whether a Hugging Face token is saved could not be read", e),
      );
    void read();
    window.addEventListener("app:settings-closed", read);
    return () => {
      alive = false;
      window.removeEventListener("app:settings-closed", read);
    };
  }, [needed]);
  return saved;
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
  const openRepo = useStudio((s) => s.openRepo);
  const noticeOpen = useStudio((s) => s.noticeOpen);
  const setNoticeOpen = useStudio((s) => s.setNoticeOpen);
  const offer = useStudio((s) => s.offer);
  const setOffer = useStudio((s) => s.setOffer);
  const titleId = useId();
  // The words asked for ("" without a query) and what asking them gave.
  const [query, setQuery] = useState("");
  const [queryError, setQueryError] = useState<ErrorInfo | null>(null);
  // The Popular list of each filter, as asked while the view is open.
  const [popular, setPopular] = useState<Partial<Record<SearchFilter, LocalSearchHit[]>>>({});
  const [popularError, setPopularError] = useState<{ filter: SearchFilter; info: ErrorInfo } | null>(null);
  const [allPopular, setAllPopular] = useState(false);
  const asked = useRef(new Set<SearchFilter>());
  const [opened, setOpened] = useState<{ repo: string; error: ErrorInfo | null } | null>(null);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const fail = (e: unknown) => setError(toErrorInfo(e));
  const kind = KIND_OF[filter];
  const gated = local !== null && [...local.recommended, ...local.notHere].some((e) => e.gate.mode !== "none" && !e.installed);
  const tokenSaved = useTokenSaved(gated);

  useEffect(
    () =>
      followDownloads((group) => {
        // A picture or video model that finished downloading is installed now.
        if (group.genModelId && group.status === "done") {
          useStudio
            .getState()
            .refreshLocal()
            .catch((e: unknown) => console.warn("Studio: the local models could not be read again", e));
        }
      }),
    [],
  );

  // The Popular list of the filter, once while the view is open (each opening mounts it): asked
  // again only when an answer was dropped for a search asked after it, or it failed (Try again).
  useEffect(() => {
    if (query !== "" || popular[filter] !== undefined || asked.current.has(filter)) return;
    asked.current.add(filter);
    const f = filter;
    search("", f).then(
      () => {
        const now = useStudio.getState().localSearch;
        if (now !== null && now.text === "" && now.filter === f) setPopular((p) => ({ ...p, [f]: now.hits }));
        else asked.current.delete(f);
      },
      (e: unknown) => setPopularError({ filter: f, info: toErrorInfo(e) }),
    );
  }, [filter, query, popular, popularError, search]);

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

  function retryPopular() {
    asked.current.delete(filter);
    setPopularError(null);
  }

  /** Opens result `repo` in the slide-over: what was read of it this session shows at once,
   *  and it is read again at its latest commit. */
  function open(repo: string) {
    setOpened({ repo, error: null });
    openRepo(repo).catch((e: unknown) =>
      setOpened((now) => (now?.repo === repo ? { repo, error: toErrorInfo(e) } : now)),
    );
  }

  const results = query !== "" && found !== null && found.text === query && found.filter === filter ? found.hits : null;
  const popularHits = popular[filter]?.filter((h) => resultShown(h, repos, fitsOnly)) ?? null;
  const openedRepo = opened ? (repos[opened.repo] ?? null) : null;

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
              <Catalog entries={catalogShown(local, kind, fitsOnly)} local={local} kind={kind} tokenSaved={tokenSaved} />
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
            {popularError?.filter === filter ? (
              <div className="flex flex-col items-start gap-2">
                <ErrorBanner error={popularError.info} onDismiss={() => setPopularError(null)} />
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
      {local && (
        <Offered
          offer={offer}
          noticeAccepted={local.noticeAccepted}
          readingNotice={noticeOpen === "read"}
          onCloseOffer={() => setOffer(null)}
          onCloseNotice={() => setNoticeOpen(null)}
        />
      )}
      <SlideOver
        open={opened !== null}
        onOpenChange={(isOpen) => !isOpen && setOpened(null)}
        title={opened ? visiblePath(opened.repo) : ""}
        width="half"
      >
        {opened?.error ? (
          <ErrorBanner error={opened.error} onDismiss={() => setOpened(null)} />
        ) : openedRepo && local ? (
          <RepoResult repo={openedRepo} local={local} kind={kind ?? kindOfRepo(openedRepo)} />
        ) : (
          <Quiet status>{LOCAL_MODELS.opening}</Quiet>
        )}
      </SlideOver>
    </section>
  );
}

/** What a repository opened from the LoRAs filter makes, for its files' downloads. */
function kindOfRepo(repo: LocalRepo): MediaKind {
  return repo.verdict !== "unknown" && "works" in repo.verdict && repo.verdict.works.kind === "video" ? "video" : "image";
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
function Catalog(props: { entries: LocalEntry[]; local: LocalModels; kind: MediaKind; tokenSaved: boolean | null }) {
  if (props.entries.length === 0) return <Quiet>{GET_MODELS.noneFit[props.kind]}</Quiet>;
  return (
    <div className={MODEL_GRID}>
      {props.entries.map((entry) => (
        <CatalogCard key={entry.id} entry={entry} local={props.local} tokenSaved={props.tokenSaved} />
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

/** The download confirmation for `offer`, and the content notice: in its place, to accept,
 *  while the notice was never accepted (accepting it shows the confirmation), and read only
 *  from the footer. */
function Offered(props: {
  offer: DownloadOffer | null;
  noticeAccepted: boolean;
  readingNotice: boolean;
  onCloseOffer: () => void;
  onCloseNotice: () => void;
}) {
  const { offer, onCloseOffer, onCloseNotice } = props;
  const accepting = offer !== null && !props.noticeAccepted;
  return (
    <>
      <LocalNotice
        open={accepting || props.readingNotice}
        readOnly={!accepting}
        // The store records it, and the confirmation follows from the offer.
        onAccepted={() => undefined}
        onClose={accepting ? onCloseOffer : onCloseNotice}
      />
      <DownloadLocalModel offer={accepting ? null : offer} onClose={onCloseOffer} />
    </>
  );
}

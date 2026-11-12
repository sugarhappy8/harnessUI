import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { Check, ChevronDown, Plus, RefreshCw, X } from "lucide-react";
import * as ipc from "../../lib/ipc";
import type {
  CatalogListEntry,
  CompareFacts,
  CompareProblem,
  ErrorInfo,
  RepoView,
} from "../../lib/types";
import { formatBytes } from "../../lib/format";
import {
  capabilityAnswer,
  fileWords,
  goodAtSentence,
  joinWords,
  languageWords,
  pipelineWords,
  quantFromFile,
  sizeAnswer,
  tagWords,
} from "../../lib/abilities";
import { backendWords, tokensWithWords } from "../../lib/words";
import {
  COMPARE_FULL,
  COMPARE_LIMIT,
  chooseFile,
  entryKey,
  isAutoFile,
  sameEntry,
  toCompareItem,
  useCompare,
  type CompareEntry,
} from "../../store/compare";
import { toErrorInfo } from "../../store/app";
import { FitBadge } from "../FitBadge";
import { SafeLink } from "../Markdown";

// Models → Compare (0.0.5 item 4): up to four models side by side, each row a plain
// question, each answer with where it comes from (D6): the publisher's model card (signed
// catalog), what the Hugging Face uploader lists, the model file, an estimate for this
// computer, or the user's own results. Nothing here is written by a model, and uploader
// text is shown as text only, its invisible characters made visible.

/** Where an answer comes from, in the small text under it. */
export const SOURCE = {
  card: "From the publisher's model card",
  catalog: "From the app's model list",
  uploader: "The uploader lists",
  hub: "From Hugging Face",
  file: "From the model file",
  disk: "On this computer",
  results: "Your results on this computer",
} as const;

/** The fit row's source: the size it was checked at. */
export function fitSource(ctx: number): string {
  return `Estimate for this computer at ${tokensWithWords(ctx)}`;
}

/** Why a column's facts are missing, in words (0.0.5 item 4). */
export const PROBLEM_WORDS: Record<CompareProblem | "noFiles", string> = {
  offline: "These facts need the internet. They are saved once the model is downloaded.",
  rateLimited: "Hugging Face is busy. Try again in a minute.",
  gated: "This model needs a Hugging Face token. Add one in Settings → Hugging Face.",
  notFound: "This model could not be found. It may have been removed.",
  failed: "Some facts could not be read. Try again later.",
  noFiles: "This repository has no model files this app can use.",
};

/** Shown when nothing is chosen (0.0.5 item 4, word for word). */
export const EMPTY_TEXT = "Nothing to compare yet. Choose Add to comparison on a model in Browse or Installed.";

/** D6: when nothing says what the model is good at. */
export const NO_DESCRIPTION = "No description from the publisher in this app.";

/** The sizes "Compare at" offers besides the defaults' size. */
export const COMPARE_SIZES = [4096, 8192, 16384, 32768, 65536, 131072] as const;

/** How a failed request reads as a column's problem. */
export function problemFromError(err: ErrorInfo): CompareProblem {
  switch (err.code) {
    case "network":
      return "offline";
    case "hf_rate_limited":
      return "rateLimited";
    case "hf_needs_token":
      return "gated";
    case "hf_not_found":
    case "hf_file_not_found":
    case "not_found":
      return "notFound";
    default:
      return "failed";
  }
}

// Bidirectional overrides, zero-width and other invisible characters, and control
// characters (tabs and new lines kept), shown as their code instead. The same set as the
// approval cards' sanitiser (crates/policy/src/sanitize.rs); the backend cleans uploader
// text too, and this keeps saved or sample text honest.
const INVISIBLE =
  // eslint-disable-next-line no-control-regex, no-misleading-character-class
  /[\u{0}-\u{8}\u{B}-\u{1F}\u{7F}-\u{9F}\u{AD}\u{61C}\u{115F}\u{1160}\u{17B4}\u{17B5}\u{180B}-\u{180F}\u{200B}-\u{200F}\u{202A}-\u{202E}\u{2060}-\u{206F}\u{3164}\u{FE00}-\u{FE0F}\u{FEFF}\u{FFA0}\u{FFF9}-\u{FFFB}\u{1D173}-\u{1D17A}\u{E0000}-\u{E007F}\u{E0100}-\u{E01EF}]/gu;

/**
 * Untrusted text as it is shown: invisible characters made visible, at most `cap`
 * characters (counted whole, so a character outside the basic plane is never cut in half).
 */
export function shown(text: string, cap = 300): string {
  const visible = text.replace(
    INVISIBLE,
    (ch) => `[U+${(ch.codePointAt(0) ?? 0).toString(16).toUpperCase().padStart(4, "0")}]`,
  );
  const chars = Array.from(visible);
  return chars.length > cap ? `${chars.slice(0, cap - 1).join("")}…` : visible;
}

/** Text from outside the app: made visible, capped and isolated from the text around it. */
function Outside(props: { text: string; cap?: number }) {
  return <bdi>{shown(props.text, props.cap)}</bdi>;
}

const MONTHS = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];

/** "2026-03" → "March 2026", "2026-03-12…" → "March 12, 2026"; null if it is not a date. */
export function dateWords(text: string): string | null {
  const m = /^(\d{4})-(\d{2})(?:-(\d{2}))?/.exec(text);
  if (!m) return null;
  const month = MONTHS[Number(m[2]) - 1];
  if (!month) return null;
  return m[3] ? `${month} ${Number(m[3])}, ${m[1]}` : `${month} ${m[1]}`;
}

/** Milliseconds since 1970 as a date in words; null when it is not a time. */
function msDateWords(ms: number): string | null {
  const d = new Date(ms);
  return Number.isNaN(d.getTime()) ? null : dateWords(d.toISOString());
}

/** The repository of a model card URL on huggingface.co ("owner/name"), else null. */
function cardRepo(url: string | null): string | null {
  if (!url) return null;
  try {
    const u = new URL(url);
    if (u.protocol !== "https:" || u.hostname !== "huggingface.co") return null;
    const m = /^\/([\w.-]+\/[\w.-]+)\/?$/.exec(u.pathname);
    return m?.[1] ?? null;
  } catch {
    return null;
  }
}

function count(n: number): string {
  return n.toLocaleString("en-US");
}

/** One answer and where it comes from; null reads "Not known". */
interface Answer {
  node: ReactNode;
  source: string | null;
}

function quote(text: string) {
  return <span className="block">“<Outside text={text} cap={600} />”</span>;
}

function goodAt(f: CompareFacts): Answer {
  const c = f.catalog;
  if (c && (c.summary || goodAtSentence(c.goodAt))) {
    const sentence = goodAtSentence(c.goodAt);
    return {
      node: (
        <>
          {c.summary && quote(c.summary)}
          {sentence && <span className="mt-1 block">{sentence}</span>}
        </>
      ),
      source: SOURCE.card,
    };
  }
  const b = f.baseCatalog;
  if (b && (b.catalog.summary || goodAtSentence(b.catalog.goodAt))) {
    const sentence = goodAtSentence(b.catalog.goodAt);
    return {
      node: (
        <>
          <span className="block">
            The uploader says this is made from <Outside text={b.baseModel} />. That model's publisher says:
          </span>
          {b.catalog.summary && quote(b.catalog.summary)}
          {sentence && <span className="mt-1 block">{sentence}</span>}
        </>
      ),
      source: SOURCE.card,
    };
  }
  const h = f.hub;
  if (h) {
    const phrases = [pipelineWords(h.pipelineTag), ...tagWords(h.tags)].filter(
      (p, i, all): p is string => !!p && all.indexOf(p) === i,
    );
    if (phrases.length) return { node: `The uploader lists: ${phrases.join(", ")}.`, source: SOURCE.uploader };
  }
  const url = h?.cardUrl ?? c?.source ?? null;
  return {
    node: (
      <>
        {NO_DESCRIPTION}
        {url && (
          <>
            {" "}
            <SafeLink href={url}>Read the model card.</SafeLink>
          </>
        )}
      </>
    ),
    source: null,
  };
}

function whoMadeIt(f: CompareFacts): Answer | null {
  if (f.catalog?.publisher) return { node: <Outside text={f.catalog.publisher} />, source: SOURCE.card };
  if (f.hub?.author)
    return {
      node: (
        <>
          <Outside text={f.hub.author} /> uploaded it to Hugging Face.
        </>
      ),
      source: SOURCE.hub,
    };
  return null;
}

function howBig(f: CompareFacts): Answer | null {
  const ff = f.fileFacts;
  const fromCard = f.catalog?.parameters ?? null;
  const parameters = fromCard ?? ff?.parameters ?? null;
  const active = f.catalog?.activeParameters ?? ff?.activeParameters ?? null;
  const text = sizeAnswer(parameters, active, !!ff?.moe);
  return text ? { node: text, source: fromCard ? SOURCE.card : SOURCE.file } : null;
}

function willItFit(f: CompareFacts): Answer | null {
  return f.fit ? { node: <FitBadge fit={f.fit} />, source: fitSource(f.ctx) } : null;
}

function itemSource(f: CompareFacts): string {
  if ("catalogId" in f.item) return SOURCE.catalog;
  if ("installedPath" in f.item) return SOURCE.disk;
  return SOURCE.hub;
}

function downloadSize(f: CompareFacts): Answer | null {
  return f.sizeBytes ? { node: formatBytes(f.sizeBytes), source: itemSource(f) } : null;
}

function capability(key: "tools" | "vision" | "thinking", fileKey: "tools" | "vision" | "thinking") {
  return (f: CompareFacts): Answer | null => {
    if (f.catalog) return { node: capabilityAnswer(f.catalog.capabilities.includes(key)), source: SOURCE.catalog };
    if (f.fileFacts) return { node: capabilityAnswer(f.fileFacts[fileKey]), source: SOURCE.file };
    return null;
  };
}

function longest(f: CompareFacts): Answer | null {
  const n = f.fileFacts?.contextLength;
  return n ? { node: tokensWithWords(n), source: SOURCE.file } : null;
}

const MAX_LANGUAGES = 6;

function languages(f: CompareFacts): Answer | null {
  const fromCard = f.catalog?.languages.length ? languageWords(f.catalog.languages) : [];
  const words = fromCard.length ? fromCard : languageWords(f.hub?.languages ?? []);
  if (!words.length) return null;
  const more = words.length - MAX_LANGUAGES;
  const text = more > 0 ? `${words.slice(0, MAX_LANGUAGES).join(", ")} and ${more} more` : joinWords(words);
  return { node: text, source: fromCard.length ? SOURCE.card : SOURCE.uploader };
}

function madeFrom(f: CompareFacts): Answer | null {
  const base = f.hub?.baseModel ?? f.baseCatalog?.baseModel ?? null;
  if (base) return { node: <Outside text={base} />, source: SOURCE.uploader };
  const repo = cardRepo(f.catalog?.source ?? null);
  return repo ? { node: <Outside text={repo} />, source: SOURCE.catalog } : null;
}

function released(f: CompareFacts): Answer | null {
  const r = f.catalog?.released ? dateWords(f.catalog.released) : null;
  if (r) return { node: `Released ${r}`, source: SOURCE.card };
  const u = f.hub?.lastModified ? dateWords(f.hub.lastModified) : null;
  return u ? { node: `Last updated ${u}`, source: SOURCE.hub } : null;
}

function license(f: CompareFacts): Answer | null {
  const l = f.catalog?.license ?? f.hub?.license ?? null;
  if (!l?.name) return null;
  return {
    node: (
      <>
        <Outside text={l.name} />
        {l.url && (
          <>
            {" "}
            <SafeLink href={l.url}>Read the license</SafeLink>
          </>
        )}
      </>
    ),
    source: f.catalog?.license ? SOURCE.catalog : SOURCE.uploader,
  };
}

function downloads(f: CompareFacts): Answer | null {
  const h = f.hub;
  if (!h || h.downloads == null) return null;
  const likes = h.likes ? `, ${count(h.likes)} ${h.likes === 1 ? "like" : "likes"}` : "";
  return { node: `${count(h.downloads)} in the last 30 days${likes}`, source: SOURCE.hub };
}

const MAX_RESULTS = 3;

function yourResults(f: CompareFacts): Answer {
  if (!f.results.length)
    return {
      node: "No tests on this computer yet. Load the model and choose Check this model in Settings → Local models.",
      source: null,
    };
  const shownResults = [...f.results].sort((a, b) => b.at - a.at).slice(0, MAX_RESULTS);
  const more = f.results.length - shownResults.length;
  return {
    node: (
      <>
        {shownResults.map((r) => {
          const what =
            r.kind === "modelCheck"
              ? `Model check: ${r.passed} of ${r.total} checks passed`
              : `Coding benchmark: ${r.passed} of ${r.total} tasks done`;
          const when = msDateWords(r.at);
          const where = r.backend ? backendWords(r.backend) : null;
          return (
            <span key={`${r.kind}:${r.id}`} className="block">
              {what}
              <span className="text-fg-muted">{[where, when].filter(Boolean).map((t) => ` · ${t}`).join("")}</span>
            </span>
          );
        })}
        {more > 0 && <span className="block text-fg-muted">and {more} more</span>}
      </>
    ),
    source: SOURCE.results,
  };
}

/** The rows, as questions (0.0.5 item 4, word for word). */
export const ROWS: { question: string; answer: (f: CompareFacts) => Answer | null }[] = [
  { question: "What is it good at?", answer: goodAt },
  { question: "Who made it?", answer: whoMadeIt },
  { question: "How big is it?", answer: howBig },
  { question: "Will it fit on this computer?", answer: willItFit },
  { question: "Download size", answer: downloadSize },
  { question: "Can it use tools?", answer: capability("tools", "tools") },
  { question: "Can it read images?", answer: capability("vision", "vision") },
  { question: "Does it think before answering?", answer: capability("thinking", "thinking") },
  { question: "Longest conversation it was made for", answer: longest },
  { question: "Languages", answer: languages },
  { question: "Made from", answer: madeFrom },
  { question: "Released or last updated", answer: released },
  { question: "License", answer: license },
  { question: "Downloads on Hugging Face", answer: downloads },
  { question: "Your test results", answer: yourResults },
];

const SMALL_BUTTON =
  "inline-flex shrink-0 items-center gap-1 rounded-md border border-border px-2 py-0.5 text-xs hover:bg-bg-muted disabled:opacity-40 disabled:hover:bg-transparent";

/**
 * Add to comparison / Remove from comparison for one model. Disabled while the list is full,
 * with the reason as its tooltip (pages also show it once as text). Installed rows (WP2) can
 * use it with `useAddToComparison`.
 */
export function AddToComparison(props: {
  added: boolean;
  full: boolean;
  onAdd: () => void;
  onRemove: () => void;
  /** The model's name, so screen readers can tell the buttons of a list apart. */
  name?: string;
}) {
  const named = (label: string) => (props.name ? `${label}: ${props.name}` : undefined);
  if (props.added) {
    return (
      <button type="button" onClick={props.onRemove} className={SMALL_BUTTON} aria-label={named("Remove from comparison")}>
        <Check size={12} aria-hidden /> Remove from comparison
      </button>
    );
  }
  return (
    <button
      type="button"
      onClick={props.onAdd}
      disabled={props.full}
      title={props.full ? COMPARE_FULL : undefined}
      className={SMALL_BUTTON}
      aria-label={named("Add to comparison")}
    >
      <Plus size={12} aria-hidden /> Add to comparison
    </button>
  );
}

/** The Compare tab's label, with how many models are chosen. */
export function CompareTabLabel() {
  const n = useCompare((s) => s.entries.length);
  return (
    <>
      Compare
      {n > 0 && (
        <>
          <span aria-hidden className="ml-1.5 rounded-full bg-accent px-1.5 text-[11px] text-accent-fg tabular-nums">
            {n}
          </span>
          <span className="sr-only"> ({n} chosen)</span>
        </>
      )}
    </>
  );
}

/** One column: an entry, what is known about it, and why something is missing. */
interface Column {
  entry: CompareEntry;
  key: string;
  facts: CompareFacts | null;
  /** Waiting for the file choice or the facts. */
  loading: boolean;
  problem: CompareProblem | "noFiles" | null;
  /** The error behind a "failed" problem, for its own words. */
  error: ErrorInfo | null;
}

function factsKey(ctx: number | null, entry: CompareEntry): string {
  return `${ctx ?? ""}\n${entryKey(entry)}`;
}

/** A file the app chose at another size, about to be chosen again at this one. */
function rechoosing(
  entry: CompareEntry,
  auto: string[],
  repos: Record<string, { ctx: number | null; view: RepoView }>,
  repoErrors: Record<string, ErrorInfo>,
  ctx: number | null,
): boolean {
  if (!("repo" in entry) || !isAutoFile({ auto }, entry) || repoErrors[entry.repo]) return false;
  const cached = repos[entry.repo];
  if (!cached || cached.ctx !== ctx) return true;
  const chosen = chooseFile(cached.view.variants);
  return chosen !== null && chosen !== entry.file;
}

/** Models → Compare. `onOpen` switches the dialog's tab (the empty state's buttons). */
export function Compare(props: { onOpen?: (tab: "installed" | "browse") => void }) {
  const entries = useCompare((s) => s.entries);
  const auto = useCompare((s) => s.auto);
  const ctx = useCompare((s) => s.ctx);
  const repos = useCompare((s) => s.repos);
  const setCtx = useCompare((s) => s.setCtx);
  const add = useCompare((s) => s.add);
  const remove = useCompare((s) => s.remove);
  const setFile = useCompare((s) => s.setFile);
  const autoFile = useCompare((s) => s.autoFile);
  const rememberRepo = useCompare((s) => s.rememberRepo);

  const [facts, setFacts] = useState<Record<string, CompareFacts>>({});
  const [factErrors, setFactErrors] = useState<Record<string, ErrorInfo>>({});
  const [repoErrors, setRepoErrors] = useState<Record<string, ErrorInfo>>({});
  const [suggested, setSuggested] = useState<{ ctx: number | null; list: CatalogListEntry[] } | null>(null);
  const [suggestError, setSuggestError] = useState<ErrorInfo | null>(null);
  const requested = useRef(new Set<string>());
  const repoRequests = useRef(new Map<string, Promise<RepoView>>());
  const suggestRequest = useRef<string | null>(null);

  /** One repository page at the comparison size, fetched once even when asked twice. */
  const fetchRepo = useCallback(
    (repo: string, size: number | null): Promise<RepoView> => {
      const k = `${repo}\n${size ?? ""}`;
      let p = repoRequests.current.get(k);
      if (!p) {
        p = ipc
          .hubRepo(repo, size)
          .then((view) => {
            // A late answer for an older size must not replace the page for the current one.
            const now = useCompare.getState();
            if (now.ctx === size || !now.repos[repo]) rememberRepo(view, size);
            return view;
          })
          .finally(() => repoRequests.current.delete(k));
        repoRequests.current.set(k, p);
      }
      return p;
    },
    [rememberRepo],
  );

  // Repository entries without a file, or with a file the app chose at another size: choose
  // (again) at this size, from the page already fetched or by fetching that one page.
  useEffect(() => {
    for (const e of entries) {
      if (!("repo" in e)) continue;
      if (e.file !== null && !isAutoFile({ auto }, e)) continue;
      if (repoErrors[e.repo]) continue;
      const cached = repos[e.repo];
      if (cached && cached.ctx === ctx) {
        const chosen = chooseFile(cached.view.variants);
        if (chosen !== null && chosen !== e.file) autoFile(e.repo, e.file, chosen);
        continue;
      }
      fetchRepo(e.repo, ctx).catch((err) => {
        const info = toErrorInfo(err);
        setRepoErrors((old) => ({ ...old, [e.repo]: info }));
      });
    }
  }, [entries, auto, ctx, repos, repoErrors, autoFile, fetchRepo]);

  // Facts for the entries not asked for yet at this size, in one request. A file the app
  // chose at another size waits for the new choice, so no request is spent on the old one.
  useEffect(() => {
    const missing: { key: string; item: NonNullable<ReturnType<typeof toCompareItem>> }[] = [];
    for (const e of entries) {
      const item = toCompareItem(e);
      if (!item) continue;
      if (rechoosing(e, auto, repos, repoErrors, ctx)) continue;
      const key = factsKey(ctx, e);
      if (facts[key] || factErrors[key] || requested.current.has(key)) continue;
      missing.push({ key, item });
    }
    if (!missing.length) return;
    for (const m of missing) requested.current.add(m.key);
    ipc
      .compareFacts(
        missing.map((m) => m.item),
        ctx,
      )
      .then((list) => {
        setFacts((old) => {
          const next = { ...old };
          missing.forEach((m, i) => {
            const f = list[i];
            if (f) next[m.key] = f;
          });
          return next;
        });
        const lost = missing.filter((_, i) => !list[i]);
        if (lost.length) {
          const info = toErrorInfo("compare_facts returned fewer items than asked");
          setFactErrors((old) => Object.fromEntries([...Object.entries(old), ...lost.map((m) => [m.key, info])]));
        }
      })
      .catch((err) => {
        const info = toErrorInfo(err);
        setFactErrors((old) => Object.fromEntries([...Object.entries(old), ...missing.map((m) => [m.key, info])]));
      })
      .finally(() => {
        for (const m of missing) requested.current.delete(m.key);
      });
  }, [entries, auto, repos, repoErrors, ctx, facts, factErrors]);

  // Suggested models while fewer than four are chosen, at the comparison size.
  const wantSuggestions = entries.length < COMPARE_LIMIT;
  useEffect(() => {
    if (!wantSuggestions || (suggested && suggested.ctx === ctx)) return;
    const k = String(ctx);
    if (suggestRequest.current === k) return;
    suggestRequest.current = k;
    ipc
      .catalogList(ctx)
      .then((list) => {
        if (useCompare.getState().ctx !== ctx) return;
        setSuggested({ ctx, list });
        setSuggestError(null);
      })
      .catch((err) => setSuggestError(toErrorInfo(err)))
      .finally(() => {
        if (suggestRequest.current === k) suggestRequest.current = null;
      });
  }, [wantSuggestions, ctx, suggested]);

  function retry(col: Column) {
    setFacts((old) => {
      const next = { ...old };
      delete next[col.key];
      return next;
    });
    setFactErrors((old) => {
      const next = { ...old };
      delete next[col.key];
      return next;
    });
    if ("repo" in col.entry) {
      const repo = col.entry.repo;
      setRepoErrors((old) => {
        const next = { ...old };
        delete next[repo];
        return next;
      });
    }
  }

  const columns: Column[] = entries.map((entry) => {
    const key = factsKey(ctx, entry);
    const f = facts[key] ?? null;
    if ("repo" in entry && entry.file === null) {
      const err = repoErrors[entry.repo];
      const cached = repos[entry.repo];
      const noFiles = !err && cached?.ctx === ctx && chooseFile(cached.view.variants) === null;
      return {
        entry,
        key,
        facts: null,
        loading: !err && !noFiles,
        problem: err ? problemFromError(err) : noFiles ? "noFiles" : null,
        error: err ?? null,
      };
    }
    const err = factErrors[key];
    return {
      entry,
      key,
      facts: f,
      loading: !f && !err,
      problem: f?.problem ?? (err ? problemFromError(err) : null),
      error: err ?? null,
    };
  });

  // The defaults' size, once a request at it has said what it is.
  const defaultCtx = Object.entries(facts).find(([k]) => k.startsWith("\n"))?.[1].ctx ?? null;
  const names = new Map((suggested?.list ?? []).map((s) => [s.catalog.id, s.catalog.name]));

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-2">
        <label htmlFor="compare-ctx" className="text-sm font-medium">
          Compare at
        </label>
        <select
          id="compare-ctx"
          value={ctx === null ? "" : String(ctx)}
          onChange={(e) => setCtx(e.target.value === "" ? null : Number(e.target.value))}
          className="rounded-md border border-border bg-bg px-2 py-1 text-sm"
        >
          <option value="">Same as your default{defaultCtx ? ` (${tokensWithWords(defaultCtx)})` : ""}</option>
          {COMPARE_SIZES.map((n) => (
            <option key={n} value={String(n)}>
              {tokensWithWords(n)}
            </option>
          ))}
        </select>
        <p className="w-full text-xs text-fg-muted">
          How much of a chat each model should remember. “Will it fit on this computer?” is checked at this size.
        </p>
      </div>

      {columns.length === 0 ? (
        <div className="rounded-md border border-dashed border-border px-4 py-6 text-center text-sm text-fg-muted">
          <p>{EMPTY_TEXT}</p>
          {props.onOpen && (
            <div className="mt-3 flex justify-center gap-2">
              <button type="button" className={SMALL_BUTTON} onClick={() => props.onOpen?.("browse")}>
                Open Browse
              </button>
              <button type="button" className={SMALL_BUTTON} onClick={() => props.onOpen?.("installed")}>
                Open Installed
              </button>
            </div>
          )}
        </div>
      ) : (
        <div className="overflow-x-auto">
          <table
            aria-label="Model comparison"
            className="w-full table-fixed border-collapse text-left text-sm"
            style={{ minWidth: `${9 + columns.length * 10}rem` }}
          >
            <caption className="sr-only">
              The chosen models side by side. Under each answer, small text says where it comes from.
            </caption>
            <colgroup>
              <col className="w-36" />
              {columns.map((c) => (
                <col key={c.key} />
              ))}
            </colgroup>
            <thead>
              <tr>
                <th scope="col" className="align-bottom">
                  <span className="sr-only">Question</span>
                </th>
                {columns.map((c) => (
                  <th key={c.key} scope="col" className="px-2 pb-2 align-top font-normal">
                    <ColumnHead
                      column={c}
                      ctx={ctx}
                      fallbackName={"catalogId" in c.entry ? names.get(c.entry.catalogId) : undefined}
                      auto={isAutoFile({ auto }, c.entry)}
                      repoView={"repo" in c.entry ? repos[c.entry.repo] : undefined}
                      fetchRepo={fetchRepo}
                      onRemove={() => remove(c.entry)}
                      onRetry={() => retry(c)}
                      onPickFile={(to) => {
                        if ("repo" in c.entry) setFile(c.entry.repo, c.entry.file, to);
                      }}
                    />
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {ROWS.map((row) => (
                <tr key={row.question} className="border-t border-border">
                  <th scope="row" className="py-2 pr-3 align-top text-xs font-medium">
                    {row.question}
                  </th>
                  {columns.map((c) => (
                    <td key={c.key} className="px-2 py-2 align-top text-xs break-words">
                      <Cell column={c} answer={c.facts ? row.answer(c.facts) : null} />
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {wantSuggestions && (
        <Suggestions
          list={suggested?.ctx === ctx ? suggested.list : null}
          error={suggestError}
          entries={entries}
          full={entries.length >= COMPARE_LIMIT}
          onAdd={(id) => add({ catalogId: id })}
        />
      )}
    </div>
  );
}

function Cell(props: { column: Column; answer: Answer | null }) {
  const { column, answer } = props;
  if (!column.facts) {
    return column.loading ? (
      <span className="text-fg-muted" aria-label="Loading">
        …
      </span>
    ) : (
      <span className="text-fg-muted">Not known</span>
    );
  }
  if (!answer) return <span className="text-fg-muted">Not known</span>;
  return (
    <>
      <div>{answer.node}</div>
      {answer.source && <p className="mt-0.5 text-[11px] text-fg-muted">{answer.source}</p>}
    </>
  );
}

function columnTitle(c: Column, fallbackName: string | undefined): string {
  if (c.facts?.title) return c.facts.title;
  const e = c.entry;
  if ("catalogId" in e) return fallbackName ?? e.catalogId;
  if ("installedPath" in e) return e.installedPath.split(/[\\/]/).pop() || e.installedPath;
  return e.repo;
}

function ColumnHead(props: {
  column: Column;
  ctx: number | null;
  fallbackName: string | undefined;
  auto: boolean;
  repoView: { ctx: number | null; view: RepoView } | undefined;
  fetchRepo: (repo: string, ctx: number | null) => Promise<RepoView>;
  onRemove: () => void;
  onRetry: () => void;
  onPickFile: (file: string) => void;
}) {
  const { column: c } = props;
  const f = c.facts;
  const title = columnTitle(c, props.fallbackName);
  const file = f?.file ?? ("repo" in c.entry ? c.entry.file : null);
  const quant = f?.quant ?? f?.fileFacts?.quantization ?? quantFromFile(file);
  const fileLine = fileWords(quant) ?? (file ? (file.split("/").pop() ?? file) : null);
  const cardUrl = f?.hub?.cardUrl ?? f?.catalog?.source ?? null;
  const problemText =
    c.problem === null
      ? null
      : c.problem === "failed" && c.error && c.error.code !== "unexpected"
        ? `${c.error.message} ${c.error.nextStep}`
        : PROBLEM_WORDS[c.problem];

  return (
    <div className="space-y-1">
      <div className="flex items-start gap-1">
        <p className="min-w-0 flex-1 text-sm font-semibold break-words">
          <Outside text={title} />
        </p>
        <button
          type="button"
          onClick={props.onRemove}
          aria-label={`Remove ${shown(title)} from comparison`}
          title="Remove from comparison"
          className="rounded-md p-0.5 text-fg-muted hover:bg-bg-muted hover:text-fg"
        >
          <X size={14} aria-hidden />
        </button>
      </div>
      {fileLine && (
        <p className="text-xs text-fg-muted">
          <Outside text={fileLine} />
        </p>
      )}
      {"repo" in c.entry && c.entry.file !== null && (
        <FileChange
          repo={c.entry.repo}
          file={c.entry.file}
          ctx={props.ctx}
          auto={props.auto}
          repoView={props.repoView}
          fetchRepo={props.fetchRepo}
          onPick={props.onPickFile}
        />
      )}
      {"repo" in c.entry && c.entry.file === null && c.loading && (
        <p className="text-xs text-fg-muted">Choosing a file…</p>
      )}
      {f?.installedPath && <p className="text-xs text-success">On this computer</p>}
      {cardUrl && (
        <p className="text-xs">
          <SafeLink href={cardUrl}>Show model card</SafeLink>
        </p>
      )}
      {c.loading && c.facts === null && !("repo" in c.entry && c.entry.file === null) && (
        <p className="text-xs text-fg-muted">Getting the facts…</p>
      )}
      {problemText && (
        <div role="status" className="rounded-md border border-warning/40 bg-warning/10 px-2 py-1 text-xs">
          <p>{problemText}</p>
          {c.problem !== "noFiles" && c.problem !== "notFound" && (
            <button type="button" onClick={props.onRetry} className="mt-1 inline-flex items-center gap-1 underline">
              <RefreshCw size={11} aria-hidden /> Try again
            </button>
          )}
        </div>
      )}
    </div>
  );
}

/** "Change": the repository's files with their fit at the comparison size. */
function FileChange(props: {
  repo: string;
  file: string;
  ctx: number | null;
  auto: boolean;
  repoView: { ctx: number | null; view: RepoView } | undefined;
  fetchRepo: (repo: string, ctx: number | null) => Promise<RepoView>;
  onPick: (file: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const view = props.repoView?.ctx === props.ctx ? props.repoView.view : null;
  const picked = view?.variants.find((v) => v.files[0]?.path === props.file);

  function toggle() {
    const next = !open;
    setOpen(next);
    setError(null);
    if (next && !view) props.fetchRepo(props.repo, props.ctx).catch((err) => setError(toErrorInfo(err)));
  }

  return (
    <div className="text-xs">
      {props.auto && picked && (
        <p className="text-fg-muted">
          {picked.fit?.verdict === "green"
            ? "Picked for you: the largest file that fits."
            : "Picked for you: the smallest file."}
        </p>
      )}
      <button type="button" aria-expanded={open} onClick={toggle} className="inline-flex items-center gap-0.5 underline">
        Change <ChevronDown size={11} aria-hidden />
      </button>
      {open && (
        <div role="group" aria-label="Files in this repository" className="mt-1 space-y-0.5">
          {error && <p className="text-danger">{PROBLEM_WORDS[problemFromError(error)]}</p>}
          {!error && !view && <p className="text-fg-muted">Loading files…</p>}
          {view?.variants.map((v) => {
            const path = v.files[0]?.path;
            if (!path) return null;
            const current = path === props.file;
            return (
              <button
                key={path}
                type="button"
                aria-current={current ? "true" : undefined}
                onClick={() => {
                  props.onPick(path);
                  setOpen(false);
                }}
                className={`block w-full rounded px-1.5 py-1 text-left hover:bg-bg-muted ${current ? "font-medium" : ""}`}
              >
                {current && <Check size={11} className="mr-1 inline" aria-hidden />}
                {fileWords(v.label) ?? v.label} · {formatBytes(v.totalSize)}
                {v.fit ? ` · ${FIT_LABEL[v.fit.verdict]}` : ""}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}

const FIT_LABEL = { green: "Fits", yellow: "Fits with limits", red: "Too large" } as const;

function Suggestions(props: {
  list: CatalogListEntry[] | null;
  error: ErrorInfo | null;
  entries: CompareEntry[];
  full: boolean;
  onAdd: (catalogId: string) => void;
}) {
  const chosen = (s: CatalogListEntry) =>
    props.entries.some(
      (e) =>
        sameEntry(e, { catalogId: s.catalog.id }) ||
        sameEntry(e, { repo: s.repo, file: s.file }) ||
        (s.installedPath !== null && sameEntry(e, { installedPath: s.installedPath })),
    );
  const list = props.list?.filter((s) => !chosen(s)) ?? null;
  if (list && list.length === 0 && !props.error) return null;
  return (
    <section aria-labelledby="compare-suggested" className="space-y-2">
      <div>
        <h3 id="compare-suggested" className="text-sm font-semibold">
          Suggested models
        </h3>
        <p className="text-xs text-fg-muted">From the list of models that comes with this app.</p>
      </div>
      {props.error && <p className="text-xs text-fg-muted">Suggestions could not be loaded. {props.error.message}</p>}
      {!list && !props.error && <p className="text-xs text-fg-muted">Loading suggestions…</p>}
      <ul className="space-y-2">
        {list?.map((s) => {
          const sentence = goodAtSentence(s.catalog.goodAt);
          return (
            <li key={s.catalog.id} className="flex items-start gap-3 rounded-md border border-border px-3 py-2 text-sm">
              <div className="min-w-0 flex-1">
                <p className="font-medium">
                  <Outside text={s.catalog.name} />
                  <span className="ml-2 text-xs font-normal text-fg-muted">
                    {[fileWords(s.quant), `${formatBytes(s.sizeBytes)} download`, s.installedPath ? "On this computer" : null]
                      .filter(Boolean)
                      .join(" · ")}
                  </span>
                </p>
                {sentence && (
                  <p className="text-xs text-fg-muted">
                    {sentence} ({SOURCE.card.charAt(0).toLowerCase() + SOURCE.card.slice(1)})
                  </p>
                )}
                {s.fit && <FitBadge fit={s.fit} />}
              </div>
              <AddToComparison
                added={false}
                full={props.full}
                onAdd={() => props.onAdd(s.catalog.id)}
                onRemove={() => undefined}
                name={shown(s.catalog.name)}
              />
            </li>
          );
        })}
      </ul>
    </section>
  );
}

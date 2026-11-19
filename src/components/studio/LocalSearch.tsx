import { useId, useState } from "react";
import { Search } from "lucide-react";
import { formatCount } from "../../lib/format";
import type { ErrorInfo } from "../../lib/types";
import { LOCAL_MODELS } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { useStudio, type StudioTab } from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import { visiblePath } from "../FolderChip";
import { FitLine, LicenceBadge, offerForFile, TimeLine } from "./LocalModelRow";
import { CARD_BUTTON } from "./styles";
import type { LocalFileChoice, LocalModels, LocalRepo } from "./types";

// Hugging Face search in Get local models (0.1.0 Task B13): Hugging Face is asked only on
// Search or Enter (never as typing goes) and when one result is opened, and the box says first
// that the words go to huggingface.co. A hit shows its downloads and the repository's own
// licence label; opened, it is read at its latest commit and shows each file Studio recognises
// with its parts, total size, licence and fit, and *Download*, or why it has none ("Not
// supported yet", "Not offered in harnessUI"). A result whose licence is red shows its files only
// while *Show non-commercial models* is ticked. Names from Hugging Face can hold any character:
// they are shown with invisible ones as codes.

/** An opened search result: the repository's name, and what reading it gave (`null` while it
 *  is being read). */
interface Opened {
  repo: string;
  result: LocalRepo | null;
}

/** Hugging Face search for `tab`'s kind of models. */
export function LocalSearch(props: { tab: StudioTab }) {
  const { tab } = props;
  const local = useStudio((s) => s.local);
  const found = useStudio((s) => s.localSearch);
  const search = useStudio((s) => s.search);
  const openRepo = useStudio((s) => s.openRepo);
  const [text, setText] = useState(found !== null && found.tab === tab ? found.text : "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const [opened, setOpened] = useState<Opened | null>(null);
  const sendsId = useId();
  const hits = found !== null && found.tab === tab ? found.hits : null;

  function ask() {
    const words = text.trim();
    if (words === "" || busy) return;
    setBusy(true);
    setError(null);
    setOpened(null);
    search(words, tab)
      .catch((e: unknown) => setError(toErrorInfo(e)))
      .finally(() => setBusy(false));
  }

  /** Opens result `repo` (reading it at its latest commit), or closes it when it is open. */
  function toggle(repo: string) {
    if (opened?.repo === repo) {
      setOpened(null);
      return;
    }
    setError(null);
    setOpened({ repo, result: null });
    openRepo(repo).then(
      (result) => setOpened((now) => (now?.repo === repo ? { repo, result } : now)),
      (e: unknown) => {
        setError(toErrorInfo(e));
        setOpened((now) => (now?.repo === repo ? null : now));
      },
    );
  }

  return (
    <section className="flex flex-col gap-2 pt-2">
      <div className="flex flex-wrap items-center gap-2">
        <label className="flex min-w-48 flex-1 items-center gap-1.5 rounded-lg border border-border bg-bg px-2 py-1 text-sm focus-within:border-border-strong">
          <Search size={14} aria-hidden className="text-fg-muted" />
          <input
            type="search"
            aria-label={LOCAL_MODELS.search}
            aria-describedby={sendsId}
            placeholder={LOCAL_MODELS.searchPlaceholder}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key !== "Enter") return;
              e.preventDefault();
              ask();
            }}
            className="min-w-0 flex-1 bg-transparent outline-none placeholder:text-fg-muted"
          />
        </label>
        <button type="button" className={CARD_BUTTON} disabled={busy} onClick={ask}>
          {LOCAL_MODELS.searchButton}
        </button>
      </div>
      <p id={sendsId} className="text-xs text-fg-muted">
        {LOCAL_MODELS.searchSends}
      </p>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      {busy && (
        <p className="text-sm text-fg-muted" role="status">
          {LOCAL_MODELS.searching}
        </p>
      )}
      {!busy && hits !== null && hits.length === 0 && <p className="text-sm text-fg-muted">{LOCAL_MODELS.noResults}</p>}
      {!busy && hits !== null && hits.length > 0 && (
        <ul aria-label={LOCAL_MODELS.results} className="flex flex-col gap-1.5">
          {hits.map((hit) => {
            const open = opened?.repo === hit.repo ? opened : null;
            const name = visiblePath(hit.repo);
            return (
              <li key={hit.repo} className="flex flex-col gap-1.5">
                <div className="flex flex-wrap items-center gap-x-3 gap-y-0.5 text-sm">
                  <button
                    type="button"
                    aria-expanded={open !== null}
                    onClick={() => toggle(hit.repo)}
                    className="min-w-0 truncate text-left font-medium underline-offset-2 hover:underline"
                  >
                    <bdi dir="ltr">{name}</bdi>
                  </button>
                  <span className="text-xs text-fg-muted">{LOCAL_MODELS.downloads(formatCount(hit.downloads))}</span>
                  {hit.licenseLabel && (
                    <span className="text-xs text-fg-muted">{LOCAL_MODELS.labelled(visiblePath(hit.licenseLabel))}</span>
                  )}
                </div>
                {open && local && (
                  <section aria-label={name} className="ml-3 flex flex-col gap-2 border-l border-border pl-3">
                    {open.result === null ? (
                      <p className="text-xs text-fg-muted" role="status">
                        {LOCAL_MODELS.opening}
                      </p>
                    ) : (
                      <RepoResult repo={open.result} local={local} tab={tab} />
                    )}
                  </section>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

/** What an opened result holds: its files to download, or why there are none. */
function RepoResult(props: { repo: LocalRepo; local: LocalModels; tab: StudioTab }) {
  const { repo, local, tab } = props;
  const showNonCommercial = useStudio((s) => s.showNonCommercial);
  if (repo.status === "notOffered") {
    return <p className="text-sm">{LOCAL_MODELS.notOffered(repo.reason)}</p>;
  }
  if (repo.status === "notSupportedYet") {
    return (
      <div className="flex flex-col gap-0.5 text-sm">
        <p className="font-medium">{LOCAL_MODELS.notSupportedYet}</p>
        {repo.reason && <p className="text-fg-muted">{repo.reason}</p>}
      </div>
    );
  }
  const hidden = repo.licence?.badge === "red" && !showNonCommercial;
  return (
    <div className="flex flex-col gap-2">
      {repo.licence && (
        <div>
          <LicenceBadge licence={repo.licence} />
        </div>
      )}
      {repo.labelMismatch && <p className="text-xs text-fg-muted">{repo.labelMismatch}</p>}
      {hidden ? (
        <p className="text-xs text-fg-muted">{LOCAL_MODELS.hiddenNonCommercial}</p>
      ) : (
        repo.files.map((f) => <FileChoice key={f.file} repo={repo} file={f} local={local} tab={tab} />)
      )}
    </div>
  );
}

/** One file of a searched repository, with the parts it needs beside it. */
function FileChoice(props: { repo: LocalRepo; file: LocalFileChoice; local: LocalModels; tab: StudioTab }) {
  const { repo, file: f, local, tab } = props;
  const setOffer = useStudio((s) => s.setOffer);
  return (
    <div className="flex flex-col gap-1 rounded-lg border border-border p-2.5">
      <p className="text-sm font-medium">
        <bdi dir="ltr">{visiblePath(f.file)}</bdi>
      </p>
      <p className="text-xs">{LOCAL_MODELS.fileLine(f.parts.length, f.totalBytes, f.toFetchBytes)}</p>
      <ul className="text-xs text-fg-muted">
        {f.parts.map((p) => (
          <li key={`${p.repo}/${p.path}`}>{LOCAL_MODELS.part(p.label, p.sizeBytes, p.alreadyHere)}</li>
        ))}
      </ul>
      <FitLine fit={f.fit} />
      <TimeLine fit={f.fit} kind={tab} />
      {f.fit.word !== "notHere" && (
        <div className="pt-1">
          <button
            type="button"
            className="btn-primary rounded-lg px-3 py-1 text-sm font-medium"
            onClick={() => setOffer(offerForFile(repo, f, local))}
          >
            {LOCAL_MODELS.download}
          </button>
        </div>
      )}
    </div>
  );
}

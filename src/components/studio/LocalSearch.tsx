import { useId, useState } from "react";
import { Search } from "lucide-react";
import { formatBytes, formatCount } from "../../lib/format";
import type { ErrorInfo } from "../../lib/types";
import { LOCAL_MODELS } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { useStudio, type StudioTab } from "../../store/studio";
import { useDownloads } from "../../store/downloads";
import { BrowserLink, modelPage } from "./BrowserLink";
import { studioLocalDownloadFile } from "./api";
import { ErrorBanner } from "../ErrorBanner";
import { visiblePath } from "../FolderChip";
import { DownloadProgress, FitLine, LicenceBadge, TimeLine, useModelDownload } from "./LocalModelRow";
import { offerForFile, searchedModelId } from "./localOffers";
import { CARD_BUTTON, PRIMARY_BUTTON } from "./styles";
import type { LocalFileChoice, LocalModels, LocalRepo } from "./types";

// Hugging Face search in Get local models (0.1.0 Task B13): Hugging Face is asked only on
// Search or Enter (never as typing goes) and when one result is opened, and the box says first
// that the words go to huggingface.co. A hit shows its downloads and the repository's own
// licence label; opened, it is read at its latest commit and shows each file Studio recognises
// with its parts, total size, licence and fit, and *Download* (how far its download got while
// it runs), or why the engine cannot run it. Names from Hugging Face are shown with
// invisible characters as codes.

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
        <label className="flex min-w-32 flex-1 items-center gap-1.5 rounded-lg border border-border bg-bg px-2 py-1 text-sm focus-within:border-border-strong">
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
  const [error, setError] = useState<ErrorInfo | null>(null);
  const canRun = repo.status === "supported";
  const standalone = repo.standaloneFiles.length > 0 && <StandaloneFile repo={repo} onError={setError} />;
  return (
    <div className="flex flex-col gap-2">
      {repo.licence && (
        <div>
          <LicenceBadge licence={repo.licence} />
        </div>
      )}
      {repo.labelMismatch && <p className="text-xs text-fg-muted">{repo.labelMismatch}</p>}
      {repo.files.map((f) => <FileChoice key={f.file} repo={repo} file={f} local={local} tab={tab} />)}
      {standalone && canRun ? (
        <details className="text-sm">
          <summary>{LOCAL_MODELS.otherFiles}</summary>
          {standalone}
        </details>
      ) : standalone}
      {!canRun && (
        <div className="text-xs text-fg-muted">
          <p>{LOCAL_MODELS.noLocalSetup}</p>
          <BrowserLink href={`${modelPage(repo.repo)}/tree/${repo.commit}`} onError={setError}>
            {LOCAL_MODELS.openFiles}
          </BrowserLink>
        </div>
      )}
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
    </div>
  );
}

/** Save one set of weights, without installing an inference recipe or any companions. */
function StandaloneFile({ repo, onError }: { repo: LocalRepo; onError: (error: ErrorInfo) => void }) {
  const [path, setPath] = useState(repo.standaloneFiles[0]!.path);
  const file = repo.standaloneFiles.find((f) => f.path === path)!;
  const browserUrl = `${modelPage(repo.repo)}/resolve/${repo.commit}/${file.path.split("/").map(encodeURIComponent).join("/")}?download=true`;
  const [starting, setStarting] = useState(false);
  const [queued, setQueued] = useState<string | null>(null);
  const group = useDownloads((s) => s.groups.find((g) =>
    g.repo === repo.repo && g.savedFile?.commit === repo.commit && g.label === path
  ));
  async function download() {
    setStarting(true);
    try {
      await studioLocalDownloadFile(repo.repo, repo.commit, path);
      setQueued(path);
    } catch (e) {
      onError(toErrorInfo(e));
    } finally {
      setStarting(false);
    }
  }
  return (
    <div className="flex flex-col items-start gap-1.5 text-sm">
      <select
        aria-label={LOCAL_MODELS.weightFile}
        className="max-w-full rounded border border-border bg-bg p-1"
        value={path}
        disabled={starting}
        onChange={(e) => setPath(e.target.value)}
      >
        {repo.standaloneFiles.map((file) => (
          <option key={file.path} value={file.path}>{visiblePath(file.path)} · {formatBytes(file.size)}</option>
        ))}
      </select>
      {!file.sha256 || file.size === 0 ? (
        <BrowserLink href={browserUrl} onError={onError}>{LOCAL_MODELS.browserDownload}</BrowserLink>
      ) : group && group.status !== "cancelled" && group.status !== "done" ? (
        <div className="w-full"><DownloadProgress group={group} name={visiblePath(path)} /></div>
      ) : (
        <button type="button" className={PRIMARY_BUTTON} disabled={starting} onClick={() => void download()}>
          {LOCAL_MODELS.saveWeight}
        </button>
      )}
      {group?.status === "done" && group.savedFile && (
        <p className="max-w-full break-all text-xs"><bdi dir="ltr">{visiblePath(group.savedFile.path)}</bdi></p>
      )}
      {queued === path && !group && <p role="status">{LOCAL_MODELS.queuedFile}</p>}
    </div>
  );
}

/** One file of a searched repository, with the parts it needs beside it. */
function FileChoice(props: { repo: LocalRepo; file: LocalFileChoice; local: LocalModels; tab: StudioTab }) {
  const { repo, file: f, local, tab } = props;
  const setOffer = useStudio((s) => s.setOffer);
  const download = useModelDownload(searchedModelId(repo.repo, f.file));
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
      {download ? (
        <div className="pt-1">
          <DownloadProgress group={download} name={visiblePath(f.file)} />
        </div>
      ) : (
        <div className="pt-1">
          <button type="button" className={PRIMARY_BUTTON} onClick={() => setOffer(offerForFile(repo, f, local, tab))}>
            {LOCAL_MODELS.download}
          </button>
        </div>
      )}
    </div>
  );
}

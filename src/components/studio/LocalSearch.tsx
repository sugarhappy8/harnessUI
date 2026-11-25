import { useId, useState } from "react";
import { Search } from "lucide-react";
import { formatBytes, formatCount } from "../../lib/format";
import type { ErrorInfo } from "../../lib/types";
import { LOCAL_MODELS, SLOT_LABEL, fileLabelWords, verdictWords } from "../../lib/words";
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
import type { LocalFileChoice, LocalModels, LocalRepo, RepoFileView } from "./types";

// Hugging Face search in Get local models (0.1.0 Task B13, 0.2.0 Task S6): Hugging Face is
// asked only on Search or Enter (never as typing goes) and when one result is opened, and the
// box says first that the words go to huggingface.co. A hit shows its downloads, the
// repository's own licence label and what it offers (from its name until it is opened);
// opened, it is read at its latest commit and shows each main file that works with its set-up
// plan (each part, total size, licence and fit) and *Download* (how far its download got while
// it runs), then every file Studio labelled with what it is and a plain *Download file
// anyway*. Names from Hugging Face are shown with invisible characters as codes. (Phase U
// replaces this view.)

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
  const filter = tab === "image" ? "pictures" : "video";
  const [text, setText] = useState(found !== null && found.filter === filter ? found.text : "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const [opened, setOpened] = useState<Opened | null>(null);
  const sendsId = useId();
  const hits = found !== null && found.filter === filter ? found.hits : null;

  function ask() {
    const words = text.trim();
    if (words === "" || busy) return;
    setBusy(true);
    setError(null);
    setOpened(null);
    search(words, filter)
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
        <label className="flex min-w-32 flex-1 items-center gap-1.5 rounded-lg border border-border bg-bg px-2 py-1 text-sm focus-within:border-border-strong focus-within:ring-2 focus-within:ring-accent/40">
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
                  {verdictWords(hit.verdict) && <span className="text-xs">{verdictWords(hit.verdict)}</span>}
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

/** What an opened result holds: its main files to download with their set-up, or that Studio
 *  has none, then every file it labelled with a plain download. */
function RepoResult(props: { repo: LocalRepo; local: LocalModels; tab: StudioTab }) {
  const { repo, local, tab } = props;
  const [error, setError] = useState<ErrorInfo | null>(null);
  const canRun = repo.files.length > 0;
  const views = repo.fileViews.length > 0 && <FileViews repo={repo} onError={setError} />;
  return (
    <div className="flex flex-col gap-2">
      {repo.licence && (
        <div>
          <LicenceBadge licence={repo.licence} />
        </div>
      )}
      {repo.labelMismatch && <p className="text-xs text-fg-muted">{repo.labelMismatch}</p>}
      {repo.files.map((f) => <FileChoice key={f.file} repo={repo} file={f} local={local} tab={tab} />)}
      {views && canRun ? (
        <details className="text-sm">
          <summary>{LOCAL_MODELS.otherFiles}</summary>
          {views}
        </details>
      ) : views}
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

/** Every file Studio labelled, each with what it is and its plain download (0.1.2's: the file
 *  alone, without a set-up or any companion). */
function FileViews({ repo, onError }: { repo: LocalRepo; onError: (error: ErrorInfo) => void }) {
  return (
    <ul aria-label={LOCAL_MODELS.repoFiles} className="flex flex-col gap-1.5 text-sm">
      {repo.fileViews.map((f) => (
        <FileView key={f.path} repo={repo} file={f} onError={onError} />
      ))}
    </ul>
  );
}

/** One labelled file: its name, size and label, and *Download file anyway* (a file with no
 *  published checksum is offered in the browser instead), or how far its download got. */
function FileView(props: { repo: LocalRepo; file: RepoFileView; onError: (error: ErrorInfo) => void }) {
  const { repo, file: f, onError } = props;
  const nameId = useId();
  const [starting, setStarting] = useState(false);
  const [queued, setQueued] = useState(false);
  const group = useDownloads((s) =>
    s.groups.find((g) => g.repo === repo.repo && g.savedFile?.commit === repo.commit && g.label === f.path),
  );
  const noChecksum = "cannotRun" in f.label && f.label.cannotRun.reason === "noSha";
  const browserUrl = `${modelPage(repo.repo)}/resolve/${repo.commit}/${f.path.split("/").map(encodeURIComponent).join("/")}?download=true`;
  async function download() {
    setStarting(true);
    try {
      await studioLocalDownloadFile(repo.repo, repo.commit, f.path);
      setQueued(true);
    } catch (e) {
      onError(toErrorInfo(e));
    } finally {
      setStarting(false);
    }
  }
  return (
    <li className="flex flex-col items-start gap-1">
      <p id={nameId} className="max-w-full break-all">
        <bdi dir="ltr">{visiblePath(f.path)}</bdi> · {formatBytes(f.sizeBytes)}
      </p>
      <p className="text-xs text-fg-muted">{fileLabelWords(f.label, f.path)}</p>
      {noChecksum ? (
        <BrowserLink href={browserUrl} onError={onError}>{LOCAL_MODELS.browserDownload}</BrowserLink>
      ) : group && group.status !== "cancelled" && group.status !== "done" ? (
        <div className="w-full"><DownloadProgress group={group} name={visiblePath(f.path)} /></div>
      ) : (
        <button
          type="button"
          className={CARD_BUTTON}
          aria-describedby={nameId}
          disabled={starting}
          onClick={() => void download()}
        >
          {LOCAL_MODELS.downloadAnyway}
        </button>
      )}
      {group?.status === "done" && group.savedFile && (
        <p className="max-w-full break-all text-xs"><bdi dir="ltr">{visiblePath(group.savedFile.path)}</bdi></p>
      )}
      {queued && !group && <p role="status" className="text-xs">{LOCAL_MODELS.queuedFile}</p>}
    </li>
  );
}

/** One main file of a searched repository, with the files its set-up plan lists. */
function FileChoice(props: { repo: LocalRepo; file: LocalFileChoice; local: LocalModels; tab: StudioTab }) {
  const { repo, file: f, local, tab } = props;
  const setOffer = useStudio((s) => s.setOffer);
  const download = useModelDownload(searchedModelId(repo.repo, f.file));
  const plan = f.plan;
  const parts = plan.slots.flatMap((s) => (s.part === null ? [] : [{ role: s.role, part: s.part }]));
  return (
    <div className="flex flex-col gap-1 rounded-lg border border-border p-2.5">
      <p className="text-sm font-medium">
        <bdi dir="ltr">{visiblePath(f.file)}</bdi>
      </p>
      {f.best && <p className="text-xs font-medium">{LOCAL_MODELS.best}</p>}
      <p className="text-xs">{LOCAL_MODELS.fileLine(parts.length, plan.totalBytes, plan.toFetchBytes)}</p>
      <ul className="text-xs text-fg-muted">
        {parts.map(({ role, part: p }) => (
          <li key={`${role}:${p.repo}/${p.path}`}>{LOCAL_MODELS.part(SLOT_LABEL[role], p.sizeBytes, p.alreadyHere)}</li>
        ))}
      </ul>
      <FitLine fit={plan.fit} />
      <TimeLine fit={plan.fit} kind={tab} />
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

import { useId, useState } from "react";
import { formatBytes } from "../../lib/format";
import type { ErrorInfo } from "../../lib/types";
import { LOCAL_MODELS, SETUP, SLOT_LABEL, fileLabelWords } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { useStudio } from "../../store/studio";
import { useDownloads } from "../../store/downloads";
import { BrowserLink, modelPage } from "./BrowserLink";
import { studioLocalDownloadFile } from "./api";
import { ErrorBanner } from "../ErrorBanner";
import { visiblePath } from "../FolderChip";
import { DownloadProgress, FitLine, gateProblem, LicenceBadge, TimeLine, useModelDownload } from "./ModelCard";
import { offerForFile, searchedModelId } from "./localOffers";
import { CARD_BUTTON, PRIMARY_BUTTON } from "./styles";
import type { LocalFileChoice, LocalModels, LocalRepo, MediaKind, RepoFileView } from "./types";

// A Hugging Face result opened from Get models (0.1.0 Task B13, 0.2.0 Tasks S6 and U5), shown in
// a slide-over until Task U6's detail panel replaces it: read at its latest commit, it shows each
// main file that works with its set-up plan (each part, total size, licence and fit) and
// *Download* (how far its download got while it runs), then every file Studio labelled with
// what it is and a plain *Download file anyway*. A gated repository whose files could not be
// read says what it needs first, and offers no download that would only fail (ruling S7).
// Names from Hugging Face are shown with invisible characters as codes.

/** What an opened result holds: its main files to download with their set-up, or that Studio
 *  has none, then every file it labelled with a plain download. */
export function RepoResult(props: { repo: LocalRepo; local: LocalModels; kind: MediaKind }) {
  const { repo, local, kind } = props;
  const [error, setError] = useState<ErrorInfo | null>(null);
  const canRun = repo.files.length > 0;
  const gate = gateProblem(repo);
  const views = repo.fileViews.length > 0 && <FileViews repo={repo} onError={setError} />;
  return (
    <div className="flex flex-col gap-2">
      {repo.summary && <p className="text-sm text-fg-muted">{repo.summary}</p>}
      {gate && <p className="rounded-xl bg-warning/10 px-3 py-2 text-sm">{SETUP.problem[gate]}</p>}
      {repo.licence && (
        <div>
          <LicenceBadge licence={repo.licence} />
        </div>
      )}
      {repo.labelMismatch && <p className="text-xs text-fg-muted">{repo.labelMismatch}</p>}
      {repo.files.map((f) => <FileChoice key={f.file} repo={repo} file={f} local={local} kind={kind} />)}
      {views && canRun ? (
        <details className="text-sm">
          <summary>{LOCAL_MODELS.otherFiles}</summary>
          {views}
        </details>
      ) : views}
      {!canRun && !gate && (
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
  // A file that could not be read for want of a token or the model's terms would not download
  // either: its label says what it needs.
  const gated = "notRead" in f.label && (f.label.notRead.reason === "needsToken" || f.label.notRead.reason === "needsTerms");
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
      {gated ? null : noChecksum ? (
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
function FileChoice(props: { repo: LocalRepo; file: LocalFileChoice; local: LocalModels; kind: MediaKind }) {
  const { repo, file: f, local, kind } = props;
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
      <TimeLine fit={plan.fit} kind={kind} />
      {download ? (
        <div className="pt-1">
          <DownloadProgress group={download} name={visiblePath(f.file)} />
        </div>
      ) : (
        <div className="pt-1">
          <button type="button" className={PRIMARY_BUTTON} onClick={() => setOffer(offerForFile(repo, f, local, kind))}>
            {LOCAL_MODELS.download}
          </button>
        </div>
      )}
    </div>
  );
}

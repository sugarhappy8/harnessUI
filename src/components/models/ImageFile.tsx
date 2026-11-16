import { useEffect, useState, type ReactNode } from "react";
import { Download } from "lucide-react";
import * as ipc from "../../lib/ipc";
import type { DownloadGroup, ImageFileOffer, InstalledModel } from "../../lib/types";
import { formatBytes } from "../../lib/format";
import { IMAGE_FILE_WORDS, imageFileProgress } from "../../lib/words";
import { toErrorInfo, useApp } from "../../store/app";
import { isLoadedNow, reloadModel } from "../../store/modelChange";
import { visiblePath } from "../FolderChip";

/** Whether the model loads with an image file it can read (the "vision" badge). */
export function seesImages(m: InstalledModel): boolean {
  return !!m.projectorPath && !m.imageFileUnreadable;
}

/** Models whose repository was asked by itself this session (after their image file couldn't
 *  be read), so it happens once. */
const askedOnce = new Set<string>();

/**
 * [0.0.7 P2] An installed model's image file, on its row in Models → Installed: that it
 * couldn't be read (the model then loads without image input), with "Try it again"; and, for
 * a model with a Hugging Face repository and no image file it can read, what its repository
 * offers ("Download the image file"; another one, or the same one again, when it couldn't be
 * read), with the download's progress and what to do after it. Nothing shows for a model that
 * sees images, or one whose repository is known to have none.
 *
 * What the repository offers was saved on the model's row when it was downloaded, so the row
 * asks Hugging Face for nothing (P2 review M2). For a model downloaded before 0.0.7, "Look for
 * an image file" asks when clicked; the row asks by itself only once, for a model whose image
 * file couldn't be read.
 */
export function ImageFileLine(props: { model: InstalledModel; onChanged: () => void }) {
  const { model: m, onChanged } = props;
  const unreadable = !!m.imageFileUnreadable;
  const wanted = !!m.repo && !seesImages(m);
  // What was asked for on this row, over what was saved with the model.
  const [looked, setLooked] = useState<ImageFileOffer | null>(null);
  const offer = looked ?? m.imageFileOffer ?? null;
  const [looking, setLooking] = useState(false);
  const [download, setDownload] = useState<{ id: string; done: number; total: number } | null>(null);
  const [finished, setFinished] = useState(false);
  const [triedAgain, setTriedAgain] = useState(false);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const runtime = useApp((s) => s.runtime);

  async function look() {
    setError(null);
    setLooking(true);
    try {
      setLooked(await ipc.imageFileOffer(m.path));
    } catch (err) {
      setError(toErrorInfo(err).message);
    } finally {
      setLooking(false);
    }
  }

  // Right after its image file couldn't be read, another one is looked for once by itself
  // (offline, nothing is said: the link stays).
  const askNow = wanted && unreadable && m.imageFileOffer == null && !askedOnce.has(m.path);
  useEffect(() => {
    if (!askNow) return;
    askedOnce.add(m.path);
    let alive = true;
    ipc
      .imageFileOffer(m.path)
      .then((o) => alive && setLooked(o))
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [askNow, m.path]);

  // The download's progress, until it is done or stops.
  const downloadId = download?.id ?? null;
  useEffect(() => {
    if (!downloadId) return;
    let alive = true;
    let stop: (() => void) | null = null;
    const onProgress = (g: DownloadGroup) => {
      if (!alive || g.id !== downloadId) return;
      if (g.status === "done") {
        setDownload(null);
        setFinished(true);
        onChanged();
      } else if (g.status === "error" || g.status === "cancelled") {
        setDownload(null);
        setError(g.error?.message ?? "The download stopped. Try again.");
      } else {
        setDownload({ id: g.id, done: g.bytesDone, total: g.bytesTotal });
      }
    };
    void ipc
      .onDownloadProgress(onProgress)
      .then((unlisten) => {
        if (alive) stop = unlisten;
        else unlisten();
      })
      .catch(() => undefined);
    return () => {
      alive = false;
      stop?.();
    };
  }, [downloadId, onChanged]);

  async function start(size: number) {
    setError(null);
    setStarting(true);
    try {
      const id = await ipc.downloadImageFile(m.path);
      setDownload({ id, done: 0, total: size });
    } catch (err) {
      setError(toErrorInfo(err).message);
    } finally {
      setStarting(false);
    }
  }

  async function tryAgain() {
    setError(null);
    try {
      await ipc.forgetUnreadableImageFile(m.path);
      setTriedAgain(true);
      onChanged();
    } catch (err) {
      setError(toErrorInfo(err).message);
    }
  }

  const loadedNow = isLoadedNow(runtime, m.path);
  const reload = () => void reloadModel(useApp.getState().runtime, true).catch((err) => setError(toErrorInfo(err).message));
  const reloadButton = loadedNow && (
    <button type="button" onClick={reload} className="underline hover:text-fg">
      Reload
    </button>
  );
  const link = "flex items-center gap-1 underline hover:text-fg disabled:opacity-50";

  const lines: ReactNode[] = [];
  if (unreadable && !finished && !triedAgain) {
    lines.push(
      <span key="unreadable" className="text-warning">
        {IMAGE_FILE_WORDS.unreadable}
      </span>,
      <button key="try" type="button" onClick={() => void tryAgain()} className={link}>
        {IMAGE_FILE_WORDS.tryAgain}
      </button>,
    );
  }
  if (finished) {
    lines.push(
      <span key="done" className="flex items-center gap-1.5">
        {loadedNow ? IMAGE_FILE_WORDS.done : IMAGE_FILE_WORDS.doneNextLoad}
        {reloadButton}
      </span>,
    );
  } else if (triedAgain) {
    lines.push(
      <span key="tried" className="flex items-center gap-1.5">
        {IMAGE_FILE_WORDS.triedAgain}
        {reloadButton}
      </span>,
    );
  } else if (download) {
    lines.push(
      <span key="progress" role="status">
        {imageFileProgress(download.done, download.total)}
      </span>,
    );
  } else if (wanted && (offer?.kind === "available" || offer?.kind === "again")) {
    const label =
      offer.kind === "again"
        ? IMAGE_FILE_WORDS.downloadAgain
        : unreadable
          ? IMAGE_FILE_WORDS.downloadAnother
          : IMAGE_FILE_WORDS.download;
    lines.push(
      <button
        key="download"
        type="button"
        disabled={starting}
        onClick={() => void start(offer.sizeBytes)}
        title={visiblePath(offer.file)}
        className={link}
      >
        <Download size={12} aria-hidden /> {label} ({formatBytes(offer.sizeBytes)})
      </button>,
    );
  } else if (wanted && offer?.kind === "noOther" && unreadable) {
    lines.push(<span key="none">{IMAGE_FILE_WORDS.noOther}</span>);
  } else if (wanted && offer?.kind === "none" && (unreadable || looked !== null)) {
    lines.push(<span key="none">{IMAGE_FILE_WORDS.none}</span>);
  } else if (wanted && offer === null) {
    lines.push(
      looking ? (
        <span key="looking" role="status">
          {IMAGE_FILE_WORDS.looking}
        </span>
      ) : (
        <button key="look" type="button" onClick={() => void look()} className={link}>
          {IMAGE_FILE_WORDS.look}
        </button>
      ),
    );
  }
  if (error) {
    lines.push(
      <span key="error" className="text-danger" role="alert">
        {error}
      </span>,
    );
  }
  if (lines.length === 0) return null;
  return <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-fg-muted">{lines}</div>;
}

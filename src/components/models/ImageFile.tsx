import { useEffect, useState, type ReactNode } from "react";
import { Download } from "lucide-react";
import * as ipc from "../../lib/ipc";
import type { DownloadGroup, ImageFileOffer, InstalledModel } from "../../lib/types";
import { formatBytes } from "../../lib/format";
import { IMAGE_FILE_WORDS, imageFileProgress } from "../../lib/words";
import { toErrorInfo, useApp } from "../../store/app";
import { isLoadedNow, reloadModel } from "../../store/modelChange";

/** Whether the model loads with an image file it can read (the "vision" badge). */
export function seesImages(m: InstalledModel): boolean {
  return !!m.projectorPath && !m.imageFileUnreadable;
}

/**
 * [0.0.7 P2] An installed model's image file, on its row in Models → Installed: that it
 * couldn't be read (the model then loads without image input), and, for a model with a
 * Hugging Face repository and no image file it can read, the one its repository offers
 * ("Download the image file"; another one when it couldn't be read), with the download's
 * progress and what to do after it. Nothing shows for a model that sees images, or one
 * whose repository has none. The repository is asked once per row (the backend reuses its
 * listing for a while); offline, nothing is offered.
 */
export function ImageFileLine(props: { model: InstalledModel; onChanged: () => void }) {
  const { model: m, onChanged } = props;
  const unreadable = !!m.imageFileUnreadable;
  const wanted = !!m.repo && !seesImages(m);
  const [offer, setOffer] = useState<ImageFileOffer | null>(null);
  const [download, setDownload] = useState<{ id: string; done: number; total: number } | null>(null);
  const [finished, setFinished] = useState(false);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const runtime = useApp((s) => s.runtime);

  useEffect(() => {
    if (!wanted) return;
    let alive = true;
    ipc
      .imageFileOffer(m.path)
      .then((o) => alive && setOffer(o))
      .catch(() => alive && setOffer(null));
    return () => {
      alive = false;
    };
  }, [m.path, wanted, unreadable]);

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

  const reload = () => void reloadModel(useApp.getState().runtime, true).catch((err) => setError(toErrorInfo(err).message));

  const lines: ReactNode[] = [];
  if (unreadable && !finished) {
    lines.push(
      <span key="unreadable" className="text-warning">
        {IMAGE_FILE_WORDS.unreadable}
      </span>,
    );
  }
  if (finished) {
    lines.push(
      <span key="done" className="flex items-center gap-1.5">
        {IMAGE_FILE_WORDS.done}
        {isLoadedNow(runtime, m.path) && (
          <button type="button" onClick={reload} className="underline hover:text-fg">
            Reload
          </button>
        )}
      </span>,
    );
  } else if (download) {
    lines.push(
      <span key="progress" role="status">
        {imageFileProgress(download.done, download.total)}
      </span>,
    );
  } else if (wanted && offer?.kind === "available") {
    const label = unreadable ? IMAGE_FILE_WORDS.downloadAnother : IMAGE_FILE_WORDS.download;
    lines.push(
      <button
        key="download"
        type="button"
        disabled={starting}
        onClick={() => void start(offer.sizeBytes)}
        title={offer.file}
        className="flex items-center gap-1 underline hover:text-fg disabled:opacity-50"
      >
        <Download size={12} aria-hidden /> {label} ({formatBytes(offer.sizeBytes)})
      </button>,
    );
  } else if (wanted && unreadable && (offer?.kind === "noOther" || offer?.kind === "none")) {
    lines.push(<span key="none">{IMAGE_FILE_WORDS.noOther}</span>);
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

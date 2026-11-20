import { useEffect, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { Pause, Play, X } from "lucide-react";
import * as ipc from "../../lib/ipc";
import type { DownloadGroup, ErrorInfo } from "../../lib/types";
import { formatBytes } from "../../lib/format";
import { DOWNLOAD_KIND_WORDS, LOCAL_MODELS, downloadProgressWords } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { ErrorBanner } from "../ErrorBanner";
import { visiblePath } from "../FolderChip";
import { Badge } from "./Badge";

/** Models → Downloads: progress, pause, resume and cancel; a picture or video model's
 *  download (0.1.0 Studio) says which it is. */
export function Downloads() {
  const [groups, setGroups] = useState<DownloadGroup[]>([]);
  const [error, setError] = useState<ErrorInfo | null>(null);

  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    ipc
      .listDownloads()
      .then((g) => alive && setGroups(g))
      .catch((err) => alive && setError(toErrorInfo(err)));
    void ipc
      .onDownloadProgress((g) => {
        setGroups((old) => {
          const i = old.findIndex((x) => x.id === g.id);
          if (i < 0) return [g, ...old];
          const copy = old.slice();
          copy[i] = g;
          return copy;
        });
      })
      .then((u) => {
        if (alive) unlisten = u;
        else u();
      });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);

  async function act(f: () => Promise<void>) {
    try {
      await f();
    } catch (err) {
      setError(toErrorInfo(err));
    }
  }

  return (
    <div className="space-y-2">
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      {groups.length === 0 && <p className="text-sm text-fg-muted">No downloads.</p>}
      <ul className="space-y-2">
        {groups.map((g) => {
          const savedPath = g.savedFile?.path;
          const pct = g.bytesTotal > 0 ? Math.min(100, (g.bytesDone / g.bytesTotal) * 100) : 0;
          return (
            <li key={g.id} className="rounded-md border border-border px-3 py-2 text-sm">
              <div className="flex items-center gap-2">
                <div className="min-w-0 flex-1">
                  <div className="flex min-w-0 items-center gap-2">
                    <p className="truncate font-medium">
                      <bdi dir="ltr">
                        {visiblePath(g.repo)} · {visiblePath(g.label)}
                      </bdi>
                    </p>
                    {/* 0.1.0 Studio: a picture or video model's download says so. */}
                    {g.generatorKind && <Badge>{DOWNLOAD_KIND_WORDS[g.generatorKind]}</Badge>}
                  </div>
                  {/* 0.1.0 Studio: a picture or video model's row in Get local models says it too. */}
                  <p className="text-xs text-fg-muted">{downloadProgressWords(g, formatBytes)}</p>
                </div>
                {g.status === "done" && savedPath && (
                  <button type="button" className="text-accent underline" onClick={() => void act(() => revealItemInDir(savedPath))}>
                    {LOCAL_MODELS.showInFolder}
                  </button>
                )}
                {g.status === "downloading" || g.status === "queued" ? (
                  <button type="button" aria-label="Pause" onClick={() => void act(() => ipc.pauseDownload(g.id))}>
                    <Pause size={14} aria-hidden />
                  </button>
                ) : null}
                {g.status === "paused" || g.status === "error" ? (
                  <button type="button" aria-label="Resume" onClick={() => void act(() => ipc.resumeDownload(g.id))}>
                    <Play size={14} aria-hidden />
                  </button>
                ) : null}
                {g.status !== "done" && g.status !== "cancelled" ? (
                  <button
                    type="button"
                    aria-label="Cancel"
                    onClick={() => void act(() => ipc.cancelDownload(g.id))}
                    className="text-fg-muted hover:text-danger"
                  >
                    <X size={14} aria-hidden />
                  </button>
                ) : null}
              </div>
              <div
                className="mt-2 h-1.5 overflow-hidden rounded bg-bg-muted"
                role="progressbar"
                aria-valuenow={Math.round(pct)}
                aria-valuemin={0}
                aria-valuemax={100}
              >
                <div className="h-full bg-accent" style={{ width: `${pct}%` }} />
              </div>
              {g.error && (
                <p className="mt-1 text-xs text-danger">
                  {g.error.message} {g.error.nextStep}
                </p>
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

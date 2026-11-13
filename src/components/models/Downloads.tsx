import { useEffect, useState } from "react";
import { Pause, Play, X } from "lucide-react";
import * as ipc from "../../lib/ipc";
import type { DownloadGroup, ErrorInfo } from "../../lib/types";
import { formatBytes, formatEta } from "../../lib/format";
import { toErrorInfo } from "../../store/app";
import { ErrorBanner } from "../ErrorBanner";
import { visiblePath } from "../FolderChip";

/** Models → Downloads: progress, pause, resume and cancel. */
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
          const pct = g.bytesTotal > 0 ? Math.min(100, (g.bytesDone / g.bytesTotal) * 100) : 0;
          const eta = g.status === "downloading" ? formatEta(g.bytesTotal - g.bytesDone, g.bytesPerSecond) : null;
          return (
            <li key={g.id} className="rounded-md border border-border px-3 py-2 text-sm">
              <div className="flex items-center gap-2">
                <div className="min-w-0 flex-1">
                  <p className="truncate font-medium">
                    <bdi dir="ltr">
                      {visiblePath(g.repo)} · {visiblePath(g.label)}
                    </bdi>
                  </p>
                  <p className="text-xs text-fg-muted">
                    {formatBytes(g.bytesDone)} of {formatBytes(g.bytesTotal)} · {g.status}
                    {eta ? ` · ${eta}` : ""}
                    {g.unverified ? " · no published checksum" : ""}
                  </p>
                </div>
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

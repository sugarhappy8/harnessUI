import { useCallback, useEffect, useState } from "react";
import { AlertDialog } from "radix-ui";
import { Download, ExternalLink, Trash2, X } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import * as ipc from "../lib/ipc";
import type { ErrorInfo, PackInfo, PackProgress } from "../lib/types";
import { formatBytes } from "../lib/format";
import { toErrorInfo } from "../store/app";
import { ErrorBanner } from "./ErrorBanner";

/**
 * CUDA and ROCm as optional, verified downloads (decisions D1, D2, D4). Each file is
 * pinned by SHA-256 in the app and checked before the backend is used.
 */
export function BackendDownloads(props: { onChanged: () => void }) {
  const [packs, setPacks] = useState<PackInfo[]>([]);
  const [progress, setProgress] = useState<Record<string, PackProgress>>({});
  const [confirm, setConfirm] = useState<PackInfo | null>(null);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const { onChanged } = props;

  const load = useCallback(() => {
    return ipc
      .listPacks()
      .then(setPacks)
      .catch((err) => setError(toErrorInfo(err)));
  }, []);

  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    ipc
      .listPacks()
      .then((p) => alive && setPacks(p))
      .catch((err) => alive && setError(toErrorInfo(err)));
    void ipc
      .onPackProgress((p) => {
        setProgress((old) => ({ ...old, [p.id]: p }));
        if (p.finished) {
          if (p.error) setError(p.error);
          void ipc.listPacks().then((list) => alive && setPacks(list));
          onChanged();
        }
      })
      .then((u) => {
        if (alive) unlisten = u;
        else u();
      });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, [onChanged]);

  async function start(p: PackInfo) {
    setConfirm(null);
    try {
      await ipc.downloadPack(p.id, true);
      setProgress((old) => ({ ...old, [p.id]: { id: p.id, done: p.bytesDone, total: p.downloadBytes, unpacking: false, finished: false, error: null } }));
      await load();
    } catch (err) {
      setError(toErrorInfo(err));
    }
  }

  async function remove(p: PackInfo) {
    if (!window.confirm(`Remove the ${p.title} backend? You can download it again later.`)) return;
    try {
      await ipc.removePack(p.id);
      await load();
      onChanged();
    } catch (err) {
      setError(toErrorInfo(err));
    }
  }

  if (packs.length === 0) return null;
  return (
    <section>
      <h3 className="mb-1 text-sm font-medium">Backend downloads</h3>
      <p className="mb-2 text-xs text-fg-muted">
        Optional GPU backends, downloaded from llama.cpp's GitHub releases and checked against
        checksums built into the app before they are used.
      </p>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <ul className="space-y-2">
        {packs.map((p) => {
          const pr = progress[p.id];
          const running = p.state === "downloading" || (pr && !pr.finished);
          const pct = pr && pr.total > 0 ? Math.min(100, (pr.done / pr.total) * 100) : 0;
          return (
            <li key={p.id} className="rounded-md border border-border px-3 py-2 text-sm">
              <div className="flex items-center gap-2">
                <div className="min-w-0 flex-1">
                  <p className="font-medium">{p.title}</p>
                  <p className="text-xs text-fg-muted">
                    {p.requirements} · {formatBytes(p.downloadBytes)}
                  </p>
                  {p.reason && <p className="text-xs text-fg-muted">{p.reason}</p>}
                </div>
                {p.state === "installed" ? (
                  <button
                    type="button"
                    aria-label={`Remove ${p.title}`}
                    onClick={() => void remove(p)}
                    className="flex items-center gap-1 text-xs text-fg-muted hover:text-danger"
                  >
                    <Trash2 size={13} aria-hidden /> Remove
                  </button>
                ) : running ? (
                  <button
                    type="button"
                    onClick={() => void ipc.cancelPackDownload(p.id)}
                    className="flex items-center gap-1 text-xs text-fg-muted hover:text-danger"
                  >
                    <X size={13} aria-hidden /> Cancel
                  </button>
                ) : (
                  <button
                    type="button"
                    disabled={!p.available}
                    onClick={() => setConfirm(p)}
                    className="flex items-center gap-1 rounded-md bg-accent px-3 py-1 text-xs text-accent-fg disabled:opacity-40"
                  >
                    <Download size={12} aria-hidden /> {p.state === "partial" ? "Resume" : "Download"}
                  </button>
                )}
              </div>
              {running && (
                <div className="mt-2">
                  <div
                    className="h-1.5 overflow-hidden rounded bg-bg-muted"
                    role="progressbar"
                    aria-valuenow={Math.round(pct)}
                    aria-valuemin={0}
                    aria-valuemax={100}
                  >
                    <div className="h-full bg-accent" style={{ width: `${pct}%` }} />
                  </div>
                  <p className="mt-1 text-xs text-fg-muted">
                    {pr?.unpacking ? "Verifying and unpacking…" : `${formatBytes(pr?.done ?? 0)} of ${formatBytes(p.downloadBytes)}`}
                  </p>
                </div>
              )}
            </li>
          );
        })}
      </ul>
      <AlertDialog.Root open={confirm !== null} onOpenChange={(o) => !o && setConfirm(null)}>
        <AlertDialog.Portal>
          <AlertDialog.Overlay className="dialog-overlay fixed inset-0" />
          <AlertDialog.Content className="fixed top-1/2 left-1/2 w-[30rem] max-w-[92vw] -translate-x-1/2 -translate-y-1/2 rounded-2xl border border-border bg-surface p-5 shadow-pop">
            <AlertDialog.Title className="text-base font-semibold">Download {confirm?.title}?</AlertDialog.Title>
            <AlertDialog.Description className="mt-2 text-sm text-fg-muted">
              {formatBytes(confirm?.downloadBytes)} from GitHub. This backend includes software under:{" "}
              <span className="text-fg">{confirm?.licenseName}</span>. Downloading means you accept these terms.
            </AlertDialog.Description>
            {confirm && (
              <button
                type="button"
                className="mt-2 flex items-center gap-1 text-xs underline"
                onClick={() => void openUrl(confirm.licenseUrl)}
              >
                Read the license <ExternalLink size={11} aria-hidden />
              </button>
            )}
            <div className="mt-4 flex justify-end gap-2 text-sm">
              <AlertDialog.Cancel className="rounded-md px-3 py-1.5 hover:bg-bg-muted">Cancel</AlertDialog.Cancel>
              <AlertDialog.Action
                className="rounded-md bg-accent px-3 py-1.5 text-accent-fg"
                onClick={() => confirm && void start(confirm)}
              >
                Accept and download
              </AlertDialog.Action>
            </div>
          </AlertDialog.Content>
        </AlertDialog.Portal>
      </AlertDialog.Root>
    </section>
  );
}

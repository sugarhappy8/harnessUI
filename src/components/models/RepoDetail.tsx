import { useEffect, useState } from "react";
import { ArrowLeft, Check, Download, ExternalLink, Lock } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import * as ipc from "../../lib/ipc";
import type { ErrorInfo, RepoView, VariantView } from "../../lib/types";
import { formatBytes, formatParams } from "../../lib/format";
import { toErrorInfo } from "../../store/app";
import { ErrorBanner } from "../ErrorBanner";
import { FitBadge } from "../FitBadge";
import { Badge } from "./Badge";

/** One Hugging Face repository: its files with a fit badge each, and Download. */
export function RepoDetail(props: { repoId: string; onBack: () => void; onStarted: () => void }) {
  const [view, setView] = useState<RepoView | null>(null);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const [withProjector, setWithProjector] = useState(true);
  const [showCard, setShowCard] = useState(false);

  useEffect(() => {
    let alive = true;
    ipc
      .hubRepo(props.repoId, null)
      .then((v) => alive && setView(v))
      .catch((err) => alive && setError(toErrorInfo(err)));
    return () => {
      alive = false;
    };
  }, [props.repoId]);

  async function download(v: VariantView) {
    if (!view) return;
    try {
      await ipc.startDownload(view.id, view.commit, v.label, withProjector && !!view.projector);
      props.onStarted();
    } catch (err) {
      setError(toErrorInfo(err));
    }
  }

  const hfUrl = `https://huggingface.co/${props.repoId}`;
  return (
    <div className="space-y-3">
      <button type="button" onClick={props.onBack} className="flex items-center gap-1 text-xs text-fg-muted hover:text-fg">
        <ArrowLeft size={12} aria-hidden /> Back to results
      </button>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <h3 className="truncate text-base font-semibold">{props.repoId}</h3>
          {view && (
            <div className="mt-1 flex flex-wrap gap-1">
              {view.architecture && <Badge>{view.architecture}</Badge>}
              {formatParams(view.parameters) && <Badge>{formatParams(view.parameters)}</Badge>}
              {view.contextLength && <Badge>{(view.contextLength / 1024).toFixed(0)}k context</Badge>}
              {view.templateTools && <Badge>tools</Badge>}
              {view.templateThinking && <Badge>thinking</Badge>}
              {view.projector && <Badge>vision</Badge>}
              {view.license && <Badge>{view.license}</Badge>}
              {view.gated !== "no" && (
                <Badge title="You need a Hugging Face token and must accept the terms on the model page">
                  <Lock size={10} className="mr-0.5 inline" aria-hidden /> gated
                </Badge>
              )}
            </div>
          )}
        </div>
        <button type="button" onClick={() => void openUrl(hfUrl)} className="flex shrink-0 items-center gap-1 text-xs underline">
          Open on Hugging Face <ExternalLink size={11} aria-hidden />
        </button>
      </div>
      {view?.architectureSupported === false && (
        <p className="rounded-md border border-danger/40 bg-danger/10 px-3 py-2 text-sm">
          The llama.cpp build in this app does not know the “{view.architecture}” architecture, so
          these files would not load. Pick a different model.
        </p>
      )}
      {!view && !error && <p className="text-sm text-fg-muted">Loading files…</p>}
      {view && (
        <>
          {view.projector && (
            <label className="flex items-center gap-2 text-xs">
              <input type="checkbox" checked={withProjector} onChange={(e) => setWithProjector(e.target.checked)} />
              Also download the image projector ({formatBytes(view.projector.size)}) so the model can read images
            </label>
          )}
          <p className="text-xs text-fg-muted">
            Memory estimates assume a context of {view.fitContext.toLocaleString()} tokens.
          </p>
          <ul className="space-y-2">
            {view.variants.map((v) => (
              <li key={v.label} className="flex items-center gap-3 rounded-md border border-border px-3 py-2 text-sm">
                <div className="min-w-0 flex-1">
                  <p className="font-medium">
                    {v.label}
                    <span className="ml-2 text-xs font-normal text-fg-muted">
                      {formatBytes(v.totalSize)}
                      {v.files.length > 1 ? ` · ${v.files.length} parts` : ""}
                    </span>
                  </p>
                  {v.fit && <FitBadge fit={v.fit} />}
                  {!v.hasChecksum && (
                    <p className="text-xs text-fg-muted">No checksum is published for this file.</p>
                  )}
                </div>
                {v.installed ? (
                  <span className="flex items-center gap-1 text-xs text-success">
                    <Check size={12} aria-hidden /> Installed
                  </span>
                ) : (
                  <button
                    type="button"
                    onClick={() => void download(v)}
                    disabled={view.architectureSupported === false}
                    className="flex items-center gap-1 rounded-md bg-accent px-3 py-1 text-xs text-accent-fg disabled:opacity-40"
                  >
                    <Download size={12} aria-hidden /> Download
                  </button>
                )}
              </li>
            ))}
            {view.variants.length === 0 && (
              <li className="text-sm text-fg-muted">This repository has no GGUF model files.</li>
            )}
          </ul>
          {view.incomplete.length > 0 && (
            <p className="text-xs text-fg-muted">
              Not shown because parts are missing: {view.incomplete.join(", ")}
            </p>
          )}
          {view.card && (
            <div>
              <button type="button" className="text-xs underline" onClick={() => setShowCard((s) => !s)}>
                {showCard ? "Hide model card" : "Show model card"}
              </button>
              {showCard && (
                <pre className="mt-2 max-h-64 overflow-auto rounded-md bg-bg-muted p-3 text-xs whitespace-pre-wrap">
                  {view.card}
                </pre>
              )}
            </div>
          )}
        </>
      )}
    </div>
  );
}

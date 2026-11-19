import { useEffect, useState, type ReactNode } from "react";
import type { ErrorInfo } from "../../lib/types";
import { downloadAmount, engineDownloadWords, engineSizeWords, ENGINE_LINE } from "../../lib/words";
import { listenInEffect } from "../../lib/listen";
import { toErrorInfo } from "../../store/app";
import { useStudio } from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import * as api from "./api";
import { ConfirmDialog } from "./ConfirmDialog";
import type { EngineProgress, EngineStatus } from "./types";

// [0.1.0 Studio, Task B10] The Studio engine's line, in Settings → Studio and in Get local
// models' footer (Task B13): built in on a Mac; on Windows the pack that comes with the first
// local model, the pack missing or partly there (*Download the Studio engine (29 MB, MIT)* once
// a local model is installed: antivirus quarantine, *Remove*, or a failed engine download while
// the model finished), its download as `studio-engine-progress` says it goes, or installed with
// *Remove* (which asks first). Every download goes through the store's `downloadEngine`. The
// engine is read again after a removal (no `finished` event follows one), when a download
// finishes, and when one could not start.

const LINK = "text-sm underline hover:text-fg disabled:opacity-60";

/** What the line says while the pack downloads: how far `progress` got, else what the engine's
 *  state says is on disk. */
function downloadingWords(engine: EngineStatus, progress: EngineProgress | null): string {
  if (progress?.unpacking) return ENGINE_LINE.unpacking;
  const done = progress ? progress.done : (engine.bytesDone ?? 0);
  const total = progress ? progress.total : engine.downloadBytes;
  return done > 0 && total > 0
    ? ENGINE_LINE.downloading(downloadAmount(done), downloadAmount(total))
    : ENGINE_LINE.downloading(null, null);
}

/** The Studio engine on this computer. `hasModel`: a local model is installed (a missing pack
 *  is then offered for download; else it comes with the first model). `removable`: an
 *  installed pack offers *Remove*. */
export function EngineLine(props: { engine: EngineStatus; hasModel: boolean; removable: boolean }) {
  const { engine, hasModel, removable } = props;
  const [progress, setProgress] = useState<EngineProgress | null>(null);
  const [busy, setBusy] = useState(false);
  const [asking, setAsking] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);

  // How the pack's download goes, whoever started it (this line, or the first local model).
  useEffect(
    () =>
      listenInEffect(() =>
        api.onEngineProgress((p) => {
          if (!p.finished) {
            setProgress(p);
            return;
          }
          setProgress(null);
          if (p.error) setError(p.error);
          void useStudio.getState().refreshEngine();
        }),
      ),
    [],
  );

  async function download() {
    setBusy(true);
    setError(null);
    try {
      await useStudio.getState().downloadEngine();
    } catch (e) {
      setError(toErrorInfo(e));
      // It never started, so no `finished` event comes.
      await useStudio.getState().refreshEngine();
    } finally {
      setBusy(false);
    }
  }

  async function remove() {
    setBusy(true);
    setError(null);
    try {
      await api.studioEngineRemove();
    } catch (e) {
      setError(toErrorInfo(e));
    } finally {
      // No `finished` event follows a removal, and while one runs the state reads downloading.
      await useStudio.getState().refreshEngine();
      setBusy(false);
    }
  }

  const missing = engine.state === "notInstalled" || engine.state === "partial";
  const size = engineSizeWords(engine.downloadBytes, engine.licenseName);
  let line: ReactNode;
  if (progress !== null || engine.state === "downloading") {
    line = <span>{downloadingWords(engine, progress)}</span>;
  } else if (engine.state === "builtIn") {
    line = <span>{ENGINE_LINE.builtIn}</span>;
  } else if (engine.state === "installed") {
    line = (
      <>
        <span>{ENGINE_LINE.installed(engine.build)}</span>
        {removable && (
          <>
            <span aria-hidden>{ENGINE_LINE.separator}</span>
            <button type="button" className={LINK} disabled={busy} onClick={() => setAsking(true)}>
              {ENGINE_LINE.remove}
            </button>
          </>
        )}
      </>
    );
  } else if (missing && hasModel) {
    line = (
      <>
        <span>{engine.state === "partial" ? ENGINE_LINE.partial : ENGINE_LINE.missing}</span>
        <button
          type="button"
          title={engine.licenseName}
          disabled={busy}
          onClick={() => void download()}
          className="rounded-md border border-border px-3 py-1 text-sm hover:bg-bg-muted disabled:opacity-60"
        >
          {engineDownloadWords(engine.downloadBytes, engine.licenseName)}
        </button>
      </>
    );
  } else if (missing) {
    line = <span title={engine.licenseName}>{ENGINE_LINE.withFirstModel(size)}</span>;
  } else {
    line = <span>{ENGINE_LINE.unavailable}</span>;
  }

  return (
    <div className="space-y-2">
      <div className="flex flex-wrap items-center gap-2 text-sm">{line}</div>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <ConfirmDialog
        open={asking}
        onOpenChange={setAsking}
        title={ENGINE_LINE.removeTitle}
        body={ENGINE_LINE.removeBody(downloadAmount(engine.downloadBytes))}
        cancelLabel={ENGINE_LINE.cancel}
        actionLabel={ENGINE_LINE.remove}
        destructive
        onConfirm={() => void remove()}
      />
    </div>
  );
}

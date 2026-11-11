import { useEffect, useState } from "react";
import { Cpu, Download, FileUp, Globe } from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import * as ipc from "../lib/ipc";
import type { CatalogSuggestion, ErrorInfo, FirstRunInfo } from "../lib/types";
import { formatBytes } from "../lib/format";
import { APP_NAME } from "../lib/app";
import { toErrorInfo } from "../store/app";
import { ErrorBanner } from "./ErrorBanner";

function hardwareLine(info: FirstRunInfo): string {
  const h = info.hardware;
  if (h.target === "gpu" && h.gpuName) {
    return `${h.gpuName} with ${formatBytes(h.gpuFreeBytes)} free${h.backend ? ` (${h.backend})` : ""}.`;
  }
  if (h.target === "apple") {
    return `Apple silicon; the GPU can use about ${formatBytes(h.usableBytes)}.`;
  }
  // A Mac with an Intel processor: its graphics are unreliable for this, so the processor is
  // used. Not a missing GPU, so the line does not say one was not found.
  if (h.intelMac) {
    return `Intel Mac: models run on the processor, with ${formatBytes(h.ramAvailableBytes)} of free memory.`;
  }
  return `No supported GPU found; models will run on the CPU with ${formatBytes(h.ramAvailableBytes)} of free memory.`;
}

/**
 * First run (ARCHITECTURE.md 6.3): detect the hardware, suggest a model that fits,
 * and let the user download it, import a file, or connect an endpoint instead.
 */
export function FirstRun(props: { onDone: () => void; onOpenEndpoints: () => void; onOpenModels: () => void }) {
  const [info, setInfo] = useState<FirstRunInfo | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [showAll, setShowAll] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);

  useEffect(() => {
    let alive = true;
    ipc
      .firstRunInfo()
      .then((i) => {
        if (!alive) return;
        setInfo(i);
        // Only a tested pick is preselected; an untested entry never is (ARCHITECTURE.md 11).
        setSelected(i.pick ?? null);
      })
      .catch((err) => alive && setError(toErrorInfo(err)));
    return () => {
      alive = false;
    };
  }, []);

  async function finish(action: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await action();
      await ipc.completeFirstRun();
      props.onDone();
    } catch (err) {
      setError(toErrorInfo(err));
    } finally {
      setBusy(false);
    }
  }

  async function importFile() {
    const picked = await openDialog({
      multiple: false,
      directory: false,
      filters: [{ name: "GGUF model", extensions: ["gguf"] }],
    });
    if (typeof picked !== "string") return;
    await finish(async () => {
      const m = await ipc.importModel(picked);
      const settings = await ipc.getLocalModelSettings();
      await ipc.loadLocalModel(m.path, settings);
    });
  }

  const list: CatalogSuggestion[] = info
    ? showAll
      ? info.suggestions
      : info.suggestions.filter((s) => s.fits).slice(0, 4)
    : [];
  const chosen = info?.suggestions.find((s) => s.id === selected) ?? null;

  return (
    <div className="flex h-full items-center justify-center overflow-y-auto p-6">
      <div className="w-full max-w-2xl space-y-5">
        <div>
          <h1 className="text-xl font-semibold">Welcome to {APP_NAME}</h1>
          <p className="mt-1 text-sm text-fg-muted">
            Everything runs on this computer. Pick a model to download, or use one you already have.
          </p>
        </div>
        {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
        {info && (
          <p className="flex items-center gap-2 text-sm">
            <Cpu size={16} aria-hidden className="text-fg-muted" /> {hardwareLine(info)}
          </p>
        )}
        {info && (
          <fieldset className="space-y-2">
            <legend className="mb-1 text-sm font-medium">Suggested models</legend>
            {list.map((s) => (
              <label
                key={s.id}
                className={
                  "flex cursor-pointer items-start gap-3 rounded-md border px-3 py-2 text-sm " +
                  (selected === s.id ? "border-accent" : "border-border")
                }
              >
                <input
                  type="radio"
                  name="first-run-model"
                  className="mt-1"
                  checked={selected === s.id}
                  onChange={() => setSelected(s.id)}
                />
                <span className="min-w-0 flex-1">
                  <span className="font-medium">{s.name}</span>
                  <span className="ml-2 text-xs text-fg-muted">
                    {s.quant} · {formatBytes(s.totalBytes)} download · {s.license}
                  </span>
                  <span className="mt-0.5 block text-xs text-fg-muted">
                    {s.capabilities.join(", ")}
                    {" · "}
                    {s.fits ? "fits this computer" : `needs about ${formatBytes(s.memoryNeeded)}`}
                    {!s.tested && " · not yet tested on real hardware"}
                    {s.installed && " · already downloaded"}
                  </span>
                </span>
              </label>
            ))}
            {list.length === 0 && (
              <p className="text-sm text-fg-muted">
                None of the suggested models fits the free memory on this computer. You can still
                pick one from the full list, import a smaller GGUF, or connect an endpoint.
              </p>
            )}
            <button type="button" className="text-xs underline" onClick={() => setShowAll((v) => !v)}>
              {showAll ? "Show only models that fit" : "Show all suggestions"}
            </button>
          </fieldset>
        )}
        {info && !info.pick && info.suggestions.some((s) => s.fits) && (
          <p className="text-xs text-fg-muted">
            No suggestion has passed the tool-calling test on real hardware yet, so none is
            preselected automatically. Choose one above to download it.
          </p>
        )}
        <div className="flex flex-wrap gap-2">
          <button
            type="button"
            disabled={!chosen || busy}
            onClick={() =>
              void finish(async () => {
                if (chosen && !chosen.installed) await ipc.startCatalogDownload(chosen.id);
                props.onOpenModels();
              })
            }
            className="flex items-center gap-1.5 rounded-md bg-accent px-4 py-2 text-sm text-accent-fg disabled:opacity-40"
          >
            <Download size={14} aria-hidden />
            {chosen ? (chosen.installed ? `Use ${chosen.name}` : `Download ${chosen.name}`) : "Download"}
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={() => void importFile()}
            className="flex items-center gap-1.5 rounded-md border border-border px-4 py-2 text-sm hover:bg-bg-muted"
          >
            <FileUp size={14} aria-hidden /> I have a GGUF file
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={() => void finish(async () => props.onOpenEndpoints())}
            className="flex items-center gap-1.5 rounded-md border border-border px-4 py-2 text-sm hover:bg-bg-muted"
          >
            <Globe size={14} aria-hidden /> Use Ollama, LM Studio or another server
          </button>
        </div>
        <button
          type="button"
          className="text-xs text-fg-muted underline"
          onClick={() => void finish(async () => undefined)}
        >
          Skip for now
        </button>
      </div>
    </div>
  );
}

import { useCallback, useEffect, useState } from "react";
import { FolderOpen, Trash2 } from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import * as ipc from "../../lib/ipc";
import type { DiskUsage, ErrorInfo, InstalledModel } from "../../lib/types";
import { formatBytes } from "../../lib/format";
import { toErrorInfo, useApp } from "../../store/app";
import { ErrorBanner } from "../ErrorBanner";
import { Badge } from "./Badge";

/** Models → Installed: the models on this computer, with Use and Delete. */
export function Installed(props: { onClose: () => void }) {
  const [models, setModels] = useState<InstalledModel[] | null>(null);
  const [usage, setUsage] = useState<DiskUsage | null>(null);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const refreshModelOptions = useApp((s) => s.refreshModelOptions);
  const setChatModel = useApp((s) => s.setChatModel);
  const showError = useApp((s) => s.showError);

  async function use(m: InstalledModel) {
    try {
      const settings = await ipc.getLocalModelSettings();
      // The chat gets the model first, so the header shows it loading.
      await setChatModel(`local:${m.path}`);
      props.onClose();
      await ipc.loadLocalModel(m.path, settings);
      await refreshModelOptions();
    } catch (err) {
      showError(toErrorInfo(err));
    }
  }

  const load = useCallback(() => {
    return Promise.all([ipc.listInstalledModels(), ipc.diskUsage()])
      .then(([m, u]) => {
        setModels(m);
        setUsage(u);
      })
      .catch((err) => setError(toErrorInfo(err)));
  }, []);
  useEffect(() => {
    let alive = true;
    Promise.all([ipc.listInstalledModels(), ipc.diskUsage()])
      .then(([m, u]) => {
        if (!alive) return;
        setModels(m);
        setUsage(u);
      })
      .catch((err) => alive && setError(toErrorInfo(err)));
    return () => {
      alive = false;
    };
  }, []);

  async function importFile() {
    const picked = await openDialog({
      multiple: false,
      directory: false,
      filters: [{ name: "GGUF model", extensions: ["gguf"] }],
    });
    if (typeof picked !== "string") return;
    try {
      await ipc.importModel(picked);
      await load();
    } catch (err) {
      setError(toErrorInfo(err));
    }
  }

  async function remove(m: InstalledModel) {
    const files = m.source === "download";
    const question = files
      ? `Delete ${m.fileName}? This removes ${formatBytes(m.sizeBytes)} from disk.`
      : `Remove ${m.fileName} from the list? The file itself stays where it is.`;
    if (!window.confirm(question)) return;
    try {
      await ipc.deleteModel(m.id, files);
      await load();
    } catch (err) {
      setError(toErrorInfo(err));
    }
  }

  async function chooseFolder() {
    const picked = await openDialog({ directory: true, multiple: false });
    if (typeof picked !== "string") return;
    try {
      await ipc.setModelsDir(picked);
      await load();
    } catch (err) {
      setError(toErrorInfo(err));
    }
  }

  return (
    <div className="space-y-3">
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <div className="flex items-center justify-between text-xs text-fg-muted">
        <span className="truncate" title={usage?.modelsDir}>
          {usage ? `${formatBytes(usage.totalBytes)} in ${usage.modelsDir}` : ""}
        </span>
        <div className="flex shrink-0 gap-3">
          <button type="button" className="flex items-center gap-1 underline" onClick={() => void chooseFolder()}>
            <FolderOpen size={12} aria-hidden /> Change folder
          </button>
          <button type="button" className="underline" onClick={() => void importFile()}>
            Import a GGUF file…
          </button>
        </div>
      </div>
      <ul className="space-y-2">
        {(models ?? []).map((m) => (
          <li key={m.id} className="flex items-center gap-3 rounded-md border border-border px-3 py-2 text-sm">
            <div className="min-w-0 flex-1">
              <p className="truncate font-medium" title={m.path}>
                {m.fileName}
              </p>
              <div className="mt-1 flex flex-wrap gap-1">
                {m.repo && <Badge>{m.repo}</Badge>}
                <Badge>{formatBytes(m.sizeBytes)}</Badge>
                {m.summary?.quantization && <Badge>{m.summary.quantization}</Badge>}
                {m.summary?.contextLength && (
                  <Badge title="Context length the model was trained for">
                    {(m.summary.contextLength / 1024).toFixed(0)}k context
                  </Badge>
                )}
                {m.summary?.templateTools && <Badge>tools</Badge>}
                {m.summary?.templateThinking && <Badge>thinking</Badge>}
                {m.projectorPath && <Badge>vision</Badge>}
                {m.summary?.isMoe && <Badge>MoE</Badge>}
                {m.source === "import" && <Badge>imported</Badge>}
                {m.architectureSupported === false && (
                  <span className="text-xs text-danger">
                    The bundled llama.cpp cannot load {m.summary?.architecture ?? "this model"}.
                  </span>
                )}
              </div>
            </div>
            <button
              type="button"
              onClick={() => void use(m)}
              disabled={m.architectureSupported === false}
              className="rounded-md bg-accent px-3 py-1 text-xs text-accent-fg disabled:opacity-40"
            >
              Use
            </button>
            <button
              type="button"
              aria-label={`Delete ${m.fileName}`}
              onClick={() => void remove(m)}
              className="text-fg-muted hover:text-danger"
            >
              <Trash2 size={14} aria-hidden />
            </button>
          </li>
        ))}
        {models?.length === 0 && (
          <li className="text-sm text-fg-muted">
            No models yet. Find one under Browse, or import a GGUF file you already have.
          </li>
        )}
      </ul>
    </div>
  );
}

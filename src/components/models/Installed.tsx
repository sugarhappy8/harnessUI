import { useCallback, useEffect, useState } from "react";
import { Check, FolderOpen, Plus, Settings2, Trash2 } from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import * as ipc from "../../lib/ipc";
import type { DiskUsage, ErrorInfo, InstalledModel, ModelLoadInfo, Verdict } from "../../lib/types";
import { formatBytes } from "../../lib/format";
import { FIT_WORDS, tokensWithWords } from "../../lib/words";
import { toErrorInfo, useApp } from "../../store/app";
import { useAddToComparison } from "../../store/compare";
import { askTooLarge } from "../../store/modelChange";
import { ErrorBanner } from "../ErrorBanner";
import { ModelSettings } from "../ModelSettings";
import { Badge } from "./Badge";

const DOT: Record<Verdict, string> = { green: "bg-success", yellow: "bg-warning", red: "bg-danger" };

/** Whether Use should ask "Load it anyway?" first. */
function markedTooLarge(info: ModelLoadInfo | undefined): boolean {
  return !!info && (info.tooLarge || info.fit === "red");
}

/** Models → Installed: the models on this computer, with Use, their settings, Add to comparison and Delete. */
export function Installed(props: { onClose: () => void }) {
  const [models, setModels] = useState<InstalledModel[] | null>(null);
  const [usage, setUsage] = useState<DiskUsage | null>(null);
  const [error, setError] = useState<ErrorInfo | null>(null);
  // Each model's fit at its own settings (`model_load_info`), by path.
  const [infos, setInfos] = useState<Record<string, ModelLoadInfo>>({});
  const [settingsFor, setSettingsFor] = useState<string | null>(null);
  const pickModel = useApp((s) => s.pickModel);

  async function use(m: InstalledModel) {
    if (markedTooLarge(infos[m.path]) && !(await askTooLarge())) return;
    // No settings: the model loads with its own. The chat gets the model first, so the
    // header shows it loading; pickModel asks first when the loaded model is busy (D3).
    await pickModel(`local:${m.path}`, { onChatSet: props.onClose });
  }

  const readInfos = useCallback((list: InstalledModel[]) => {
    let alive = true;
    void Promise.all(
      list.map((m) =>
        ipc
          .modelLoadInfo(m.path)
          .then((i): [string, ModelLoadInfo] => [m.path, i])
          .catch(() => null),
      ),
    ).then((pairs) => {
      if (!alive) return;
      const next: Record<string, ModelLoadInfo> = {};
      for (const p of pairs) if (p) next[p[0]] = p[1];
      setInfos(next);
    });
    return () => {
      alive = false;
    };
  }, []);

  const load = useCallback(() => {
    return Promise.all([ipc.listInstalledModels(), ipc.diskUsage()])
      .then(([m, u]) => {
        setModels(m);
        setUsage(u);
        readInfos(m);
      })
      .catch((err) => setError(toErrorInfo(err)));
  }, [readInfos]);
  useEffect(() => {
    let alive = true;
    let stop: () => void = () => undefined;
    Promise.all([ipc.listInstalledModels(), ipc.diskUsage()])
      .then(([m, u]) => {
        if (!alive) return;
        setModels(m);
        setUsage(u);
        stop = readInfos(m);
      })
      .catch((err) => alive && setError(toErrorInfo(err)));
    return () => {
      alive = false;
      stop();
    };
  }, [readInfos]);

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
          <Row
            key={m.id}
            model={m}
            info={infos[m.path]}
            onUse={() => void use(m)}
            onSettings={() => setSettingsFor(m.path)}
            onRemove={() => void remove(m)}
          />
        ))}
        {models?.length === 0 && (
          <li className="text-sm text-fg-muted">
            No models yet. Find one under Browse, or import a GGUF file you already have.
          </li>
        )}
      </ul>
      {settingsFor && (
        <ModelSettings
          key={settingsFor}
          path={settingsFor}
          onClose={() => setSettingsFor(null)}
          onSaved={(info) => setInfos((all) => ({ ...all, [info.path]: info }))}
        />
      )}
    </div>
  );
}

function Row(props: {
  model: InstalledModel;
  info: ModelLoadInfo | undefined;
  onUse: () => void;
  onSettings: () => void;
  onRemove: () => void;
}) {
  const { model: m, info } = props;
  const compare = useAddToComparison({ installedPath: m.path });
  const verdict: Verdict | null = info ? (info.tooLarge ? "red" : info.fit) : null;
  return (
    <li className="flex items-center gap-3 rounded-md border border-border px-3 py-2 text-sm">
      <div className="min-w-0 flex-1">
        <p className="truncate font-medium" title={m.path}>
          {m.fileName}
        </p>
        <div className="mt-1 flex flex-wrap items-center gap-1">
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
        <div className="mt-1 flex flex-wrap items-center gap-3 text-xs text-fg-muted">
          {info && verdict && (
            <span
              className="flex items-center gap-1.5"
              title={`With this model's settings: remembers ${tokensWithWords(info.resolved.ctx)}`}
            >
              <span className={`inline-block h-2 w-2 rounded-full ${DOT[verdict]}`} aria-hidden />
              {FIT_WORDS[verdict]}
            </span>
          )}
          {compare.added ? (
            <button
              type="button"
              onClick={compare.remove}
              aria-label={`Remove from comparison: ${m.fileName}`}
              className="flex items-center gap-1 underline hover:text-fg"
            >
              <Check size={12} aria-hidden /> Remove from comparison
            </button>
          ) : (
            <button
              type="button"
              onClick={() => void compare.add()}
              disabled={compare.full}
              title={compare.full ? "You can compare up to 4 models. Remove one first." : undefined}
              aria-label={`Add to comparison: ${m.fileName}`}
              className="flex items-center gap-1 underline hover:text-fg disabled:no-underline disabled:opacity-50"
            >
              <Plus size={12} aria-hidden /> Add to comparison
            </button>
          )}
        </div>
      </div>
      <button
        type="button"
        onClick={props.onSettings}
        aria-label={`Settings for this model: ${m.fileName}`}
        title="Settings for this model"
        className="text-fg-muted hover:text-fg"
      >
        <Settings2 size={15} aria-hidden />
      </button>
      <button
        type="button"
        onClick={props.onUse}
        disabled={m.architectureSupported === false}
        className="rounded-md bg-accent px-3 py-1 text-xs text-accent-fg disabled:opacity-40"
      >
        Use
      </button>
      <button
        type="button"
        aria-label={`Delete ${m.fileName}`}
        onClick={props.onRemove}
        className="text-fg-muted hover:text-danger"
      >
        <Trash2 size={14} aria-hidden />
      </button>
    </li>
  );
}

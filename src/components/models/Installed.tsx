import { useCallback, useEffect, useId, useState } from "react";
import { Check, FolderOpen, Plus, Settings2, Trash2 } from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import * as ipc from "../../lib/ipc";
import type { DiskUsage, ErrorInfo, InstalledModel, ModelLoadInfo, Verdict } from "../../lib/types";
import { formatBytes } from "../../lib/format";
import { CANT_OPEN, FIT_WORDS, INSTALLED_STUDIO_LINE, tokensWithWords } from "../../lib/words";
import { toErrorInfo, useApp } from "../../store/app";
import { COMPARE_FULL, COMPARE_LIMIT, sameEntry, useAddToComparison, useCompare } from "../../store/compare";
import { askTooLarge, isLoadedNow, unloadModel } from "../../store/modelChange";
import { ErrorBanner } from "../ErrorBanner";
import { visiblePath } from "../FolderChip";
import { ModelSettings } from "../ModelSettings";
import { Badge } from "./Badge";
import { CantOpenLine } from "./CantOpen";
import { ImageFileLine, seesImages } from "./ImageFile";
import * as studioApi from "../studio/api";

const DOT: Record<Verdict, string> = { green: "bg-success", yellow: "bg-warning", red: "bg-danger" };

/** Whether Use should ask "Load it anyway?" first. */
function markedTooLarge(info: ModelLoadInfo | undefined): boolean {
  return !!info && (info.tooLarge || info.fit === "red");
}

/** [0.0.7 P6b] Delete's question for a download: what really goes from disk (every part and the
 *  image file the app downloaded, inside the models folder), and what stays outside it. */
function deleteQuestion(m: InstalledModel, shown: string): string {
  const bytes = m.deleteBytes ?? m.sizeBytes;
  const outside = m.keptOutside ?? 0;
  if (bytes > 0) {
    const kept = outside > 0 ? " Files outside the models folder stay where they are." : "";
    return `Delete ${shown}? This removes ${formatBytes(bytes)} from disk.${kept}`;
  }
  if (outside > 1) {
    return `Remove ${shown} from the list? Its files are outside the models folder, so they stay where they are.`;
  }
  if (outside === 1) {
    const what = m.onlyImageOutside ? "Its image file" : "Its file";
    return `Remove ${shown} from the list? ${what} is outside the models folder, so it stays where it is.`;
  }
  // Round 2: a file another model on the list uses is on disk, and stays.
  const inUse = m.keptInUse ?? 0;
  if (inUse > 1) {
    return `Remove ${shown} from the list? Another model on the list uses its files, so they stay where they are.`;
  }
  if (inUse === 1) {
    return `Remove ${shown} from the list? Another model on the list uses its file, so it stays where it is.`;
  }
  return `Remove ${shown} from the list? Its file is no longer on disk.`;
}

/** Models → Installed: the models on this computer, with Use, their settings, Add to comparison and Delete.
 *  0.1.0 Studio: picture and video models are not on this list (nothing here can Use them); one
 *  line counts them and opens Studio's Get models. */
export function Installed(props: { onClose: () => void }) {
  const [models, setModels] = useState<InstalledModel[] | null>(null);
  // How many picture and video models are installed, and the disk they take; `null` until read,
  // or when Studio's list could not be read (the line is left out).
  const [pictureModels, setPictureModels] = useState<{ count: number; bytes: number } | null>(null);
  const [usage, setUsage] = useState<DiskUsage | null>(null);
  const [error, setError] = useState<ErrorInfo | null>(null);
  // [0.0.7 P6] What the last Delete left in place (files outside the models folder).
  const [note, setNote] = useState<string | null>(null);
  // Each model's fit at its own settings (`model_load_info`), by path.
  const [infos, setInfos] = useState<Record<string, ModelLoadInfo>>({});
  const [settingsFor, setSettingsFor] = useState<string | null>(null);
  const pickModel = useApp((s) => s.pickModel);
  const compared = useCompare((s) => s.entries);
  // The comparison is full while a model here is not in it: said once, as visible text.
  const compareFull =
    compared.length >= COMPARE_LIMIT &&
    (models ?? []).some((m) => !compared.some((e) => sameEntry(e, { installedPath: m.path })));

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
    // [0.0.6 item 8, WP8] An import or a delete changes "On this computer" in the chat's model list.
    useApp.getState().refreshModelOptions().catch(() => undefined);
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

  // Studio's installed models: a read of what is saved, nothing is fetched.
  useEffect(() => {
    let alive = true;
    Promise.resolve()
      .then(() => studioApi.studioLocalModels())
      .then(
        (local) => alive && setPictureModels({ count: local.installed.length, bytes: local.installedBytes }),
        // Left out without a word: Studio's own read of its models reports the error, and this
        // dialog has nowhere to put it.
        () => undefined,
      );
    return () => {
      alive = false;
    };
  }, []);

  /** *Manage in Studio*: Studio opens on Get models, and this dialog closes as Use closes it. */
  function manageInStudio() {
    window.dispatchEvent(new CustomEvent("app:open-studio", { detail: "local" }));
    props.onClose();
  }

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
    const shown = visiblePath(m.fileName);
    const question = files ? deleteQuestion(m, shown) : `Remove ${shown} from the list? The file itself stays where it is.`;
    if (!window.confirm(question)) return;
    setNote(null);
    try {
      // The loaded model is unloaded first, the way Unload does it: when something uses it,
      // the D3 question comes first, and with Restart when it ends the delete waits for it
      // (a cancelled wait deletes nothing). Removing it never cuts a reply off unasked.
      // [0.0.6 item 8] The same file loaded through a link or a mapped drive counts too.
      const runtime = useApp.getState().runtime;
      const loaded = isLoadedNow(runtime, m.path) || (!!m.loadedAs && isLoadedNow(runtime, m.loadedAs));
      if (loaded && !(await unloadModel(runtime))) return;
      setNote((await ipc.deleteModel(m.id, files)) ?? null);
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
      {note && (
        <p className="text-xs text-fg-muted" role="status">
          {note}
        </p>
      )}
      <div className="flex items-center justify-between text-xs text-fg-muted">
        <span className="truncate" title={usage ? visiblePath(usage.modelsDir) : undefined}>
          {usage && (
            <>
              {formatBytes(usage.totalBytes)} in <bdi dir="ltr">{visiblePath(usage.modelsDir)}</bdi>
            </>
          )}
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
      {compareFull && <p className="text-xs text-fg-muted">{COMPARE_FULL}</p>}
      <ul className="space-y-2">
        {(models ?? []).map((m) => (
          <Row
            key={m.id}
            model={m}
            info={infos[m.path]}
            onUse={() => void use(m)}
            onSettings={() => setSettingsFor(m.path)}
            onRemove={() => void remove(m)}
            onChanged={load}
          />
        ))}
        {models?.length === 0 && (
          <li className="text-sm text-fg-muted">
            No models yet. Find one under Browse, or import a GGUF file you already have.
          </li>
        )}
      </ul>
      {/* With none yet ("Picture and video models: none yet") it still leads to Get models. */}
      {pictureModels !== null && (
        <p className="text-xs text-fg-muted">
          {INSTALLED_STUDIO_LINE.lead(pictureModels.count, pictureModels.bytes)}
          <button type="button" className="underline hover:text-fg" onClick={manageInStudio}>
            {INSTALLED_STUDIO_LINE.manage}
          </button>
        </p>
      )}
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
  /** [0.0.7 P2] The row changed (its image file was downloaded): read the list again. */
  onChanged: () => void;
}) {
  const { model: m, info } = props;
  const compare = useAddToComparison({ installedPath: m.path });
  // This version's model runner does not know the file's kind (the list is of the pinned build,
  // judged from the file's own header): nothing that says it could run is shown with it.
  const cantOpen = m.architectureSupported === false;
  const whyId = useId();
  const verdict: Verdict | null = info && !cantOpen ? (info.tooLarge ? "red" : info.fit) : null;
  // File names, folders and what the file says about itself can hold any character: shown
  // with invisible and direction-changing characters as codes, so one file cannot pose as another.
  const name = visiblePath(m.fileName);
  return (
    <li className="flex items-center gap-3 rounded-md border border-border px-3 py-2 text-sm">
      <div className="min-w-0 flex-1">
        <p className="truncate font-medium" title={visiblePath(m.path)}>
          <bdi dir="ltr">{name}</bdi>
        </p>
        <div className="mt-1 flex flex-wrap items-center gap-1">
          {m.repo && <Badge>{visiblePath(m.repo)}</Badge>}
          <Badge>{formatBytes(m.sizeBytes)}</Badge>
          {m.summary?.quantization && <Badge>{visiblePath(m.summary.quantization)}</Badge>}
          {m.summary?.contextLength && (
            <Badge title="The most this model can remember">remembers up to {tokensWithWords(m.summary.contextLength)}</Badge>
          )}
          {m.summary?.templateTools && <Badge>tools</Badge>}
          {m.summary?.templateThinking && <Badge>thinking</Badge>}
          {seesImages(m) && <Badge>vision</Badge>}
          {m.summary?.isMoe && (
            <Badge title="It uses only part of itself at a time, so it runs faster than its size suggests.">
              uses part of itself at a time
            </Badge>
          )}
          {m.source === "import" && <Badge>imported</Badge>}
        </div>
        {cantOpen && <CantOpenLine id={whyId} architecture={m.summary?.architecture} />}
        {!cantOpen && <ImageFileLine model={m} onChanged={props.onChanged} />}
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
              aria-label={`Remove from comparison: ${name}`}
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
              aria-label={`Add to comparison: ${name}`}
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
        aria-label={`Settings for this model: ${name}`}
        title="Settings for this model"
        className="text-fg-muted hover:text-fg"
      >
        <Settings2 size={15} aria-hidden />
      </button>
      <button
        type="button"
        onClick={props.onUse}
        disabled={cantOpen}
        title={cantOpen ? CANT_OPEN.title : undefined}
        aria-describedby={cantOpen ? whyId : undefined}
        className="rounded-md bg-accent px-3 py-1 text-xs text-accent-fg disabled:opacity-40"
      >
        Use
      </button>
      <button
        type="button"
        aria-label={`Delete ${name}`}
        onClick={props.onRemove}
        className="text-fg-muted hover:text-danger"
      >
        <Trash2 size={14} aria-hidden />
      </button>
    </li>
  );
}

import { useCallback, useEffect, useRef, useState } from "react";
import { Dialog, Tabs } from "radix-ui";
import {
  ArrowLeft,
  Check,
  Download,
  ExternalLink,
  FolderOpen,
  Lock,
  Pause,
  Play,
  Search,
  Trash2,
  X,
} from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import * as ipc from "../lib/ipc";
import type {
  DiskUsage,
  DownloadGroup,
  ErrorInfo,
  HubSort,
  InstalledModel,
  RepoSummary,
  RepoView,
  VariantView,
} from "../lib/types";
import { formatBytes, formatCount, formatEta, formatParams } from "../lib/format";
import { toErrorInfo, useApp } from "../store/app";
import { ErrorBanner } from "./ErrorBanner";
import { FitBadge } from "./FitBadge";

const tabClass =
  "rounded-md px-3 py-1.5 text-sm data-[state=active]:bg-bg-muted data-[state=active]:font-medium hover:bg-bg-muted";

function Badge(props: { children: React.ReactNode; title?: string }) {
  return (
    <span title={props.title} className="rounded border border-border px-1.5 py-0.5 text-[11px] text-fg-muted">
      {props.children}
    </span>
  );
}

// ---- Installed -------------------------------------------------------------

function Installed(props: { onUse: (m: InstalledModel) => void }) {
  const [models, setModels] = useState<InstalledModel[] | null>(null);
  const [usage, setUsage] = useState<DiskUsage | null>(null);
  const [error, setError] = useState<ErrorInfo | null>(null);

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
              onClick={() => props.onUse(m)}
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

// ---- Browse ----------------------------------------------------------------

function RepoDetail(props: { repoId: string; onBack: () => void; onStarted: () => void }) {
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

function Browse(props: { onStarted: () => void }) {
  const [text, setText] = useState("");
  const [sort, setSort] = useState<HubSort>("downloads");
  const [allGguf, setAllGguf] = useState(false);
  const [items, setItems] = useState<RepoSummary[]>([]);
  const [next, setNext] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const timer = useRef<number | undefined>(undefined);

  const run = useCallback(async (q: string, s: HubSort, all: boolean) => {
    // "owner/repo" opens that repository directly.
    if (/^[\w.-]+\/[\w.-]+$/.test(q.trim())) {
      setSelected(q.trim());
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const page = await ipc.hubSearch(q, s, all);
      setItems(page.items);
      setNext(page.next);
    } catch (err) {
      setError(toErrorInfo(err));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    // Debounced so typing does not spend the anonymous rate limit.
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => void run(text, sort, allGguf), text ? 400 : 0);
    return () => window.clearTimeout(timer.current);
  }, [text, sort, allGguf, run]);

  async function more() {
    if (!next) return;
    setBusy(true);
    try {
      const page = await ipc.hubSearchNext(next);
      setItems((old) => [...old, ...page.items]);
      setNext(page.next);
    } catch (err) {
      setError(toErrorInfo(err));
    } finally {
      setBusy(false);
    }
  }

  if (selected) {
    return <RepoDetail repoId={selected} onBack={() => setSelected(null)} onStarted={props.onStarted} />;
  }
  return (
    <div className="space-y-3">
      <div className="flex gap-2">
        <label className="relative flex-1">
          <span className="sr-only">Search Hugging Face</span>
          <Search size={14} className="absolute top-2 left-2 text-fg-muted" aria-hidden />
          <input
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="Search models, or paste owner/repo"
            className="w-full rounded-md border border-border bg-bg py-1.5 pr-2 pl-7 text-sm"
          />
        </label>
        <select
          aria-label="Sort by"
          value={sort}
          onChange={(e) => setSort(e.target.value as HubSort)}
          className="rounded-md border border-border bg-bg px-2 text-sm"
        >
          <option value="downloads">Most downloaded</option>
          <option value="trendingScore">Trending</option>
          <option value="likes">Most liked</option>
          <option value="lastModified">Recently updated</option>
        </select>
      </div>
      <label className="flex items-center gap-2 text-xs text-fg-muted">
        <input type="checkbox" checked={allGguf} onChange={(e) => setAllGguf(e.target.checked)} />
        Show all GGUF repositories (including speech, image and video models that cannot chat)
      </label>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <ul className="space-y-1">
        {items.map((r) => (
          <li key={r.id}>
            <button
              type="button"
              onClick={() => setSelected(r.id)}
              className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-bg-muted"
            >
              <span className="min-w-0 flex-1 truncate">{r.id}</span>
              {r.gated !== "no" && <Lock size={12} className="text-fg-muted" aria-label="gated" />}
              {r.license && <Badge>{r.license}</Badge>}
              <span className="w-16 text-right text-xs text-fg-muted" title="Downloads">
                {formatCount(r.downloads)}
              </span>
            </button>
          </li>
        ))}
      </ul>
      {!busy && items.length === 0 && !error && <p className="text-sm text-fg-muted">No matching models.</p>}
      {next && (
        <button type="button" disabled={busy} onClick={() => void more()} className="text-sm text-accent disabled:opacity-40">
          {busy ? "Loading…" : "Show more"}
        </button>
      )}
      <p className="text-xs text-fg-muted">
        Results come from huggingface.co. Model cards are shown as plain text, without images.
      </p>
    </div>
  );
}

// ---- Downloads -------------------------------------------------------------

function Downloads() {
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
                    {g.repo} · {g.label}
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

// ---- Dialog ----------------------------------------------------------------

export function ModelsDialog(props: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const [tab, setTab] = useState("installed");
  const refreshModelOptions = useApp((s) => s.refreshModelOptions);
  const setChatModel = useApp((s) => s.setChatModel);
  const showError = useApp((s) => s.showError);

  async function use(m: InstalledModel) {
    try {
      const settings = await ipc.getLocalModelSettings();
      await ipc.loadLocalModel(m.path, settings);
      await refreshModelOptions();
      await setChatModel(`local:${m.path}`);
      props.onOpenChange(false);
    } catch (err) {
      showError(toErrorInfo(err));
    }
  }

  return (
    <Dialog.Root open={props.open} onOpenChange={props.onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/40" />
        <Dialog.Content className="fixed top-1/2 left-1/2 flex h-[38rem] w-[52rem] max-w-[94vw] -translate-x-1/2 -translate-y-1/2 flex-col rounded-lg border border-border bg-bg shadow-xl">
          <div className="flex items-center justify-between border-b border-border px-5 py-3">
            <Dialog.Title className="text-base font-semibold">Models</Dialog.Title>
            <Dialog.Close className="rounded-md p-1 text-fg-muted hover:bg-bg-muted hover:text-fg" aria-label="Close models">
              <X size={16} aria-hidden />
            </Dialog.Close>
          </div>
          <Dialog.Description className="sr-only">Installed models, model search and downloads</Dialog.Description>
          <Tabs.Root value={tab} onValueChange={setTab} className="flex min-h-0 flex-1 flex-col">
            <Tabs.List className="flex gap-1 border-b border-border px-4 py-2" aria-label="Model sections">
              <Tabs.Trigger value="installed" className={tabClass}>Installed</Tabs.Trigger>
              <Tabs.Trigger value="browse" className={tabClass}>Browse</Tabs.Trigger>
              <Tabs.Trigger value="downloads" className={tabClass}>Downloads</Tabs.Trigger>
            </Tabs.List>
            <div className="min-h-0 flex-1 overflow-y-auto p-5">
              <Tabs.Content value="installed"><Installed onUse={(m) => void use(m)} /></Tabs.Content>
              <Tabs.Content value="browse"><Browse onStarted={() => setTab("downloads")} /></Tabs.Content>
              <Tabs.Content value="downloads"><Downloads /></Tabs.Content>
            </div>
          </Tabs.Root>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

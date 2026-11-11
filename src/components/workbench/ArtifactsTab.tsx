import { useCallback, useEffect, useState, type ComponentType } from "react";
import { AlertDialog, DropdownMenu } from "radix-ui";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  AppWindow,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Copy,
  Download,
  FileText,
  Image,
  Pencil,
  RefreshCw,
  ShieldAlert,
  Trash2,
  Workflow,
} from "lucide-react";
import { toErrorInfo, useApp } from "../../store/app";
import { Markdown } from "../Markdown";
import * as api from "../artifacts/api";
import { ArtifactFrame, type Settled } from "../artifacts/ArtifactFrame";
import { ProblemList } from "../artifacts/ProblemList";
import { SourceEditor } from "../artifacts/SourceEditor";
import { inApp, runIsolationCheck } from "../artifacts/startup";
import { reportKey, useArtifacts } from "../artifacts/state";
import {
  EXTENSION,
  KIND_LABEL,
  type ArtifactDetail,
  type ArtifactDto,
  type ArtifactKind,
  type Isolation,
  type Problem,
} from "../artifacts/types";
import { fileName, when } from "../artifacts/when";

const KIND_ICON: Record<ArtifactKind, ComponentType<{ size?: number; className?: string }>> = {
  markdown: FileText,
  html: AppWindow,
  svg: Image,
  mermaid: Workflow,
};

type Mode = "preview" | "source";

/** The artifact and version the panel shows. `version: null` is the newest. */
interface Selection {
  id: number;
  version: number | null;
}

const SMALL_BUTTON =
  "rounded-lg border border-border px-2.5 py-1 text-xs font-medium hover:border-border-strong hover:bg-bg-muted disabled:opacity-50";
const ICON_BUTTON = "rounded-lg p-1.5 text-fg-muted hover:bg-bg-muted hover:text-fg disabled:opacity-40";

/**
 * Tells the tool that waits for it how a version's preview went, once. A preview that
 * settles when no tool is waiting (the user opened it) reports nothing.
 */
function report(id: number, version: number, problems: Problem[], notShown: "scripts-off" | "cannot-draw" | null) {
  const store = useArtifacts.getState();
  if (!store.pending[reportKey(id, version)]) return;
  store.clearReport(id, version);
  void api.reportRender(id, version, problems, notShown).catch(() => undefined);
}

/**
 * Shows a version out of sight, only to tell the waiting tool how it went. Used when the
 * panel is busy with something else: unsaved edits, or another artifact.
 */
function Reporter(props: { chatId: number; id: number; version: number }) {
  const { chatId, id, version } = props;
  const [detail, setDetail] = useState<ArtifactDetail | null>(null);
  useEffect(() => {
    let stale = false;
    api
      .getArtifact(chatId, id, version)
      .then((d) => {
        if (!stale) setDetail(d);
      })
      .catch(() => undefined);
    return () => {
      stale = true;
    };
  }, [chatId, id, version]);
  if (!detail?.frameUrl) return null;
  return (
    <ArtifactFrame
      id={id}
      version={version}
      kind={detail.artifact.kind}
      url={detail.frameUrl}
      title=""
      scripts
      hidden
      onProblems={() => undefined}
      onSettled={(o: Settled) => report(id, version, o.problems, o.unavailable ? "cannot-draw" : null)}
    />
  );
}

/** Says that previews show without scripts, why, and offers to check again. */
function IsolationNotice(props: { isolation: Isolation }) {
  const { isolation } = props;
  if (isolation.state === "proven") return null;
  if (isolation.state !== "broken") {
    return (
      <p role="status" className="border-b border-border px-3 py-1.5 text-xs text-fg-muted">
        Checking that previews are kept apart from the app…
      </p>
    );
  }
  return (
    <div role="status" className="flex items-start gap-2 border-b border-border bg-surface px-3 py-2 text-xs">
      <ShieldAlert size={14} className="mt-0.5 shrink-0 text-warning" aria-hidden />
      <p className="min-w-0 flex-1">
        Scripts are switched off in previews on this system, so pages show without running. {isolation.reason}
      </p>
      <button type="button" className={SMALL_BUTTON} onClick={() => void runIsolationCheck()}>
        Check again
      </button>
    </div>
  );
}

/**
 * The "Artifacts" tab of the panel beside the chat: the documents, pages, pictures and
 * diagrams the model made in this chat. Each can be previewed, read and edited as source,
 * stepped through its versions, restored, exported, renamed and deleted.
 *
 * Titles, contents and the problems a preview reports all come from the model. They are
 * shown as text; a page, a picture or a diagram is shown only inside `ArtifactFrame`.
 */
export function ArtifactsTab(props: { chatId: number }) {
  const { chatId } = props;
  const showError = useApp((s) => s.showError);
  const changes = useArtifacts((s) => s.changes[chatId] ?? 0);
  const focus = useArtifacts((s) => s.focus);
  const pending = useArtifacts((s) => s.pending);
  const isolation = useArtifacts((s) => s.isolation);
  const drafts = useArtifacts((s) => s.drafts);
  const setDraft = useArtifacts((s) => s.setDraft);

  const [list, setList] = useState<ArtifactDto[] | null>(null);
  const [selected, setSelected] = useState<Selection | null>(null);
  const [detail, setDetail] = useState<ArtifactDetail | null>(null);
  const [mode, setMode] = useState<Mode>("preview");
  const [problems, setProblems] = useState<{ key: string; list: Problem[] }>({ key: "", list: [] });
  /** Counts the user's own changes here, so the list and the artifact are read again. */
  const [edits, setEdits] = useState(0);
  const [reload, setReload] = useState(0);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [copied, setCopied] = useState(false);
  const [busy, setBusy] = useState(false);

  // A request to show an artifact (the model just saved it, or "Open artifact" on a tool
  // card). Unsaved edits are never pushed aside: with a draft open in the source view,
  // the panel stays where it is and the new version is shown out of sight for its report.
  const [seenFocus, setSeenFocus] = useState(0);
  if (focus && focus.chatId === chatId && focus.seq !== seenFocus) {
    setSeenFocus(focus.seq);
    const editing = mode === "source" && selected !== null && drafts[selected.id] !== undefined;
    if (!editing) {
      setSelected({ id: focus.id, version: focus.version });
      setMode("preview");
    }
  }

  useEffect(() => {
    let stale = false;
    api
      .listArtifacts(chatId)
      .then((l) => {
        if (!stale) setList(l);
      })
      .catch((e: unknown) => showError(toErrorInfo(e)));
    return () => {
      stale = true;
    };
  }, [chatId, changes, edits, showError]);

  // Nothing chosen yet: the most recently changed one.
  const first = list?.[0];
  const current: Selection | null = selected ?? (first ? { id: first.id, version: null } : null);
  const currentId = current?.id ?? null;
  const currentVersion = current?.version ?? null;

  useEffect(() => {
    if (currentId === null) return;
    let stale = false;
    api
      .getArtifact(chatId, currentId, currentVersion)
      .then((d) => {
        if (!stale) setDetail(d);
      })
      .catch((e: unknown) => {
        if (stale) return;
        // It is gone (deleted, or its chat was): back to the list.
        setSelected(null);
        setDetail(null);
        if (toErrorInfo(e).code !== "not_found") showError(toErrorInfo(e));
      });
    return () => {
      stale = true;
    };
  }, [chatId, currentId, currentVersion, changes, edits, showError]);

  const shown = detail && detail.artifact.id === currentId ? detail : null;
  const scripts = isolation.state === "proven";
  const visibleKey =
    shown?.frameUrl && mode === "preview" ? reportKey(shown.artifact.id, shown.version) : null;
  const waiting = Object.values(pending).filter((p) => p.chatId === chatId);
  const waitingCount = waiting.length;

  // With scripts off there is nothing to wait for: say so to the tools that wait.
  useEffect(() => {
    if (isolation.state !== "broken" || waitingCount === 0) return;
    for (const p of Object.values(useArtifacts.getState().pending)) {
      if (p.chatId === chatId) report(p.id, p.version, [], "scripts-off");
    }
  }, [chatId, isolation.state, waitingCount]);

  const act = useCallback(
    async (what: () => Promise<void>) => {
      setBusy(true);
      try {
        await what();
      } catch (e) {
        showError(toErrorInfo(e));
      } finally {
        setBusy(false);
      }
    },
    [showError],
  );

  if (list === null) {
    return <p className="p-6 text-sm text-fg-muted">Loading…</p>;
  }
  if (list.length === 0) {
    return (
      <div className="p-6 text-sm text-fg-muted">
        <p className="mb-2">Documents, pages and diagrams the model makes in this chat appear here.</p>
        <p>Ask for one, for example a page that converts units, a diagram of a process, or a written plan.</p>
      </div>
    );
  }

  const artifact = shown?.artifact ?? null;
  const draft = artifact ? drafts[artifact.id] : undefined;
  const sourceText = shown ? (draft?.text ?? shown.content) : "";
  const dirty = shown !== null && draft !== undefined && draft.text !== shown.content;
  const versionIndex = shown ? shown.versions.findIndex((v) => v.version === shown.version) : -1;
  const versionInfo = shown && versionIndex >= 0 ? shown.versions[versionIndex] : undefined;
  const older = shown && versionIndex > 0 ? shown.versions[versionIndex - 1] : undefined;
  const newer = shown && versionIndex >= 0 ? shown.versions[versionIndex + 1] : undefined;
  const frameKey = visibleKey ? `${visibleKey}:${reload}` : "";
  const shownProblems = problems.key === frameKey ? problems.list : [];

  const select = (id: number, version: number | null) => {
    setSelected({ id, version });
    setRenaming(null);
  };

  return (
    <div className="flex h-full min-h-0 flex-col" data-chat={chatId}>
      <ul aria-label="Artifacts of this chat" className="max-h-40 shrink-0 space-y-0.5 overflow-y-auto border-b border-border p-1.5">
        {list.map((a) => {
          const Icon = KIND_ICON[a.kind];
          const chosen = a.id === currentId;
          return (
            <li key={a.id}>
              <button
                type="button"
                aria-current={chosen ? "true" : undefined}
                onClick={() => select(a.id, null)}
                className={
                  "flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-sm " +
                  (chosen ? "bg-bg-muted font-medium" : "hover:bg-bg-muted/70")
                }
              >
                <Icon size={14} className="shrink-0 text-accent" aria-hidden />
                <span className="min-w-0 flex-1 truncate">{a.title}</span>
                <span className="shrink-0 text-xs font-normal text-fg-muted">
                  {KIND_LABEL[a.kind]} · {a.versions === 1 ? "1 version" : `${a.versions} versions`} ·{" "}
                  {when(a.updatedAt)}
                </span>
              </button>
            </li>
          );
        })}
      </ul>

      {!shown || !artifact ? (
        <p className="p-6 text-sm text-fg-muted">Loading…</p>
      ) : (
        <>
          <div className="flex shrink-0 items-center gap-1 border-b border-border px-3 py-1.5">
            {renaming === null ? (
              <>
                <h2 className="min-w-0 flex-1 truncate text-sm font-semibold" title={artifact.title}>
                  {artifact.title}
                </h2>
                <button
                  type="button"
                  aria-label="Rename"
                  title="Rename"
                  className={ICON_BUTTON}
                  onClick={() => setRenaming(artifact.title)}
                >
                  <Pencil size={14} aria-hidden />
                </button>
                <button
                  type="button"
                  aria-label="Delete artifact"
                  title="Delete"
                  className={ICON_BUTTON}
                  onClick={() => setConfirmDelete(true)}
                >
                  <Trash2 size={14} aria-hidden />
                </button>
              </>
            ) : (
              <form
                className="flex min-w-0 flex-1 items-center gap-1.5"
                onSubmit={(event) => {
                  event.preventDefault();
                  const title = renaming.trim();
                  if (title === "" || title === artifact.title) {
                    setRenaming(null);
                    return;
                  }
                  void act(async () => {
                    await api.renameArtifact(chatId, artifact.id, title);
                    setRenaming(null);
                    setEdits((n) => n + 1);
                  });
                }}
              >
                <input
                  autoFocus
                  aria-label="Title"
                  value={renaming}
                  maxLength={120}
                  onChange={(event) => setRenaming(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Escape") {
                      event.stopPropagation();
                      setRenaming(null);
                    }
                  }}
                  className="min-w-0 flex-1 rounded-lg border border-border bg-bg px-2 py-1 text-sm"
                />
                <button type="submit" className={SMALL_BUTTON} disabled={busy}>
                  Rename
                </button>
                <button type="button" className={SMALL_BUTTON} onClick={() => setRenaming(null)}>
                  Cancel
                </button>
              </form>
            )}
          </div>

          <div className="flex shrink-0 flex-wrap items-center gap-x-2 gap-y-1 border-b border-border px-3 py-1.5 text-xs">
            <div role="tablist" aria-label="Artifact view" className="flex rounded-lg bg-bg-muted p-0.5">
              {(["preview", "source"] as const).map((m) => (
                <button
                  key={m}
                  type="button"
                  role="tab"
                  aria-selected={mode === m}
                  onClick={() => setMode(m)}
                  className={
                    "rounded-md px-2.5 py-1 font-medium " +
                    (mode === m ? "bg-surface text-fg shadow-soft" : "text-fg-muted hover:text-fg")
                  }
                >
                  {m === "preview" ? "Preview" : "Source"}
                </button>
              ))}
            </div>

            <div className="flex items-center gap-0.5" aria-label="Versions">
              <button
                type="button"
                aria-label="Older version"
                title={dirty ? "Save or discard your changes first" : "Older version"}
                disabled={!older || dirty}
                className={ICON_BUTTON}
                onClick={() => older && select(artifact.id, older.version)}
              >
                <ChevronLeft size={14} aria-hidden />
              </button>
              <span className="tabular-nums whitespace-nowrap">
                Version {shown.version} of {artifact.latestVersion}
              </span>
              <button
                type="button"
                aria-label="Newer version"
                title={dirty ? "Save or discard your changes first" : "Newer version"}
                disabled={!newer || dirty}
                className={ICON_BUTTON}
                onClick={() => newer && select(artifact.id, newer.version)}
              >
                <ChevronRight size={14} aria-hidden />
              </button>
            </div>
            {versionInfo && (
              <span className="min-w-0 truncate text-fg-muted">
                {versionInfo.author === "model" ? "by the model" : "by you"} · {when(versionInfo.createdAt)}
                {versionInfo.note ? ` · ${versionInfo.note}` : ""}
              </span>
            )}
            {shown.version !== artifact.latestVersion && (
              <button
                type="button"
                className={SMALL_BUTTON}
                disabled={busy || dirty}
                onClick={() =>
                  void act(async () => {
                    await api.restoreArtifactVersion(chatId, artifact.id, shown.version);
                    select(artifact.id, null);
                    setEdits((n) => n + 1);
                  })
                }
              >
                Restore this version
              </button>
            )}
            <div className="flex-1" />
            {visibleKey && (
              <button
                type="button"
                aria-label="Reload preview"
                title="Load the preview again"
                className={ICON_BUTTON}
                onClick={() => setReload((n) => n + 1)}
              >
                <RefreshCw size={14} aria-hidden />
              </button>
            )}
            <DropdownMenu.Root>
              <DropdownMenu.Trigger asChild>
                <button type="button" className={`${SMALL_BUTTON} flex items-center gap-1`}>
                  Export <ChevronDown size={12} aria-hidden />
                </button>
              </DropdownMenu.Trigger>
              <DropdownMenu.Portal>
                <DropdownMenu.Content
                  align="end"
                  sideOffset={4}
                  className="z-50 min-w-44 rounded-xl border border-border bg-surface p-1.5 shadow-pop"
                >
                  <DropdownMenu.Item
                    className="flex cursor-default items-center gap-2 rounded-lg px-2.5 py-1.5 text-sm outline-none data-[highlighted]:bg-bg-muted"
                    onSelect={() =>
                      void act(async () => {
                        const ext = EXTENSION[artifact.kind];
                        const path = await saveDialog({
                          defaultPath: fileName(artifact.title, ext),
                          filters: [{ name: KIND_LABEL[artifact.kind], extensions: [ext] }],
                        });
                        if (typeof path !== "string") return;
                        await api.exportArtifact(chatId, artifact.id, shown.version, path);
                      })
                    }
                  >
                    <Download size={14} aria-hidden /> Save to file…
                  </DropdownMenu.Item>
                  <DropdownMenu.Item
                    className="flex cursor-default items-center gap-2 rounded-lg px-2.5 py-1.5 text-sm outline-none data-[highlighted]:bg-bg-muted"
                    onSelect={() =>
                      void act(async () => {
                        if (inApp()) await writeText(sourceText);
                        else await navigator.clipboard.writeText(sourceText);
                        setCopied(true);
                        setTimeout(() => setCopied(false), 1500);
                      })
                    }
                  >
                    <Copy size={14} aria-hidden /> Copy
                  </DropdownMenu.Item>
                </DropdownMenu.Content>
              </DropdownMenu.Portal>
            </DropdownMenu.Root>
            {copied && (
              <span role="status" className="flex items-center gap-1 text-fg-muted">
                <Check size={12} aria-hidden /> Copied
              </span>
            )}
          </div>

          {shown.frameUrl && <IsolationNotice isolation={isolation} />}
          {mode === "preview" && (
            <ProblemList title={artifact.title} id={artifact.id} version={shown.version} problems={shownProblems} />
          )}

          <div className="min-h-0 flex-1">
            {mode === "source" ? (
              <div className="flex h-full min-h-0 flex-col">
                <div className="min-h-0 flex-1">
                  <SourceEditor
                    label={`Source of ${artifact.title}`}
                    value={sourceText}
                    onChange={(text) => setDraft(artifact.id, { base: draft?.base ?? shown.version, text })}
                  />
                </div>
                <div className="flex shrink-0 flex-wrap items-center gap-2 border-t border-border px-3 py-2 text-xs">
                  <button
                    type="button"
                    disabled={!dirty || busy}
                    className="btn-primary rounded-lg px-3 py-1.5 text-xs font-medium"
                    onClick={() =>
                      void act(async () => {
                        await api.saveArtifactContent(chatId, artifact.id, sourceText);
                        setDraft(artifact.id, null);
                        select(artifact.id, null);
                        setEdits((n) => n + 1);
                      })
                    }
                  >
                    Save as new version
                  </button>
                  <button
                    type="button"
                    disabled={draft === undefined || busy}
                    className={SMALL_BUTTON}
                    onClick={() => setDraft(artifact.id, null)}
                  >
                    Discard changes
                  </button>
                  <span id="artifact-editor-keys" className="min-w-0 flex-1 text-fg-muted">
                    {dirty && draft && draft.base !== artifact.latestVersion
                      ? `Your changes started from version ${draft.base}; the newest is version ${artifact.latestVersion}. `
                      : ""}
                    Tab indents. Press Escape, then Tab, to leave the box.
                  </span>
                </div>
              </div>
            ) : shown.frameUrl ? (
              <ArtifactFrame
                id={artifact.id}
                version={shown.version}
                kind={artifact.kind}
                url={shown.frameUrl}
                title={`Preview of ${artifact.title}`}
                scripts={scripts}
                reload={reload}
                onProblems={(list) => setProblems({ key: frameKey, list })}
                onSettled={(o) =>
                  report(artifact.id, shown.version, o.problems, o.unavailable ? "cannot-draw" : null)
                }
              />
            ) : (
              <div className="chat-text h-full overflow-y-auto p-4">
                <Markdown text={shown.content} />
              </div>
            )}
          </div>
        </>
      )}

      {/* Versions a tool is waiting to hear about that the panel is not showing right now. */}
      {scripts &&
        waiting
          .filter((p) => reportKey(p.id, p.version) !== visibleKey)
          .map((p) => <Reporter key={reportKey(p.id, p.version)} chatId={chatId} id={p.id} version={p.version} />)}

      <AlertDialog.Root open={confirmDelete} onOpenChange={setConfirmDelete}>
        <AlertDialog.Portal>
          <AlertDialog.Overlay className="dialog-overlay fixed inset-0" />
          <AlertDialog.Content className="fixed top-1/2 left-1/2 w-96 max-w-[90vw] -translate-x-1/2 -translate-y-1/2 rounded-xl border border-border bg-surface p-5 shadow-pop">
            <AlertDialog.Title className="mb-2 font-semibold">Delete this artifact?</AlertDialog.Title>
            <AlertDialog.Description className="mb-4 text-sm text-fg-muted">
              {artifact
                ? `"${artifact.title}" and ${artifact.versions === 1 ? "its one version" : `its ${artifact.versions} versions`} are removed from this computer. This cannot be undone.`
                : ""}
            </AlertDialog.Description>
            <div className="flex justify-end gap-2">
              <AlertDialog.Cancel className="rounded-md border border-border px-3 py-1.5 text-sm">Cancel</AlertDialog.Cancel>
              <AlertDialog.Action
                className="rounded-md bg-danger px-3 py-1.5 text-sm text-white"
                onClick={() => {
                  if (!artifact) return;
                  void act(async () => {
                    await api.deleteArtifact(chatId, artifact.id);
                    setDraft(artifact.id, null);
                    setSelected(null);
                    setDetail(null);
                    setEdits((n) => n + 1);
                  });
                }}
              >
                Delete artifact
              </AlertDialog.Action>
            </div>
          </AlertDialog.Content>
        </AlertDialog.Portal>
      </AlertDialog.Root>
    </div>
  );
}

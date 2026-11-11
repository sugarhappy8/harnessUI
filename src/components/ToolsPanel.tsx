import { useCallback, useEffect, useMemo, useState } from "react";
import { Popover } from "radix-ui";
import { AlertTriangle, FolderPlus, Trash2, Wrench } from "lucide-react";
import * as ipc from "../lib/ipc";
import type { ApprovedFolder, ToolDto } from "../lib/types";
import { useApp, toErrorInfo } from "../store/app";

const SERVER_LABEL: Record<string, string> = {
  files: "Files",
  terminal: "Terminal",
  git: "Git",
  skills: "Skills",
  browser: "Browser",
  computer: "Computer",
  subagents: "Subagents",
};

/** One line under a group's name, for groups that need a word of explanation. */
const SERVER_NOTE: Record<string, string> = {
  subagents:
    "Lets the model hand a task to a helper that works in its own context with the same tools and the same approvals.",
};

/** Groups shown after every other group. */
const LAST_GROUPS = ["subagents"];

/**
 * The tools switched on for this chat (9.3). Only switched-on tools are offered to the
 * model; each costs context, shown as an estimate. Also the chat's loaded skills (10)
 * and the folders its file tools may use, with the working folder.
 */
export function ToolsPanel(props: {
  chatId: number;
  nCtx: number | null;
  unsupported?: boolean;
  /** The chat's stored working folder (null: the first approved folder). */
  workingFolder?: string | null;
}) {
  const [tools, setTools] = useState<ToolDto[]>([]);
  const [folders, setFolders] = useState<ApprovedFolder[]>([]);
  const [loaded, setLoaded] = useState<string[]>([]);
  const [open, setOpen] = useState(false);
  const [gitFallback, setGitFallback] = useState<string | null>(null);
  const showError = useApp((s) => s.showError);
  const openChat = useApp((s) => s.openChat);

  const refresh = useCallback(() => {
    ipc
      .listChatTools(props.chatId)
      .then(setTools)
      .catch((e) => showError(toErrorInfo(e)));
  }, [props.chatId, showError]);

  // Global folders and this chat's own; loaded skills. Read when the panel opens.
  const refreshChatState = useCallback(() => {
    ipc
      .listApprovedFolders(props.chatId)
      .then(setFolders)
      .catch((e) => showError(toErrorInfo(e)));
    ipc
      .loadedSkills(props.chatId)
      .then(setLoaded)
      .catch((e) => showError(toErrorInfo(e)));
  }, [props.chatId, showError]);

  useEffect(() => {
    void ipc
      .gitInfo()
      .then((i) => setGitFallback(i.notice))
      .catch(() => setGitFallback(null));
  }, []);

  // Also re-read when the model's context becomes known or changes: whether the Browser
  // group is on in a new chat depends on it.
  useEffect(() => {
    refresh();
    let un: (() => void) | undefined;
    void ipc.onMcpChanged(refresh).then((u) => (un = u));
    return () => un?.();
  }, [refresh, props.nCtx]);

  const enabled = tools.filter((t) => t.enabledInChat && t.available && !t.schemaError);
  const cost = enabled.reduce((n, t) => n + t.tokenEstimate, 0);
  const groups = useMemo(() => {
    const m = new Map<string, ToolDto[]>();
    for (const t of tools) m.set(t.server, [...(m.get(t.server) ?? []), t]);
    const all = [...m.entries()];
    return [...all.filter(([s]) => !LAST_GROUPS.includes(s)), ...all.filter(([s]) => LAST_GROUPS.includes(s))];
  }, [tools]);

  async function save(next: string[]) {
    try {
      await ipc.setChatTools(props.chatId, next);
      refresh();
    } catch (e) {
      showError(toErrorInfo(e));
    }
  }

  function toggle(names: string[], on: boolean) {
    const current = new Set(tools.filter((t) => t.enabledInChat).map((t) => t.exposed));
    for (const n of names) {
      if (on) current.add(n);
      else current.delete(n);
    }
    void save([...current]);
  }

  // The folder the backend uses: the stored one while it is still approved, else the first.
  const stored = props.workingFolder ?? null;
  const workingId = (folders.find((f) => f.path === stored) ?? folders[0])?.id ?? null;

  async function approveFolder() {
    try {
      if (await ipc.approveFolder(props.chatId)) setFolders(await ipc.listApprovedFolders(props.chatId));
    } catch (e) {
      showError(toErrorInfo(e));
    }
  }

  async function removeFolder(id: number) {
    try {
      await ipc.removeApprovedFolder(id);
      setFolders(await ipc.listApprovedFolders(props.chatId));
    } catch (e) {
      showError(toErrorInfo(e));
    }
  }

  async function chooseWorkingFolder(path: string) {
    try {
      await ipc.setChatWorkingFolder(props.chatId, path);
      if (useApp.getState().activeChatId === props.chatId) await openChat(props.chatId);
    } catch (e) {
      showError(toErrorInfo(e));
    }
  }

  async function unload(name: string) {
    try {
      await ipc.unloadSkill(props.chatId, name);
      setLoaded((l) => l.filter((n) => n !== name));
    } catch (e) {
      showError(toErrorInfo(e));
    }
  }

  return (
    <Popover.Root
      open={open}
      onOpenChange={(o) => {
        setOpen(o);
        if (o) {
          refresh();
          refreshChatState();
        }
      }}
    >
      <Popover.Trigger asChild>
        <button
          type="button"
          className="flex items-center gap-1 rounded-md px-2 py-1 text-sm text-fg-muted hover:bg-bg-muted hover:text-fg"
          title="Tools for this chat"
        >
          <Wrench size={14} aria-hidden /> {props.unsupported ? "Tools (unavailable)" : `Tools (${enabled.length})`}
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          align="end"
          sideOffset={6}
          className="z-50 max-h-[70vh] w-96 overflow-y-auto rounded-lg border border-border bg-bg p-3 text-sm shadow-lg"
        >
          <div className="mb-2 flex items-baseline justify-between">
            <h2 className="font-medium">Tools for this chat</h2>
            <span className="text-xs text-fg-muted" title="Context the tool descriptions take on every request">
              ~{cost.toLocaleString()} tokens
              {props.nCtx ? ` (${Math.round((cost / props.nCtx) * 100)}% of context)` : ""}
            </span>
          </div>
          {props.unsupported && (
            <p className="mb-2 flex items-start gap-1 text-xs text-warning">
              <AlertTriangle size={12} className="mt-0.5 shrink-0" aria-hidden />
              This model&apos;s chat template has no tool-calling format, so it can&apos;t request tools. Pick a model
              with the Tools badge to use files, terminal or browser.
            </p>
          )}
          {props.nCtx && cost > props.nCtx * 0.25 && (
            <p className="mb-2 flex items-start gap-1 text-xs text-warning">
              <AlertTriangle size={12} className="mt-0.5 shrink-0" aria-hidden />
              The switched-on tools take a large part of the context. Switch off the ones this chat does not need.
            </p>
          )}
          {groups.map(([server, list]) => {
            const names = list.filter((t) => t.available && !t.schemaError).map((t) => t.exposed);
            const allOn = names.length > 0 && names.every((n) => list.find((t) => t.exposed === n)?.enabledInChat);
            // The Browser group when no browser is installed: every tool carries the same
            // reason, shown once on the group with the way to fix it.
            const reason = list[0]?.schemaError ?? null;
            const noBrowser =
              server === "browser" && reason !== null && list.every((t) => !t.available && t.schemaError === reason);
            return (
              <section key={server} className="mb-3">
                <label className="flex items-center gap-2 font-medium">
                  <input
                    type="checkbox"
                    checked={allOn}
                    disabled={names.length === 0}
                    onChange={(e) => toggle(names, e.target.checked)}
                  />
                  {SERVER_LABEL[server] ?? server}
                  {server === "terminal" && (
                    <span className="text-xs font-normal text-fg-muted">— runs commands as you; not a sandbox</span>
                  )}
                  {server === "git" && gitFallback && (
                    <span className="text-xs font-normal text-warning" title={gitFallback}>
                      — built-in Git library (no hooks, https only)
                    </span>
                  )}
                </label>
                {SERVER_NOTE[server] && <p className="mt-0.5 pl-6 text-xs text-fg-muted">{SERVER_NOTE[server]}</p>}
                {noBrowser && (
                  <div className="mt-1 pl-6 text-xs">
                    <p className="flex items-start gap-1 text-warning">
                      <AlertTriangle size={12} className="mt-0.5 shrink-0" aria-hidden />
                      {reason}
                    </p>
                    <button
                      type="button"
                      className="mt-1 rounded-md border border-border px-2 py-0.5 hover:bg-bg-muted"
                      onClick={() => {
                        setOpen(false);
                        window.dispatchEvent(new CustomEvent("app:open-settings", { detail: "browser" }));
                      }}
                    >
                      Set up the browser
                    </button>
                  </div>
                )}
                <ul className="mt-1 space-y-1 pl-6">
                  {list.map((t) => (
                    <li key={t.exposed}>
                      <label className="flex items-start gap-2" title={t.description}>
                        <input
                          type="checkbox"
                          className="mt-0.5"
                          checked={t.enabledInChat && t.available}
                          disabled={!t.available || t.schemaError !== null}
                          onChange={(e) => toggle([t.exposed], e.target.checked)}
                        />
                        <span className="min-w-0 flex-1">
                          <span className="font-mono text-xs">{t.exposed}</span>
                          {!t.ready && t.available && <span className="ml-1 text-xs text-warning">not running</span>}
                          {t.changed && (
                            <span className="ml-1 text-xs text-warning">changed — review it in Settings → MCP servers</span>
                          )}
                          {!t.available && !t.changed && !t.builtin && (
                            <span className="ml-1 text-xs text-fg-muted">off in Settings → MCP servers</span>
                          )}
                          {t.schemaError && !noBrowser && (
                            <span className="ml-1 text-xs text-danger">{t.schemaError}</span>
                          )}
                        </span>
                        <span className="text-xs text-fg-muted">~{t.tokenEstimate}</span>
                      </label>
                    </li>
                  ))}
                </ul>
              </section>
            );
          })}
          {loaded.length > 0 && (
            <section className="mb-3 border-t border-border pt-2" aria-labelledby={`loaded-skills-${props.chatId}`}>
              <h3 id={`loaded-skills-${props.chatId}`} className="font-medium">
                Skills loaded in this chat
              </h3>
              <p className="text-xs text-fg-muted">
                A loaded skill&apos;s full text is sent with every message. Unload the ones this chat no longer needs.
              </p>
              <ul className="mt-1 space-y-1">
                {loaded.map((name) => (
                  <li key={name} className="flex items-center gap-2">
                    <span className="min-w-0 flex-1 truncate font-mono text-xs" title={name}>
                      {name}
                    </span>
                    <button
                      type="button"
                      aria-label={`Unload ${name}`}
                      onClick={() => void unload(name)}
                      className="text-xs text-fg-muted underline hover:text-fg"
                    >
                      Unload
                    </button>
                  </li>
                ))}
              </ul>
            </section>
          )}
          <section className="mb-3 border-t border-border pt-2" aria-labelledby={`chat-folders-${props.chatId}`}>
            <h3 id={`chat-folders-${props.chatId}`} className="font-medium">
              Folders for this chat
            </h3>
            <p className="text-xs text-fg-muted">
              File tools work only inside these folders. The selected one is the working folder, where relative paths
              start.
            </p>
            <div role="radiogroup" aria-label="Working folder">
              <ul className="mt-1 space-y-1">
                {folders.map((f) => (
                  <li key={f.id} className="flex items-center gap-2">
                    <label className="flex min-w-0 flex-1 items-center gap-2" title={f.path}>
                      <input
                        type="radio"
                        name={`working-folder-${props.chatId}`}
                        checked={f.id === workingId}
                        onChange={() => void chooseWorkingFolder(f.path)}
                      />
                      <span className="min-w-0 flex-1 truncate font-mono text-xs">{f.path}</span>
                      {f.chatId === null && <span className="shrink-0 text-xs text-fg-muted">all chats</span>}
                    </label>
                    {f.chatId === props.chatId && (
                      <button
                        type="button"
                        aria-label={`Remove ${f.path} from this chat`}
                        onClick={() => void removeFolder(f.id)}
                        className="text-fg-muted hover:text-danger"
                      >
                        <Trash2 size={13} aria-hidden />
                      </button>
                    )}
                  </li>
                ))}
                {folders.length === 0 && <li className="text-xs text-fg-muted">No folders approved yet.</li>}
              </ul>
            </div>
            <button
              type="button"
              onClick={() => void approveFolder()}
              className="mt-1 flex items-center gap-1 text-xs text-accent"
            >
              <FolderPlus size={13} aria-hidden /> Approve a folder for this chat…
            </button>
          </section>
          <button
            type="button"
            className="mt-1 text-xs text-fg-muted underline hover:text-fg"
            onClick={() =>
              void ipc
                .clearChatGrants(props.chatId)
                .then(() => setOpen(false))
                .catch((e) => showError(toErrorInfo(e)))
            }
          >
            Forget everything I allowed in this chat
          </button>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

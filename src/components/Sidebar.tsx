import { Fragment, useEffect, useRef, useState } from "react";
import { Boxes, Plus, Search, Settings, Trash2, Pencil } from "lucide-react";
import { AlertDialog } from "radix-ui";
import { useApp } from "../store/app";
import { APP_NAME } from "../lib/app";
import { dayGroup } from "../lib/format";
import { backendWords } from "../lib/words";
import { BrandMark } from "./Brand";
import { visiblePath } from "./FolderChip";

/** What is loaded right now, at the bottom of the list. Opens the model manager. */
function RuntimeChip(props: { onOpenModels: () => void }) {
  const runtime = useApp((s) => s.runtime);
  const state = runtime?.state;
  const loaded = state === "ready" || state === "starting";
  // Plain words (D4): "Graphics card (Vulkan)", never "vulkan · Vulkan0".
  const where = runtime?.backend ? backendWords(runtime.backend) : "";
  // File names and paths with their hidden and right-to-left characters as codes, like the
  // header's model menu (a GGUF named "X-\u{202E}fuug.gguf" must not read reversed).
  const unloaded =
    state === "stopped" && runtime?.unloadedPath ? visiblePath(runtime.unloadedPath.split(/[\\/]/).pop() ?? "") : null;
  return (
    <button
      type="button"
      onClick={props.onOpenModels}
      title={
        loaded ? (runtime?.modelPath ? visiblePath(runtime.modelPath) : undefined) : "Open the model manager"
      }
      className="flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-left hover:bg-bg-muted"
    >
      <span
        aria-hidden
        className={
          "h-2 w-2 shrink-0 rounded-full " +
          (state === "ready"
            ? "bg-success"
            : state === "starting"
              ? "pulse bg-warning"
              : state === "failed"
                ? "bg-danger"
                : "bg-border-strong")
        }
      />
      <span className="min-w-0 flex-1">
        <span className="block truncate text-xs font-medium">
          {loaded ? (
            <bdi dir="ltr">{visiblePath(runtime?.modelName ?? "Local model")}</bdi>
          ) : state === "failed" ? (
            "The model did not load"
          ) : unloaded ? (
            <bdi dir="ltr">{unloaded}</bdi>
          ) : (
            "No model loaded"
          )}
        </span>
        <span className="block truncate text-[11px] text-fg-muted">
          {state === "ready"
            ? where || "Ready"
            : state === "starting"
              ? "Loading…"
              : unloaded
                ? "Unloaded to free memory"
                : "Runs on this computer"}
        </span>
      </span>
    </button>
  );
}

export function Sidebar(props: {
  onOpenSettings: () => void;
  onOpenModels: () => void;
}) {
  const {
    chats,
    activeChatId,
    search,
    setSearch,
    openChat,
    newChat,
    renameChat,
    deleteChat,
  } = useApp();
  const [editing, setEditing] = useState<number | null>(null);
  const [draft, setDraft] = useState("");
  const [confirmDelete, setConfirmDelete] = useState<number | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    function onFocusSearch() {
      searchRef.current?.focus();
    }
    window.addEventListener("app:focus-search", onFocusSearch);
    return () => window.removeEventListener("app:focus-search", onFocusSearch);
  }, []);

  return (
    <aside
      className="flex w-64 shrink-0 flex-col border-r border-border bg-bg-subtle"
      aria-label="Chats"
    >
      <div className="flex items-center justify-between px-3 pt-3 pb-2.5">
        <span className="flex items-center gap-2 font-semibold tracking-tight">
          <BrandMark size={22} />
          {APP_NAME}
        </span>
        <div className="flex gap-0.5">
          <button
            type="button"
            className="rounded-md p-1.5 text-fg-muted hover:bg-bg-muted hover:text-fg"
            aria-label="Models"
            title="Models (Ctrl+M)"
            onClick={props.onOpenModels}
          >
            <Boxes size={16} aria-hidden />
          </button>
          <button
            type="button"
            className="rounded-md p-1.5 text-fg-muted hover:bg-bg-muted hover:text-fg"
            aria-label="Settings"
            title="Settings (Ctrl+,)"
            onClick={props.onOpenSettings}
          >
            <Settings size={16} aria-hidden />
          </button>
        </div>
      </div>
      <div className="space-y-2 px-3">
        <button
          type="button"
          onClick={() => void newChat()}
          className="btn-primary flex w-full items-center justify-center gap-1.5 rounded-lg px-3 py-2 text-sm font-medium"
          title="New chat (Ctrl+N)"
        >
          <Plus size={16} aria-hidden />
          New chat
        </button>
        <label className="flex items-center gap-2 rounded-lg border border-border bg-bg px-2.5 py-1.5 focus-within:border-border-strong">
          <Search size={14} className="text-fg-muted" aria-hidden />
          <span className="sr-only">Search chats</span>
          <input
            ref={searchRef}
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Search chats (Ctrl+K)"
            className="w-full bg-transparent text-sm outline-none"
          />
        </label>
      </div>
      <nav
        className="mt-2 flex-1 overflow-y-auto px-2 pb-3"
        aria-label="Chat history"
      >
        {chats.length === 0 && (
          <p className="px-2 py-4 text-xs text-fg-muted">
            {search ? "No chats match." : "No chats yet."}
          </p>
        )}
        <ul className="space-y-0.5">
          {chats.map((c, index) => {
            // Headings by day, except in search results, which are ordered by match.
            const group = search ? null : dayGroup(c.updatedAt);
            const heading =
              group !== null &&
              (index === 0 || dayGroup(chats[index - 1]?.updatedAt ?? 0) !== group);
            return (
              <Fragment key={c.id}>
                {heading && (
                  <li
                    aria-hidden
                    className="px-2 pt-3 pb-1 text-[11px] font-medium tracking-wide text-fg-muted uppercase first:pt-1"
                  >
                    {group}
                  </li>
                )}
                <li className="group">
                  {editing === c.id ? (
                    <form
                      onSubmit={(e) => {
                        e.preventDefault();
                        const title = draft.trim();
                        if (title) void renameChat(c.id, title);
                        setEditing(null);
                      }}
                    >
                      <input
                        autoFocus
                        aria-label="Chat title"
                        value={draft}
                        onChange={(e) => setDraft(e.target.value)}
                        onBlur={() => setEditing(null)}
                        onKeyDown={(e) =>
                          e.key === "Escape" && setEditing(null)
                        }
                        className="w-full rounded-md border border-accent bg-bg px-2 py-1.5 text-sm"
                      />
                    </form>
                  ) : (
                    <div
                      className={
                        "relative flex items-center rounded-lg transition-colors duration-150 " +
                        (c.id === activeChatId
                          ? "bg-bg-muted before:absolute before:top-2 before:bottom-2 before:left-0 before:w-[3px] before:rounded-full before:bg-accent"
                          : "hover:bg-bg-muted/70")
                      }
                    >
                      <button
                        type="button"
                        onClick={() => void openChat(c.id)}
                        onDoubleClick={() => {
                          setEditing(c.id);
                          setDraft(c.title);
                        }}
                        className="min-w-0 flex-1 px-2.5 py-1.5 text-left"
                        aria-current={
                          c.id === activeChatId ? "page" : undefined
                        }
                      >
                        <span className="block truncate text-sm">
                          {c.title}
                        </span>
                        {c.snippet && (
                          <span className="block truncate text-xs text-fg-muted">
                            {c.snippet}
                          </span>
                        )}
                      </button>
                      <button
                        type="button"
                        aria-label={`Rename ${c.title}`}
                        className="hidden p-1 text-fg-muted group-hover:block hover:text-fg focus:block"
                        onClick={() => {
                          setEditing(c.id);
                          setDraft(c.title);
                        }}
                      >
                        <Pencil size={13} aria-hidden />
                      </button>
                      <button
                        type="button"
                        aria-label={`Delete ${c.title}`}
                        className="hidden p-1 pr-2 text-fg-muted group-hover:block hover:text-danger focus:block"
                        onClick={() => setConfirmDelete(c.id)}
                      >
                        <Trash2 size={13} aria-hidden />
                      </button>
                    </div>
                  )}
                </li>
              </Fragment>
            );
          })}
        </ul>
      </nav>
      <div className="border-t border-border p-2">
        <RuntimeChip onOpenModels={props.onOpenModels} />
      </div>
      <AlertDialog.Root
        open={confirmDelete !== null}
        onOpenChange={(o) => !o && setConfirmDelete(null)}
      >
        <AlertDialog.Portal>
          <AlertDialog.Overlay className="dialog-overlay fixed inset-0" />
          <AlertDialog.Content className="fixed top-1/2 left-1/2 w-96 -translate-x-1/2 -translate-y-1/2 rounded-xl border border-border bg-surface p-5 shadow-pop">
            <AlertDialog.Title className="mb-2 font-semibold">
              Delete this chat?
            </AlertDialog.Title>
            <AlertDialog.Description className="mb-4 text-sm text-fg-muted">
              Its messages are removed from this computer. This cannot be
              undone.
            </AlertDialog.Description>
            <div className="flex justify-end gap-2">
              <AlertDialog.Cancel className="rounded-md border border-border px-3 py-1.5 text-sm">
                Cancel
              </AlertDialog.Cancel>
              <AlertDialog.Action
                className="rounded-md bg-danger px-3 py-1.5 text-sm text-white"
                onClick={() =>
                  confirmDelete !== null && void deleteChat(confirmDelete)
                }
              >
                Delete
              </AlertDialog.Action>
            </div>
          </AlertDialog.Content>
        </AlertDialog.Portal>
      </AlertDialog.Root>
    </aside>
  );
}

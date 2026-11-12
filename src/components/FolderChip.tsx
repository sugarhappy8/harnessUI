// [0.0.6 item 7] Which folders the chat can use, in the chat header.
//
// The chip shows the chat's working folder (where relative paths start) and how many other
// folders are approved, or, in Full access, that the whole computer is reachable. Its menu
// changes folders only through the commands the Tools panel already uses
// (`list_approved_folders`, `approve_folder`, `remove_approved_folder`,
// `set_chat_working_folder`, granted to the main window only); nothing here is reachable by
// a tool or the model. Folder paths can contain any character, so every path is shown as
// plain text with its invisible and direction-changing characters made visible.
import { useEffect, useRef } from "react";
import { DropdownMenu } from "radix-ui";
import { Check, Folder, FolderCheck, FolderPlus, HardDrive, Trash2, Wrench } from "lucide-react";
import { create } from "zustand";
import * as ipc from "../lib/ipc";
import type { ApprovedFolder } from "../lib/types";
import { useChatMode, type ApprovalModeName } from "../lib/approvalMode";
import { MODE_LOOK } from "./AllowAllButton";
import { toErrorInfo, useApp } from "../store/app";

/** The chip when the chat has no folder (the file tools have nowhere to work). */
export const NO_FOLDER = "No folder · Choose";
/** The chip in Full access. */
export const WHOLE_COMPUTER = "Whole computer";
/** The first line of the menu when the chat has no folder. */
export const NO_FOLDER_LINE = "No folder yet, so the file tools have nowhere to work.";
/** The first sentence of the menu in Full access (what `approvals::widen` lets the tools reach). */
export const FULL_ACCESS_LEAD =
  "Full access is on, so the file and terminal tools can reach every folder on this computer except harnessUI's own.";
/** In Full access a chat with no folder works in the home folder (`approvals::widen`). */
export const HOME_FOLDER = "your home folder";
/** Under the list when a folder is approved for every chat: the chip does not remove those. */
export const ALL_CHATS_NOTE = "Folders for all chats are changed in Settings → Permissions.";

/** The menu's sentence in Full access, with the working folder as shown (already made visible). */
export function fullAccessLine(working: string | null): string {
  return `${FULL_ACCESS_LEAD} Working folder: ${working ?? HOME_FOLDER}.`;
}

/** How often the chip reads the folders again while the window is visible, so a change made
 *  in the Tools panel or in Settings shows without a reload. */
export const FOLDERS_POLL_MS = 2000;

// Characters that change how text is displayed without being visible: control characters
// (new lines and tabs too, a path is one line), bidirectional embeddings, overrides and
// isolates, zero-width characters, line and paragraph separators, variation selectors and
// tag characters. The same set as the approval cards' sanitiser (crates/policy/src/sanitize.rs)
// plus U+00AD, U+2028 and U+2029.
const INVISIBLE =
  // eslint-disable-next-line no-control-regex, no-misleading-character-class
  /[\u{0}-\u{1F}\u{7F}-\u{9F}\u{AD}\u{61C}\u{115F}\u{1160}\u{17B4}\u{17B5}\u{180B}-\u{180F}\u{200B}-\u{200F}\u{2028}-\u{202E}\u{2060}-\u{206F}\u{3164}\u{FE00}-\u{FE0F}\u{FEFF}\u{FFA0}\u{FFF9}-\u{FFFB}\u{1D173}-\u{1D17A}\u{E0000}-\u{E007F}\u{E0100}-\u{E01EF}]/gu;

/** A path as it is shown: every invisible or direction-changing character as `\u{XXXX}`,
 *  the way approval cards show them. */
export function visiblePath(text: string): string {
  return text.replace(
    INVISIBLE,
    (ch) => `\\u{${(ch.codePointAt(0) ?? 0).toString(16).toUpperCase().padStart(4, "0")}}`,
  );
}

/** The last name of a folder path ("harnessUI" for "C:\\Users\\me\\harnessUI" or
 *  "/home/me/harnessUI/"); a drive or the root as it is. */
export function folderName(path: string): string {
  const drive = /^([A-Za-z]:)[\\/]*$/.exec(path);
  if (drive) return `${drive[1]}\\`;
  const parts = path.split(/[\\/]+/).filter((p) => p.length > 0);
  return parts[parts.length - 1] ?? path;
}

/** The folder relative paths start in: the chat's stored choice while it is still approved,
 *  else the first approved folder (`turns::working_folder_for`, and the Tools panel). */
export function workingFolderOf(folders: readonly ApprovedFolder[], stored: string | null): ApprovedFolder | null {
  return folders.find((f) => f.path === stored) ?? folders[0] ?? null;
}

/** The chip's text. `folders` is undefined until they were read. */
export function chipText(
  mode: ApprovalModeName,
  folders: readonly ApprovedFolder[] | undefined,
  stored: string | null,
): string {
  if (mode === "fullAccess") return WHOLE_COMPUTER;
  if (folders === undefined) return "";
  const working = workingFolderOf(folders, stored);
  if (!working) return NO_FOLDER;
  const name = visiblePath(folderName(working.path));
  return folders.length > 1 ? `${name} + ${folders.length - 1} more` : name;
}

/** The message box's placeholder: it names the working folder when there is one. */
export function messagePlaceholder(workingName: string | null): string {
  return workingName ? `Ask anything. Files: ${workingName}` : "Ask anything, or type @ to mention a file";
}

// ---- The folders of each chat, as the window last read them --------------------------------

interface ChatFoldersState {
  /** Each chat's approved folders (global and its own), in the backend's order. */
  byChat: Record<number, ApprovedFolder[]>;
  /** Reads a chat's folders from the app. Rejects when the app refuses. */
  load: (chatId: number) => Promise<void>;
}

/** Reads started per chat; an older read that ends after a newer one is dropped. */
const reads: Record<number, number> = {};

function sameFolders(a: readonly ApprovedFolder[], b: readonly ApprovedFolder[]): boolean {
  return a.length === b.length && a.every((f, i) => f.id === b[i]?.id && f.path === b[i]?.path && f.chatId === b[i]?.chatId);
}

export const useChatFolders = create<ChatFoldersState>((set, get) => ({
  byChat: {},
  load: async (chatId) => {
    const mine = (reads[chatId] = (reads[chatId] ?? 0) + 1);
    const list = await ipc.listApprovedFolders(chatId);
    if (reads[chatId] !== mine) return;
    const known = get().byChat[chatId];
    if (known && sameFolders(known, list)) return;
    set({ byChat: { ...get().byChat, [chatId]: list } });
  },
}));

/** The working folder's last name, made visible, for the message box (null: no folder, or
 *  not read yet). */
export function useWorkingFolderName(chatId: number | null): string | null {
  const folders = useChatFolders((s) => (chatId === null ? undefined : s.byChat[chatId]));
  const stored = useApp((s) =>
    chatId !== null && s.detail?.chat.id === chatId ? (s.detail.chat.workingFolder ?? null) : null,
  );
  const working = folders ? workingFolderOf(folders, stored) : null;
  return working ? visiblePath(folderName(working.path)) : null;
}

/** Opens the Tools panel of the header the chip is in, as a click on its own button would. */
function openToolsPanel(from: HTMLElement | null) {
  const scope: ParentNode = from?.closest("header") ?? document;
  scope.querySelector<HTMLButtonElement>('button[title="Tools for this chat"]')?.click();
}

const ITEM =
  "flex cursor-default items-center gap-2 rounded-lg px-2.5 py-1.5 text-sm outline-none data-[highlighted]:bg-bg-muted";

/**
 * The chip in the chat header: the working folder's last name ("harnessUI", "harnessUI + 2
 * more"), "No folder · Choose", or "Whole computer" in Full access (in the colour of the
 * mode menu's Full access). Its menu lists the folders with their full paths and changes
 * them through the Tools panel's commands.
 */
export function FolderChip(props: { chatId: number; workingFolder: string | null }) {
  const { chatId, workingFolder } = props;
  // The mode the header's approval menu reads and keeps up to date (it loads it and
  // listens for changes; this chip is always mounted beside it).
  const mode = useChatMode(chatId);
  const folders = useChatFolders((s) => s.byChat[chatId]);
  const showError = useApp((s) => s.showError);
  const openChat = useApp((s) => s.openChat);
  const trigger = useRef<HTMLButtonElement>(null);
  // Set when "Open in Tools panel" is chosen: the panel opens once this menu has closed, so
  // the focus going back to the chip does not close the panel again.
  const toTools = useRef(false);

  // Read now, again when the stored working folder changes, when the window comes back, and
  // every few seconds while it is visible (the Tools panel and Settings change folders too).
  useEffect(() => {
    const read = () => {
      if (document.visibilityState === "hidden") return;
      void useChatFolders
        .getState()
        .load(chatId)
        .catch(() => undefined);
    };
    read();
    const timer = window.setInterval(read, FOLDERS_POLL_MS);
    window.addEventListener("focus", read);
    document.addEventListener("visibilitychange", read);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", read);
      document.removeEventListener("visibilitychange", read);
    };
  }, [chatId, workingFolder]);

  const reload = () => useChatFolders.getState().load(chatId);
  const run = (work: () => Promise<unknown>) => void work().catch((e) => showError(toErrorInfo(e)));

  const makeWorking = (path: string) =>
    run(async () => {
      await ipc.setChatWorkingFolder(chatId, path);
      if (useApp.getState().activeChatId === chatId) await openChat(chatId);
      await reload();
    });
  const add = () =>
    run(async () => {
      if (await ipc.approveFolder(chatId)) await reload();
    });
  const remove = (id: number) =>
    run(async () => {
      await ipc.removeApprovedFolder(id);
      await reload();
    });

  const full = mode === "fullAccess";
  const list = folders ?? [];
  const working = workingFolderOf(list, workingFolder);
  const text = chipText(mode, folders, workingFolder);
  const name = working ? visiblePath(folderName(working.path)) : "";
  const workingShown = working ? visiblePath(working.path) : null;
  const title = full
    ? fullAccessLine(workingShown)
    : working
      ? `Working folder: ${workingShown}. Click to see or change the folders this chat can use.`
      : folders
        ? `${NO_FOLDER_LINE} Click to choose one.`
        : "Folders for this chat";
  const Icon = full ? HardDrive : Folder;

  return (
    <DropdownMenu.Root
      onOpenChange={(open) => {
        if (open) void reload().catch((e) => showError(toErrorInfo(e)));
      }}
    >
      <DropdownMenu.Trigger asChild>
        <button
          ref={trigger}
          type="button"
          aria-label={text ? `Folders for this chat: ${text}` : "Folders for this chat"}
          title={title}
          data-folder-chip
          className={
            "flex shrink-0 items-center gap-1.5 rounded-lg px-2 py-1.5 text-sm whitespace-nowrap " +
            (full ? MODE_LOOK.fullAccess.button : "text-fg-muted hover:bg-bg-muted hover:text-fg")
          }
        >
          <Icon size={15} className="shrink-0" aria-hidden />
          {full ? (
            <span className="hidden @2xl:inline">{WHOLE_COMPUTER}</span>
          ) : working ? (
            <>
              <bdi dir="ltr" className="max-w-[5rem] truncate @3xl:max-w-[14rem]">
                {name}
              </bdi>
              {list.length > 1 && (
                <>
                  {" "}
                  <span>
                    {`+ ${list.length - 1}`}
                    <span className="hidden @3xl:inline"> more</span>
                  </span>
                </>
              )}
            </>
          ) : (
            folders && (
              <span>
                No folder
                <span className="hidden @3xl:inline">
                  {" · "}
                  <span className="text-accent">Choose</span>
                </span>
              </span>
            )
          )}
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="end"
          sideOffset={4}
          onCloseAutoFocus={(e) => {
            if (!toTools.current) return;
            toTools.current = false;
            e.preventDefault();
            openToolsPanel(trigger.current);
          }}
          className="z-50 max-h-[70vh] w-96 max-w-[92vw] overflow-y-auto rounded-xl border border-border bg-surface p-1.5 text-sm shadow-pop"
        >
          {full && (
            <p className="mx-1 mb-1.5 rounded-lg bg-danger/10 px-2.5 py-2 text-danger" data-full-access-line>
              {FULL_ACCESS_LEAD} Working folder:{" "}
              {workingShown ? (
                <bdi dir="ltr" className="font-mono text-xs break-all">
                  {workingShown}
                </bdi>
              ) : (
                HOME_FOLDER
              )}
              .
            </p>
          )}
          {!full && folders && list.length === 0 && (
            <p className="mx-1 mb-1.5 px-2.5 py-1.5 text-fg-muted">{NO_FOLDER_LINE}</p>
          )}
          {list.length > 0 && (
            <DropdownMenu.Label className="px-2.5 py-1 text-xs text-fg-muted">Folders for this chat</DropdownMenu.Label>
          )}
          {list.map((f) => {
            const shown = visiblePath(f.path);
            const isWorking = f.id === working?.id;
            return (
              <DropdownMenu.Group key={f.id} className="mb-1" aria-label={shown}>
                <div className="px-2.5 py-1" data-folder-path={f.id}>
                  <bdi dir="ltr" className="block font-mono text-xs break-all">
                    {shown}
                  </bdi>
                  <span className="mt-0.5 flex items-center gap-1 text-xs text-fg-muted">
                    {isWorking && (
                      <>
                        <Check size={12} className="text-accent" aria-hidden />
                        <span className="text-accent">Working folder</span>
                      </>
                    )}
                    {isWorking && f.chatId === null && <span aria-hidden>·</span>}
                    {f.chatId === null && <span>all chats</span>}
                  </span>
                </div>
                {!isWorking && (
                  <DropdownMenu.Item
                    className={ITEM + " pl-6"}
                    aria-label={`Make ${shown} the working folder`}
                    onSelect={() => makeWorking(f.path)}
                  >
                    <FolderCheck size={14} className="shrink-0 text-fg-muted" aria-hidden />
                    Make this the working folder
                  </DropdownMenu.Item>
                )}
                {f.chatId === chatId && (
                  <DropdownMenu.Item
                    className={ITEM + " pl-6 data-[highlighted]:text-danger"}
                    aria-label={`Remove ${shown} from this chat`}
                    onSelect={() => remove(f.id)}
                  >
                    <Trash2 size={14} className="shrink-0 text-fg-muted" aria-hidden />
                    Remove
                  </DropdownMenu.Item>
                )}
              </DropdownMenu.Group>
            );
          })}
          {list.some((f) => f.chatId === null) && (
            <p className="px-2.5 pb-1 text-xs text-fg-muted">{ALL_CHATS_NOTE}</p>
          )}
          <DropdownMenu.Separator className="my-1 h-px bg-border" />
          <DropdownMenu.Item className={ITEM} onSelect={add}>
            <FolderPlus size={14} className="shrink-0 text-fg-muted" aria-hidden />
            Add a folder…
          </DropdownMenu.Item>
          <DropdownMenu.Item
            className={ITEM}
            onSelect={() => {
              toTools.current = true;
            }}
          >
            <Wrench size={14} className="shrink-0 text-fg-muted" aria-hidden />
            Open in Tools panel
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

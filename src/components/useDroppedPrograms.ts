import { type Dispatch, type RefObject, type SetStateAction, useCallback, useEffect, useMemo, useState } from "react";
import * as ipc from "../lib/ipc";
import type { DroppedProgram } from "../lib/types";
import { toErrorInfo, useApp } from "../store/app";
import { useChatFolders } from "./FolderChip";

/**
 * Dropping a program file on the window points the message box at it (0.0.6 item 5,
 * `docs/dev/0.0.6/item5-spec.md` R22 and WP9h).
 *
 * Tauri delivers the paths of files dropped on the window (the page's own drop events carry
 * none). While rea is installed, `reverse_dropped` says for each path what the box does: a
 * program or library inside the chat's folders (anywhere but harnessUI's own folders in Full
 * access) has its full path inserted at the caret, in quotes; one outside them gets a note with
 * **Add a folder…**; one the chat can never reach gets a note saying so; anything else is left
 * alone, as before. Nothing is sent: the owner reads the box and sends it, and the model's
 * reverse tools then ask as every tool does.
 *
 * Outside the app (a plain browser, component tests) there is no drop event and no command:
 * the message box works as it did before.
 */

/** Files looked at per drop, at most (the command refuses more). */
export const MAX_DROPPED = 8;

/** A note under the message box about one dropped file. */
export interface DropNote {
  kind: "addFolder" | "notReachable";
  /** The file's name, made safe to show by the backend. */
  fileName: string;
  /** The path as dropped, looked at again once a folder is added. */
  path: string;
}

/** What a note says. */
export function dropNoteText(note: DropNote): string {
  return note.kind === "addFolder"
    ? `To let the model look inside ${note.fileName}, add its folder to this chat.`
    : `rea can't reach ${note.fileName} from this chat.`;
}

/** A path as it goes into the message box: in double quotes, or single ones if it holds a double quote. */
export function quotePath(path: string): string {
  return path.includes('"') && !path.includes("'") ? `'${path}'` : `"${path}"`;
}

/**
 * Puts `insertion` at the caret of the message box (replacing a selection), with a space on
 * each side where the text there is not already a space, and leaves the caret after it.
 */
export function insertAtCaret(
  box: HTMLTextAreaElement | null,
  setText: Dispatch<SetStateAction<string>>,
  insertion: string,
) {
  const value = box?.value ?? "";
  const start = box?.selectionStart ?? value.length;
  const end = box?.selectionEnd ?? start;
  const before = value.slice(0, start);
  const after = value.slice(end);
  const lead = before && !/\s$/.test(before) ? " " : "";
  const trail = after && !/^\s/.test(after) ? " " : "";
  const head = before + lead + insertion + trail;
  setText(head + after);
  requestAnimationFrame(() => {
    box?.focus();
    box?.setSelectionRange(head.length, head.length);
  });
}

/**
 * Whether the first bytes of a file are a program's or a library's: Windows (`MZ`), Linux
 * (ELF), Mac (Mach-O, and universal files built for several processors, which share their
 * first four bytes with Java class files) or compiled Python. The same rule as the backend's
 * (`reverse_drop::is_program_magic`).
 */
export function isProgramHead(b: Uint8Array): boolean {
  if (b.length >= 2 && b[0] === 0x4d && b[1] === 0x5a) return true;
  if (b.length < 4) return false;
  const [b0, b1, b2, b3] = [b[0]!, b[1]!, b[2]!, b[3]!];
  if (b0 === 0x7f && b1 === 0x45 && b2 === 0x4c && b3 === 0x46) return true;
  const word = ((b0 << 24) | (b1 << 16) | (b2 << 8) | b3) >>> 0;
  if ([0xfeedface, 0xcefaedfe, 0xfeedfacf, 0xcffaedfe, 0xcafebabf].includes(word)) return true;
  if (word === 0xcafebabe) {
    if (b.length < 8) return false;
    const count = ((b[4]! << 24) | (b[5]! << 16) | (b[6]! << 8) | b[7]!) >>> 0;
    return count >= 1 && count < 45;
  }
  if (b2 === 0x0d && b3 === 0x0a) {
    const magic = b0 | (b1 << 8);
    return (magic >= 3000 && magic <= 4999) || (magic >= 20121 && magic <= 62211);
  }
  return false;
}

/** The first `n` bytes of a file. */
function headOf(file: File, n: number): Promise<Uint8Array> {
  return new Promise((resolve) => {
    const reader = new FileReader();
    reader.onerror = () => resolve(new Uint8Array());
    reader.onload = () => resolve(new Uint8Array(reader.result as ArrayBuffer));
    reader.readAsArrayBuffer(file.slice(0, n));
  });
}

/** Whether a file from the page's own drop event is a program (or an Electron `.asar` archive). */
export async function isProgramFile(file: File): Promise<boolean> {
  if (file.type.startsWith("image/")) return false;
  if (file.name.toLowerCase().endsWith(".asar")) return true;
  return isProgramHead(await headOf(file, 8));
}

/** A call to the app that may not exist here (a browser, tests): nothing then. */
function quietly<T>(call: () => Promise<T>): Promise<T | undefined> {
  try {
    return call().catch(() => undefined);
  } catch {
    return Promise.resolve(undefined);
  }
}

/**
 * The message box's handling of dropped program files, for the open chat (none: nothing
 * happens). Returns whether rea is installed (the box's own drop handler then leaves program
 * files alone instead of attaching them), the notes to show under the box, **Add a folder…**,
 * and a filter for the box's own drop handler.
 */
export function useDroppedPrograms(
  chatId: number | null,
  box: RefObject<HTMLTextAreaElement | null>,
  setText: Dispatch<SetStateAction<string>>,
) {
  // Both belong to the chat they were found for: another chat starts with none.
  const [known, setKnown] = useState<{ chat: number; installed: boolean } | null>(null);
  const [noted, setNoted] = useState<{ chat: number; notes: DropNote[] } | null>(null);
  const installed = known !== null && known.chat === chatId && known.installed;
  const notes = useMemo(() => (noted !== null && noted.chat === chatId ? noted.notes : []), [noted, chatId]);
  const showError = useApp((s) => s.showError);

  // Whether rea is installed: asked for each chat, and again whenever the tools may have
  // changed (Settings → Reverse engineering tells the app so after an install or removal).
  useEffect(() => {
    if (chatId === null) return;
    let alive = true;
    const ask = () =>
      void quietly(() => ipc.reverseDropped(chatId, [])).then((a) => {
        if (alive) setKnown({ chat: chatId, installed: a?.installed ?? false });
      });
    ask();
    const unlisten = quietly(() => ipc.onMcpChanged(ask));
    return () => {
      alive = false;
      void unlisten.then((u) => u?.());
    };
  }, [chatId]);

  /** Inserts the paths to insert; returns the notes for the rest. */
  const apply = useCallback(
    (paths: string[], items: DroppedProgram[]): DropNote[] => {
      const inserts: string[] = [];
      const out: DropNote[] = [];
      items.forEach((item, i) => {
        if (item.kind === "insert") inserts.push(quotePath(item.path));
        else if (item.kind !== "ignore") out.push({ kind: item.kind, fileName: item.fileName, path: paths[i] ?? "" });
      });
      if (inserts.length > 0) insertAtCaret(box.current, setText, inserts.join(" "));
      return out;
    },
    [box, setText],
  );

  // Files dropped on the window.
  useEffect(() => {
    if (chatId === null) return;
    let alive = true;
    const onDrop = (dropped: string[]) => {
      const paths = dropped.slice(0, MAX_DROPPED);
      if (paths.length === 0) return;
      ipc
        .reverseDropped(chatId, paths)
        .then((answer) => {
          if (!alive) return;
          setKnown({ chat: chatId, installed: answer.installed });
          // Nothing to do with these files (not programs, or no rea): nothing changes.
          if (!answer.installed || answer.items.every((i) => i.kind === "ignore")) return;
          setNoted({ chat: chatId, notes: apply(paths, answer.items) });
        })
        .catch((e) => showError(toErrorInfo(e)));
    };
    const unlisten = quietly(() => ipc.onDroppedPaths(onDrop));
    return () => {
      alive = false;
      void unlisten.then((u) => u?.());
    };
  }, [chatId, apply, showError]);

  /** **Add a folder…**: the folder picker and approval the Tools panel uses; then the files waiting for a folder are looked at again. */
  const addFolder = useCallback(async () => {
    if (chatId === null) return;
    try {
      if (!(await ipc.approveFolder(chatId))) return;
      // The folder chip in the header shows the new folder.
      void useChatFolders
        .getState()
        .load(chatId)
        .catch(() => undefined);
      const waiting = notes.filter((n) => n.kind === "addFolder").map((n) => n.path);
      if (waiting.length === 0) return;
      const answer = await ipc.reverseDropped(chatId, waiting);
      const still = apply(waiting, answer.items);
      setNoted((old) => ({
        chat: chatId,
        notes: [...(old?.chat === chatId ? old.notes : []).filter((n) => n.kind !== "addFolder"), ...still],
      }));
    } catch (e) {
      showError(toErrorInfo(e));
    }
  }, [chatId, notes, apply, showError]);

  const dismiss = useCallback(() => setNoted(null), []);

  /** For the box's own drop handler: while rea is installed, program files are left to the window's drop event. */
  const withoutPrograms = useCallback(
    async (files: File[]): Promise<File[]> => {
      if (!installed) return files;
      const programs = await Promise.all(files.map(isProgramFile));
      return files.filter((_, i) => !programs[i]);
    },
    [installed],
  );

  return { installed, notes, addFolder, dismiss, withoutPrograms };
}

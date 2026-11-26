// What the app keeps of an error that stopped part of a screen from being drawn (0.2.1). The
// error boundaries show it under *Details* and write it to the developer console: the error's
// name and the start of its message, with anything that could be private left out (a file
// path, which can hold the home folder's name; a web address, which can hold a key; anything
// shaped like a key; a long quoted run, which can be the person's own text). Nothing is sent
// anywhere: the webview has no log of its own on disk. The root's handlers (rootErrorHandlers)
// keep React from also writing the raw error and its component stack to the console.
import { CRASH } from "./words";

/** At most this many characters of a message are kept. */
export const MESSAGE_LIMIT = 200;

const HIDDEN = "…";

/** Patterns of the private parts a message can carry, each replaced by an ellipsis (`$1` keeps
 *  what a pattern matched before the part). No lookbehind: Safari before 16.4 cannot read it. */
const PRIVATE: [RegExp, string][] = [
  // Web and app addresses (https://…, ipc://…, asset://…), whose query can hold a key.
  [/\b[a-z][a-z0-9+.-]*:\/\/\S+/gi, HIDDEN],
  // Windows paths: C:\Users\name\…, C:/Users/…, \\server\share\….
  [/\b[a-z]:[\\/][^\s'"`<>|]*/gi, HIDDEN],
  [/\\\\[^\s'"`<>|]+/g, HIDDEN],
  // Home-relative and absolute POSIX paths with at least one folder: ~/…, /home/name/….
  [/~[\\/][^\s'"`<>|]*/g, HIDDEN],
  [/(^|[\s'"`(=:,])\/(?:[^\s/'"`<>|]+\/)+[^\s'"`<>|]*/g, `$1${HIDDEN}`],
  // Key-shaped words: sk-…, hf_…, ghp_…, xoxb-…, and long runs of letters and digits.
  [/\b(?:sk|pk|rk|hf|ghp|gho|ghs|github_pat|xox[a-z])[-_][\w-]{6,}/gi, HIDDEN],
  [/\b(?=[\w-]*\d)[\w-]{32,}\b/g, HIDDEN],
  // A quoted run of 40 characters or more (a JSON parser quotes what it could not read).
  [/(["'`])[^"'`]{40,}\1/g, `$1${HIDDEN}$1`],
];

/** At most this much of a message is read: the patterns above cost more on longer text. */
const READ_LIMIT = MESSAGE_LIMIT * 4;

/** `text` with its private parts left out and cut to MESSAGE_LIMIT characters. */
export function cleanMessage(text: string): string {
  let out = text.slice(0, READ_LIMIT);
  for (const [pattern, by] of PRIVATE) out = out.replace(pattern, by);
  out = out.replace(/\s+/g, " ").trim();
  const cut = out.length > MESSAGE_LIMIT || text.length > READ_LIMIT;
  return cut ? `${out.slice(0, MESSAGE_LIMIT - 1)}${HIDDEN}` : out;
}

/** `value` as text, or `fallback` when it cannot be made one (an object with no prototype, a
 *  `toString` that throws). */
function asText(value: unknown, fallback: string): string {
  if (typeof value === "string") return value;
  try {
    return String(value);
  } catch {
    return fallback;
  }
}

/** A property of `error`, or undefined when reading it throws. */
function read(error: Error, key: "name" | "message"): unknown {
  try {
    return error[key];
  } catch {
    return undefined;
  }
}

/** The thrown value as an Error (a string or anything else thrown becomes one). */
export function asError(thrown: unknown): Error {
  if (thrown instanceof Error) return thrown;
  return new Error(asText(thrown, "Unknown error"));
}

/** What is kept of `thrown`: its name and its cleaned message. */
export function crashSummary(thrown: unknown): { name: string; message: string } {
  const error = asError(thrown);
  const raw = read(error, "name");
  const name = typeof raw === "string" && /^[A-Za-z][\w$]{0,40}$/.test(raw) ? raw : "Error";
  const message = read(error, "message");
  return { name, message: cleanMessage(message === undefined ? "" : asText(message, "")) };
}

/** Writes what is kept of `thrown` to the developer console, with the part of the app that
 *  could not be drawn (a component's name). Never the stack or the component tree, which name
 *  the app's files. */
export function reportCrash(where: string, thrown: unknown): void {
  const { name, message } = crashSummary(thrown);
  console.error(`[crash] ${where} could not be drawn: ${name}: ${message}`);
}

/** The options for `createRoot` (main.tsx and the preview). An error a boundary caught is
 *  reported by that boundary (reportCrash), so React's own report, which carries the raw
 *  message and the component stack, is left out. An error no boundary caught (the root's own
 *  notice failed) is reported the same way, and a plain notice is written in the window in
 *  place of the app, with Reload: never a blank window. */
export function rootErrorHandlers(container: HTMLElement) {
  return {
    onCaughtError: () => {},
    onUncaughtError: (thrown: unknown) => {
      reportCrash("App", thrown);
      const box = document.createElement("div");
      box.setAttribute("role", "alert");
      box.className = "m-auto flex max-w-md flex-col gap-3 p-6 text-fg";
      const title = document.createElement("p");
      title.className = "font-semibold";
      title.textContent = CRASH.title;
      const next = document.createElement("p");
      next.className = "text-sm text-fg-muted";
      next.textContent = `${CRASH.safe} ${CRASH.nextAgain}`;
      const reload = document.createElement("button");
      reload.type = "button";
      reload.className = "w-fit rounded-lg border border-border px-3 py-1.5 text-sm";
      reload.textContent = CRASH.reload;
      reload.addEventListener("click", () => window.location.reload());
      box.append(title, next, reload);
      container.replaceChildren(box);
    },
  };
}

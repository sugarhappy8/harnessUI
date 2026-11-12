// Changing what is loaded (0.0.5 D3): Reload, Apply and restart, Unload and picking another
// local model ask first when the model is busy ("A reply is being written. Stop it and
// restart the model?"). The backend does the waiting (`when_idle`); this module asks the
// question, remembers which change waits (for the indicator's waiting line) and notes a
// restart (the first reply after it is slower).
//
// No React here: questions are shown by a `QuestionHost` (ModelSettings.tsx) that is on
// screen with the chat or the first-run screen. Callers pass the runtime status. The app
// store imports this module and this module reads the store only inside `reloadModel`
// (never while the modules load), so the two can import each other.
import { create } from "zustand";
import * as ipc from "../lib/ipc";
import type { RuntimeStatus } from "../lib/types";
import { busyQuestion, TOO_LARGE_QUESTION, type WaitingChange } from "../lib/words";
import { useApp } from "./app";

// ---- Questions -------------------------------------------------------------------------

export interface QuestionChoice {
  label: string;
  value: string;
  /** The look of the button; the last choice is the default action. */
  tone?: "primary" | "danger";
}

export interface Question {
  id: number;
  text: string;
  /** The label of the button that answers null. */
  cancel: string;
  choices: QuestionChoice[];
  /** The D3 question: the host re-asks `model_busy` while it is open and keeps the text current. */
  busy: boolean;
}

interface QuestionState {
  question: Question | null;
  /** How many hosts are on screen (one in practice). */
  hosts: number;
}

export const useQuestion = create<QuestionState>(() => ({ question: null, hosts: 0 }));

let open: { id: number; resolve: (value: string | null) => void } | null = null;
let nextId = 1;

/** Shows a question and resolves with the chosen value, or null for Cancel. A newer question cancels an open one. */
export function ask(question: Omit<Question, "id">): Promise<string | null> {
  if (open) {
    const previous = open;
    open = null;
    previous.resolve(null);
  }
  const id = nextId++;
  if (useQuestion.getState().hosts === 0) {
    // No host on screen (only before the first screen has rendered): ask the plain way.
    const last = question.choices[question.choices.length - 1];
    return Promise.resolve(window.confirm(question.text) && last ? last.value : null);
  }
  return new Promise((resolve) => {
    open = { id, resolve };
    useQuestion.setState({ question: { ...question, id } });
  });
}

/** Answers question `id`; a stale id (a question already replaced) is ignored. */
export function answerQuestion(id: number, value: string | null): void {
  if (!open || open.id !== id) return;
  const current = open;
  open = null;
  useQuestion.setState({ question: null });
  current.resolve(value);
}

/** Replaces the text of open question `id` (the busy counts changed while it was open). */
export function updateQuestionText(id: number, text: string): void {
  const q = useQuestion.getState().question;
  if (q && q.id === id && q.text !== text) useQuestion.setState({ question: { ...q, text } });
}

/** Called by a question host when it mounts; the returned function unregisters it. */
export function registerQuestionHost(): () => void {
  useQuestion.setState((s) => ({ hosts: s.hosts + 1 }));
  return () => useQuestion.setState((s) => ({ hosts: Math.max(0, s.hosts - 1) }));
}

// ---- D3 --------------------------------------------------------------------------------

/** How to make a change: `whenIdle` absent when nothing used the model (no question was asked). */
export interface ChangeAnswer {
  whenIdle?: boolean;
}

/**
 * What the backend is told for an answer (`when_idle`). With no question it is told to
 * wait too: when nothing uses the model it acts at once, and a reply or another app's
 * request that started after the check (or while the count could not be read) is waited
 * for, with the waiting line and its Cancel, instead of being stopped unasked. Only "Stop
 * and restart" stops what uses the model.
 */
export function backendWhenIdle(answer: ChangeAnswer): boolean {
  return answer.whenIdle ?? true;
}

/**
 * The D3 question, asked only when a local model is loaded (or loading) and something uses
 * it. Resolves `{}` when nothing uses it (no question; see `backendWhenIdle`),
 * `{whenIdle: true}` for "Restart when it ends", `{whenIdle: false}` for "Stop and
 * restart", and null for Cancel.
 */
export async function askBeforeChange(runtime: RuntimeStatus | null): Promise<ChangeAnswer | null> {
  if (!runtime || (runtime.state !== "ready" && runtime.state !== "starting")) return {};
  let text: string | null;
  try {
    text = busyQuestion(await ipc.modelBusy());
  } catch {
    // The counter could not be read. No question (its text would be a guess); the change
    // is sent to wait, so it stops nothing (see `backendWhenIdle`).
    return {};
  }
  if (text === null) return {};
  const choice = await ask({
    text,
    cancel: "Cancel",
    choices: [
      { label: "Restart when it ends", value: "wait" },
      { label: "Stop and restart", value: "now", tone: "danger" },
    ],
    busy: true,
  });
  if (choice === "wait") return { whenIdle: true };
  if (choice === "now") return { whenIdle: false };
  return null;
}

/** "Use" on a model marked Too large (WP2): true for Load anyway. */
export async function askTooLarge(): Promise<boolean> {
  const choice = await ask({
    text: TOO_LARGE_QUESTION,
    cancel: "Cancel",
    choices: [{ label: "Load anyway", value: "load", tone: "primary" }],
    busy: false,
  });
  return choice === "load";
}

interface WaitingState {
  /** The change the user asked to wait for the model to be free, while its call runs. */
  change: WaitingChange | null;
  /**
   * The change whose call runs now, asked to wait or not: when the backend waits on its
   * own (something started to use the model after the check), the waiting line it shows
   * (`reloadWaiting`) names this change.
   */
  kind: WaitingChange | null;
  token: number;
}

export const useWaiting = create<WaitingState>(() => ({ change: null, kind: null, token: 0 }));

/**
 * Runs a change of what is loaded. With `whenIdle` (the user's "Restart when it ends") the
 * call can last until the reply ends, so the change is remembered meanwhile and the waiting
 * line shows at once. Resolves with the wrapper's answer: false when the change was
 * cancelled or replaced.
 */
export async function runChange(
  change: WaitingChange,
  whenIdle: boolean | undefined,
  run: () => Promise<boolean>,
): Promise<boolean> {
  const token = useWaiting.getState().token + 1;
  useWaiting.setState({ change: whenIdle ? change : null, kind: change, token });
  try {
    return await run();
  } finally {
    if (useWaiting.getState().token === token) useWaiting.setState({ change: null, kind: null });
  }
}

/** Shows "The first reply after a restart takes longer…" until the next reply ends. */
export const useRestartNote = create<{ shown: boolean }>(() => ({ shown: false }));

/**
 * Reload (`force`: restart even when nothing changed, which also retries the graphics card
 * after a step-down) or Apply and restart (no `force`). `onStart` runs once the question
 * is answered (a panel closes then, not before). Resolves true when it restarted.
 */
export async function reloadModel(
  runtime: RuntimeStatus | null,
  force: boolean,
  onStart?: () => void,
): Promise<boolean> {
  const answer = await askBeforeChange(runtime);
  if (!answer) return false;
  onStart?.();
  // Whether the model really restarts (it goes through "starting"): without `force` the
  // backend restarts nothing when the loaded copy already uses the settings, and then the
  // header must not say the next reply is slower.
  let restarted = false;
  const unsubscribe = useApp.subscribe((s) => {
    if (s.runtime?.state === "starting") restarted = true;
  });
  let done: boolean;
  try {
    done = await runChange("reload", answer.whenIdle, () => ipc.reloadLocalModel(force, backendWhenIdle(answer)));
  } finally {
    unsubscribe();
  }
  if (done && restarted) useRestartNote.setState({ shown: true });
  return done;
}

/** "Unload to free memory". Resolves true when it unloaded. */
export async function unloadModel(runtime: RuntimeStatus | null): Promise<boolean> {
  const answer = await askBeforeChange(runtime);
  if (!answer) return false;
  return runChange("unload", answer.whenIdle, () => ipc.unloadModel(backendWhenIdle(answer)));
}

/** Loads `path` after the D3 question (first run). Resolves false when it was cancelled. */
export async function switchModel(runtime: RuntimeStatus | null, path: string): Promise<boolean> {
  const answer = await askBeforeChange(runtime);
  if (!answer) return false;
  return runChange("switch", answer.whenIdle, () => ipc.loadLocalModel(path, null, backendWhenIdle(answer)));
}

/** Whether `path` is the local model loaded or loading now (picking it again loads nothing). */
export function isLoadedNow(runtime: RuntimeStatus | null, path: string): boolean {
  return !!runtime && runtime.modelPath === path && (runtime.state === "ready" || runtime.state === "starting");
}

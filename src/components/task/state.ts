import { create } from "zustand";
import * as api from "./api";
import type { Task } from "./types";

/** Whether this is the app's own window. Anywhere else (a test, a plain browser) no
 *  listener is started. */
function inApp(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** Why a chat's task could not be shown. */
export interface TaskProblem {
  code: string;
  message: string;
}

interface TaskState {
  /** Each chat's task as last read: null when it has none. Absent: not read yet. */
  byChat: Record<number, Task | null>;
  problems: Record<number, TaskProblem | undefined>;
  /** Reads a chat's task from the app. Never rejects. */
  load: (chatId: number) => Promise<void>;
  /** Keeps what a command answered. */
  put: (chatId: number, task: Task | null) => void;
  setProblem: (chatId: number, problem: TaskProblem | undefined) => void;
}

/** A command's error as a short problem the tab can show. */
export function toProblem(err: unknown): TaskProblem {
  if (err && typeof err === "object" && "message" in err) {
    const e = err as { code?: unknown; message?: unknown };
    return { code: typeof e.code === "string" ? e.code : "unexpected", message: String(e.message) };
  }
  return { code: "unexpected", message: "Something went wrong. Try again." };
}

export const useTasks = create<TaskState>((set, get) => ({
  byChat: {},
  problems: {},
  load: async (chatId) => {
    startTasks();
    try {
      const task = await api.getTask(chatId);
      set({ byChat: { ...get().byChat, [chatId]: task }, problems: { ...get().problems, [chatId]: undefined } });
    } catch (err) {
      set({ problems: { ...get().problems, [chatId]: toProblem(err) } });
    }
  },
  put: (chatId, task) =>
    set({ byChat: { ...get().byChat, [chatId]: task }, problems: { ...get().problems, [chatId]: undefined } }),
  setProblem: (chatId, problem) => set({ problems: { ...get().problems, [chatId]: problem } }),
}));

let started = false;

/** Starts listening for task changes: once, and only inside the app. If listening cannot
 *  start, the next read tries again. */
export function startTasks(): void {
  if (started || !inApp()) return;
  started = true;
  api
    .onTaskChanged((chatId) => {
      void useTasks.getState().load(chatId);
    })
    .catch(() => {
      started = false;
    });
}

/** For tests: forgets everything. */
export function resetTasks(): void {
  started = false;
  useTasks.setState({ byChat: {}, problems: {} });
}

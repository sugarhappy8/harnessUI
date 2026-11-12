// The task commands (src-tauri/src/task.rs), as typed calls.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Task, TaskEdit, TaskStatus } from "./types";

/** The chat's task, or null when it has none. */
export const getTask = (chatId: number) => invoke<Task | null>("get_task", { chatId });

/** The user's correction; starts a task when the chat has none. */
export const editTask = (chatId: number, edit: TaskEdit) => invoke<Task | null>("edit_task", { chatId, edit });

/** Pause (`paused`), Resume or Reopen (`active`), Mark done (`done`). */
export const setTaskStatus = (chatId: number, status: TaskStatus) =>
  invoke<Task | null>("set_task_status", { chatId, status });

/** Removes the chat's task. */
export const clearTask = (chatId: number) => invoke<void>("clear_task", { chatId });

/** A chat's task changed (a tool call, the user's edit, the start of a turn). */
export function onTaskChanged(handler: (chatId: number) => void): Promise<UnlistenFn> {
  return listen<number>("task-changed", (e) => handler(e.payload));
}

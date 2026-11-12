// The chat's task as the app sends it (src-tauri/src/task.rs, `TaskDto`).

export type TaskStatus = "active" | "paused" | "done";
export type StepStatus = "todo" | "doing" | "done";
export type FileChange = "created" | "edited" | "deleted";

export interface TaskItem {
  text: string;
  /** The user wrote it. */
  byUser: boolean;
}

export interface TaskStep {
  text: string;
  status: StepStatus;
  byUser: boolean;
}

export interface ChangedFile {
  /** The path as it is, for copying. */
  path: string;
  /** The path for showing: invisible characters as escapes. */
  shown: string;
  kind: FileChange;
  at: number;
}

export interface TaskCheck {
  command: string;
  passed: boolean;
  /** It ran to its end. One that did not (timed out, stopped) has no verdict. */
  finished: boolean;
  at: number;
  /** One line of the command's output: text from outside, shown as plain text. */
  summary: string;
  exitCode: number | null;
}

export interface TaskNote {
  text: string;
  at: number;
}

export interface Task {
  chatId: number;
  status: TaskStatus;
  updatedAt: number;
  goal: string;
  goalByUser: boolean;
  constraints: TaskItem[];
  steps: TaskStep[];
  /** The most recently changed first. */
  changedFiles: ChangedFile[];
  filesNotListed: number;
  /** The most recent first. */
  checks: TaskCheck[];
  nextAction: string;
  nextActionByUser: boolean;
  /** The newest first. */
  notes: TaskNote[];
}

/** The user's correction. A field left out stays as it is. */
export interface TaskEdit {
  goal?: string;
  constraints?: string[];
  steps?: { text: string; status: StepStatus }[];
  nextAction?: string;
}

import type { ReactNode } from "react";
import type { StepStatus, TaskStatus } from "./types";

export const STATUS_LABEL: Record<TaskStatus, string> = {
  active: "Active",
  paused: "Paused",
  done: "Done",
};

/** The task's status, as a small chip. */
export function StatusChip(props: { status: TaskStatus }) {
  const tone =
    props.status === "active"
      ? "bg-accent/10 text-accent"
      : props.status === "paused"
        ? "bg-warning/10 text-warning"
        : "bg-success/10 text-success";
  return (
    <span
      data-task-status={props.status}
      className={"rounded-md px-1.5 py-0.5 text-xs font-medium " + tone}
      aria-label={`Status: ${STATUS_LABEL[props.status].toLowerCase()}`}
    >
      {STATUS_LABEL[props.status]}
    </span>
  );
}

/** Marks text the user wrote, so it can be told apart from what the model wrote. */
export function ByYou() {
  return <span className="ml-1.5 rounded bg-bg-muted px-1 text-[11px] text-fg-muted">written by you</span>;
}

export const STEP_MARK: Record<StepStatus, string> = {
  todo: "○",
  doing: "◐",
  done: "●",
};

export const STEP_LABEL: Record<StepStatus, string> = {
  todo: "to do",
  doing: "in progress",
  done: "done",
};

/** One part of the tab: a heading, an optional action, and its content. */
export function Section(props: { title: string; action?: ReactNode; children: ReactNode }) {
  return (
    <section aria-label={props.title} className="rounded-xl border border-border bg-surface p-3 text-sm shadow-soft">
      <header className="mb-1.5 flex items-center justify-between gap-2">
        <h3 className="text-xs font-medium text-fg-muted">{props.title}</h3>
        {props.action}
      </header>
      {props.children}
    </section>
  );
}

export function when(ms: number): string {
  return new Date(ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

/** A small text button for a section's action. */
export function LinkButton(props: { label: string; onClick: () => void; ariaLabel?: string; disabled?: boolean }) {
  return (
    <button
      type="button"
      aria-label={props.ariaLabel}
      disabled={props.disabled}
      onClick={props.onClick}
      className="rounded px-1.5 py-0.5 text-xs text-accent hover:bg-bg-muted disabled:opacity-50"
    >
      {props.label}
    </button>
  );
}

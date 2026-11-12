import { useState } from "react";
import { Plus, Trash2 } from "lucide-react";
import { STEP_LABEL, STEP_MARK } from "./TaskParts";
import type { StepStatus, TaskStep } from "./types";

const field =
  "w-full rounded-md border border-border bg-bg px-2 py-1 text-sm focus:border-accent focus:outline-none";

function Buttons(props: { onApply: () => void; onCancel: () => void; busy: boolean }) {
  return (
    <div className="mt-2 flex justify-end gap-2">
      <button
        type="button"
        onClick={props.onCancel}
        className="rounded-md border border-border px-2.5 py-1 text-xs hover:bg-bg-muted"
      >
        Cancel
      </button>
      <button
        type="button"
        disabled={props.busy}
        onClick={props.onApply}
        className="btn-primary rounded-md px-2.5 py-1 text-xs disabled:opacity-50"
      >
        Apply
      </button>
    </div>
  );
}

/** Edits one piece of text (the goal, the next action). */
export function TextEditor(props: {
  label: string;
  value: string;
  multiline?: boolean;
  busy: boolean;
  onApply: (value: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(props.value);
  return (
    <div>
      {props.multiline ? (
        <textarea
          aria-label={props.label}
          className={field + " min-h-16 resize-y"}
          value={value}
          autoFocus
          onChange={(e) => setValue(e.target.value)}
        />
      ) : (
        <input
          aria-label={props.label}
          className={field}
          value={value}
          autoFocus
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") props.onApply(value);
            if (e.key === "Escape") props.onCancel();
          }}
        />
      )}
      <Buttons busy={props.busy} onApply={() => props.onApply(value)} onCancel={props.onCancel} />
    </div>
  );
}

/** Edits the constraints: one per line. */
export function ConstraintsEditor(props: {
  value: string[];
  busy: boolean;
  onApply: (value: string[]) => void;
  onCancel: () => void;
}) {
  const [text, setText] = useState(props.value.join("\n"));
  const lines = () =>
    text
      .split("\n")
      .map((l) => l.trim())
      .filter((l) => l !== "");
  return (
    <div>
      <textarea
        aria-label="Constraints, one per line"
        className={field + " min-h-20 resize-y"}
        value={text}
        autoFocus
        onChange={(e) => setText(e.target.value)}
      />
      <Buttons busy={props.busy} onApply={() => props.onApply(lines())} onCancel={props.onCancel} />
    </div>
  );
}

export const NEXT_STATUS: Record<StepStatus, StepStatus> = { todo: "doing", doing: "done", done: "todo" };

/** Edits the steps: their text, status and order of entry. */
export function StepsEditor(props: {
  value: TaskStep[];
  busy: boolean;
  onApply: (value: { text: string; status: StepStatus }[]) => void;
  onCancel: () => void;
}) {
  const [rows, setRows] = useState(props.value.map((s) => ({ text: s.text, status: s.status })));
  const change = (i: number, row: { text: string; status: StepStatus }) =>
    setRows(rows.map((r, j) => (j === i ? row : r)));
  return (
    <div>
      <ol className="space-y-1.5">
        {rows.map((row, i) => (
          <li key={i} className="flex items-center gap-1.5">
            <button
              type="button"
              className="w-6 shrink-0 text-center text-accent"
              aria-label={`Step ${i + 1} is ${STEP_LABEL[row.status]}. Change it.`}
              onClick={() => change(i, { ...row, status: NEXT_STATUS[row.status] })}
            >
              {STEP_MARK[row.status]}
            </button>
            <input
              aria-label={`Step ${i + 1}`}
              className={field}
              value={row.text}
              onChange={(e) => change(i, { ...row, text: e.target.value })}
            />
            <button
              type="button"
              aria-label={`Remove step ${i + 1}`}
              className="shrink-0 rounded p-1 text-fg-muted hover:bg-bg-muted"
              onClick={() => setRows(rows.filter((_, j) => j !== i))}
            >
              <Trash2 size={14} aria-hidden />
            </button>
          </li>
        ))}
      </ol>
      <button
        type="button"
        className="mt-1.5 inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-xs text-accent hover:bg-bg-muted"
        onClick={() => setRows([...rows, { text: "", status: "todo" }])}
      >
        <Plus size={12} aria-hidden /> Add a step
      </button>
      <Buttons
        busy={props.busy}
        onApply={() => props.onApply(rows.filter((r) => r.text.trim() !== ""))}
        onCancel={props.onCancel}
      />
    </div>
  );
}

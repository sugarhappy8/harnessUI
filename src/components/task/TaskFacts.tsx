import { useState, type ReactNode } from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { CheckCircle2, CircleAlert, CircleSlash, Copy } from "lucide-react";
import { when } from "./TaskParts";
import type { ChangedFile, TaskCheck } from "./types";

function inApp(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

async function copy(text: string): Promise<void> {
  if (inApp()) await writeText(text);
  else await navigator.clipboard.writeText(text);
}

const KIND: Record<ChangedFile["kind"], string> = { created: "new", edited: "edited", deleted: "deleted" };

/** Files the work changed with the file tools, newest first. A click copies the path. */
export function ChangedFiles(props: { files: ChangedFile[]; notListed: number }) {
  const [copied, setCopied] = useState<string | null>(null);
  if (props.files.length === 0) {
    return <p className="text-xs text-fg-muted">None yet.</p>;
  }
  return (
    <>
      <ul className="space-y-0.5" aria-label="Changed files">
        {props.files.map((f) => (
          <li key={f.path}>
            <button
              type="button"
              title="Copy the path"
              data-path={f.path}
              onClick={() => {
                void copy(f.path).then(
                  () => setCopied(f.path),
                  () => setCopied(null),
                );
              }}
              className="group flex w-full items-start gap-2 rounded px-1 py-0.5 text-left hover:bg-bg-muted"
            >
              <span
                className={
                  "mt-0.5 w-12 shrink-0 text-[11px] " +
                  (f.kind === "deleted" ? "text-danger" : f.kind === "created" ? "text-success" : "text-fg-muted")
                }
              >
                {KIND[f.kind]}
              </span>
              <span className="min-w-0 flex-1 font-mono text-xs break-all">{f.shown}</span>
              <span className="shrink-0 text-[11px] text-fg-muted">
                {copied === f.path ? "copied" : <Copy size={12} className="opacity-0 group-hover:opacity-100" aria-hidden />}
              </span>
            </button>
          </li>
        ))}
      </ul>
      {props.notListed > 0 && (
        <p className="mt-1 text-xs text-fg-muted">
          {props.notListed === 1 ? "One earlier file is" : `${props.notListed} earlier files are`} not listed.
        </p>
      )}
    </>
  );
}

function verdict(c: TaskCheck): { label: string; tone: string; icon: ReactNode } {
  if (!c.finished) {
    return { label: "did not finish", tone: "text-warning", icon: <CircleSlash size={14} aria-hidden /> };
  }
  if (c.passed) {
    return { label: "passed", tone: "text-success", icon: <CheckCircle2 size={14} aria-hidden /> };
  }
  return {
    label: c.exitCode !== null ? `failed (exit code ${c.exitCode})` : "failed",
    tone: "text-danger",
    icon: <CircleAlert size={14} aria-hidden />,
  };
}

/** Tests, builds, lints and type checks that ran, the most recent first. */
export function Checks(props: { checks: TaskCheck[] }) {
  if (props.checks.length === 0) {
    return <p className="text-xs text-fg-muted">None yet.</p>;
  }
  return (
    <ul className="space-y-1.5" aria-label="Checks">
      {props.checks.map((c) => {
        const v = verdict(c);
        return (
          <li key={c.command} data-check-passed={c.passed} className="text-xs">
            <div className="flex items-start gap-2">
              <span className={"mt-0.5 shrink-0 " + v.tone}>{v.icon}</span>
              <span className="min-w-0 flex-1">
                <code className="font-mono break-all">{c.command}</code>
                <span className={"ml-1.5 font-medium " + v.tone}>{v.label}</span>
                <span className="ml-1.5 text-fg-muted">{when(c.at)}</span>
              </span>
            </div>
            {c.summary && <p className="mt-0.5 ml-6 break-words whitespace-pre-wrap text-fg-muted">{c.summary}</p>}
          </li>
        );
      })}
    </ul>
  );
}

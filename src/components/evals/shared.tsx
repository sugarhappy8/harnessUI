import { useEffect, useState } from "react";
import { AlertDialog } from "radix-ui";
import { FlaskConical } from "lucide-react";
import * as ipc from "../../lib/ipc";
import type { ModelOption, Provider } from "../../lib/types";
import { useApp } from "../../store/app";
import type { EvalProgress } from "./types";

/** Clicks on the start button are ignored for this long after the dialog opens. */
export const CONFIRM_INPUT_DELAY_MS = 700;

/** What starting a capability check does, in the words the user reads before it starts.
 *  `host` is the endpoint's host for a model of an endpoint, `null` for a local model. */
export function checkConfirm(host: string | null): string[] {
  return [
    "The app gives the model three short tasks and tries each with a few settings.",
    "The model gets only tools to read and change files, and only in a temporary folder made for the check. It cannot run commands, and the app runs nothing the model writes. The folder is removed afterwards.",
    "Nothing is asked while it runs: anything that would need your approval is refused.",
    "The settings that work best are saved for this model and used in your chats, unless you chose a setting yourself.",
    host === null
      ? "It can take a few minutes. Nothing leaves this computer."
      : `It can take a few minutes. The tasks and the model's replies go to ${host}.`,
  ];
}

/** What starting a benchmark does. There is no sandbox around the code the model writes,
 *  and the text says so. */
export function benchConfirm(host: string | null): string[] {
  return [
    "The app gives the model a fixed set of small coding tasks, one at a time, each in a temporary folder that is removed afterwards.",
    "The model writes code, and the app runs it on this computer with your rights: the task's tests, the model's scripts, and the app's own checks of the result.",
    "The app lets the model run only the task's tests and scripts and commands that look at files, and refuses everything else. Nothing is asked while it runs.",
    "The app cannot see inside code the model writes. Such code could read or change files elsewhere on this computer, or go online.",
    "Run the benchmark only with a model you trust.",
    "Tasks that need Python or Node.js are skipped when they are not installed.",
    host === null
      ? "It can take a long time. The results stay on this computer."
      : `It can take a long time. The tasks and the model's replies go to ${host}. The results stay on this computer.`,
  ];
}

/** The host a model's requests go to: `null` for a local model, else its endpoint's host. */
export function useEndpointHost(modelRef: string | null): string | null {
  const [providers, setProviders] = useState<Provider[]>([]);
  const remote = modelRef?.startsWith("remote:") ?? false;
  useEffect(() => {
    if (!remote) return;
    let alive = true;
    ipc
      .listProviders()
      .then((all) => {
        if (alive) setProviders(all);
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [remote]);
  if (!modelRef || !remote) return null;
  const id = Number(modelRef.split(":")[1]);
  const provider = providers.find((p) => p.id === id);
  if (!provider) return "its endpoint";
  try {
    return new URL(provider.baseUrl).hostname || provider.baseUrl;
  } catch {
    return provider.baseUrl;
  }
}

/** The models a run can use: the loaded local model first, then the endpoints' models. */
export function useModelChoice() {
  const runtime = useApp((s) => s.runtime);
  const [options, setOptions] = useState<ModelOption[]>([]);
  const [chosen, setChosen] = useState<string | null>(null);
  const loaded = runtime?.modelPath ? `local:${runtime.modelPath}` : null;
  useEffect(() => {
    let alive = true;
    ipc
      .listModelOptions()
      .then((all) => {
        if (alive) setOptions(all);
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [loaded]);
  const usable = options.filter((o) => o.source === "provider" || o.ref === loaded);
  if (loaded && !usable.some((o) => o.ref === loaded)) {
    usable.unshift({ ref: loaded, label: runtime?.modelName ?? "Loaded model", source: "local", contextLength: null });
  }
  const value = chosen && usable.some((o) => o.ref === chosen) ? chosen : (usable[0]?.ref ?? null);
  return { options: usable, value, setChosen };
}

export function ModelSelect(props: {
  id: string;
  options: ModelOption[];
  value: string | null;
  disabled?: boolean;
  onChange: (ref: string) => void;
}) {
  if (props.options.length === 0) {
    return <p className="text-sm text-fg-muted">Load a local model or add an endpoint first.</p>;
  }
  return (
    <select
      id={props.id}
      value={props.value ?? ""}
      disabled={props.disabled}
      onChange={(e) => props.onChange(e.target.value)}
      className="min-w-0 rounded-md border border-border bg-bg px-2 py-1 text-sm"
    >
      {props.options.map((o) => (
        <option key={o.ref} value={o.ref}>
          {o.source === "local" ? `${o.label} (loaded)` : o.label}
        </option>
      ))}
    </select>
  );
}

/** The latest progress of a running check or benchmark. */
export function useEvalProgress(kind: EvalProgress["kind"], running: boolean) {
  const [progress, setProgress] = useState<EvalProgress | null>(null);
  useEffect(() => {
    if (!running) return;
    let off: (() => void) | undefined;
    let gone = false;
    void ipc
      .onEvalProgress((p) => {
        if (p.kind === kind) setProgress(p);
      })
      .then((unlisten) => {
        if (gone) unlisten();
        else off = unlisten;
      });
    return () => {
      gone = true;
      off?.();
      setProgress(null);
    };
  }, [kind, running]);
  return running ? progress : null;
}

export function ProgressLine(props: { progress: EvalProgress | null; idle: string }) {
  const p = props.progress;
  const text = p && p.total > 0 ? `${p.current || "Finishing"} (${Math.min(p.done + 1, p.total)} of ${p.total})` : props.idle;
  return (
    <p role="status" className="text-xs text-fg-muted">
      {text}
    </p>
  );
}

/** The plain-words confirmation shown before a run starts. Cancel has the focus. */
export function RunConfirm(props: {
  open: boolean;
  title: string;
  lines: readonly string[];
  model: string;
  startLabel: string;
  onOpenChange: (open: boolean) => void;
  onStart: () => void;
}) {
  return (
    <AlertDialog.Root open={props.open} onOpenChange={props.onOpenChange}>
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="dialog-overlay fixed inset-0" />
        <AlertDialog.Content className="fixed top-1/2 left-1/2 flex max-h-[90vh] w-[32rem] max-w-[92vw] -translate-x-1/2 -translate-y-1/2 flex-col rounded-2xl border border-border bg-surface text-sm shadow-pop">
          <div className="min-h-0 flex-1 overflow-y-auto p-5 pb-3">
            <AlertDialog.Title className="flex items-center gap-2 text-base font-semibold">
              <FlaskConical size={18} className="shrink-0 text-accent" aria-hidden />
              {props.title}
            </AlertDialog.Title>
            <AlertDialog.Description className="mt-2 text-fg-muted">Model: {props.model}</AlertDialog.Description>
            <ul aria-label="What happens" className="mt-3 list-disc space-y-1 pl-5 text-fg-muted">
              {props.lines.map((line) => (
                <li key={line}>{line}</li>
              ))}
            </ul>
          </div>
          {props.open && (
            <StartButtons
              label={props.startLabel}
              onStart={() => {
                props.onOpenChange(false);
                props.onStart();
              }}
            />
          )}
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}

function StartButtons(props: { label: string; onStart: () => void }) {
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    const t = window.setTimeout(() => setArmed(true), CONFIRM_INPUT_DELAY_MS);
    return () => window.clearTimeout(t);
  }, []);
  return (
    <div className="flex shrink-0 justify-end gap-2 border-t border-border px-5 py-3">
      <AlertDialog.Cancel className="rounded-md border border-border px-3 py-1.5 hover:bg-bg-muted">Cancel</AlertDialog.Cancel>
      <button
        type="button"
        aria-disabled={!armed}
        onClick={() => armed && props.onStart()}
        className="rounded-md bg-accent px-3 py-1.5 font-medium text-accent-fg aria-disabled:opacity-50"
      >
        {props.label}
      </button>
    </div>
  );
}

export function seconds(ms: number): string {
  return ms < 10_000 ? `${(ms / 1000).toFixed(1)} s` : `${Math.round(ms / 1000)} s`;
}

export function arrivalWords(a: string): string {
  switch (a) {
    case "native":
      return "native";
    case "text":
      return "from text";
    case "mixed":
      return "both";
    default:
      return "none";
  }
}

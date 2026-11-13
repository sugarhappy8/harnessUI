import { useEffect, useState } from "react";
import * as ipc from "../../lib/ipc";
import type { ErrorInfo } from "../../lib/types";
import { toErrorInfo } from "../../store/app";
import { ErrorBanner } from "../ErrorBanner";
import { visiblePath } from "../FolderChip";
import {
  checkConfirm,
  useEndpointHost,
  ModelSelect,
  ProgressLine,
  RunConfirm,
  arrivalWords,
  useEvalProgress,
  useModelChoice,
} from "./shared";
import type { ModelProfile, ProbeName, ProfileView } from "./types";

const PROBES: { name: ProbeName; title: string }[] = [
  { name: "toolCalling", title: "Tool calling" },
  { name: "editing", title: "Editing" },
  { name: "errorRecovery", title: "Error recovery" },
];

/** What a profile changes, in words. */
export function chosenWords(profile: ModelProfile): string {
  if (!profile.chosen || !profile.defaults) return "Nothing passed, so nothing changes for this model.";
  const parts: string[] = [];
  if (profile.defaults.thinking === false) parts.push("thinking off");
  if (profile.defaults.temperature != null) parts.push(`temperature ${profile.defaults.temperature}`);
  return parts.length === 0
    ? `${profile.chosen}: the model's own settings work best.`
    : `${profile.chosen}: chats with this model use ${parts.join(" and ")} unless you choose otherwise.`;
}

/**
 * Settings → Local models: the optional capability check. Shows the saved results for
 * the chosen model, runs the check when asked (after a confirmation), and forgets them.
 */
export function ModelCheck() {
  const { options, value, setChosen } = useModelChoice();
  const [view, setView] = useState<ProfileView | null>(null);
  const [confirm, setConfirm] = useState(false);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const progress = useEvalProgress("check", running);

  const [reload, setReload] = useState(0);
  useEffect(() => {
    if (!value) return;
    let alive = true;
    ipc
      .getModelProfile(value)
      .then((v) => {
        if (alive) setView(v);
      })
      .catch((err) => {
        if (alive) setError(toErrorInfo(err));
      });
    return () => {
      alive = false;
    };
  }, [value, reload]);

  async function start() {
    if (!value) return;
    setRunning(true);
    setError(null);
    try {
      await ipc.startCapabilityCheck(value);
    } catch (err) {
      setError(toErrorInfo(err));
    } finally {
      setRunning(false);
      setReload((n) => n + 1);
    }
  }

  async function forget() {
    if (!current?.key) return;
    try {
      await ipc.forgetModelProfile(current.key);
    } catch (err) {
      setError(toErrorInfo(err));
    }
    setReload((n) => n + 1);
  }

  const current = value ? view : null;
  const profile = current?.profile ?? null;
  const label = options.find((o) => o.ref === value)?.label ?? "";
  const host = useEndpointHost(value);
  return (
    <section aria-label="Model check" className="mt-6 space-y-3 border-t border-border pt-5">
      <div>
        <h3 className="text-sm font-medium">Tested settings</h3>
        <p className="text-xs text-fg-muted">
          An optional check of how well a model calls tools, edits files and recovers from a failed edit. The settings
          that work best are used in chats with that model. Nothing runs unless you start it.
        </p>
      </div>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <div className="flex flex-wrap items-center gap-2">
        <ModelSelect id="check-model" options={options} value={value} disabled={running} onChange={setChosen} />
        {running ? (
          <button
            type="button"
            onClick={() => void ipc.cancelEval().catch((err) => setError(toErrorInfo(err)))}
            className="rounded-md border border-border px-3 py-1.5 text-sm hover:bg-bg-muted"
          >
            End the check
          </button>
        ) : (
          <button
            type="button"
            disabled={!value || current?.available === false}
            onClick={() => setConfirm(true)}
            className="rounded-md bg-accent px-3 py-1.5 text-sm text-accent-fg disabled:opacity-50"
          >
            Check this model
          </button>
        )}
      </div>
      {current && !current.available && current.reason && <p className="text-xs text-fg-muted">{current.reason}</p>}
      {running && <ProgressLine progress={progress} idle="Starting the check" />}
      {profile && !running && (
        <div className="space-y-2">
          <p className="text-xs text-fg-muted">
            Checked {new Date(profile.checkedAt).toLocaleString()} · <bdi dir="ltr">{visiblePath(profile.label)}</bdi>
          </p>
          <p aria-label="Chosen settings" className="text-sm">
            {chosenWords(profile)}
          </p>
          <div className="overflow-x-auto">
            <table aria-label="Check results" className="w-full text-left text-xs">
              <thead className="text-fg-muted">
                <tr>
                  <th className="py-1 pr-3 font-normal">Settings</th>
                  {PROBES.map((p) => (
                    <th key={p.name} className="py-1 pr-3 font-normal">
                      {p.title}
                    </th>
                  ))}
                  <th className="py-1 pr-3 font-normal">Tool calls</th>
                  <th className="py-1 font-normal">Tool failures</th>
                </tr>
              </thead>
              <tbody>
                {profile.results.map((v) => {
                  const arrivals = [...new Set(v.probes.map((p) => p.arrival).filter((a) => a !== "none"))];
                  return (
                    <tr key={v.label} className={`border-t border-border ${v.label === profile.chosen ? "font-medium" : ""}`}>
                      <td className="py-1 pr-3">
                        {v.label}
                        {v.label === profile.chosen ? " (chosen)" : ""}
                      </td>
                      {PROBES.map((p) => {
                        const r = v.probes.find((x) => x.probe === p.name);
                        return (
                          <td key={p.name} className={`py-1 pr-3 ${r?.passed ? "text-success" : "text-fg-muted"}`} title={r?.note}>
                            {r ? (r.passed ? "passed" : "failed") : "-"}
                          </td>
                        );
                      })}
                      <td className="py-1 pr-3">{arrivals.length === 0 ? "none" : arrivals.map(arrivalWords).join(", ")}</td>
                      <td className="py-1">{v.toolFailures}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
          <button type="button" onClick={() => void forget()} className="text-xs text-fg-muted underline hover:text-fg">
            Forget these settings
          </button>
        </div>
      )}
      <RunConfirm
        open={confirm}
        title="Check this model?"
        lines={checkConfirm(host)}
        model={label}
        startLabel="Start the check"
        onOpenChange={setConfirm}
        onStart={() => void start()}
      />
    </section>
  );
}

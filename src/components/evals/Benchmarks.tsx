import { useEffect, useState } from "react";
import * as ipc from "../../lib/ipc";
import type { ErrorInfo } from "../../lib/types";
import { toErrorInfo } from "../../store/app";
import { ErrorBanner } from "../ErrorBanner";
import {
  benchConfirm,
  useEndpointHost,
  ModelSelect,
  ProgressLine,
  RunConfirm,
  seconds,
  useEvalProgress,
  useModelChoice,
} from "./shared";
import type { BenchRun, BenchSummary, BenchTaskInfo, BenchTotals, TaskResult } from "./types";

function runName(run: { startedAt: number; model: string }): string {
  return `${new Date(run.startedAt).toLocaleString()} · ${run.model}`;
}

export function totalsWords(t: BenchTotals): string {
  return `${t.completed} of ${t.ran} completed${t.skipped ? `, ${t.skipped} skipped` : ""}`;
}

const STATUS_CLASS: Record<TaskResult["status"], string> = {
  passed: "text-success",
  failed: "text-danger",
  skipped: "text-fg-muted",
  stopped: "text-fg-muted",
};

/** One run: a row per task, then the totals. */
export function RunTable(props: { run: BenchRun }) {
  const { run } = props;
  const t = run.totals;
  return (
    <div className="overflow-x-auto">
      <table aria-label="Benchmark results" className="w-full text-left text-xs">
        <thead className="text-fg-muted">
          <tr>
            <th className="py-1 pr-2 font-normal">Task</th>
            <th className="py-1 pr-2 font-normal">Result</th>
            <th className="py-1 pr-2 font-normal" title="Edits that failed, and files left not parsing">Broken edits</th>
            <th className="py-1 pr-2 font-normal">Tool failures</th>
            <th className="py-1 pr-2 font-normal" title="Commands that ran and ended with a failure, such as failing tests">
              Failed commands
            </th>
            <th className="py-1 pr-2 font-normal">Blocked</th>
            <th className="py-1 pr-2 font-normal">Steps</th>
            <th className="py-1 pr-2 font-normal">Tokens</th>
            <th className="py-1 font-normal">Time</th>
          </tr>
        </thead>
        <tbody>
          {run.tasks.map((r) => (
            <tr key={r.name} className="border-t border-border">
              <td className="py-1 pr-2">{r.title}</td>
              <td className={`py-1 pr-2 ${STATUS_CLASS[r.status]}`} title={r.note}>
                {r.status}
              </td>
              <td className="py-1 pr-2">{r.brokenEdits}</td>
              <td className="py-1 pr-2">{r.toolFailures}</td>
              <td className="py-1 pr-2">{r.failedCommands}</td>
              <td className="py-1 pr-2">{r.blocked}</td>
              <td className="py-1 pr-2">{r.steps}</td>
              <td className="py-1 pr-2">{(r.promptTokens + r.completionTokens).toLocaleString()}</td>
              <td className="py-1">{seconds(r.timeMs)}</td>
            </tr>
          ))}
          <tr className="border-t border-border font-medium">
            <td className="py-1 pr-2">Total</td>
            <td className="py-1 pr-2">{totalsWords(t)}</td>
            <td className="py-1 pr-2">{t.brokenEdits}</td>
            <td className="py-1 pr-2">{t.toolFailures}</td>
            <td className="py-1 pr-2">{t.failedCommands}</td>
            <td className="py-1 pr-2">{t.blocked}</td>
            <td className="py-1 pr-2">{t.steps}</td>
            <td className="py-1 pr-2">{t.tokens.toLocaleString()}</td>
            <td className="py-1">{seconds(t.timeMs)}</td>
          </tr>
        </tbody>
      </table>
    </div>
  );
}

/** Two runs side by side, task by task. */
export function CompareTable(props: { a: BenchRun; b: BenchRun }) {
  const { a, b } = props;
  const names = [...new Set([...a.tasks, ...b.tasks].map((r) => r.name))];
  const find = (run: BenchRun, name: string) => run.tasks.find((r) => r.name === name);
  return (
    <div className="overflow-x-auto">
      <table aria-label="Benchmark comparison" className="w-full text-left text-xs">
        <thead className="text-fg-muted">
          <tr>
            <th className="py-1 pr-2 font-normal">Task</th>
            <th className="py-1 pr-2 font-normal">First run</th>
            <th className="py-1 pr-2 font-normal">Second run</th>
            <th className="py-1 pr-2 font-normal">Failed commands, first</th>
            <th className="py-1 pr-2 font-normal">Failed commands, second</th>
            <th className="py-1 pr-2 font-normal">Time, first</th>
            <th className="py-1 font-normal">Time, second</th>
          </tr>
        </thead>
        <tbody>
          {names.map((name) => {
            const x = find(a, name);
            const y = find(b, name);
            return (
              <tr key={name} className="border-t border-border">
                <td className="py-1 pr-2">{(x ?? y)?.title}</td>
                <td className={`py-1 pr-2 ${x ? STATUS_CLASS[x.status] : ""}`}>{x?.status ?? "-"}</td>
                <td className={`py-1 pr-2 ${y ? STATUS_CLASS[y.status] : ""}`}>{y?.status ?? "-"}</td>
                <td className="py-1 pr-2">{x ? x.failedCommands : "-"}</td>
                <td className="py-1 pr-2">{y ? y.failedCommands : "-"}</td>
                <td className="py-1 pr-2">{x ? seconds(x.timeMs) : "-"}</td>
                <td className="py-1">{y ? seconds(y.timeMs) : "-"}</td>
              </tr>
            );
          })}
          <tr className="border-t border-border font-medium">
            <td className="py-1 pr-2">Total</td>
            <td className="py-1 pr-2">{totalsWords(a.totals)}</td>
            <td className="py-1 pr-2">{totalsWords(b.totals)}</td>
            <td className="py-1 pr-2">{a.totals.failedCommands}</td>
            <td className="py-1 pr-2">{b.totals.failedCommands}</td>
            <td className="py-1 pr-2">{seconds(a.totals.timeMs)}</td>
            <td className="py-1">{seconds(b.totals.timeMs)}</td>
          </tr>
        </tbody>
      </table>
    </div>
  );
}

/**
 * Settings → Diagnostics: coding benchmarks. Runs the shipped tasks on a model when
 * asked (after a confirmation), lists the saved runs, shows one, and compares two.
 */
export function Benchmarks() {
  const { options, value, setChosen } = useModelChoice();
  const [tasks, setTasks] = useState<BenchTaskInfo[]>([]);
  const [runs, setRuns] = useState<BenchSummary[]>([]);
  const [shown, setShown] = useState<BenchRun | null>(null);
  const [other, setOther] = useState<BenchRun | null>(null);
  const [confirm, setConfirm] = useState(false);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const progress = useEvalProgress("bench", running);

  const [reload, setReload] = useState(0);
  useEffect(() => {
    let alive = true;
    ipc
      .listBenchRuns()
      .then((r) => {
        if (alive) setRuns(r);
      })
      .catch((err) => {
        if (alive) setError(toErrorInfo(err));
      });
    return () => {
      alive = false;
    };
  }, [reload]);

  useEffect(() => {
    let alive = true;
    ipc
      .listBenchTasks()
      .then((t) => {
        if (alive) setTasks(t);
      })
      .catch((err) => {
        if (alive) setError(toErrorInfo(err));
      });
    return () => {
      alive = false;
    };
  }, []);

  async function show(id: string, which: "shown" | "other") {
    if (!id) {
      if (which === "other") setOther(null);
      return;
    }
    try {
      const run = await ipc.getBenchRun(id);
      if (which === "shown") setShown(run);
      else setOther(run);
    } catch (err) {
      setError(toErrorInfo(err));
    }
  }

  async function start() {
    if (!value) return;
    setRunning(true);
    setError(null);
    try {
      const run = await ipc.startBenchmark(value);
      setShown(run);
      setOther(null);
    } catch (err) {
      setError(toErrorInfo(err));
    } finally {
      setRunning(false);
      setReload((n) => n + 1);
    }
  }

  const skipped = tasks.filter((t) => !t.runnable);
  const missing = [...new Set(skipped.map((t) => t.needs).filter(Boolean))];
  const label = options.find((o) => o.ref === value)?.label ?? "";
  const host = useEndpointHost(value);
  return (
    <section aria-label="Benchmarks" className="mt-6 space-y-3 border-t border-border pt-5">
      <div>
        <h3 className="text-sm font-medium">Coding benchmarks</h3>
        <p className="text-xs text-fg-muted">
          {tasks.length} small coding tasks measure how often a model finishes the work, how many edits and tool calls
          fail, and how long it takes. Results are saved on this computer.
          {missing.length > 0 && ` ${missing.join(" and ")} not found: ${skipped.length} tasks will be skipped.`}
        </p>
      </div>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <div className="flex flex-wrap items-center gap-2">
        <ModelSelect id="bench-model" options={options} value={value} disabled={running} onChange={setChosen} />
        {running ? (
          <button
            type="button"
            onClick={() => void ipc.cancelEval().catch((err) => setError(toErrorInfo(err)))}
            className="rounded-md border border-border px-3 py-1.5 text-sm hover:bg-bg-muted"
          >
            End the benchmark
          </button>
        ) : (
          <button
            type="button"
            disabled={!value}
            onClick={() => setConfirm(true)}
            className="rounded-md bg-accent px-3 py-1.5 text-sm text-accent-fg disabled:opacity-50"
          >
            Run the benchmark
          </button>
        )}
      </div>
      {running && <ProgressLine progress={progress} idle="Starting the benchmark" />}
      {runs.length > 0 && (
        <div className="grid grid-cols-[7rem_1fr] items-center gap-2 text-sm">
          <label htmlFor="bench-run">Run</label>
          <select
            id="bench-run"
            value={shown?.id ?? ""}
            onChange={(e) => void show(e.target.value, "shown")}
            className="min-w-0 rounded-md border border-border bg-bg px-2 py-1"
          >
            <option value="">Choose a run</option>
            {runs.map((r) => (
              <option key={r.id} value={r.id}>
                {runName(r)} · {totalsWords(r.totals)}
                {r.cancelled ? " (ended early)" : ""}
              </option>
            ))}
          </select>
          {shown && runs.length > 1 && (
            <>
              <label htmlFor="bench-compare">Compare with</label>
              <select
                id="bench-compare"
                value={other?.id ?? ""}
                onChange={(e) => void show(e.target.value, "other")}
                className="min-w-0 rounded-md border border-border bg-bg px-2 py-1"
              >
                <option value="">Nothing</option>
                {runs
                  .filter((r) => r.id !== shown.id)
                  .map((r) => (
                    <option key={r.id} value={r.id}>
                      {runName(r)} · {totalsWords(r.totals)}
                    </option>
                  ))}
              </select>
            </>
          )}
        </div>
      )}
      {shown && !running && (other ? <CompareTable a={shown} b={other} /> : <RunTable run={shown} />)}
      <RunConfirm
        open={confirm}
        title="Run the benchmark?"
        lines={benchConfirm(host)}
        model={label}
        startLabel="Start the benchmark"
        onOpenChange={setConfirm}
        onStart={() => void start()}
      />
    </section>
  );
}

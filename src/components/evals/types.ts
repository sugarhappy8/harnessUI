// Types of the capability check and the benchmarks (src-tauri/src/evals/).

export type Arrival = "none" | "native" | "text" | "mixed";
export type ProbeName = "toolCalling" | "editing" | "errorRecovery";

export interface RunSettings {
  thinkingOff: boolean;
  temperature: number | null;
}

export interface ProbeResult {
  probe: ProbeName;
  passed: boolean;
  arrival: Arrival;
  toolFailures: number;
  failedEdits: number;
  blocked: number;
  steps: number;
  tokens: number;
  timeMs: number;
  status: string;
  note: string;
}

export interface VariantResult {
  label: string;
  settings: RunSettings;
  passed: number;
  toolFailures: number;
  probes: ProbeResult[];
}

export interface ModelProfile {
  version: number;
  label: string;
  checkedAt: number;
  chosen: string | null;
  defaults: { thinking?: boolean; temperature?: number } | null;
  results: VariantResult[];
}

export interface ProfileView {
  available: boolean;
  reason: string | null;
  label: string | null;
  key: string | null;
  profile: ModelProfile | null;
}

export interface CheckOutcome {
  check: string;
  passed: boolean;
  detail: string;
}

export interface TaskResult {
  name: string;
  title: string;
  kind: string;
  status: "passed" | "failed" | "skipped" | "stopped";
  note: string;
  checks: CheckOutcome[];
  failedEdits: number;
  unparsable: string[];
  brokenEdits: number;
  toolFailures: number;
  blocked: number;
  steps: number;
  promptTokens: number;
  completionTokens: number;
  timeMs: number;
  arrival: Arrival;
  runStatus: string;
}

export interface BenchTotals {
  tasks: number;
  ran: number;
  completed: number;
  skipped: number;
  brokenEdits: number;
  toolFailures: number;
  blocked: number;
  steps: number;
  tokens: number;
  timeMs: number;
}

export interface BenchRun {
  id: string;
  startedAt: number;
  finishedAt: number;
  model: string;
  modelRef: string;
  settings: RunSettings;
  toolchains: string[];
  cancelled: boolean;
  tasks: TaskResult[];
  totals: BenchTotals;
}

export interface BenchSummary {
  id: string;
  startedAt: number;
  model: string;
  cancelled: boolean;
  totals: BenchTotals;
}

export interface BenchTaskInfo {
  name: string;
  title: string;
  kind: string;
  needs: string | null;
  runnable: boolean;
}

export interface EvalProgress {
  kind: "check" | "bench";
  done: number;
  total: number;
  current: string;
}

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ModelOption } from "../../lib/types";
import type { BenchRun, ModelProfile, ProfileView } from "./types";

const ipcMock = vi.hoisted(() => ({
  listModelOptions: vi.fn<() => Promise<ModelOption[]>>(),
  getModelProfile: vi.fn<(ref: string) => Promise<ProfileView>>(),
  forgetModelProfile: vi.fn<(key: string) => Promise<boolean>>(),
  startCapabilityCheck: vi.fn<(ref: string) => Promise<ModelProfile>>(),
  cancelEval: vi.fn<() => Promise<boolean>>(),
  onEvalProgress: vi.fn(async () => () => undefined),
  listBenchTasks: vi.fn(async () => [
    { name: "a", title: "A", kind: "python", needs: "Python", runnable: false },
    { name: "b", title: "B", kind: "docs", needs: null, runnable: true },
  ]),
  listBenchRuns: vi.fn(async () => []),
  getBenchRun: vi.fn<(id: string) => Promise<BenchRun>>(),
  startBenchmark: vi.fn<(ref: string) => Promise<BenchRun>>(),
}));
vi.mock("../../lib/ipc", () => ipcMock);

const { ModelCheck, chosenWords } = await import("./ModelCheck");
const { Benchmarks, RunTable, CompareTable } = await import("./Benchmarks");
const { BENCH_CONFIRM, CHECK_CONFIRM, CONFIRM_INPUT_DELAY_MS } = await import("./shared");

const MODEL: ModelOption = { ref: "remote:1:fake-model", label: "fake-model", source: "provider", contextLength: null };

function profile(over: Partial<ModelProfile> = {}): ModelProfile {
  const probe = (name: "toolCalling" | "editing" | "errorRecovery", passed: boolean) => ({
    probe: name,
    passed,
    arrival: "native" as const,
    toolFailures: 0,
    failedEdits: 0,
    blocked: 0,
    steps: 2,
    tokens: 100,
    timeMs: 1000,
    status: "completed",
    note: "",
  });
  return {
    version: 1,
    label: "fake-model",
    checkedAt: Date.UTC(2026, 10, 12),
    chosen: "Temperature 0.2",
    defaults: { temperature: 0.2 },
    results: [
      {
        label: "Default",
        settings: { thinkingOff: false, temperature: null },
        passed: 1,
        toolFailures: 2,
        probes: [probe("toolCalling", true), probe("editing", false), probe("errorRecovery", false)],
      },
      {
        label: "Temperature 0.2",
        settings: { thinkingOff: false, temperature: 0.2 },
        passed: 3,
        toolFailures: 0,
        probes: [probe("toolCalling", true), probe("editing", true), probe("errorRecovery", true)],
      },
    ],
    ...over,
  };
}

const FORBIDDEN_LABELS = ["Allow once", "Deny", "Save", "Use", "Continue", "Send", "Stop"];

function buttonLabels(): string[] {
  return screen.queryAllByRole("button").map((b) => b.textContent ?? "");
}

describe("model check", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    ipcMock.listModelOptions.mockResolvedValue([MODEL]);
    ipcMock.getModelProfile.mockResolvedValue({ available: true, reason: null, label: "fake-model", key: "modelProfile:0123456789abcdef", profile: null });
  });
  afterEach(cleanup);

  it("starts nothing until the user confirms, and says what a check does first", async () => {
    await act(async () => {
      render(<ModelCheck />);
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check this model" }));
    });
    const dialog = screen.getByRole("alertdialog");
    for (const line of CHECK_CONFIRM) expect(dialog.textContent).toContain(line);
    expect(dialog.textContent).toContain("temporary folder");
    expect(dialog.textContent).toContain("Nothing is asked while it runs");
    // The start button ignores a click that comes right away.
    fireEvent.click(screen.getByRole("button", { name: "Start the check" }));
    expect(ipcMock.startCapabilityCheck).not.toHaveBeenCalled();
    for (const label of buttonLabels()) {
      for (const bad of FORBIDDEN_LABELS) expect(label).not.toContain(bad);
    }
    ipcMock.startCapabilityCheck.mockResolvedValue(profile());
    await act(async () => {
      await new Promise((r) => setTimeout(r, CONFIRM_INPUT_DELAY_MS + 50));
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Start the check" }));
    });
    expect(ipcMock.startCapabilityCheck).toHaveBeenCalledWith("remote:1:fake-model");
  });

  it("shows the results table, the date and what was chosen, and forgets them", async () => {
    ipcMock.getModelProfile.mockResolvedValue({ available: true, reason: null, label: "fake-model", key: "modelProfile:0123456789abcdef", profile: profile() });
    await act(async () => {
      render(<ModelCheck />);
    });
    const table = screen.getByRole("table", { name: "Check results" });
    expect(table.textContent).toContain("Temperature 0.2 (chosen)");
    expect(table.textContent).toContain("native");
    expect(screen.getByLabelText("Chosen settings").textContent).toContain("temperature 0.2");
    expect(screen.getByText(/Checked /)).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Forget these settings" }));
    });
    expect(ipcMock.forgetModelProfile).toHaveBeenCalledWith("modelProfile:0123456789abcdef");
  });

  it("explains why a model cannot be checked", async () => {
    ipcMock.getModelProfile.mockResolvedValue({ available: false, reason: "Load this model to check it.", label: null, key: null, profile: null });
    await act(async () => {
      render(<ModelCheck />);
    });
    expect(screen.getByText("Load this model to check it.")).toBeTruthy();
    expect((screen.getByRole("button", { name: "Check this model" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("words what a profile changes", () => {
    expect(chosenWords(profile())).toContain("temperature 0.2");
    expect(chosenWords(profile({ chosen: null, defaults: null }))).toContain("nothing changes");
    expect(chosenWords(profile({ chosen: "Default", defaults: {} }))).toContain("own settings");
    expect(chosenWords(profile({ chosen: "Thinking off", defaults: { thinking: false } }))).toContain("thinking off");
  });
});

function run(id: string, statuses: ("passed" | "failed" | "skipped")[]): BenchRun {
  const tasks = statuses.map((status, i) => ({
    name: `t${i}`,
    title: `Task ${i}`,
    kind: "python",
    status,
    note: "",
    checks: [],
    failedEdits: 0,
    unparsable: [],
    brokenEdits: status === "failed" ? 1 : 0,
    toolFailures: 0,
    blocked: 0,
    steps: 3,
    promptTokens: 100,
    completionTokens: 10,
    timeMs: 2000,
    arrival: "native" as const,
    runStatus: "completed",
  }));
  return {
    id,
    startedAt: Date.UTC(2026, 10, 12),
    finishedAt: Date.UTC(2026, 10, 12),
    model: "fake-model",
    modelRef: MODEL.ref,
    settings: { thinkingOff: false, temperature: null },
    toolchains: ["Python"],
    cancelled: false,
    tasks,
    totals: {
      tasks: tasks.length,
      ran: tasks.filter((t) => t.status !== "skipped").length,
      completed: tasks.filter((t) => t.status === "passed").length,
      skipped: tasks.filter((t) => t.status === "skipped").length,
      brokenEdits: tasks.filter((t) => t.status === "failed").length,
      toolFailures: 0,
      blocked: 0,
      steps: 3 * tasks.length,
      tokens: 110 * tasks.length,
      timeMs: 2000 * tasks.length,
    },
  };
}

describe("benchmarks", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    ipcMock.listModelOptions.mockResolvedValue([MODEL]);
  });
  afterEach(cleanup);

  it("says which tasks will be skipped and confirms before a run", async () => {
    await act(async () => {
      render(<Benchmarks />);
    });
    expect(screen.getByText(/Python not found: 1 tasks will be skipped/)).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Run the benchmark" }));
    });
    const dialog = screen.getByRole("alertdialog");
    for (const line of BENCH_CONFIRM) expect(dialog.textContent).toContain(line);
    expect(dialog.textContent).toContain("commands that delete, go online or cannot be understood are refused");
    expect(ipcMock.startBenchmark).not.toHaveBeenCalled();
    for (const label of buttonLabels()) {
      for (const bad of FORBIDDEN_LABELS) expect(label).not.toContain(bad);
    }
  });

  it("shows a run with its totals, and two runs side by side", () => {
    const a = run("20261112-100000-aaaa", ["passed", "failed", "skipped"]);
    const b = run("20261112-110000-bbbb", ["passed", "passed", "skipped"]);
    const { unmount } = render(<RunTable run={a} />);
    const table = screen.getByRole("table", { name: "Benchmark results" });
    expect(table.textContent).toContain("1 of 2 completed, 1 skipped");
    expect(table.textContent).toContain("Task 1");
    unmount();
    render(<CompareTable a={a} b={b} />);
    const compare = screen.getByRole("table", { name: "Benchmark comparison" });
    expect(compare.textContent).toContain("1 of 2 completed");
    expect(compare.textContent).toContain("2 of 2 completed");
  });
});

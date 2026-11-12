import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ModelOption } from "../../lib/types";
import type { BenchRun, ModelProfile, ProfileView } from "./types";

const ipcMock = vi.hoisted(() => ({
  listModelOptions: vi.fn<() => Promise<ModelOption[]>>(),
  listProviders: vi.fn(async () => [
    { id: 1, name: "Fake", kind: "generic", baseUrl: "http://models.example.com:8080/v1", hasApiKey: false, enabled: true },
  ]),
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
const { benchConfirm, checkConfirm, CONFIRM_INPUT_DELAY_MS } = await import("./shared");
const { useApp } = await import("../../store/app");

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
    for (const line of checkConfirm("models.example.com")) expect(dialog.textContent).toContain(line);
    expect(dialog.textContent).toContain("go to models.example.com.");
    expect(dialog.textContent).not.toContain("Nothing leaves this computer");
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
    for (const line of benchConfirm("models.example.com")) expect(dialog.textContent).toContain(line);
    expect(dialog.textContent).toContain("with your rights");
    expect(dialog.textContent).toContain("Run the benchmark only with a model you trust.");
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

describe("what the user is told before a run", () => {
  afterEach(() => {
    cleanup();
    useApp.setState({ runtime: null });
  });

  it("says exactly this before a check, for a local model and for an endpoint", () => {
    expect(checkConfirm(null)).toEqual([
      "The app gives the model three short tasks and tries each with a few settings.",
      "The model gets only tools to read and change files, and only in a temporary folder made for the check. It cannot run commands, and the app runs nothing the model writes. The folder is removed afterwards.",
      "Nothing is asked while it runs: anything that would need your approval is refused.",
      "The settings that work best are saved for this model and used in your chats, unless you chose a setting yourself.",
      "It can take a few minutes. Nothing leaves this computer.",
    ]);
    expect(checkConfirm("models.example.com").at(-1)).toBe(
      "It can take a few minutes. The tasks and the model's replies go to models.example.com.",
    );
  });

  it("says exactly this before a benchmark, and promises no confinement", () => {
    const local = benchConfirm(null);
    expect(local).toEqual([
      "The app gives the model a fixed set of small coding tasks, one at a time, each in a temporary folder that is removed afterwards.",
      "The model writes code, and the app runs it on this computer with your rights: the task's tests, the model's scripts, and the app's own checks of the result.",
      "The app lets the model run only the task's tests and scripts and commands that look at files, and refuses everything else. Nothing is asked while it runs.",
      "The app cannot see inside code the model writes. Such code could read or change files elsewhere on this computer, or go online.",
      "Run the benchmark only with a model you trust.",
      "Tasks that need Python or Node.js are skipped when they are not installed.",
      "It can take a long time. The results stay on this computer.",
    ]);
    expect(benchConfirm("models.example.com").at(-1)).toBe(
      "It can take a long time. The tasks and the model's replies go to models.example.com. The results stay on this computer.",
    );
    for (const line of local) {
      expect(line).not.toMatch(/nowhere else|cannot leave|refused: commands that/);
    }
  });

  it("shows the local wording for the loaded local model", async () => {
    vi.clearAllMocks();
    ipcMock.listModelOptions.mockResolvedValue([]);
    ipcMock.getModelProfile.mockResolvedValue({ available: true, reason: null, label: "m.gguf (cpu)", key: "modelProfile:0123456789abcdef", profile: null });
    useApp.setState({ runtime: { modelPath: "/models/m.gguf", modelName: "m.gguf" } as never });
    await act(async () => {
      render(<ModelCheck />);
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check this model" }));
    });
    const dialog = screen.getByRole("alertdialog");
    expect(dialog.textContent).toContain("Nothing leaves this computer.");
    expect(ipcMock.listProviders).not.toHaveBeenCalled();
  });
});

import { describe, expect, it } from "vitest";
import type { BackendKind, ContextSource, LoadFailure, LoadStage, ModelBusy, Verdict } from "./types";
import {
  aboutWords,
  adjustmentWords,
  automaticResult,
  BACKEND_WORDS,
  backendWords,
  BUSY_QUESTIONS,
  busyQuestion,
  CONTEXT_SOURCE_WORDS,
  deviceLabel,
  FIT_WORDS,
  gpuHelp,
  gpuLabel,
  GPU_USE_WORDS,
  HELP,
  LOAD_FAILURE_WORDS,
  loadStageWords,
  readyLine,
  readyLineParts,
  stepDownAction,
  SLOW_WORDS,
  slowSpeedWords,
  THINKING_WORDS,
  TOO_LARGE_LINE,
  tokensWithWords,
  upgradeNoteWords,
  waitingWords,
  wordsPerSecond,
} from "./words";

const BACKENDS: BackendKind[] = ["cpu", "vulkan", "cuda", "rocm", "metal"];

// One example of every failure. A Record keyed by the code makes the compiler refuse a
// missing one, so a new failure in the contract fails here first.
const FAILURES: Record<LoadFailure["code"], LoadFailure> = {
  out_of_memory: { code: "out_of_memory" },
  invalid_device: { code: "invalid_device", device: "Vulkan1" },
  missing_runtime_dll: { code: "missing_runtime_dll" },
  port_in_use: { code: "port_in_use" },
  model_file_invalid: { code: "model_file_invalid" },
  context_too_large: { code: "context_too_large" },
  unknown: { code: "unknown" },
};

// Every stage, with every backend and failure where a stage carries them.
const STAGES: Record<LoadStage["kind"], LoadStage[]> = {
  starting: [{ kind: "starting" }],
  readingModel: [{ kind: "readingModel" }],
  readingImageReader: [{ kind: "readingImageReader" }],
  loading: [{ kind: "loading" }],
  restarting: [{ kind: "restarting", attempt: 1, max: 3 }],
  retryingWithoutSaveMemory: BACKENDS.map((backend) => ({ kind: "retryingWithoutSaveMemory", backend })),
  retryingWithLessGpu: BACKENDS.map((backend) => ({ kind: "retryingWithLessGpu", backend })),
  steppingDown: BACKENDS.flatMap((from) =>
    BACKENDS.flatMap((to) => Object.values(FAILURES).map((reason) => ({ kind: "steppingDown" as const, from, to, reason }))),
  ),
};

const SOURCES: Record<ContextSource, true> = {
  automatic: true,
  modelSetting: true,
  defaultSetting: true,
  trainedLimit: true,
};
const VERDICTS: Record<Verdict, true> = { green: true, yellow: true, red: true };

/** No identifier on screen: no snake_case, no lower-case backend name, no device name. */
function plain(text: string) {
  expect(text.trim().length).toBeGreaterThan(0);
  expect(text).not.toMatch(/_/);
  expect(text).not.toMatch(/\b(vulkan|cuda|rocm|metal|cpu|gpu)\b/);
  expect(text).not.toMatch(/Vulkan\d|ROCm\d|CUDA\d/);
  expect(text).not.toMatch(/!/);
}

describe("plain words", () => {
  it("gives token counts with words, as the specification writes them", () => {
    expect(tokensWithWords(8192)).toBe("8,192 tokens, about 6,000 words");
    expect(tokensWithWords(24_576)).toBe("24,576 tokens, about 18,000 words");
    expect(tokensWithWords(32_768)).toBe("32,768 tokens, about 24,000 words");
    expect(aboutWords(1000)).toBe(700);
    expect(aboutWords(0)).toBe(0);
  });

  it("names every backend without its identifier alone", () => {
    for (const k of BACKENDS) {
      expect(BACKEND_WORDS[k]).toBeTruthy();
      expect(backendWords(k)).not.toBe(k);
    }
    expect(backendWords("vulkan")).toBe("Graphics card (Vulkan)");
    expect(backendWords("cuda")).toBe("NVIDIA graphics card (CUDA)");
    expect(backendWords("rocm")).toBe("AMD graphics card (ROCm)");
    expect(backendWords("metal")).toBe("Apple graphics (Metal)");
    expect(backendWords("cpu")).toBe("Processor only (slower)");
    expect(backendWords(null)).toBe("Automatic");
    expect(GPU_USE_WORDS).toEqual({ all: "All of it", fit: "As much as fits", off: "Don't use it" });
    expect(THINKING_WORDS).toEqual({ noLimit: "Default", short: "Short", medium: "Medium", long: "Long" });
  });

  it("has a plain phrase for every load stage, backend and failure", () => {
    for (const examples of Object.values(STAGES)) {
      for (const s of examples) plain(loadStageWords(s));
    }
    for (const f of Object.keys(FAILURES) as LoadFailure["code"][]) plain(LOAD_FAILURE_WORDS[f]);
    for (const s of Object.keys(SOURCES) as ContextSource[]) plain(CONTEXT_SOURCE_WORDS[s]);
    for (const v of Object.keys(VERDICTS) as Verdict[]) plain(FIT_WORDS[v]);
    expect(loadStageWords({ kind: "restarting", attempt: 2, max: 3 })).toContain("2 of 3");
    expect(loadStageWords({ kind: "readingModel" })).toBe("Reading the model file");
    expect(loadStageWords({ kind: "readingImageReader" })).toBe("Loading the image reader");
  });

  it("explains every change a load had to make, in the specification's words", () => {
    for (const kind of ["retryingWithoutSaveMemory", "retryingWithLessGpu", "steppingDown", "restarting"] as const) {
      for (const s of STAGES[kind]) {
        const words = adjustmentWords(s);
        expect(words, JSON.stringify(s)).not.toBeNull();
        plain(words ?? "");
      }
    }
    for (const kind of ["starting", "readingModel", "readingImageReader", "loading"] as const) {
      for (const s of STAGES[kind]) expect(adjustmentWords(s)).toBeNull();
    }
    expect(adjustmentWords({ kind: "retryingWithoutSaveMemory", backend: "vulkan" })).toBe(
      "Save memory doesn't work with this graphics card, so it was turned off for this load.",
    );
    expect(
      adjustmentWords({ kind: "steppingDown", from: "vulkan", to: "cpu", reason: { code: "out_of_memory" } }),
    ).toBe("Running on the processor because the graphics card ran out of memory. Reload to try the graphics card again.");
    expect(
      adjustmentWords({ kind: "steppingDown", from: "rocm", to: "vulkan", reason: { code: "missing_runtime_dll" } }),
    ).toContain("Reload");
  });

  it("says why Save memory was turned off where the load ran: the model on the processor, the card on a card", () => {
    expect(adjustmentWords({ kind: "retryingWithoutSaveMemory", backend: "cpu" })).toBe(
      "Save memory doesn't work with this model, so it was turned off for this load.",
    );
    expect(adjustmentWords({ kind: "retryingWithoutSaveMemory", backend: "metal" })).toBe(
      "Save memory doesn't work with this model on Apple graphics, so it was turned off for this load.",
    );
    for (const backend of ["vulkan", "cuda", "rocm"] as const) {
      expect(adjustmentWords({ kind: "retryingWithoutSaveMemory", backend })).toBe(
        "Save memory doesn't work with this graphics card, so it was turned off for this load.",
      );
    }
    expect(adjustmentWords({ kind: "retryingWithLessGpu", backend: "vulkan" })).toBe(
      "The graphics card ran out of memory, so part of the model runs on the processor. Replies are slower.",
    );
    expect(adjustmentWords({ kind: "retryingWithLessGpu", backend: "metal" })).toBe(
      "Apple graphics ran out of memory, so part of the model runs on the processor. Replies are slower.",
    );
  });

  it("words a step-down by its reason, with an action that can help", () => {
    const down = (reason: LoadFailure, from: BackendKind = "vulkan"): LoadStage => ({ kind: "steppingDown", from, to: "cpu", reason });
    expect(adjustmentWords(down({ code: "context_too_large" }))).toBe(
      "Running on the processor because the graphics card ran out of memory. Reload to try the graphics card again.",
    );
    expect(adjustmentWords(down({ code: "invalid_device", device: "Vulkan1" }))).toBe(
      "Running on the processor because the chosen graphics card was not found. Choose another in Model settings.",
    );
    expect(adjustmentWords(down({ code: "model_file_invalid" }))).toBe(
      "Running on the processor because the graphics card could not load this model.",
    );
    expect(adjustmentWords(down({ code: "port_in_use" }))).toBe(
      "Running on the processor because the graphics card could not get the connection it needs. Reload to try the graphics card again.",
    );
    expect(adjustmentWords(down({ code: "unknown" }, "metal"))).toBe(
      "Running on the processor because Apple graphics stopped while loading. Reload to try Apple graphics again.",
    );
    expect(loadStageWords(down({ code: "invalid_device", device: "Vulkan1" }))).toBe(
      "The chosen graphics card was not found, so trying the processor",
    );
    expect(loadStageWords(down({ code: "model_file_invalid" }))).toBe("The graphics card could not load this model, so trying the processor");
    expect(stepDownAction({ code: "out_of_memory" })).toBe("reload");
    expect(stepDownAction({ code: "context_too_large" })).toBe("reload");
    expect(stepDownAction({ code: "invalid_device", device: "Vulkan1" })).toBe("settings");
    expect(stepDownAction({ code: "model_file_invalid" })).toBeNull();
    // Reload only where the sentence says to.
    for (const f of Object.values(FAILURES)) {
      expect(adjustmentWords(down(f))?.includes("Reload") ?? false, f.code).toBe(stepDownAction(f) === "reload");
    }
  });

  it("words the slow states with a reason and words per second", () => {
    expect(SLOW_WORDS.partial).toBe(
      "Part of the model runs on the processor, so replies are slower. Save memory or a smaller memory size lets more fit on the graphics card.",
    );
    expect(SLOW_WORDS.afterRestart).toBe("The first reply after a restart takes longer while the model rereads the chat.");
    expect(slowSpeedWords(4)).toBe(
      "Replies are slow here (about 3 words a second). The graphics card may be short of memory. Try a smaller memory size or a smaller model.",
    );
    expect(wordsPerSecond(4)).toBe("about 3 words a second");
    expect(wordsPerSecond(1.2)).toBe("about 1 word a second");
    expect(wordsPerSecond(0.5)).toBe("less than 1 word a second");
  });

  it("writes the ready line, the Automatic result and the panel's fixed lines", () => {
    expect(readyLine(32_768, "automatic")).toBe("Remembers about 24,000 words (Automatic)");
    expect(readyLine(8192, null)).toBe("Remembers about 6,000 words");
    expect(readyLine(8192, "modelSetting")).toBe("Remembers about 6,000 words (set for this model)");
    // The shorter forms a crowded header shows, the number always whole.
    expect(readyLineParts(32_768, "automatic")).toEqual({
      remembers: "Remembers about 24,000 words",
      reason: " (Automatic)",
      about: "About 24,000 words",
      words: "24,000 words",
    });
    expect(readyLineParts(8192, null).reason).toBe("");
    expect(automaticResult(24_576)).toBe("Automatic: 24,576 tokens, about 18,000 words, on this computer right now");
    expect(TOO_LARGE_LINE).toBe(
      "Too large for this computer, even with the shortest memory. Try Save memory, As much as fits, or a smaller download.",
    );
    expect(upgradeNoteWords(16_384)).toBe(
      "Every model now uses the size you chose before (16,384 tokens, about 12,000 words). Choose Automatic to let the app pick the largest size that fits.",
    );
    expect(HELP.context).toBe(
      "How much of the chat the model can see at once. When a chat gets longer than this, older messages are summarised automatically so it can keep going.",
    );
    expect(gpuHelp(false)).toBe(
      "The graphics card is much faster than the processor. As much as fits puts the rest on the processor, which is slower.",
    );
    expect(HELP.saveMemory).toBe("Lets the model remember more in the same memory. Answers may be slightly less accurate.");
    expect(HELP.keepInMemory).toBe(
      "Stops the computer from moving the model out of memory, so replies don't pause after a break. Other programs get less memory. Not recommended with 16 GB or less.",
    );
    expect(HELP.thinking).toBe("Longer thinking can give better answers but takes more time.");
    expect(gpuLabel(false)).toBe("Use the graphics card");
    expect(gpuLabel(true)).toBe("Use Apple graphics");
    expect(gpuHelp(true)).not.toContain("graphics card");
  });

  it("labels a graphics card by its own name and memory, never by its device id", () => {
    expect(
      deviceLabel({ name: "Vulkan0", description: "AMD Radeon RX 9070 XT", totalMib: 16_384, freeMib: 15_000, recommended: true }),
    ).toBe("AMD Radeon RX 9070 XT · 16 GB (recommended)");
    expect(
      deviceLabel({ name: "Vulkan1", description: "AMD Radeon(TM) Graphics", totalMib: 2048, freeMib: 0, recommended: false }),
    ).toBe("AMD Radeon(TM) Graphics · 2 GB");
    expect(deviceLabel({ name: "Vulkan2", description: "", totalMib: 0, freeMib: 0, recommended: false })).toBe("Graphics card 3");
  });

  it("asks the D3 question for what is using the model", () => {
    const busy = (b: Partial<ModelBusy>): ModelBusy => ({ chatTurns: 0, modelChecks: 0, otherApps: 0, ...b });
    expect(busyQuestion(busy({}))).toBeNull();
    expect(busyQuestion(busy({ chatTurns: 1 }))).toBe("A reply is being written. Stop it and restart the model?");
    expect(busyQuestion(busy({ otherApps: 2 }))).toBe(
      "Another app is using the model. Stop its request and restart the model?",
    );
    expect(busyQuestion(busy({ modelChecks: 1 }))).toBe("A model check is running. Stop it and restart the model?");
    expect(busyQuestion(busy({ chatTurns: 1, otherApps: 1 }))).toBe(BUSY_QUESTIONS.reply);
    expect(waitingWords("reload", busy({ chatTurns: 1 }))).toBe("Restarts when the reply ends");
    expect(waitingWords("reload", null)).toBe("Restarts when the reply ends");
    expect(waitingWords("reload", busy({ otherApps: 1 }))).toBe("Restarts when the other app's request ends");
    expect(waitingWords("unload", busy({ modelChecks: 1 }))).toBe("Unloads when the model check ends");
    expect(waitingWords("switch", busy({ chatTurns: 1 }))).toBe("Changes the model when the reply ends");
  });
});

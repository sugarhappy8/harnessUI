import { describe, expect, it } from "vitest";
import type { BackendKind, ContextSource, FitResult, LoadFailure, LoadStage, ModelBusy, Verdict } from "./types";
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
  fitLine,
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
  amountGb,
  amountNumber,
  appleLimitWords,
  barLegend,
  barTitle,
  capacityGb,
  customPlacementWords,
  CUSTOM_PLACEMENT_HELP,
  expertLayersWords,
  expertsAfterLayersNote,
  fineChoiceWords,
  fineHelp,
  fineLabels,
  fineNotes,
  fineSummary,
  freeAfterLoading,
  layersOfWords,
  leaveFreeWords,
  legendWords,
  modelUsesWords,
  needsWords,
  partsTotal,
  percentWords,
  pictureCaption,
  pictureLayerWords,
  pictureSourceWords,
  pictureWarning,
  placementWords,
  threadsOfWords,
  USAGE_MENU,
  usageGapWords,
  usageIcons,
  usageLine,
  wontFitQuestion,
} from "./words";
import { computerBar, gb, graphicsBar, picture, placement, usage } from "../components/fine.fixtures";

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
  retryingWithLessGpu: BACKENDS.flatMap((backend) => [
    { kind: "retryingWithLessGpu" as const, backend },
    // 0.0.6 item 6: after a placement chosen in Fine control, with and without a layer count.
    { kind: "retryingWithLessGpu" as const, backend, custom: true, chosenLayers: 40 },
    { kind: "retryingWithLessGpu" as const, backend, custom: true },
  ]),
  steppingDown: BACKENDS.flatMap((from) =>
    BACKENDS.flatMap((to) => Object.values(FAILURES).map((reason) => ({ kind: "steppingDown" as const, from, to, reason }))),
  ),
  retryingWithoutFastAttention: BACKENDS.map((backend) => ({ kind: "retryingWithoutFastAttention", backend })),
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
    for (const kind of [
      "retryingWithoutSaveMemory",
      "retryingWithLessGpu",
      "steppingDown",
      "restarting",
      "retryingWithoutFastAttention",
    ] as const) {
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

describe("0.0.6 item 6: plain words for Fine control, the picture and the usage line", () => {
  it("counts memory the way Task Manager does: capacities whole from 10 GB, amounts with one decimal", () => {
    expect(capacityGb(gb(15.9))).toBe("16 GB");
    expect(capacityGb(gb(32))).toBe("32 GB");
    expect(capacityGb(gb(7.46))).toBe("7.5 GB");
    expect(capacityGb(gb(2))).toBe("2 GB");
    expect(amountGb(gb(11.7))).toBe("11.7 GB");
    expect(amountGb(gb(12.0))).toBe("12 GB");
    expect(amountGb(gb(0.04))).toBe("0 GB");
    expect(amountNumber(gb(12.63))).toBe("12.6");
    // A number that is not a finite amount of zero or more is shown as nothing used.
    expect(amountGb(Number.NaN)).toBe("0 GB");
    expect(amountGb(-5)).toBe("0 GB");
    // The device label rounds the same way: a 15.9 GB card reads 16 GB here and in the usage line.
    expect(deviceLabel({ name: "Vulkan0", description: "AMD Radeon RX 9070 XT", totalMib: 16_304, freeMib: 0, recommended: false })).toBe(
      "AMD Radeon RX 9070 XT · 16 GB",
    );
    expect(deviceLabel({ name: "Vulkan1", description: "Small", totalMib: 7640, freeMib: 0, recommended: false })).toBe("Small · 7.5 GB");
  });

  it("words the two new retries, with Mac and processor versions", () => {
    expect(loadStageWords({ kind: "retryingWithoutFastAttention", backend: "vulkan" })).toBe("Trying again without Fast attention");
    expect(adjustmentWords({ kind: "retryingWithoutFastAttention", backend: "vulkan" })).toBe(
      "Fast attention doesn't work with this graphics card, so it was turned off for this load.",
    );
    expect(adjustmentWords({ kind: "retryingWithoutFastAttention", backend: "metal" })).toBe(
      "Fast attention doesn't work with Apple graphics, so it was turned off for this load.",
    );
    expect(adjustmentWords({ kind: "retryingWithoutFastAttention", backend: "cpu" })).toBe(
      "Fast attention doesn't work with this model, so it was turned off for this load.",
    );
    const custom: LoadStage = { kind: "retryingWithLessGpu", backend: "vulkan", custom: true, chosenLayers: 40 };
    expect(loadStageWords(custom)).toBe("Trying again with As much as fits");
    expect(adjustmentWords(custom)).toBe(
      "The 40 layers you chose didn't fit on the graphics card, so As much as fits was used for this load. Choose fewer layers in Fine control.",
    );
    expect(adjustmentWords({ kind: "retryingWithLessGpu", backend: "vulkan", custom: true })).toBe(
      "Your Fine control choices didn't fit on the graphics card, so As much as fits was used for this load.",
    );
    expect(adjustmentWords({ kind: "retryingWithLessGpu", backend: "metal", custom: true, chosenLayers: 1 })).toBe(
      "The 1 layer you chose didn't fit on Apple graphics, so As much as fits was used for this load. Choose fewer layers in Fine control.",
    );
    // Without `custom` (old JSON, or All of it) WP2's words stay.
    expect(loadStageWords({ kind: "retryingWithLessGpu", backend: "vulkan" })).toBe(
      "Trying again with part of the model on the processor",
    );
  });

  it("has the exact labels, help lines and notes of Fine control, and Mac versions that never say graphics card", () => {
    expect(fineLabels(false)).toEqual({
      layers: "Layers on the graphics card",
      experts: "Expert parts on the processor",
      kv: "Conversation memory on the graphics card",
      threads: "Processor threads",
      fastAttention: "Fast attention",
      leaveFree: "Leave free on the graphics card",
    });
    expect(fineLabels(true)).toEqual({
      layers: "Layers on Apple graphics",
      experts: "Expert parts on the processor",
      kv: "Conversation memory with Apple graphics",
      threads: "Processor threads",
      fastAttention: "Fast attention",
      leaveFree: "Memory to leave free",
    });
    const help = fineHelp(false, null);
    expect(help.layers).toBe(
      "The model is built from layers. Layers on the graphics card run much faster than on the processor, as long as they fit in its memory. The rest run on the processor.",
    );
    expect(help.experts).toBe(
      "This model is a mixture of experts: it has many expert parts but uses only a few of them for each word. Keeping expert parts on the processor frees a lot of graphics memory and is often only a little slower. Automatic keeps them on the graphics card with All of it, and moves them first when As much as fits needs room.",
    );
    expect(help.kv).toBe(
      "Where the model keeps the chat it is working on. On the graphics card is faster. Keeping it in computer memory leaves more room for layers on the graphics card, but replies slow down as the chat grows.",
    );
    expect(help.threads).toBe(
      "How many of the processor's threads the model may use for the parts that run on the processor. Automatic is usually fastest. Fewer keeps the computer more responsive for other work.",
    );
    expect(help.fastAttention).toBe(
      "A faster way for the model to look back over the chat that also uses less memory. Automatic turns it on where the graphics card supports it.",
    );
    expect(help.leaveFree).toBe(
      "With As much as fits, this much graphics memory is left free for the screen and other programs, such as games or video. More free memory means fewer layers on the graphics card.",
    );
    const mac = fineHelp(true, gb(10.7));
    expect(mac.layers).toBe(
      "The model is built from layers. Layers on Apple graphics run much faster than on the processor, as long as they fit in Apple graphics' share of memory (up to 10.7 GB here). The rest run on the processor.",
    );
    expect(mac.kv).toBe(
      "On a Mac the processor and Apple graphics share one memory. Keeping the conversation with the processor leaves Apple graphics more of its share (up to 10.7 GB here) for layers, but replies slow down as the chat grows.",
    );
    expect(mac.fastAttention).toBe(
      "A faster way for the model to look back over the chat that also uses less memory. Automatic turns it on where Apple graphics supports it.",
    );
    expect(mac.leaveFree).toBe(
      "With As much as fits, this much of Apple graphics' share of memory is left free for other programs. More free memory means fewer layers on Apple graphics.",
    );
    expect(mac.experts).toBe(
      "This model is a mixture of experts: it has many expert parts but uses only a few of them for each word. Keeping expert parts on the processor frees a lot of Apple graphics' share of memory and is often only a little slower. Automatic keeps them on Apple graphics with All of it, and moves them first when As much as fits needs room.",
    );
    expect(fineHelp(true, null).layers).toContain("share of memory. The rest");
    // A Mac's help lines, labels and notes say Apple graphics, never graphics card (checklist 19).
    for (const text of [...Object.values(mac), ...Object.values(fineLabels(true)), ...Object.values(fineNotes(true))]) {
      expect(text, text).not.toMatch(/graphics card/);
    }
    expect(fineNotes(false)).toEqual({
      gpuOff: "Not used while Use the graphics card is Don't use it.",
      leaveFreeAll: "Only used with As much as fits.",
      leaveFreeCustom: "Not used while you choose the layers or expert parts yourself.",
      expertsNoLayers: "Not used while no layers are on the graphics card.",
      noGraphics: "Not used without a graphics card.",
    });
    expect(fineNotes(true).gpuOff).toBe("Not used while Use Apple graphics is Don't use it.");
    expect(fineNotes(true).noGraphics).toBe("Not used without Apple graphics.");
    expect(expertsAfterLayersNote(32, 41)).toBe(
      "The first 9 layers already run on the processor. Only expert parts of later layers free graphics memory.",
    );
    expect(expertsAfterLayersNote(40, 41)).toBe(
      "The first layer already runs on the processor. Only expert parts of later layers free graphics memory.",
    );
    const choices = fineChoiceWords(false);
    expect(choices.layersAuto).toBe("Automatic (follows Use the graphics card)");
    expect(fineChoiceWords(true).layersAuto).toBe("Automatic (follows Use Apple graphics)");
    expect(choices.kvAuto).toBe("Yes, faster (Automatic)");
    expect(choices.kvComputer).toBe("No, keep it in computer memory");
    expect(fineChoiceWords(true).kvComputer).toBe("No, keep it with the processor");
    expect(choices.onForSaveMemory).toBe("On (Save memory needs it)");
    expect(choices.leaveFreeAuto).toBe("Automatic (1 GB)");
    expect([512, 2048, 4096].map(leaveFreeWords)).toEqual(["0.5 GB", "2 GB", "4 GB"]);
    expect(layersOfWords(30, 41)).toBe("30 of 41 layers");
    expect(expertLayersWords(20, 48)).toBe("the first 20 of 48 layers");
    expect(threadsOfWords(8, 16)).toBe("8 of 16 threads");
    expect(customPlacementWords(30, 41)).toBe("Custom: 30 of 41 layers (set in Fine control)");
    expect(customPlacementWords(null, 41)).toBe("Custom: expert parts on the processor (set in Fine control)");
    expect(CUSTOM_PLACEMENT_HELP).toBe("Fine control sets where the model goes. Choose another option here to go back to it.");
  });

  it("sums the closed section: layers on the card, and changed", () => {
    expect(fineSummary(picture(), false, false)).toBe("Fine control · 30 of 41 layers on the graphics card");
    expect(fineSummary(picture(), false, true)).toBe("Fine control · 30 of 41 layers on the graphics card · changed");
    expect(fineSummary(picture({ layersAbout: true }), true, false)).toBe("Fine control · About 30 of 41 layers on Apple graphics");
    expect(fineSummary(picture({ graphics: null, layersOnGraphics: 0 }), false, true)).toBe(
      "Fine control · On the processor only · changed",
    );
    expect(fineSummary(null, false, true)).toBe("Fine control · changed");
    expect(fineSummary(picture({ layersOnGraphics: 41 }), false, false)).toBe("Fine control · All 41 layers on the graphics card");
  });

  it("says where the layers go in the picture, for every placement", () => {
    expect(pictureLayerWords(picture({ layersOnGraphics: 41 }), false)).toBe("All 41 layers on the graphics card.");
    expect(pictureLayerWords(picture(), false)).toBe("30 of 41 layers on the graphics card, 11 on the processor.");
    expect(pictureLayerWords(picture({ layersAbout: true, custom: false }), false)).toBe(
      "About 30 of 41 layers on the graphics card. The rest run on the processor. The exact number is decided when the model loads.",
    );
    expect(pictureLayerWords(picture({ graphics: null, layersOnGraphics: 0 }), false)).toBe("On the processor only.");
    expect(pictureLayerWords(picture({ layersOnGraphics: 41, expertLayersOnProcessor: 20 }), false)).toBe(
      "All 41 layers on the graphics card. Expert parts of 20 layers stay on the processor.",
    );
    expect(
      pictureLayerWords(picture({ units: 49, layersOnGraphics: 49, layersAbout: true, expertLayersOnProcessor: 19 }), false),
    ).toBe(
      "All 49 layers on the graphics card. Expert parts of about 19 layers stay on the processor. The exact split is decided when the model loads.",
    );
    // A Mac: one bar, no graphics bar, but layers on Apple graphics.
    expect(pictureLayerWords(picture({ unified: true, graphics: null }), true)).toBe(
      "30 of 41 layers on Apple graphics, 11 on the processor.",
    );
    expect(pictureLayerWords(picture({ units: null }), false)).toBeNull();
  });

  it("has a source line for every answer", () => {
    const off = { measuring: false, fineOpen: false };
    expect(pictureSourceWords({ kind: "measured" }, off)).toBe("Worked out for these settings.");
    expect(pictureSourceWords({ kind: "estimate", why: "quick" }, off)).toBe(
      "Rough estimate from the model file. Open Fine control to work out exact figures.",
    );
    expect(pictureSourceWords({ kind: "estimate", why: "quick" }, { measuring: true, fineOpen: true })).toBe(
      "Working out exact figures…",
    );
    // Fine control is open and the answer is still the quick estimate: it could not be worked out.
    expect(pictureSourceWords({ kind: "estimate", why: "quick" }, { measuring: false, fineOpen: true })).toBe(
      "Rough estimate from the model file. Exact figures couldn't be worked out for this file.",
    );
    expect(pictureSourceWords({ kind: "estimate", why: "failed" }, off)).toBe(
      "Rough estimate from the model file. Exact figures couldn't be worked out for this file.",
    );
    expect(pictureSourceWords({ kind: "estimate", why: "timedOut" }, off)).toBe(
      "Rough estimate from the model file. Working them out took too long.",
    );
    expect(pictureSourceWords({ kind: "estimate", why: "noTool", backend: "rocm" }, off)).toBe(
      "Rough estimate from the model file. The AMD graphics card (ROCm) download can't work out exact figures.",
    );
    expect(pictureSourceWords({ kind: "estimate", why: "noTool", backend: "vulkan" }, off)).toBe(
      "Rough estimate from the model file. Exact figures can't be worked out with this download.",
    );
    expect(pictureSourceWords({ kind: "estimate", why: "notReadable" }, off)).toBe(
      "Rough estimate. The model file's details couldn't be read.",
    );
    expect(pictureCaption(false)).toBe("Memory is counted the way Task Manager counts it, so it can differ from the download size.");
    expect(pictureCaption(true)).toContain("Activity Monitor");
  });

  it("gives each bar a title, a legend that adds up to the whole, and what it needs", () => {
    const g = graphicsBar();
    expect(barTitle(g, "graphics")).toBe("Graphics card · AMD Radeon RX 9070 XT · 16 GB");
    expect(barTitle(graphicsBar({ name: " " }), "graphics")).toBe("Graphics card · 16 GB");
    expect(barTitle(computerBar(), "computer")).toBe("Computer memory · 32 GB");
    expect(barTitle(computerBar({ totalBytes: gb(16) }), "shared")).toBe("Memory · shared by the processor and Apple graphics · 16 GB");
    expect(barLegend(g).map(legendWords)).toEqual([
      "Model 9.9 GB",
      "Conversation memory 1.2 GB",
      "Working space 0.6 GB",
      "Other programs 0.9 GB",
      "Free after loading 3.3 GB",
    ]);
    // The parts add up to the card's memory (the worked example: 15.9 GB).
    const sum = barLegend(g).reduce((a, i) => a + i.bytes, 0);
    expect(Math.abs(sum - g.totalBytes)).toBeLessThan(gb(0.01));
    expect(barLegend(computerBar()).map(legendWords)).toEqual([
      "Model 4.1 GB",
      "Conversation memory 0.4 GB",
      "Working space 0.3 GB",
      "Other programs 12 GB",
      "Free after loading 15.1 GB",
    ]);
    expect(needsWords(g)).toBe("Needs about 11.7 GB of the 15 GB free now.");
    expect(needsWords(graphicsBar({ freeAfterUnload: true }))).toBe(
      "Needs about 11.7 GB of the 15 GB free once the loaded model is unloaded.",
    );
    const reader = graphicsBar({ need: { model: gb(9.9), context: gb(1.2), compute: gb(0.6), imageReader: gb(0.9) } });
    expect(barLegend(reader).map(legendWords)).toContain("Image reader 0.9 GB");
    // More than is free: nothing is left, and the legend says no free part.
    const over = graphicsBar({ need: { model: gb(14), context: gb(1.2), compute: gb(0.6), imageReader: 0 } });
    expect(freeAfterLoading(over)).toBe(0);
    expect(barLegend(over).some((i) => i.part === "free")).toBe(false);
    expect(partsTotal({ model: gb(1), context: Number.NaN, compute: -1, imageReader: gb(1) })).toBe(gb(2));
    expect(appleLimitWords(gb(10.7))).toBe("Apple graphics can use up to 10.7 GB");
  });

  it("warns once, with the figures, and with Mac words", () => {
    const opts = { apple: false, saveMemory: false };
    expect(pictureWarning(picture({ fits: "fits" }), opts)).toBeNull();
    expect(pictureWarning(picture({ fits: "partial" }), opts)).toBeNull();
    const big = picture({
      fits: "wontFitGraphics",
      custom: false,
      graphics: graphicsBar({ need: { model: gb(13.8), context: gb(1.2), compute: gb(0.6), imageReader: 0 } }),
    });
    expect(pictureWarning(big, opts)).toBe(
      "This probably won't fit in the graphics card's memory: it needs about 15.6 GB and 15 GB is free. Choose As much as fits, or turn on Save memory.",
    );
    expect(pictureWarning({ ...big, custom: true }, opts)).toBe(
      "This probably won't fit in the graphics card's memory: it needs about 15.6 GB and 15 GB is free. Choose fewer layers or keep expert parts on the processor in Fine control, or turn on Save memory.",
    );
    // Save memory already on: the warning does not ask for it again.
    expect(pictureWarning(big, { apple: false, saveMemory: true })).toBe(
      "This probably won't fit in the graphics card's memory: it needs about 15.6 GB and 15 GB is free. Choose As much as fits.",
    );
    const all = picture({
      fits: "wontFitAtAll",
      graphics: graphicsBar({ freeBytes: gb(15.0), need: { model: gb(16), context: gb(1), compute: gb(1), imageReader: 0 } }),
      computer: computerBar({ freeBytes: gb(15.2), need: { model: gb(18), context: gb(1), compute: gb(1), imageReader: 0 } }),
    });
    expect(pictureWarning(all, opts)).toBe(
      "This probably needs more memory than this computer has free: about 38 GB, with 30.2 GB free. Choose less to remember, or a smaller download.",
    );
    expect(pictureWarning(picture({ fits: "tight" }), opts)).toBe(
      "It only just fits. Programs that use the graphics card, such as games or video, may stop it from loading.",
    );
    const mac = picture({ fits: "wontFitGraphics", custom: false, unified: true, graphics: null, graphicsLimitBytes: gb(10.7) });
    expect(pictureWarning(mac, { apple: true, saveMemory: false })).toBe(
      "This probably won't fit in Apple graphics' share of memory: it needs about 4.8 GB and 10.7 GB is free. Choose As much as fits, or turn on Save memory.",
    );
    expect(pictureWarning(picture({ fits: "tight" }), { apple: true, saveMemory: false })).toBe(
      "It only just fits. Programs that use Apple graphics, such as games or video, may stop it from loading.",
    );
    expect(wontFitQuestion(false)).toBe(
      "This probably won't fit in the graphics card's memory. If it doesn't, the model won't load, or part of it moves to the processor, which is slower.",
    );
    expect(wontFitQuestion(true)).toBe(
      "This probably won't fit in Apple graphics' share of memory. If it doesn't, the model won't load, or part of it moves to the processor, which is slower.",
    );
  });

  it("says where the loaded model went, in every variant", () => {
    expect(placementWords(placement(), false)).toBe("30 of 41 layers on the graphics card, 11 on the processor.");
    expect(placementWords(placement({ layersOnGraphics: 41 }), false)).toBe("All 41 layers on the graphics card.");
    expect(placementWords(placement({ layersAbout: true, custom: false }), false)).toBe(
      "About 30 of 41 layers on the graphics card, the rest on the processor.",
    );
    expect(placementWords(placement({ graphics: null, layersOnGraphics: 0 }), false)).toBe("On the processor only.");
    expect(placementWords(placement({ units: 49, layersOnGraphics: 49, layersAbout: true, expertLayersOnProcessor: 19 }), false)).toBe(
      "All 49 layers on the graphics card. Expert parts of about 19 layers are on the processor.",
    );
    expect(placementWords(placement({ units: 49, layersOnGraphics: 49, expertLayersOnProcessor: 20 }), false)).toBe(
      "All 49 layers on the graphics card. Expert parts of 20 layers are on the processor.",
    );
    expect(
      placementWords(
        placement({ layersOnGraphics: null, graphics: { name: "x", use: { model: gb(12.1), context: 0, compute: 0, imageReader: 0 } } }),
        false,
      ),
    ).toBe("About 12.1 GB of the model is on the graphics card, the rest on the processor.");
    // Only amounts known: "the rest on the processor" only when part of it is (As much as
    // fits, or expert parts chosen in Fine control); All of it puts all its layers on the card.
    expect(placementWords(placement({ custom: false, layersAbout: true, layersOnGraphics: null }), false)).toBe(
      "About 9.9 GB of the model is on the graphics card, the rest on the processor.",
    );
    expect(placementWords(placement({ custom: false, layersAbout: false, layersOnGraphics: null, units: null }), false)).toBe(
      "About 9.9 GB of the model is on the graphics card.",
    );
    expect(placementWords(placement({ custom: false, layersAbout: false, layersOnGraphics: null, units: null }), true)).toBe(
      "About 9.9 GB of the model is on Apple graphics.",
    );
    expect(placementWords(placement(), true)).toBe("30 of 41 layers on Apple graphics, 11 on the processor.");
    expect(modelUsesWords(placement().graphics!.use)).toBe(
      "The model uses 11.7 GB: 9.9 GB for itself, 1.2 GB conversation memory, 0.6 GB working space",
    );
    expect(modelUsesWords({ model: gb(1), context: gb(1), compute: gb(1), imageReader: gb(0.9) })).toBe(
      "The model uses 3.9 GB: 1 GB for itself, 1 GB conversation memory, 1 GB working space, 0.9 GB image reader",
    );
  });

  it("writes the usage line in each width, with Mac wording and missing figures", () => {
    const now = usage();
    expect(usageLine(now, "wide")).toBe("Graphics card 63% · 12.6 of 16 GB · Processor 12% · Memory 16.8 of 32 GB");
    expect(usageLine(now, "medium")).toBe("Graphics card 63% · 12.6/16 GB · Processor 12% · Memory 16.8/32 GB");
    expect(usageLine(now, "small")).toBe("Graphics card 63% · Processor 12% · Memory 53%");
    expect(usageIcons(now)).toEqual([
      { kind: "graphics", value: "63%", label: "Graphics card 63% busy" },
      { kind: "processor", value: "12%", label: "Processor 12% busy" },
      { kind: "memory", value: "53%", label: "Memory 53% in use" },
    ]);
    const mac = usage({
      unified: true,
      graphics: {
        kind: "value",
        value: { name: "Apple M2", busyPercent: { kind: "value", value: 40 }, memoryUsedBytes: { kind: "value", value: gb(6.1) }, memoryTotalBytes: gb(10.7) },
      },
      memory: { kind: "value", value: { usedBytes: gb(11.2), totalBytes: gb(16) } },
    });
    expect(usageLine(mac, "wide")).toBe("Apple graphics 40% · Processor 12% · Memory 11.2 of 16 GB");
    expect(usageLine(mac, "small")).toBe("Apple graphics 40% · Processor 12% · Memory 70%");
    // The first sample, and half a reading.
    const first = usage({ processorPercent: { kind: "measuring" } });
    expect(usageLine(first, "wide")).toBe("Graphics card 63% · 12.6 of 16 GB · Processor … · Memory 16.8 of 32 GB");
    const noBusy = usage();
    if (noBusy.graphics.kind === "value") noBusy.graphics.value.busyPercent = { kind: "notAvailable", why: "systemGivesNone" };
    expect(usageLine(noBusy, "wide")).toBe("Graphics card – · 12.6 of 16 GB · Processor 12% · Memory 16.8 of 32 GB");
    const noMemory = usage();
    if (noMemory.graphics.kind === "value") noMemory.graphics.value.memoryUsedBytes = { kind: "notAvailable", why: "readFailed" };
    expect(usageLine(noMemory, "wide")).toBe("Graphics card 63% · – of 16 GB · Processor 12% · Memory 16.8 of 32 GB");
    expect(usageIcons(noBusy)[0]).toEqual({ kind: "graphics", value: "–", label: "Graphics card: not available" });
    // No graphics card: the graphics part is left out.
    const none = usage({ graphics: { kind: "notAvailable", why: "noGraphicsCard" } });
    expect(usageLine(none, "wide")).toBe("Processor 12% · Memory 16.8 of 32 GB");
    expect(usageIcons(none).map((i) => i.kind)).toEqual(["processor", "memory"]);
    expect(usageLine(usage({ graphics: { kind: "notAvailable", why: "notReadHere" } }), "small")).toBe(
      "Graphics card – · Processor 12% · Memory 53%",
    );
    expect(percentWords({ kind: "value", value: 140 })).toBe("100%");
    expect(percentWords({ kind: "value", value: Number.NaN })).toBe("–");
  });

  it("says why a figure is missing, on its own row", () => {
    expect(usageGapWords("systemGivesNone", "graphicsBusy", false)).toBe("This computer doesn't tell apps how busy its graphics card is.");
    expect(usageGapWords("systemGivesNone", "graphicsMemory", false)).toBe(
      "This computer doesn't tell apps how much of its graphics memory is in use.",
    );
    expect(usageGapWords("systemGivesNone", "graphicsBusy", true)).toBe("This Mac doesn't tell apps how busy Apple graphics is.");
    expect(usageGapWords("notReadHere", "graphicsBusy", false)).toBe("harnessUI can't read this graphics card on this system yet.");
    expect(usageGapWords("readFailed", "processor", false)).toBe("Couldn't read this just now.");
    expect(USAGE_MENU).toEqual({
      title: "This computer right now",
      privacy: "Read on this computer only while you can see it. Nothing is sent anywhere.",
    });
    for (const gap of ["noGraphicsCard", "systemGivesNone", "notReadHere", "readFailed"] as const) {
      for (const what of ["graphicsBusy", "graphicsMemory", "processor", "memory"] as const) {
        plain(usageGapWords(gap, what, false));
        expect(usageGapWords(gap, what, true)).not.toMatch(/graphics card's|its graphics card/);
      }
    }
  });
});

describe("compare-fit-words: \"Will it fit on this computer?\" in plain words", () => {
  const GIB = 1024 ** 3;
  // The 0.0.4 sentence the backend still sends: never shown.
  const fit = (over: Partial<FitResult>): FitResult => ({
    verdict: "green",
    neededBytes: 4.1 * GIB,
    availableBytes: 14.6 * GIB,
    suggestedCtx: null,
    partialOffload: false,
    device: "graphicsCard",
    computerBytes: 0,
    sentence: "Needs about 4.4 GB; your GPU has 15.7 GB free. Fits.",
    ...over,
  });

  it("says what it needs and what is free where it would run, counted the way Task Manager counts memory", () => {
    expect(fitLine(fit({}))).toBe("Needs about 4.1 GB; this computer has 14.6 GB free on the graphics card.");
    expect(fitLine(fit({ device: "appleGraphics", availableBytes: 10.7 * GIB }))).toBe(
      "Needs about 4.1 GB; Apple graphics can use up to 10.7 GB on this Mac.",
    );
    expect(fitLine(fit({ device: "processor", availableBytes: 11.2 * GIB }))).toBe(
      "Needs about 4.1 GB; this computer has 11.2 GB free. It would run on the processor.",
    );
  });

  it("says what a yellow or red verdict means, with token counts in words, and never repeats the verdict", () => {
    const card = "this computer has 14.6 GB free on the graphics card.";
    expect(fitLine(fit({ verdict: "yellow", neededBytes: 9.5 * GIB, suggestedCtx: 16384 }))).toBe(
      `Needs about 9.5 GB; ${card} It has room if it remembers up to 16,384 tokens, about 12,000 words.`,
    );
    expect(fitLine(fit({ verdict: "yellow", neededBytes: 18.3 * GIB, partialOffload: true }))).toBe(
      `Needs about 18.3 GB; ${card} Part of it would run on the processor, which is slower.`,
    );
    expect(fitLine(fit({ verdict: "yellow", neededBytes: 14 * GIB }))).toBe(`Needs about 14 GB; ${card} It only just fits.`);
    expect(fitLine(fit({ verdict: "red", neededBytes: 48 * GIB }))).toBe(
      `Needs about 48 GB; ${card} There is not enough memory free for it, even with part of it in the computer's memory.`,
    );
    // On the processor some memory is kept for other programs, so "has more free than it
    // needs" can still be too large: said, not left as a puzzle. A model too large to run
    // is never said to run on the processor (compare-fit-minors).
    expect(fitLine(fit({ verdict: "red", device: "processor", neededBytes: 10.5 * GIB, availableBytes: 11.2 * GIB }))).toBe(
      "Needs about 10.5 GB; this computer has 11.2 GB free. There is not enough memory free to run it on the processor. Part of that memory is kept free for other programs.",
    );
    for (const verdict of Object.keys(VERDICTS) as Verdict[]) {
      for (const device of ["graphicsCard", "appleGraphics", "processor"] as const) {
        for (const extra of [{}, { suggestedCtx: 8192 }, { partialOffload: true }]) {
          const line = fitLine(fit({ verdict, device, ...extra }));
          plain(line);
          expect(line).not.toMatch(/Fits|GPU|CPU|context|layer|your /);
          // Token counts always come with words, never as a bare number.
          expect(line).not.toMatch(/\d{4}/);
          expect(line.endsWith(".")).toBe(true);
        }
      }
    }
  });

  // compare-fit-minors (2): "It would run on the processor" only when it would run.
  it("says a model runs on the processor only when it would, and what is short when it is too large", () => {
    const cpu = { device: "processor" as const, availableBytes: 11.2 * GIB };
    expect(fitLine(fit({ ...cpu, verdict: "red", neededBytes: 12.5 * GIB }))).toBe(
      "Needs about 12.5 GB; this computer has 11.2 GB free. There is not enough memory free to run it on the processor.",
    );
    expect(fitLine(fit({ ...cpu, verdict: "red", neededBytes: 12.5 * GIB, suggestedCtx: 8192 }))).toBe(
      "Needs about 12.5 GB; this computer has 11.2 GB free. There is not enough memory free to run it on the processor. " +
        "It has room if it remembers up to 8,192 tokens, about 6,000 words.",
    );
    expect(fitLine(fit({ ...cpu, verdict: "yellow", neededBytes: 9 * GIB, suggestedCtx: 8192 }))).toBe(
      "Needs about 9 GB; this computer has 11.2 GB free. It would run on the processor. " +
        "It has room if it remembers up to 8,192 tokens, about 6,000 words.",
    );
    expect(fitLine(fit({ device: "appleGraphics", availableBytes: 10.7 * GIB, verdict: "red", neededBytes: 30 * GIB }))).toBe(
      "Needs about 30 GB; Apple graphics can use up to 10.7 GB on this Mac. There is not enough memory free for it on this Mac.",
    );
    for (const device of ["graphicsCard", "appleGraphics", "processor"] as const) {
      for (const extra of [{}, { suggestedCtx: 8192 }, { neededBytes: 9 * GIB }, { computerBytes: 3 * GIB }]) {
        const red = fitLine(fit({ verdict: "red", device, ...extra }));
        expect(red).not.toMatch(/would run|It only just fits|which is slower/);
        expect(red).toMatch(/There is not enough memory free/);
        expect(fitLine(fit({ verdict: "green", device, ...extra })).includes("It would run on the processor.")).toBe(device === "processor");
      }
    }
  });

  // compare-fit-minors (1): with part of the model in the computer's memory, the line
  // gives the whole need and how it is split, never only the card's share.
  it("gives the whole need and how it is split when part of it is in the computer's memory", () => {
    // `installed_answers_follow_the_one_verdict`: As much as fits on a 4 GB card.
    const partial = fit({ verdict: "yellow", neededBytes: 2.7 * GIB, availableBytes: 3.7 * GIB, computerBytes: 1.9 * GIB, partialOffload: true });
    expect(fitLine(partial)).toBe(
      "Needs about 4.6 GB in all: 2.7 GB on the graphics card, which has 3.7 GB free, and 1.9 GB in the computer's memory. " +
        "Part of it would run on the processor, which is slower.",
    );
    // All of it on the card but its input part, which stays in the computer's memory.
    expect(fitLine(fit({ neededBytes: 4.1 * GIB, computerBytes: 0.3 * GIB }))).toBe(
      "Needs about 4.4 GB in all: 4.1 GB on the graphics card, which has 14.6 GB free, and 0.3 GB in the computer's memory.",
    );
    // A computer's part too small to show is left out; on Apple graphics and the processor
    // the amount already counts all of it.
    expect(fitLine(fit({ computerBytes: 0.01 * GIB }))).toBe("Needs about 4.1 GB; this computer has 14.6 GB free on the graphics card.");
    expect(fitLine(fit({ device: "appleGraphics", availableBytes: 10.7 * GIB, computerBytes: 2 * GIB }))).toBe(
      "Needs about 4.1 GB; Apple graphics can use up to 10.7 GB on this Mac.",
    );
    // Whenever part of it would run on the processor on a graphics card, the line names
    // the computer's part, so "needs less than is free" never stands alone.
    const splits: [number, number, number][] = [
      [2.7, 3.7, 1.9],
      [13.6, 14.6, 6.4],
      [6.6, 6.5, 0.3],
    ];
    for (const [needed, free, computer] of splits) {
      const line = fitLine(fit({ verdict: "yellow", neededBytes: needed * GIB, availableBytes: free * GIB, computerBytes: computer * GIB, partialOffload: true }));
      expect(line).toContain(`${computer} GB in the computer's memory`);
      expect(line).toContain(`Needs about ${Math.round((needed + computer) * 10) / 10} GB in all`);
      plain(line);
      expect(line).not.toMatch(/Fits|GPU|CPU|context|layer|your /);
    }
  });
});

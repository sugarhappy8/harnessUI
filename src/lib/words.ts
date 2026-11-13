// Plain words for the app's technical values (0.0.6 D4, D8). Nothing on screen should show
// a raw identifier such as "vulkan", "Vulkan0" or "text_model". Every phrase of the model
// loading screens lives here, so the wording stays the same wherever it appears; the test
// walks every value of the contract's enums.
import { APP_NAME } from "./app";
import type {
  BackendKind,
  ContextSource,
  DeviceOption,
  FitDevice,
  FitResult,
  GpuUse,
  LoadFailure,
  LoadStage,
  MemoryBar,
  ModelBusy,
  OffloadPreview,
  Parts,
  PictureSource,
  Placement,
  Reading,
  ThinkingLimit,
  UsageGap,
  UsageNow,
  Verdict,
} from "./types";

/** Backends in plain words (D4). */
export const BACKEND_WORDS: Record<BackendKind, string> = {
  vulkan: "Graphics card (Vulkan)",
  cuda: "NVIDIA graphics card (CUDA)",
  rocm: "AMD graphics card (ROCm)",
  metal: "Apple graphics (Metal)",
  cpu: "Processor only (slower)",
};

/** A backend name as the runtime reports it (`vulkan`), in plain words; null is Auto. */
export function backendWords(kind: string | null): string {
  if (kind === null) return "Automatic";
  return (BACKEND_WORDS as Record<string, string | undefined>)[kind] ?? kind;
}

/** Where a backend runs the model, for the middle of a sentence ("runs on the processor"). */
const PLACE: Record<BackendKind, string> = {
  vulkan: "the graphics card",
  cuda: "the NVIDIA graphics card",
  rocm: "the AMD graphics card",
  metal: "Apple graphics",
  cpu: "the processor",
};

function place(kind: BackendKind): string {
  return PLACE[kind] ?? "another backend";
}

function capitalise(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** "Use the graphics card" choices (D4). */
export const GPU_USE_WORDS: Record<GpuUse, string> = {
  all: "All of it",
  fit: "As much as fits",
  off: "Don't use it",
};

/** "How long it may think" choices (D4). */
export const THINKING_WORDS: Record<ThinkingLimit, string> = {
  noLimit: "Default",
  short: "Short",
  medium: "Medium",
  long: "Long",
};

/** A fit verdict as a word, never colour alone (the same words as the download list). */
export const FIT_WORDS: Record<Verdict, string> = {
  green: "Fits",
  yellow: "Fits with limits",
  red: "Too large",
};

/** Why the loaded model remembers what it does (the ready line's reason, D1). */
export const CONTEXT_SOURCE_WORDS: Record<ContextSource, string> = {
  automatic: "Automatic",
  modelSetting: "set for this model",
  defaultSetting: "default for all models",
  trainedLimit: "the most this model can",
};

/**
 * Why a load on a backend failed, short, for the middle of a sentence after the backend
 * ("because the graphics card ran out of memory"). Running out of room for the
 * conversation memory is running out of memory too.
 */
export const LOAD_FAILURE_WORDS: Record<LoadFailure["code"], string> = {
  out_of_memory: "ran out of memory",
  invalid_device: "was not found",
  missing_runtime_dll: "is missing a file it needs",
  port_in_use: "could not get the connection it needs",
  model_file_invalid: "could not load this model",
  context_too_large: "ran out of memory",
  unknown: "stopped while loading",
};

/** What can help after a step-down for this reason: Reload, Model settings…, or nothing. */
export function stepDownAction(reason: LoadFailure): "reload" | "settings" | null {
  switch (reason.code) {
    case "invalid_device":
      return "settings";
    case "model_file_invalid":
      return null;
    default:
      return "reload";
  }
}

/** Why a step-down happened, as a clause with its subject ("the graphics card ran out of memory"). */
function stepDownCause(from: BackendKind, reason: LoadFailure): string {
  // The device was the chosen one (or the backend's own pick); "the chosen graphics card"
  // says which without its device id.
  const subject = reason.code === "invalid_device" ? "the chosen graphics card" : place(from);
  return `${subject} ${LOAD_FAILURE_WORDS[reason.code] ?? LOAD_FAILURE_WORDS.unknown}`;
}

/**
 * About how many words fit in `tokens` tokens (three quarters of a token count), rounded
 * down to a whole thousand from 1,000 words up, else to a whole hundred.
 */
export function aboutWords(tokens: number): number {
  const words = Math.floor((Math.max(0, tokens) * 3) / 4);
  return words >= 1000 ? Math.floor(words / 1000) * 1000 : Math.floor(words / 100) * 100;
}

function count(n: number): string {
  return n.toLocaleString("en-US");
}

/** A token count with words, as the rules ask: "8,192 tokens, about 6,000 words". */
export function tokensWithWords(tokens: number): string {
  return `${count(tokens)} tokens, about ${count(aboutWords(tokens))} words`;
}

/** A load stage in plain words, for the loading line ("Loading model · Reading the model file"). */
export function loadStageWords(stage: LoadStage): string {
  switch (stage.kind) {
    case "starting":
      return "Starting";
    case "readingModel":
      return "Reading the model file";
    case "readingImageReader":
      return "Loading the image reader";
    case "loading":
      return "Getting ready";
    case "restarting":
      return `Restarting after a problem (${stage.attempt} of ${stage.max})`;
    case "retryingWithoutSaveMemory":
      return "Trying again without Save memory";
    case "retryingWithLessGpu":
      // After a placement chosen in Fine control, the retry is As much as fits (item 6, 3.3).
      if (stage.custom) return "Trying again with As much as fits";
      return "Trying again with part of the model on the processor";
    case "steppingDown":
      return `${capitalise(stepDownCause(stage.from, stage.reason))}, so trying ${place(stage.to)}`;
    case "retryingWithoutFastAttention":
      return "Trying again without Fast attention";
  }
}

/**
 * What a load had to change to start, as a sentence that stays visible after the load
 * (`RuntimeStatus.adjustments`); null for the plain stages, which change nothing.
 */
export function adjustmentWords(stage: LoadStage): string | null {
  switch (stage.kind) {
    case "starting":
    case "readingModel":
    case "readingImageReader":
    case "loading":
      return null;
    case "restarting":
      return "The model stopped unexpectedly and was restarted.";
    case "retryingWithoutSaveMemory":
      // On the processor only the model refusing Save memory turns it off (WP1a); on a
      // graphics card it is D2c, word for word.
      if (stage.backend === "cpu") return "Save memory doesn't work with this model, so it was turned off for this load.";
      if (stage.backend === "metal") {
        return "Save memory doesn't work with this model on Apple graphics, so it was turned off for this load.";
      }
      return "Save memory doesn't work with this graphics card, so it was turned off for this load.";
    case "retryingWithLessGpu":
      if (stage.custom) {
        // Item 6, 3.3: the placement chosen in Fine control did not fit.
        if (stage.chosenLayers !== undefined) {
          return `The ${count(stage.chosenLayers)} ${stage.chosenLayers === 1 ? "layer" : "layers"} you chose didn't fit on ${place(stage.backend)}, so As much as fits was used for this load. Choose fewer layers in Fine control.`;
        }
        return `Your Fine control choices didn't fit on ${place(stage.backend)}, so As much as fits was used for this load.`;
      }
      return `${capitalise(place(stage.backend))} ran out of memory, so part of the model runs on the processor. Replies are slower.`;
    case "steppingDown": {
      const cause = `Running on ${place(stage.to)} because ${stepDownCause(stage.from, stage.reason)}.`;
      const action = stepDownAction(stage.reason);
      if (action === "reload") return `${cause} Reload to try ${place(stage.from)} again.`;
      if (action === "settings") return `${cause} Choose another in Model settings.`;
      return cause;
    }
    case "retryingWithoutFastAttention":
      // Item 6, 3.3, as Save memory's sentence: the model on the processor, the card on a card.
      if (stage.backend === "cpu") return "Fast attention doesn't work with this model, so it was turned off for this load.";
      if (stage.backend === "metal") return "Fast attention doesn't work with Apple graphics, so it was turned off for this load.";
      return "Fast attention doesn't work with this graphics card, so it was turned off for this load.";
  }
}

/** The slow states that need no number (D4 "Slow states as visible text"). */
export const SLOW_WORDS = {
  partial:
    "Part of the model runs on the processor, so replies are slower. Save memory or a smaller memory size lets more fit on the graphics card.",
  afterRestart: "The first reply after a restart takes longer while the model rereads the chat.",
} as const;

/** Generation speed in words per second (three quarters of the tokens). */
export function wordsPerSecond(tokensPerSecond: number): string {
  const words = Math.round(Math.max(0, tokensPerSecond) * 0.75);
  if (words < 1) return "less than 1 word a second";
  return words === 1 ? "about 1 word a second" : `about ${count(words)} words a second`;
}

/** The slow state on a graphics card ("Loaded but slow" in 0.0.4). */
export function slowSpeedWords(tokensPerSecond: number): string {
  return `Replies are slow here (${wordsPerSecond(tokensPerSecond)}). The graphics card may be short of memory. Try a smaller memory size or a smaller model.`;
}

/**
 * The ready line in parts, for a crowded header: the whole line is `remembers` + `reason`;
 * `about` and `words` are the shorter forms, each with the number whole.
 */
export function readyLineParts(
  nCtx: number,
  source: ContextSource | null,
): { remembers: string; reason: string; about: string; words: string } {
  const words = `${count(aboutWords(nCtx))} words`;
  return {
    remembers: `Remembers about ${words}`,
    reason: source ? ` (${CONTEXT_SOURCE_WORDS[source]})` : "",
    about: `About ${words}`,
    words,
  };
}

/** The ready line (D1): "Remembers about 24,000 words (Automatic)". */
export function readyLine(nCtx: number, source: ContextSource | null): string {
  const parts = readyLineParts(nCtx, source);
  return parts.remembers + parts.reason;
}

/** What Automatic chose (D2b). */
export function automaticResult(tokens: number): string {
  return `Automatic: ${tokensWithWords(tokens)}, on this computer right now`;
}

/** Not even the shortest memory fits (D2b). */
export const TOO_LARGE_LINE =
  "Too large for this computer, even with the shortest memory. Try Save memory, As much as fits, or a smaller download.";

/** The one-time note after the update (D2 upgrade rule). */
export function upgradeNoteWords(tokens: number): string {
  return `Every model now uses the size you chose before (${tokensWithWords(tokens)}). Choose Automatic to let the app pick the largest size that fits.`;
}

/** The settings' help lines (D4, word for word). */
export const HELP = {
  context:
    "How much of the chat the model can see at once. When a chat gets longer than this, older messages are summarised automatically so it can keep going.",
  saveMemory: "Lets the model remember more in the same memory. Answers may be slightly less accurate.",
  keepInMemory:
    "Stops the computer from moving the model out of memory, so replies don't pause after a break. Other programs get less memory. Not recommended with 16 GB or less.",
  thinking: "Longer thinking can give better answers but takes more time.",
} as const;

/** The label of the graphics setting; Macs say "Use Apple graphics" (D4). */
export function gpuLabel(apple: boolean): string {
  return apple ? "Use Apple graphics" : "Use the graphics card";
}

export function gpuHelp(apple: boolean): string {
  return apple
    ? "Apple graphics are much faster than the processor. As much as fits puts the rest on the processor, which is slower."
    : "The graphics card is much faster than the processor. As much as fits puts the rest on the processor, which is slower.";
}

/** Whether this computer is a Mac (the label "Use Apple graphics"). */
export function isAppleComputer(): boolean {
  return typeof navigator !== "undefined" && /Mac/i.test(navigator.platform);
}

const GIB = 1024 * 1024 * 1024;

/** A byte count that is not a finite number of zero or more reads as zero. */
function gib(bytes: number): number {
  return Number.isFinite(bytes) && bytes > 0 ? bytes / GIB : 0;
}

/**
 * A capacity (a graphics card's or the computer's memory) the way Task Manager counts it
 * (binary gigabytes, item 6 I6-10): whole from 10 GB up, else with one decimal ("16 GB",
 * "7.5 GB", "2 GB").
 */
export function capacityGb(bytes: number): string {
  const gb = gib(bytes);
  const shown = gb >= 10 ? Math.round(gb) : Math.round(gb * 10) / 10;
  return `${shown} GB`;
}

/** An amount in use or needed, with one decimal (".0" dropped) and no unit: "12.6", "12". */
export function amountNumber(bytes: number): string {
  return String(Math.round(gib(bytes) * 10) / 10);
}

/** An amount in use or needed (binary gigabytes, I6-10): "11.7 GB", "12 GB". */
export function amountGb(bytes: number): string {
  return `${amountNumber(bytes)} GB`;
}

/** Where a fit estimate's free memory is, after "Needs about 4.1 GB; ". */
const FIT_HAS: Record<FitDevice, (free: string) => string> = {
  graphicsCard: (free) => `this computer has ${free} free on the graphics card.`,
  appleGraphics: (free) => `Apple graphics can use up to ${free} on this Mac.`,
  processor: (free) => `this computer has ${free} free. It would run on the processor.`,
};

/**
 * The line after the verdict word of "Will it fit on this computer?" (Compare, the repository
 * page; compare-fit-words): what the model needs and what is free where it would run, counted
 * the way Task Manager counts memory (I6-10), then what a yellow or red verdict means: a
 * shorter memory that leaves room (tokens with words), part of it on the processor, or only
 * just. It never repeats the verdict word and never shows the backend's 0.0.4 sentence.
 */
export function fitLine(fit: FitResult): string {
  const parts = [`Needs about ${amountGb(fit.neededBytes)}; ${(FIT_HAS[fit.device] ?? FIT_HAS.processor)(amountGb(fit.availableBytes))}`];
  if (fit.verdict !== "green") {
    if (fit.suggestedCtx !== null && fit.suggestedCtx > 0) {
      parts.push(`It has room if it remembers up to ${tokensWithWords(fit.suggestedCtx)}.`);
    } else if (fit.partialOffload) {
      parts.push("Part of it would run on the processor, which is slower.");
    } else if (fit.verdict === "yellow") {
      parts.push("It only just fits.");
    } else if (fit.device === "processor" && fit.neededBytes <= fit.availableBytes) {
      parts.push("Part of that memory is kept free for other programs.");
    }
  }
  return parts.join(" ");
}

/**
 * A graphics card by its own name and memory, e.g. "AMD Radeon RX 9070 XT · 16 GB
 * (recommended)"; never the backend's device id ("Vulkan0").
 */
export function deviceLabel(device: DeviceOption): string {
  const number = /(\d+)$/.exec(device.name);
  const name = device.description.trim() || `Graphics card ${number ? Number(number[1]) + 1 : ""}`.trim();
  const memory = device.totalMib > 0 ? ` · ${capacityGb(device.totalMib * 1024 * 1024)}` : "";
  return `${name}${memory}${device.recommended ? " (recommended)" : ""}`;
}

/** "Use" on a model marked Too large (WP2). */
export const TOO_LARGE_QUESTION = "This model probably needs more memory than this computer has. Load it anyway?";

/** The D3 question, by what uses the model (word for word). */
export const BUSY_QUESTIONS = {
  reply: "A reply is being written. Stop it and restart the model?",
  otherApp: "Another app is using the model. Stop its request and restart the model?",
  check: "A model check is running. Stop it and restart the model?",
} as const;

/** The D3 question for these counts; null when nothing uses the model. */
export function busyQuestion(busy: ModelBusy): string | null {
  if (busy.chatTurns > 0) return BUSY_QUESTIONS.reply;
  if (busy.otherApps > 0) return BUSY_QUESTIONS.otherApp;
  if (busy.modelChecks > 0) return BUSY_QUESTIONS.check;
  return null;
}

/** A change of what is loaded that waits for the model to be free. */
export type WaitingChange = "reload" | "switch" | "unload";

/** The waiting line ("Restarts when the reply ends · Cancel", D3), by change and user. */
export function waitingWords(change: WaitingChange, busy: ModelBusy | null): string {
  const verb = change === "unload" ? "Unloads" : change === "switch" ? "Changes the model" : "Restarts";
  const what =
    busy && busy.chatTurns === 0 && busy.otherApps > 0
      ? "the other app's request"
      : busy && busy.chatTurns === 0 && busy.otherApps === 0 && busy.modelChecks > 0
        ? "the model check"
        : "the reply";
  return `${verb} when ${what} ends`;
}

// ---- 0.0.6 item 6: Fine control, the picture ("Where the model goes") and the usage line.
// Macs get their own sentences ("Apple graphics", never "graphics card"). Every amount is in
// binary gigabytes (`capacityGb`, `amountGb`, I6-10).

/** Where the model's graphics part runs, for the middle of a sentence. */
function graphicsPlace(apple: boolean): string {
  return apple ? "Apple graphics" : "the graphics card";
}

/** "1 layer", "30 layers". */
function layersCount(n: number): string {
  return `${count(n)} ${n === 1 ? "layer" : "layers"}`;
}

/** The six Fine control settings' labels (item 6, section 3). */
export function fineLabels(apple: boolean) {
  return {
    layers: apple ? "Layers on Apple graphics" : "Layers on the graphics card",
    experts: "Expert parts on the processor",
    kv: apple ? "Conversation memory with Apple graphics" : "Conversation memory on the graphics card",
    threads: "Processor threads",
    fastAttention: "Fast attention",
    leaveFree: apple ? "Memory to leave free" : "Leave free on the graphics card",
  } as const;
}

/** The Fine control help lines, word for word; `limitBytes` is the most Apple graphics may use. */
export function fineHelp(apple: boolean, limitBytes: number | null) {
  const upTo = limitBytes !== null && limitBytes > 0 ? ` (up to ${amountGb(limitBytes)} here)` : "";
  return {
    layers: apple
      ? `The model is built from layers. Layers on Apple graphics run much faster than on the processor, as long as they fit in Apple graphics' share of memory${upTo}. The rest run on the processor.`
      : "The model is built from layers. Layers on the graphics card run much faster than on the processor, as long as they fit in its memory. The rest run on the processor.",
    // Section 3 gives no Mac version of this line; the Mac one names Apple graphics' share
    // of memory, as help lines 1, 3 and 6 do (checklist item 19).
    experts: apple
      ? "This model is a mixture of experts: it has many expert parts but uses only a few of them for each word. Keeping expert parts on the processor frees a lot of Apple graphics' share of memory and is often only a little slower. Automatic keeps them on Apple graphics with All of it, and moves them first when As much as fits needs room."
      : "This model is a mixture of experts: it has many expert parts but uses only a few of them for each word. Keeping expert parts on the processor frees a lot of graphics memory and is often only a little slower. Automatic keeps them on the graphics card with All of it, and moves them first when As much as fits needs room.",
    kv: apple
      ? `On a Mac the processor and Apple graphics share one memory. Keeping the conversation with the processor leaves Apple graphics more of its share${upTo} for layers, but replies slow down as the chat grows.`
      : "Where the model keeps the chat it is working on. On the graphics card is faster. Keeping it in computer memory leaves more room for layers on the graphics card, but replies slow down as the chat grows.",
    threads:
      "How many of the processor's threads the model may use for the parts that run on the processor. Automatic is usually fastest. Fewer keeps the computer more responsive for other work.",
    fastAttention: `A faster way for the model to look back over the chat that also uses less memory. Automatic turns it on where ${apple ? "Apple graphics supports" : "the graphics card supports"} it.`,
    leaveFree: apple
      ? "With As much as fits, this much of Apple graphics' share of memory is left free for other programs. More free memory means fewer layers on Apple graphics."
      : "With As much as fits, this much graphics memory is left free for the screen and other programs, such as games or video. More free memory means fewer layers on the graphics card.",
  } as const;
}

/** The Fine control choices' words (section 3). */
export function fineChoiceWords(apple: boolean) {
  return {
    layersAuto: apple ? "Automatic (follows Use Apple graphics)" : "Automatic (follows Use the graphics card)",
    choose: "Choose how many",
    automatic: "Automatic",
    expertsAll: "All of them",
    expertsSome: "Some",
    kvAuto: "Yes, faster (Automatic)",
    kvComputer: apple ? "No, keep it with the processor" : "No, keep it in computer memory",
    on: "On",
    off: "Off",
    onForSaveMemory: "On (Save memory needs it)",
    leaveFreeAuto: "Automatic (1 GB)",
    reset: "Reset fine control",
  } as const;
}

/** The number beside the layers slider: "30 of 41 layers". */
export function layersOfWords(n: number, units: number): string {
  return `${count(n)} of ${layersCount(units)}`;
}

/** The number beside the expert parts slider: "the first 20 of 48 layers". */
export function expertLayersWords(k: number, of: number): string {
  return `the first ${count(k)} of ${layersCount(of)}`;
}

/** The number beside the threads slider: "8 of 16 threads". */
export function threadsOfWords(t: number, of: number): string {
  return `${count(t)} of ${count(of)} ${of === 1 ? "thread" : "threads"}`;
}

/** Leave free choices in MiB, as shown ("0.5 GB"). */
export function leaveFreeWords(mib: number): string {
  return `${Math.round((mib / 1024) * 10) / 10} GB`;
}

/** The notes under Fine control's settings (visible text; the setting is greyed out). */
export function fineNotes(apple: boolean) {
  return {
    gpuOff: `Not used while ${gpuLabel(apple)} is Don't use it.`,
    leaveFreeAll: "Only used with As much as fits.",
    leaveFreeCustom: "Not used while you choose the layers or expert parts yourself.",
    expertsNoLayers: `Not used while no layers are on ${graphicsPlace(apple)}.`,
    noGraphics: apple ? "Not used without Apple graphics." : "Not used without a graphics card.",
  } as const;
}

/** Under "Expert parts on the processor" when only `n` of `units` layers are on the card. */
export function expertsAfterLayersNote(n: number, units: number): string {
  const first = Math.max(0, units - n);
  const lead = first === 1 ? "The first layer already runs" : `The first ${count(first)} layers already run`;
  return `${lead} on the processor. Only expert parts of later layers free graphics memory.`;
}

/** The closed section's summary: "Fine control · 30 of 41 layers on the graphics card · changed". */
export function fineSummary(picture: OffloadPreview | null, apple: boolean, changed: boolean): string {
  const parts = ["Fine control"];
  if (picture) {
    if (processorOnly(picture)) parts.push("On the processor only");
    else if (picture.units !== null) {
      const n = Math.min(picture.layersOnGraphics, picture.units);
      parts.push(
        n >= picture.units
          ? `All ${layersCount(picture.units)} on ${graphicsPlace(apple)}`
          : `${picture.layersAbout ? "About " : ""}${layersOfWords(n, picture.units)} on ${graphicsPlace(apple)}`,
      );
    }
  }
  if (changed) parts.push("changed");
  return parts.join(" · ");
}

/** The plain graphics setting's extra option with a custom placement (6.3). */
export function customPlacementWords(layers: number | null, units: number | null): string {
  if (layers === null) return "Custom: expert parts on the processor (set in Fine control)";
  return units !== null
    ? `Custom: ${layersOfWords(layers, units)} (set in Fine control)`
    : `Custom: ${layersCount(layers)} (set in Fine control)`;
}

export const CUSTOM_PLACEMENT_HELP = "Fine control sets where the model goes. Choose another option here to go back to it.";

/** Whether the picture puts nothing on the graphics card. */
function processorOnly(p: OffloadPreview): boolean {
  return p.layersOnGraphics <= 0 || (!p.unified && p.graphics === null);
}

/** What the picture says about the layers (6.2), or null when the file's layers are unknown. */
export function pictureLayerWords(p: OffloadPreview, apple: boolean): string | null {
  const where = graphicsPlace(apple);
  if (processorOnly(p)) return "On the processor only.";
  if (p.units === null) return null;
  const n = Math.min(p.layersOnGraphics, p.units);
  const experts = p.expertLayersOnProcessor ?? 0;
  const all = n >= p.units;
  let text = all
    ? `All ${layersCount(p.units)} on ${where}.`
    : p.layersAbout
      ? `About ${layersOfWords(n, p.units)} on ${where}. The rest run on the processor.`
      : `${layersOfWords(n, p.units)} on ${where}, ${count(p.units - n)} on the processor.`;
  if (experts > 0) text += ` Expert parts of ${p.layersAbout ? "about " : ""}${layersCount(experts)} stay on the processor.`;
  if (p.layersAbout && (experts > 0 || !all)) {
    text += experts > 0 ? " The exact split is decided when the model loads." : " The exact number is decided when the model loads.";
  }
  return text;
}

/**
 * The picture's source line (6.2). While exact figures are being worked out the estimate
 * stays shown with "Working out exact figures…".
 */
export function pictureSourceWords(source: PictureSource, opts: { measuring: boolean; fineOpen: boolean }): string {
  if (source.kind === "measured") return "Worked out for these settings.";
  if (opts.measuring) return "Working out exact figures…";
  // With Fine control open, a quick estimate is the measurement's own answer: the exact
  // figures could not be worked out (the model file changed since, for example).
  const why = source.why === "quick" && opts.fineOpen ? "failed" : source.why;
  switch (why) {
    case "quick":
      return "Rough estimate from the model file. Open Fine control to work out exact figures.";
    case "failed":
      return "Rough estimate from the model file. Exact figures couldn't be worked out for this file.";
    case "timedOut":
      return "Rough estimate from the model file. Working them out took too long.";
    case "noTool":
      return source.backend === "rocm"
        ? "Rough estimate from the model file. The AMD graphics card (ROCm) download can't work out exact figures."
        : "Rough estimate from the model file. Exact figures can't be worked out with this download.";
    case "notReadable":
      return "Rough estimate. The model file's details couldn't be read.";
  }
}

/** The caption under the picture (I6-10, open question 3 A). */
export function pictureCaption(mac: boolean): string {
  return `Memory is counted the way ${mac ? "Activity Monitor" : "Task Manager"} counts it, so it can differ from the download size.`;
}

/** The sum of a bar's parts. */
export function partsTotal(p: Parts): number {
  return [p.model, p.context, p.compute, p.imageReader].reduce((a, b) => a + (Number.isFinite(b) && b > 0 ? b : 0), 0);
}

/** What is left of a bar's free memory once the model is loaded (never below zero). */
export function freeAfterLoading(bar: MemoryBar): number {
  return Math.max(0, bar.freeBytes - partsTotal(bar.need));
}

/** A bar's title: "Graphics card · AMD Radeon RX 9070 XT · 16 GB", "Computer memory · 32 GB". */
export function barTitle(bar: MemoryBar, kind: "graphics" | "computer" | "shared"): string {
  if (kind === "shared") return `Memory · shared by the processor and Apple graphics · ${capacityGb(bar.totalBytes)}`;
  if (kind === "computer") return `Computer memory · ${capacityGb(bar.totalBytes)}`;
  const name = bar.name.trim();
  return name ? `Graphics card · ${name} · ${capacityGb(bar.totalBytes)}` : `Graphics card · ${capacityGb(bar.totalBytes)}`;
}

/** A bar's legend, part by part, in words and numbers (never colour alone). */
export function barLegend(bar: MemoryBar): { part: "model" | "context" | "compute" | "imageReader" | "other" | "free"; words: string; bytes: number }[] {
  const items: { part: "model" | "context" | "compute" | "imageReader" | "other" | "free"; words: string; bytes: number }[] = [
    { part: "model", words: "Model", bytes: bar.need.model },
    { part: "context", words: "Conversation memory", bytes: bar.need.context },
    { part: "compute", words: "Working space", bytes: bar.need.compute },
  ];
  if (bar.need.imageReader > 0) items.push({ part: "imageReader", words: "Image reader", bytes: bar.need.imageReader });
  items.push({ part: "other", words: "Other programs", bytes: bar.otherBytes });
  const free = freeAfterLoading(bar);
  // When the model needs more than is free, nothing is left; the needs line says by how much.
  if (free > 0 || partsTotal(bar.need) <= bar.freeBytes) items.push({ part: "free", words: "Free after loading", bytes: free });
  return items;
}

/** One legend item: "Model 9.9 GB". */
export function legendWords(item: { words: string; bytes: number }): string {
  return `${item.words} ${amountGb(item.bytes)}`;
}

/** "Needs about 11.7 GB of the 15 GB free now." (or "… free once the loaded model is unloaded."). */
export function needsWords(bar: MemoryBar): string {
  const when = bar.freeAfterUnload ? "free once the loaded model is unloaded" : "free now";
  return `Needs about ${amountGb(partsTotal(bar.need))} of the ${amountGb(bar.freeBytes)} ${when}.`;
}

/** Apple's marker on the single bar. */
export function appleLimitWords(limitBytes: number): string {
  return `Apple graphics can use up to ${amountGb(limitBytes)}`;
}

/**
 * The one warning line of the panel (4.1, 6.2), from the latest answer for the choices on
 * screen; null when it fits. `too_large` (WP2's line) comes before this and is not here.
 */
export function pictureWarning(p: OffloadPreview, opts: { apple: boolean; saveMemory: boolean }): string | null {
  const memory = opts.apple ? "Apple graphics' share of memory" : "the graphics card's memory";
  const save = opts.saveMemory ? "" : ", or turn on Save memory";
  switch (p.fits) {
    case "fits":
    case "partial":
      return null;
    case "tight":
      return `It only just fits. Programs that use ${graphicsPlace(opts.apple)}, such as games or video, may stop it from loading.`;
    case "wontFitGraphics": {
      const bar = p.graphics ?? p.computer;
      const free =
        p.graphics === null && p.graphicsLimitBytes !== null ? Math.min(bar.freeBytes, p.graphicsLimitBytes) : bar.freeBytes;
      const needs = `it needs about ${amountGb(partsTotal(bar.need))} and ${amountGb(free)} is free`;
      const advice = p.custom
        ? `Choose fewer layers or keep expert parts on the processor in Fine control${save}.`
        : `Choose As much as fits${save}.`;
      return `This probably won't fit in ${memory}: ${needs}. ${advice}`;
    }
    case "wontFitAtAll": {
      const bars = p.graphics ? [p.graphics, p.computer] : [p.computer];
      const need = bars.reduce((a, b) => a + partsTotal(b.need), 0);
      const free = bars.reduce((a, b) => a + Math.max(0, b.freeBytes), 0);
      return `This probably needs more memory than this computer has free: about ${amountGb(need)}, with ${amountGb(free)} free. Choose less to remember, or a smaller download.`;
    }
  }
}

/** "Load anyway" for an answer that probably won't fit on the graphics card (6.2). */
export function wontFitQuestion(apple: boolean): string {
  return `This probably won't fit in ${apple ? "Apple graphics' share of memory" : "the graphics card's memory"}. If it doesn't, the model won't load, or part of it moves to the processor, which is slower.`;
}

/** "Where the model went" after the load (6.1), from the ready line's memory report. */
export function placementWords(p: Placement, apple: boolean): string {
  const where = graphicsPlace(apple);
  if (p.graphics === null || p.layersOnGraphics === 0) return "On the processor only.";
  if (p.layersOnGraphics === null) {
    // Only amounts known. Part of it is on the processor after As much as fits, or with
    // expert parts chosen in Fine control; All of it puts every layer on the card.
    const rest = p.layersAbout || p.custom ? ", the rest on the processor" : "";
    return `About ${amountGb(p.graphics.use.model)} of the model is on ${where}${rest}.`;
  }
  const n = p.layersOnGraphics;
  let text: string;
  if (p.units === null) text = `${layersCount(n)} on ${where}, the rest on the processor.`;
  else if (n >= p.units) text = `All ${layersCount(p.units)} on ${where}.`;
  else if (p.layersAbout) text = `About ${layersOfWords(n, p.units)} on ${where}, the rest on the processor.`;
  else text = `${layersOfWords(n, p.units)} on ${where}, ${count(p.units - n)} on the processor.`;
  const experts = p.expertLayersOnProcessor ?? 0;
  if (experts > 0) text += ` Expert parts of ${p.layersAbout ? "about " : ""}${layersCount(experts)} are on the processor.`;
  return text;
}

/** "The model uses 11.7 GB: 9.9 GB for itself, 1.2 GB conversation memory, 0.6 GB working space". */
export function modelUsesWords(use: Parts): string {
  const parts = [
    `${amountGb(use.model)} for itself`,
    `${amountGb(use.context)} conversation memory`,
    `${amountGb(use.compute)} working space`,
  ];
  if (use.imageReader > 0) parts.push(`${amountGb(use.imageReader)} image reader`);
  return `The model uses ${amountGb(partsTotal(use))}: ${parts.join(", ")}`;
}

/** Why a usage figure is missing, as the visible text on its row (6.1). */
export function usageGapWords(gap: UsageGap, what: "graphicsBusy" | "graphicsMemory" | "processor" | "memory", apple: boolean): string {
  switch (gap) {
    case "systemGivesNone":
      if (what === "graphicsBusy") {
        return apple ? "This Mac doesn't tell apps how busy Apple graphics is." : "This computer doesn't tell apps how busy its graphics card is.";
      }
      if (what === "graphicsMemory") {
        return apple
          ? "This Mac doesn't tell apps how much memory Apple graphics uses."
          : "This computer doesn't tell apps how much of its graphics memory is in use.";
      }
      if (what === "processor") return `This ${apple ? "Mac" : "computer"} doesn't tell apps how busy its processor is.`;
      return `This ${apple ? "Mac" : "computer"} doesn't tell apps how much of its memory is in use.`;
    case "notReadHere":
      return what === "graphicsBusy" || what === "graphicsMemory"
        ? `${APP_NAME} can't read this graphics card on this system yet.`
        : `${APP_NAME} can't read this on this system yet.`;
    case "noGraphicsCard":
      return "No graphics card was found.";
    case "readFailed":
      return "Couldn't read this just now.";
  }
}

/** A busy percentage as shown: "63%", "…" on the first sample, "–" when missing. */
export function percentWords(r: Reading<number>): string {
  if (r.kind === "measuring") return "…";
  if (r.kind === "notAvailable" || !Number.isFinite(r.value)) return "–";
  return `${Math.min(100, Math.max(0, Math.round(r.value)))}%`;
}

/** An amount in use as shown, without its unit: "12.6", "…", "–". */
function amountReading(r: Reading<number>): string {
  if (r.kind === "measuring") return "…";
  if (r.kind === "notAvailable") return "–";
  return amountNumber(r.value);
}

/** The usage line's forms, widest first (6.1). */
export type UsageForm = "wide" | "medium" | "small";

/** One reading of "x of y GB" in the wide ("12.6 of 16 GB") or medium ("12.6/16 GB") form. */
function ofWords(used: string, totalBytes: number | null, form: "wide" | "medium"): string {
  if (totalBytes === null || !(totalBytes > 0)) return used === "–" || used === "…" ? used : `${used} GB`;
  return form === "wide" ? `${used} of ${capacityGb(totalBytes)}` : `${used}/${capacityGb(totalBytes)}`;
}

/**
 * The usage line in one of its forms: "Graphics card 63% · 12.6 of 16 GB · Processor 12% ·
 * Memory 16.8 of 32 GB" (wide), "… 12.6/16 GB …" (medium), "Graphics card 63% · Processor
 * 12% · Memory 53%" (small). A Mac says "Apple graphics" and leaves graphics memory to the
 * menu (it is part of Memory). No graphics card: the graphics part is left out.
 */
export function usageLine(now: UsageNow, form: UsageForm): string {
  const parts: string[] = [];
  const g = now.graphics;
  const name = now.unified ? "Apple graphics" : "Graphics card";
  if (g.kind === "value") {
    let text = `${name} ${percentWords(g.value.busyPercent)}`;
    if (!now.unified && form !== "small") {
      text += ` · ${ofWords(amountReading(g.value.memoryUsedBytes), g.value.memoryTotalBytes, form)}`;
    }
    parts.push(text);
  } else if (g.kind === "measuring") parts.push(`${name} …`);
  else if (g.why !== "noGraphicsCard") parts.push(`${name} –`);
  parts.push(`Processor ${percentWords(now.processorPercent)}`);
  parts.push(`Memory ${memoryShort(now.memory, form)}`);
  return parts.join(" · ");
}

function memoryShort(m: UsageNow["memory"], form: UsageForm): string {
  if (m.kind === "measuring") return "…";
  if (m.kind === "notAvailable") return "–";
  if (form === "small") return memoryPercent(m.value);
  return ofWords(amountNumber(m.value.usedBytes), m.value.totalBytes, form);
}

function memoryPercent(v: { usedBytes: number; totalBytes: number }): string {
  if (!(v.totalBytes > 0) || !Number.isFinite(v.usedBytes)) return "–";
  return `${Math.min(100, Math.max(0, Math.round((v.usedBytes / v.totalBytes) * 100)))}%`;
}

/** The narrowest form: a number per icon, with the words for its label. */
export function usageIcons(now: UsageNow): { kind: "graphics" | "processor" | "memory"; value: string; label: string }[] {
  const out: { kind: "graphics" | "processor" | "memory"; value: string; label: string }[] = [];
  const name = now.unified ? "Apple graphics" : "Graphics card";
  const said = (what: string, value: string, verb: string) =>
    value === "…" ? `${what}: measuring` : value === "–" ? `${what}: not available` : `${what} ${value} ${verb}`;
  const g = now.graphics;
  if (g.kind !== "notAvailable" || g.why !== "noGraphicsCard") {
    const value = g.kind === "value" ? percentWords(g.value.busyPercent) : g.kind === "measuring" ? "…" : "–";
    out.push({ kind: "graphics", value, label: said(name, value, "busy") });
  }
  const cpu = percentWords(now.processorPercent);
  out.push({ kind: "processor", value: cpu, label: said("Processor", cpu, "busy") });
  const m = now.memory;
  const mem = m.kind === "value" ? memoryPercent(m.value) : m.kind === "measuring" ? "…" : "–";
  out.push({ kind: "memory", value: mem, label: said("Memory", mem, "in use") });
  return out;
}

/** The usage menu's fixed lines (6.1). */
export const USAGE_MENU = {
  title: "This computer right now",
  privacy: "Read on this computer only while you can see it. Nothing is sent anywhere.",
} as const;

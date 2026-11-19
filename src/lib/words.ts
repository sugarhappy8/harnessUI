// Plain words for the app's technical values (0.0.6 D4, D8). Nothing on screen should show
// a raw identifier such as "vulkan", "Vulkan0" or "text_model". Every phrase of the model
// loading screens lives here, so the wording stays the same wherever it appears; the test
// walks every value of the contract's enums.
import type { Company, GpuPolicy, Licence, LocalFit, MediaKind, RefRole } from "../components/studio/types";
import { APP_NAME } from "./app";
import { formatEta } from "./format";
import type {
  BackendKind,
  ContextSource,
  DeviceOption,
  DownloadGroup,
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
  // [0.0.7 P2] Never a reason to step down; listed so every failure has words.
  image_file_invalid: "could not read the image file",
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
    case "retryingWithoutImageFile":
      return "Trying again without the image file";
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
    case "retryingWithoutImageFile":
      return stage.memory ? IMAGE_FILE_WORDS.loadedWithoutMemory : IMAGE_FILE_WORDS.loadedWithout;
  }
}

/** The slow states that need no number (D4 "Slow states as visible text"). */
export const SLOW_WORDS = {
  partial:
    "Part of the model runs on the processor, so replies are slower. Save memory or a smaller memory size lets more fit on the graphics card.",
  afterRestart: "The first reply after a restart takes longer while the model rereads the chat.",
} as const;

/** [0.0.7 P2] The image file a model reads pictures with (llama.cpp's projector), in plain words. */
export const IMAGE_FILE_WORDS = {
  /** The header's note after a load without it, word for word as the owner was promised. */
  loadedWithout: "Loaded without image input: the image file next to this model couldn't be read.",
  /** The same, when it was left out because there was not enough memory for it too. */
  loadedWithoutMemory: "Loaded without image input: there wasn't enough memory for the image file too.",
  /** Models → Installed, on the model's row. */
  unreadable: "The image file couldn't be read, so this model can't see images.",
  tryAgain: "Try it again",
  triedAgain: "It will be tried again the next time the model loads.",
  download: "Download the image file",
  downloadAnother: "Download another image file",
  downloadAgain: "Download the image file again",
  look: "Look for an image file",
  looking: "Looking for an image file…",
  noOther: "Its repository has no other image file.",
  none: "Its repository has no image file.",
  /** After the download: for the model loaded now, and for one that is not. */
  done: "Image file downloaded. Reload the model to use it.",
  doneNextLoad: "Image file downloaded. It will be used the next time the model loads.",
} as const;

/** [0.0.7 P2] An image file downloading on its model's Installed row ("Downloading the image file… 45%"). */
export function imageFileProgress(done: number, total: number): string {
  if (!(total > 0) || !Number.isFinite(done)) return "Downloading the image file…";
  const pct = Math.min(100, Math.max(0, Math.floor((done / total) * 100)));
  return `Downloading the image file… ${pct}%`;
}

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

const MIB = 1024 * 1024;

/** A download's size in the same binary units (0.1.0 Studio): whole megabytes, rounded up as
 *  the backend counts the Studio engine's pack, under 1 GB ("29 MB"), else `amountGb`. */
export function downloadAmount(bytes: number): string {
  return Number.isFinite(bytes) && bytes > 0 && bytes < GIB ? `${Math.ceil(bytes / MIB)} MB` : amountGb(bytes);
}

/** A licence line's short name (0.1.0 Studio): the last word of its first part, so
 *  "stable-diffusion.cpp MIT; ggml MIT; libwebp and libwebm BSD-3-Clause" reads "MIT". The
 *  whole line goes in the tooltip. */
export function licenceWord(licenseName: string): string {
  const first = licenseName.split("; ", 1)[0] ?? "";
  return first.trim().split(/\s+/).pop() ?? "";
}

/** Where a fit estimate's free memory is, after "Needs about 4.1 GB; ". */
const FIT_HAS: Record<FitDevice, (free: string) => string> = {
  graphicsCard: (free) => `this computer has ${free} free on the graphics card.`,
  appleGraphics: (free) => `Apple graphics can use up to ${free} on this Mac.`,
  processor: (free) => `this computer has ${free} free.`,
};

/** What is short for a model that is too large, by where it would run (compare-fit-minors). */
const FIT_SHORT: Record<FitDevice, string> = {
  graphicsCard: "There is not enough memory free for it, even with part of it in the computer's memory.",
  appleGraphics: "There is not enough memory free for it on this Mac.",
  processor: "There is not enough memory free to run it on the processor.",
};

/**
 * The line after the verdict word of "Will it fit on this computer?" (Compare, the repository
 * page; compare-fit-words): what the model needs and what is free where it would run, counted
 * the way Task Manager counts memory (I6-10); on a graphics card with part of the model in the
 * computer's memory, the whole need and how it is split (compare-fit-minors). Then what the
 * verdict means: "It would run on the processor" only when it would; for a model too large,
 * what is short (on a graphics card, the computer's memory with how much is free when its
 * part does not fit there, else the whole); a shorter memory that leaves room (tokens with
 * words), part of it on the processor, or only just. It never repeats the verdict word and
 * never shows the backend's 0.0.4 sentence.
 */
export function fitLine(fit: FitResult): string {
  const device = FIT_HAS[fit.device] ? fit.device : "processor";
  const computer = device === "graphicsCard" && fit.computerBytes > 0 && amountNumber(fit.computerBytes) !== "0";
  const parts = [
    computer
      ? `Needs about ${amountGb(fit.neededBytes + fit.computerBytes)} in all: ${amountGb(fit.neededBytes)} on the graphics card, ` +
        `which has ${amountGb(fit.availableBytes)} free, and ${amountGb(fit.computerBytes)} in the computer's memory.`
      : `Needs about ${amountGb(fit.neededBytes)}; ${FIT_HAS[device](amountGb(fit.availableBytes))}`,
  ];
  if (fit.verdict === "red" && device === "graphicsCard" && fit.computerShort) {
    // The model's part in the computer's memory does not fit there, as Settings for this
    // model's second bar shows (the saved copies of a long chat never make it too large).
    parts.push(`The computer's memory has ${amountGb(fit.computerFreeBytes)} free, not enough for its part.`);
    if (fit.computerBytes <= fit.computerFreeBytes) {
      parts.push("Part of that memory is kept free for other programs.");
    }
  } else if (fit.verdict === "red") {
    parts.push(FIT_SHORT[device]);
    if (device === "processor" && fit.neededBytes <= fit.availableBytes) {
      parts.push("Part of that memory is kept free for other programs.");
    }
  } else if (device === "processor") {
    parts.push("It would run on the processor.");
  }
  const copies = copiesShownBytes(fit.copiesBytes);
  if (fit.verdict !== "green") {
    if (fit.suggestedCtx !== null && fit.suggestedCtx > 0) {
      parts.push(`It has room if it remembers up to ${tokensWithWords(fit.suggestedCtx)}.`);
    } else if (fit.verdict === "yellow" && fit.partialOffload) {
      parts.push("Part of it would run on the processor, which is slower.");
    } else if (fit.verdict === "yellow" && !(fit.copiesShort && copies > 0)) {
      parts.push("It only just fits.");
    }
  }
  if (fit.computerUnknown) {
    parts.push("The computer's memory couldn't be read.");
    if (copies > 0) parts.push(`Long chats will use up to ${amountGb(copies)} of it.`);
  } else if (fit.copiesShort && copies > 0 && fit.verdict !== "red") {
    parts.push(longChatsWords(copies));
  }
  return parts.join(" ");
}

/** A saved copies figure as shown: a finite amount above zero, else 0. */
function copiesShownBytes(bytes: number | undefined): number {
  return bytes !== undefined && Number.isFinite(bytes) && bytes > 0 ? bytes : 0;
}

/**
 * Why a model is only yellow when not even one saved copy of a long chat fits the computer's
 * memory that is left (0.0.7): never "too large" for that.
 */
export function longChatsWords(copiesBytes: number): string {
  return `Long chats will use about ${amountGb(copiesBytes)} more of the computer's memory, which may not have that much free.`;
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

/**
 * A bar's title: "Graphics card · AMD Radeon RX 9070 XT · 16 GB", "Computer memory · 32 GB";
 * without a size when the computer's memory could not be read (`unknown`).
 */
export function barTitle(bar: MemoryBar, kind: "graphics" | "computer" | "shared", unknown = false): string {
  if (kind === "shared") {
    return unknown ? "Memory · shared by the processor and Apple graphics" : `Memory · shared by the processor and Apple graphics · ${capacityGb(bar.totalBytes)}`;
  }
  if (kind === "computer") return unknown ? "Computer memory" : `Computer memory · ${capacityGb(bar.totalBytes)}`;
  const name = bar.name.trim();
  return name ? `Graphics card · ${name} · ${capacityGb(bar.totalBytes)}` : `Graphics card · ${capacityGb(bar.totalBytes)}`;
}

/** One part of a bar's legend. */
export type LegendPart = "model" | "context" | "compute" | "imageReader" | "other" | "copies" | "free";

/**
 * A bar's legend, part by part, in words and numbers (never colour alone). The saved copies
 * of a long chat (computer memory, 0.0.7) are their own item, after other programs: they
 * build up during a chat, so "Free after loading" does not count them.
 */
export function barLegend(bar: MemoryBar, unknown = false): { part: LegendPart; words: string; bytes: number }[] {
  const items: { part: LegendPart; words: string; bytes: number }[] = [
    { part: "model", words: "Model", bytes: bar.need.model },
    { part: "context", words: "Conversation memory", bytes: bar.need.context },
    { part: "compute", words: "Working space", bytes: bar.need.compute },
  ];
  if (bar.need.imageReader > 0) items.push({ part: "imageReader", words: "Image reader", bytes: bar.need.imageReader });
  // The computer's memory could not be read: what other programs use and what is left are
  // not known either.
  if (!unknown) items.push({ part: "other", words: "Other programs", bytes: bar.otherBytes });
  const copies = copiesShownBytes(bar.copiesBytes);
  if (copies > 0) items.push({ part: "copies", words: "Saved copies of long chats", bytes: copies });
  if (unknown) return items;
  const free = freeAfterLoading(bar);
  // When the model needs more than is free, nothing is left; the needs line says by how much.
  if (free > 0 || partsTotal(bar.need) <= bar.freeBytes) items.push({ part: "free", words: "Free after loading", bytes: free });
  return items;
}

/** One legend item: "Model 9.9 GB"; the saved copies "Saved copies of long chats (up to 2.5 GB)". */
export function legendWords(item: { part?: LegendPart; words: string; bytes: number }): string {
  if (item.part === "copies") return `${item.words} (up to ${amountGb(item.bytes)})`;
  return `${item.words} ${amountGb(item.bytes)}`;
}

/**
 * "Needs about 11.7 GB of the 15 GB free now." (or "… free once the loaded model is unloaded.");
 * with the computer's memory unknown, the need and that it couldn't be read.
 */
export function needsWords(bar: MemoryBar, unknown = false): string {
  if (unknown) return `Needs about ${amountGb(partsTotal(bar.need))}. The computer's memory couldn't be read.`;
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
      // Yellow only because not even one saved copy of a long chat fits what is left.
      return p.copiesShort && copiesShownBytes(p.computer.copiesBytes) > 0
        ? longChatsWords(copiesShownBytes(p.computer.copiesBytes))
        : null;
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

// ---- Studio (0.1.0) -----------------------------------------------------------------------

/** Studio's place in the app: the sidebar button and the shortcut sheet. */
export const STUDIO_NAV = {
  name: "Studio",
  title: "Studio (Ctrl+G)",
} as const;

/** The sidebar button's name, with how many runs are working ("Studio (2 working)"). */
export function studioButtonLabel(working: number): string {
  return working > 0 ? `${STUDIO_NAV.name} (${working} working)` : STUDIO_NAV.name;
}

/** What an added picture is, as the note after a model switch names it: one, then several. */
const PICTURE_NOUNS: Record<RefRole, readonly [string, string]> = {
  reference: ["added picture", "added pictures"],
  startFrame: ["start frame", "start frames"],
  endFrame: ["end frame", "end frames"],
};

/** How a setting and its value read inside Studio's sentences and chips (Tasks B4 and B6). */
export const SETTING_WORDS = {
  /** A name or a word value in the middle of a sentence: its first letter lower-cased ("Extra
   *  high" → "extra high"), unless its first word has a capital past its first letter, as a
   *  name in capitals does ("PNG", "WebP" and "CFG scale" stay as they are). */
  midSentence(text: string): string {
    const first = text.split(" ", 1)[0] ?? "";
    if (/[A-Z]/.test(first.slice(1))) return text;
    return text.charAt(0).toLowerCase() + text.slice(1);
  },
  /** A number with its field's unit: "80%", "120 ms". */
  withUnit: (value: string, unit: string) => (unit === "%" ? `${value}%` : `${value} ${unit}`),
  /** A setting known only by its key: "foo_bar" → "foo bar". */
  keyName: (key: string) => key.replaceAll("_", " "),
} as const;

/**
 * The note after switching models (Task B4): one sentence for each setting that did not carry
 * over as it was. `name` is a field's name in the middle of a sentence ("seed", "how many"),
 * `label` its name as the field shows it ("Shape"), and values are as their fields show them.
 */
export const CARRY_WORDS = {
  became: (label: string, was: string, now: string) => `${label} ${was} became ${now}.`,
  noSetting: (model: string, name: string) => `${model} has no ${name} setting, so it was removed.`,
  notOffered: (model: string, name: string, value: string) => `${model} does not offer ${name} ${value}, so it was removed.`,
  notAccepted: (model: string, key: string) => `${model} does not accept ${key}, so it was removed.`,
  /** Added pictures past the new model's limit for their role (`fit` may be 0). */
  leftOut(model: string, role: RefRole, fit: number, dropped: number): string {
    const [one, many] = PICTURE_NOUNS[role];
    const was = dropped === 1 ? "1 was" : `${dropped} were`;
    if (fit === 0) return dropped === 1 ? `${model} takes no ${one}, so it was left out.` : `${model} takes no ${many}, so ${was} left out.`;
    return fit === 1 ? `Only 1 ${one} fits, so ${was} left out.` : `Only ${fit} ${many} fit, so ${was} left out.`;
  },
} as const;

/** Studio's tabs. */
export const STUDIO_TABS: Record<MediaKind, string> = { image: "Pictures", video: "Video" };

/** Studio's top bar, its menu and the prompt box (Task B2). */
export const STUDIO_BAR = {
  tabs: "Pictures or video",
  search: "Search",
  searchPlaceholder: "Search…",
  starred: "Starred",
  grid: "Grid",
  menu: "Studio menu",
  updateLists: "Update model lists",
  updatingLists: "Updating model lists…",
  /** Cloud spending today, after the amount ("$0.84 today"). */
  spentToday: (amount: string) => `${amount} today`,
  spentTitle: "Spent on cloud models today. Runs on this computer are free.",
  prompt: "Prompt",
  promptPlaceholder: {
    image: "Describe the picture you want…",
    video: "Describe the video you want…",
  } satisfies Record<MediaKind, string>,
} as const;

/** The mark for a model that runs on this computer. */
export const LOCAL_MARK = {
  name: "On this computer",
  title: "Your prompt and pictures stay on this computer.",
} as const;

/** Where a cloud run's prompt goes ("Cloud: your prompt and added pictures go to openrouter.ai"). */
export function cloudGoesTo(host: string): string {
  return `Cloud: your prompt and added pictures go to ${host}`;
}

/** The chip for a cloud model, naming its endpoint's host when known: "Cloud · openrouter.ai". */
export function cloudChipWords(host: string): string {
  return host ? `Cloud · ${host}` : "Cloud";
}

/** What a screen reader hears after a place chip's words (`PlaceMark`): its tooltip, so the
 *  chip reads "Cloud: Your prompt and added pictures go to openrouter.ai.". */
export function placeMarkSpoken(title: string): string {
  return `: ${title}`;
}

/** Buttons that more than one Studio screen shows. */
export const STUDIO_ACTIONS = {
  getLocalModels: "Get local models",
  openEndpoints: "Open Endpoints",
  getTheList: "Get the list",
  notNow: "Not now",
  /** A local model's parts checked, and what is missing or damaged fetched again. */
  downloadAgain: "Download again",
} as const;

/** The Studio engine's download size and the short name of its licence: "29 MB, MIT". */
export function engineSizeWords(bytes: number, licenseName: string): string {
  return `${downloadAmount(bytes)}, ${licenceWord(licenseName)}`;
}

/** The Studio engine's download, where it is missing (Tasks B8, B10 and B13): its size and the
 *  short name of its licence, "Download the Studio engine (29 MB, MIT)". */
export function engineDownloadWords(bytes: number, licenseName: string): string {
  return `Download the Studio engine (${engineSizeWords(bytes, licenseName)})`;
}

/** The Studio engine's line (Settings → Studio and Get local models' footer, Tasks B10 and
 *  B13): built in on a Mac; on Windows a pack that comes with the first model, is missing,
 *  downloads or is installed. */
export const ENGINE_LINE = {
  builtIn: "Studio engine: built in",
  /** No local model yet: the pack downloads with the first one ("29 MB, MIT"). */
  withFirstModel: (size: string) => `Studio engine: comes with your first model (${size})`,
  missing: "Studio engine: missing",
  partial: "Studio engine: partly downloaded",
  installed: (build: string) => `Studio engine: installed · build ${build}`,
  /** How much of the pack is here; amounts are `downloadAmount`'s ("12 MB of 29 MB"). */
  downloading: (done: string | null, total: string | null) =>
    done !== null && total !== null ? `Studio engine: downloading · ${done} of ${total}` : "Studio engine: downloading…",
  unpacking: "Studio engine: unpacking…",
  /** Linux without a development engine. */
  unavailable: "Studio engine: not available on this computer",
  /** Between the installed line and *Remove*. */
  separator: "·",
  remove: "Remove",
  removeTitle: "Remove the Studio engine?",
  /** Under the question; `size` is its download ("29 MB"). */
  removeBody: (size: string) =>
    `Pictures and videos cannot be made on this computer until it is downloaded again (${size}). Your models and results stay.`,
  cancel: "Cancel",
} as const;

/** A recommended local model that fits, and its whole download: "FLUX.2 klein 4B fits this
 *  computer (5.3 GB)." */
export function fitsThisComputer(name: string, bytes: number): string {
  return `${name} fits this computer (${amountGb(bytes)}).`;
}

/** Studio's empty states (Task B2), local first; the backup and cloud lines name the companies
 *  that make the tab's kind (OpenAI makes no video). */
export const STUDIO_EMPTY = {
  getLocal: "Make pictures and videos on this computer: free, and nothing is sent.",
  /** The second, smaller line under `getLocal` when no saved endpoint makes the tab's kind. */
  backup: {
    image: "No good graphics card? Cloud models are the backup: OpenRouter or OpenAI.",
    video: "Or make videos with OpenRouter: add it in Settings → Endpoints.",
  } satisfies Record<MediaKind, string>,
  /** No local model fits, and no saved endpoint makes the tab's kind. */
  cloud: {
    image:
      "This computer cannot run local models well. Cloud models run on a company's computers instead: add OpenRouter or OpenAI.",
    video:
      "This computer cannot run local video models well. OpenRouter makes videos in the cloud: add it in Settings → Endpoints.",
  } satisfies Record<MediaKind, string>,
  /** The Video tab on a computer that cannot make video here. */
  noVideoHere:
    "Video on this computer needs a graphics card or an Apple-silicon Mac. OpenRouter makes videos in the cloud: add it in Settings → Endpoints.",
  /** A saved endpoint makes the tab's kind but has no key, so none of its models can be used. */
  noKey: (name: string) => `${name} has no key, so Studio cannot use its models. Add a key in Settings → Endpoints.`,
  /** A saved endpoint makes the tab's kind, but its saved lists have no such model Studio can use. */
  noModel: (name: string, kind: MediaKind) =>
    `${name}'s saved lists have no ${kind === "image" ? "picture" : "video"} model Studio can use. Update model lists from the menu, or add another endpoint in Settings → Endpoints.`,
} as const;

/** The one-time suggestion while nothing is installed locally. */
export const STUDIO_SUGGESTION = {
  text: (name: string) => `Studio works on this computer. ${name} fits here: free and private.`,
  size: (bytes: number) => `One download of ${amountGb(bytes)}.`,
} as const;

/** An endpoint whose lists are not saved yet ("Get OpenRouter's picture and video models"). */
export function getListWords(name: string, company: Company): string {
  return company === "openai" ? `Get ${name}'s picture models` : `Get ${name}'s picture and video models`;
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** A day as Studio's menu names it: "17 Nov", with the year when it is not this year. */
export function studioDay(at: number, now: number = Date.now()): string {
  const day = new Date(at);
  const short = `${day.getDate()} ${MONTHS[day.getMonth()]}`;
  return day.getFullYear() === new Date(now).getFullYear() ? short : `${short} ${day.getFullYear()}`;
}

/** A day and time as Settings → Studio names it, always with the year: "17 Nov 2026 09:00". */
export function studioDateTime(at: number): string {
  const day = new Date(at);
  const two = (n: number) => String(n).padStart(2, "0");
  return `${day.getDate()} ${MONTHS[day.getMonth()]} ${day.getFullYear()} ${two(day.getHours())}:${two(day.getMinutes())}`;
}

/** When an endpoint's model lists were saved: "OpenRouter's lists updated 17 Nov". */
export function listsUpdatedWords(name: string, fetchedAt: number | null, now: number = Date.now()): string {
  return fetchedAt === null ? `${name}'s lists: none saved yet` : `${name}'s lists updated ${studioDay(fetchedAt, now)}`;
}

/** A local model's fit word (Task B3): "Fits", "Slow here", "Fits after the chat model is
 *  unloaded", or not for this computer at all. */
export const LOCAL_FIT_WORDS: Record<LocalFit["word"], string> = {
  fits: "Fits",
  slow: "Slow here",
  afterUnload: "Fits after the chat model is unloaded",
  notHere: "Not for this computer",
};

/** How long a local run takes (Task B3; `eta.ts` works out the figure): "about 4 min a picture",
 *  "(first guess)" until it was measured on this computer. */
export const STUDIO_TIME = {
  about: (time: string) => `about ${time}`,
  seconds: (n: number) => `${n} s`,
  minutes: (n: number) => `${n} min`,
  hours: (n: number) => `${n} h`,
  firstGuess: "(first guess)",
  perPicture: "a picture",
  /** After the time, with the clip's length as its Length field names it ("2 s"). */
  perClip: (length: string) => `for a ${length} clip`,
  aClip: "a clip",
  /** Nothing measured here and no first guess. */
  notYet: "time shown after your first run here",
} as const;

/** A cloud model's price as the picker hints it (Task B3); amounts are `formatUsd`'s. */
export const STUDIO_PRICE = {
  perPicture: (usd: string) => `${usd} a picture`,
  fromPerPicture: (usd: string) => `from ${usd} a picture`,
  perSecond: (usd: string) => `${usd} a second`,
  rangePerSecond: (low: string, high: string) => `${low}–${high} a second`,
  /** An OpenRouter picture model whose price was not asked for yet. */
  whenPicked: "price shown when picked",
  unknown: "price unknown",
  free: "free",
} as const;

/** Studio's model picker (Task B3). */
export const STUDIO_PICKER = {
  label: "Model",
  choose: "Choose a model",
  search: "Search models",
  searchPlaceholder: "Search models…",
  filters: "Show only",
  filter: {
    edit: "Edit",
    references: "References",
    startFrame: "Start frame",
    endFrame: "End frame",
    sound: "Sound",
    transparent: "Transparent",
  },
  /** What a model takes or makes, beside its name. */
  badge: {
    edit: "Edit",
    refs: (n: number) => `Refs ${n}`,
    startFrame: "Start frame",
    endFrame: "End frame",
    sound: "Sound",
    transparent: "Transparent",
    gif: "GIF",
    svg: "SVG",
  },
  recent: "Recent",
  localTag: "free · private · nothing is sent",
  /** An endpoint's group: "via OpenRouter". */
  via: (name: string) => `via ${name}`,
  /** The Cloud mark's tooltip on an endpoint's group. */
  goesTo: (host: string) => `Your prompt and added pictures go to ${host}.`,
  noKey: "Add a key in Settings → Endpoints",
  /** The local group's last line while more recommended models that fit are not installed. */
  getMoreLocal: (n: number) => `+ ${STUDIO_ACTIONS.getLocalModels} (${n} more fit this computer)`,
  noLocalYet: (name: string, bytes: number) => `No local models yet. ${fitsThisComputer(name, bytes)}`,
  /** No local model of either kind fits this computer, or the Video tab where it cannot make video. */
  noLocalHere: "Local models need a graphics card or an Apple-silicon Mac; the cloud models below are the backup.",
  /** No local model of the tab's kind fits this computer, though one of the other kind does. */
  noKindHere: {
    image: "No picture model fits this computer yet; the cloud models below are the backup.",
    video: "No video model fits this computer yet; the cloud models below are the backup.",
  } satisfies Record<MediaKind, string>,
  nothingMatches: "No models match.",
  getPrice: "Get the price",
  /** An endpoint of the tab's kind whose lists are not saved yet: "OpenRouter: Get the list". */
  getList: (name: string) => `${name}: ${STUDIO_ACTIONS.getTheList}`,
} as const;

/** Studio's right panel and its fields (Task B5). */
export const STUDIO_FIELDS = {
  /** An unset field: not sent, so the company decides. */
  automatic: "Automatic (company default)",
  /** An unset field of a model on this computer. */
  automaticHere: "Automatic",
  reset: (label: string) => `Reset ${label}`,
  moreShapes: "More shapes",
  more: "More",
  /** An unset seed: Studio picks one for each run. */
  random: "Random",
  /** A chip with its time here: "2 s · about 3 min". */
  withTime: (value: string, time: string) => `${value} · ${time}`,
  customSize: "Custom size",
  width: "Width",
  height: "Height",
  /** Why a custom size is not sent yet, checked as it is typed. */
  size: {
    both: "Enter a width and a height.",
    multipleOf: (n: number) => `Both sides must be multiples of ${n}.`,
    ratio: (low: string, high: string) => `The shape must be between ${low} and ${high}.`,
    longestSide: (n: number) => `The longer side can be at most ${n}.`,
    tooSmall: "This size is too small for this model.",
    tooLarge: "This size is too large for this model.",
  },
  /** Characters left in a text field. */
  charsLeft: (n: number) => `${n} left`,
  advanced: "Advanced",
  advancedChanged: (n: number) => `Advanced · ${n} changed`,
  /** Choosing a transparent background changed the file type. */
  transparentNote: (type: string) => `Transparent needs PNG or WebP, so the file type is now ${type}.`,
  putNoteAway: "Put the note away",
  /** The seed's dice (Task B6). */
  newSeed: "New seed",
  /** The seed's lock: it stays as it is for the next run. */
  keepIt: "Keep it",
} as const;

/** The Advanced drawer (Task B6, design Wireframe 2): the chips under the prompt, Reset all, an
 *  OpenRouter picture's routing, Extra fields (JSON) and the exact request or command. */
export const STUDIO_ADVANCED = {
  resetAll: "Reset all",
  /** A changed setting's chip: "background: transparent" (`name` and `value` mid-sentence). */
  chip: (name: string, value: string) => `${name}: ${value}`,
  /** A seed's chip: "seed 81723". */
  seedChip: (name: string, value: string) => `${name} ${value}`,
  /** A switch's value in a chip. */
  on: "on",
  off: "off",
  removeChip: (chip: string) => `Remove ${chip}`,
  /** The chips row's name for a screen reader. */
  chips: "Changed settings",
  company: "Company that runs it",
  /** The company list's first choice: no routing is sent. */
  openRouterChooses: "OpenRouter chooses",
  /** A company saved in a setup that the model's answer no longer lists (its tag is still sent). */
  unlistedCompany: "A company no longer listed",
  allowAnother: "Allow another company if that one is busy",
  zdr: "Only companies that keep no data",
  /** Extra fields (JSON) with nothing set. */
  noneSet: "none set",
  editJson: "Edit JSON",
  save: "Save",
  cancel: "Cancel",
  /** The names the model's Extra field takes, as written in JSON. */
  accepts: (names: string[]) => `This model accepts: ${names.join(", ")}`,
  /** An OpenRouter picture model whose price, and with it its Extra field, was not asked for. */
  pickToSee: "Pick the model to see its extra settings.",
  notJson: "This is not valid JSON.",
  notObject: 'Write the extra fields as one JSON object, like {"moderation": "low"}.',
  notAccepted: (key: string) => `This model does not accept ${key}.`,
  /** A name the model takes as one of its own fields. */
  typed: (key: string) => `Set ${key} with its own field above.`,
  seeRequest: "See the exact request",
  seeCommand: "See the exact command",
  /** Under the exact request: "key removed · goes to openrouter.ai". */
  goesTo: (host: string) => `key removed · goes to ${host}`,
  /** The exact command's prompt, which the engine reads from a file. */
  promptFile: (prompt: string) => `Prompt (passed in prompt.txt): ${prompt}`,
  negativeFile: (text: string) => `Negative prompt (passed in negative.txt): ${text}`,
  runsHere: "Runs on this computer; nothing is sent.",
} as const;

/** My setups (Task B6): a model and all its settings saved under a name. */
export const STUDIO_SETUPS = {
  label: "My setups",
  none: "None",
  saveAs: "Save as setup…",
  name: "Setup name",
  save: "Save setup",
  cancel: "Cancel",
  /** The same limit and words as the backend's (`errors::TOO_MANY_SETUPS`). */
  tooMany: "Keep at most 50 setups.",
  noName: "Give the setup a name.",
  noModel: "Choose a model first.",
  /** The setup's model is gone, and `model` of its family is used in its place. */
  replaced: (endpoint: string, model: string) => `${endpoint} no longer offers this setup's model, so ${model} is used.`,
  gone: (endpoint: string) => `${endpoint} no longer offers this setup's model.`,
  goneHere: "This setup's model is no longer on this computer.",
  endpointGone: "This setup's endpoint was removed in Settings → Endpoints.",
} as const;

/** Under the Model field: where the chosen model runs. */
export const STUDIO_PLACE = {
  here: (device: string) => `${LOCAL_MARK.name} · ${device}`,
  cloud: (name: string) => `via ${name} · Cloud`,
} as const;

/** Added pictures: the tray, the frame drop zones and their role chips (Task B5). */
export const STUDIO_REFS = {
  add: "+ Add picture",
  choose: "Choose a picture",
  role: {
    reference: "Reference",
    startFrame: "Start frame",
    endFrame: "End frame",
  } satisfies Record<RefRole, string>,
  roleMenu: "What this picture is for",
  picture: (n: number) => `Picture ${n}`,
  remove: (n: number) => `Remove picture ${n}`,
  /** A frame's drop zone, empty. */
  dropHere: "Drop a picture here, or choose one",
  /** A frame's drop zone with its picture, which another replaces. */
  change: (label: string) => `${label}: change the picture`,
  removeFrame: (label: string) => `Remove ${label.charAt(0).toLowerCase()}${label.slice(1)}`,
  /** The most pictures of `role` the model takes (`null`: of any role, together). */
  takes(max: number, role: RefRole | null): string {
    const [one, many] = PICTURE_NOUNS[role ?? "reference"];
    if (max === 0) return `This model takes no ${many}.`;
    return `This model takes ${max} ${max === 1 ? one : many}.`;
  },
} as const;

/** Each company's name as Studio shows it (the backend's `errors::name`). */
export const STUDIO_COMPANY: Record<Company, string> = {
  openrouter: "OpenRouter",
  openai: "OpenAI",
  local: "This computer",
};

/** The prompt dock (Task B7, design Wireframe 1 and local addendum Wireframe L4): the line
 *  above Create, the price or time on it, the daily limit, and cloud as the backup. */
export const STUDIO_DOCK = {
  create: "Create",
  /** Under the prompt, judged on it as it is: the backend's own words for the same checks
   *  (`errors::invalid_sentences`, `errors::PROMPT_TOO_LONG_HERE`). */
  writePrompt: "Write a prompt first.",
  promptTooLong: "Keep the prompt under 32,000 characters.",
  promptTooLongHere: "Keep the prompt under 4,000 characters for local models.",
  /** A cloud run's estimate for the whole run, on Create: "~ $0.16". */
  estimate: (usd: string) => `~ ${usd}`,
  /** The line above Create for a model on this computer. */
  here: "On this computer: your prompt and pictures stay here",
  /** After `here`, when the chat model is unloaded to make room for the run. */
  unloadChat: "the chat model will be unloaded to make room",
  /** Joins the parts of the line above Create. */
  joiner: " · ",
  /** Under Create, in amber, for a local run that takes more than 15 minutes. */
  over15: (time: string) =>
    `This will take ${time} and keep the graphics card busy. On a MacBook it will get warm.`,
  /** Beside the greyed-out Create of a local model that does not fit this computer, with why
   *  ("needs 6 GB of graphics memory"). */
  notHere: (reason: string | null) => (reason ? `Not for this computer: ${reason}` : "Not for this computer"),
  /** A local run that is very slow here, while an endpoint can make it. */
  tooSlow: "Too slow here? A cloud model makes this in minutes, for a price you see first.",
  /** A local model that cannot run on this computer at all, while an endpoint can make it. */
  cloudInstead: "A cloud model can make this instead.",
  chooseCloud: "Choose a cloud model",
  /** The same, while no saved endpoint can make the tab's kind (OpenAI makes no video). */
  addEndpoint: {
    image: "Add OpenRouter or OpenAI in Settings → Endpoints to use a cloud model instead.",
    video: "Add OpenRouter in Settings → Endpoints to use a cloud model instead.",
  } satisfies Record<MediaKind, string>,
  /** In place of Create while a cloud run would pass the daily limit (the same words as the
   *  backend's `errors::daily_limit`). */
  limitReached: (spent: string, limit: string) => `Daily limit reached: ${spent} of ${limit} spent today`,
  /** The same, for a run whose price is unknown while less than the reserve of the limit is
   *  left (the same words as the backend's `errors::unknown_price_needs_reserve`). */
  limitNeedsReserve: (reserve: string, left: string) =>
    `This model's price is unknown, so Studio needs ${reserve} of today's limit left (${left} left).`,
  raiseLimit: "Raise the limit",
} as const;

/** The confirmation before a cloud run (Task B7, design Wireframe 3). */
export const STUDIO_CONFIRM = {
  title: (kind: MediaKind, count: number) =>
    kind === "video" ? "Make this video?" : count === 1 ? "Make this picture?" : "Make these pictures?",
  /** The model and the endpoint it is sent to: "Veo 3.1 through OpenRouter". */
  through: (model: string, endpoint: string) => `${model} through ${endpoint}`,
  sound: (on: boolean) => (on ? "sound on" : "sound off"),
  count: (n: number, kind: MediaKind) =>
    kind === "video" ? (n === 1 ? "1 video" : `${n} videos`) : n === 1 ? "1 picture" : `${n} pictures`,
  /** Joins the settings: "8 s · 720p · sound on · 1 video". */
  joiner: " · ",
  estimate: "Estimated cost",
  /** Where the estimate comes from, and when the company's prices were saved. */
  priceList: (company: string, day: string | null) =>
    day ? `(${company}'s price list, ${day})` : `(${company}'s price list)`,
  /** What leaves the computer and where it goes; OpenRouter passes the job on to a company. */
  goesTo(host: string, pictures: boolean, routed: boolean): string {
    const what = pictures ? "Your prompt and added pictures go" : "Your prompt goes";
    return routed ? `${what} to ${host} and the company it sends the job to.` : `${what} to ${host}.`;
  },
  videoCharge: (company: string) =>
    `${company} charges for a video once it has started, even if you stop watching it.`,
  dontAskVideos: (usd: string) => `Don't ask again for videos under ${usd}`,
  cancel: "Cancel",
  makeVideo: "Make video",
} as const;

/** A run's card (Task B8, design Wireframe 3 and local addendum Wireframe L4): the settings
 *  line, the status line, what a local run waits for or how it runs, the charge line and the
 *  card's buttons. Clocks are "m:ss" ("1:12"); times in words come from `eta.ts`. */
export const STUDIO_CARD = {
  /** The run's prompt, quoted. */
  prompt: (text: string) => `"${text}"`,
  /** Joins the parts of the settings and status lines. */
  joiner: " · ",
  /** Where a local run ran, in the settings line. */
  here: "on this computer",
  count: (n: number) => `×${n}`,
  seed: (seed: string) => `seed ${seed}`,
  size: (width: string, height: string) => `${width}×${height}`,
  addedPictures: (n: number) => (n === 1 ? "1 added picture" : `${n} added pictures`),
  startFrame: "start frame",
  endFrame: "end frame",
  /** How long a finished local run took ("took 3 min 48 s"). */
  took: (time: string) => `took ${time}`,
  /** A run waiting for its place in the queue (from 1). */
  queued: (place: number | null) => (place === null ? "Waiting" : `Waiting · number ${place} in the queue`),
  making: "Making it",
  soFar: (clock: string) => `${clock} so far`,
  /** How long this model's videos usually take ("usually about 2 min"). */
  usually: (time: string) => `usually ${time}`,
  keepWorking: "You can keep working or close the app; it will appear here.",
  stillNotReady: "Still not ready.",
  progress: "Progress",
  cancel: "Cancel",
  stop: "Stop",
  stopWatching: "Stop watching",
  getAnyway: "Get it anyway",
  checkAgain: "Check again",
  again: "Again",
  runAgain: "Run again",
  /** Fills the prompt and every setting from the run (Task B9); never starts it. */
  reuse: "Reuse",
  editPrompt: "Edit prompt",
  details: "Details",
  /** Whether an ended cloud run was charged, as the backend recorded it. */
  notCharged: "Not charged",
  mayBeCharged: "May have been charged",
  charged: (usd: string | null) => (usd === null ? "Charged" : `Charged ${usd}`),
  /** After the charge line: the company's id for the request. */
  request: (id: string) => `request ${id}`,
  /** A local run's status line: "Picture 1 of 2 · step 3 of 4 · 0:21 so far · about 0:30 left". */
  picture: (n: number, of: number) => `Picture ${n} of ${of}`,
  pass: (n: number, of: number) => `pass ${n} of ${of}`,
  step: (n: number, of: number) => `step ${n} of ${of}`,
  stage: {
    loading: "Loading the model",
    readingPrompt: "Reading the prompt",
    drawing: "Drawing",
    decoding: "Decoding",
    saving: "Saving",
  },
  /** Before the engine says which stage it is at. */
  starting: "Starting",
  left: (clock: string) => `about ${clock} left`,
  /** What a queued local run waits for. */
  waitingFor: {
    reply: "Waiting for the graphics card: a chat reply is being written. Starts when the reply ends.",
    studioJob: "Waiting for the graphics card: another Studio job is using it. Starts when that one ends.",
    chatModelLoading: "Waiting for the chat model to finish loading.",
  },
  /** Settings → Studio's Ask, when the run does not fit beside the chat model. */
  askUnload: "This needs the graphics card's memory. Unload the chat model?",
  unload: "Unload",
  keepChat: "Keep the chat model",
  unloaded: "The chat model was unloaded to make room.",
  loadAgain: "Load it again",
  alongside: "Chat replies may be slower while this runs",
  onProcessor: "Running on the processor: slower, and chat will be slow while this runs.",
  retrying: "Ran out of graphics memory, trying again with memory saving (slower)",
  stalled: "No progress for 10 minutes",
  keepWaiting: "Keep waiting",
  /** After the engine's or a damaged part's download started from the card. */
  engineDownloading: "The Studio engine is downloading. Press Run again when it is done.",
  repairing: "The model's damaged parts are downloading again. Press Run again when it is done.",
} as const;

/** The strip above the card stream: this session's runs of the tab, by state ("1 making ·
 *  1 waiting · 3 done · 1 stopped"); a state with none is left out. */
export const STUDIO_QUEUE = {
  making: (n: number) => `${n} making`,
  waiting: (n: number) => `${n} waiting`,
  done: (n: number) => `${n} done`,
  stopped: (n: number) => `${n} stopped`,
  failed: (n: number) => `${n} failed`,
} as const;

/** The notice in the chat for a video that finished while the chat was shown. */
export const STUDIO_NOTICE = {
  ready: "Your video is ready",
  open: "Open",
  putAway: "Put the notice away",
} as const;

/** The question before quitting while a local job works (`studio-quit-requested`). */
export const STUDIO_QUIT = {
  title: "Quit while Studio is working?",
  /** `null` when the job is not known (the window reloaded and heard nothing of it). */
  making: (kind: MediaKind | null) =>
    kind === "video"
      ? "A video is being made on this computer. Quitting stops it."
      : kind === "image"
        ? "A picture is being made on this computer. Quitting stops it."
        : "Studio is making something on this computer. Quitting stops it.",
  keepWorking: "Keep working",
  quitAndStop: "Quit and stop",
} as const;

/** The chat header while a chat model's load waits for a local Studio job. */
export const STUDIO_CHAT_WAIT = {
  waiting: "Studio is using the graphics card. The chat model loads when it finishes.",
  stop: "Stop Studio job",
} as const;

/** Studio and the chat (Task B11): *Send to chat* on a picture, and *Open in Studio* on a
 *  picture attached in a chat. */
export const STUDIO_CHAT_LINK = {
  sendToChat: "Send to chat",
  /** On a chat picture; with several in the message, which one: "Open picture 2 in Studio". */
  openInStudio: (n: number, of: number) => (of > 1 ? `Open picture ${n} in Studio` : "Open in Studio"),
  /** The name a chat picture goes to Studio under, as the backend's checks of it say it
   *  ("This picture is larger than 20 MB."). */
  pictureName: "This picture",
  /** Why a chat picture did not go into Studio's Pictures tab: the tray's own words (the model
   *  takes no more), or `noModel`. */
  notAdded: (why: string) => `Not added to Studio. ${why}`,
  noModel: "There is no picture model yet: open Studio to get one.",
} as const;

/** The chat's message box (Task B11): its list of attached files, and the limit it keeps to
 *  (the backend's `MAX_IMAGES`). */
export const COMPOSER_WORDS = {
  attachments: "Attachments",
  /** A drop, a paste or *Send to chat* that went past the limit. */
  tooManyPictures: (max: number) => `${max} pictures at most in one message. The rest were not added.`,
} as const;

/** A result's name in Studio's gallery: "picture 2 of 4" among a run's results, "video" alone. */
function resultName(kind: MediaKind, n: number, of: number): string {
  const noun = kind === "video" ? "video" : "picture";
  return of > 1 ? `${noun} ${n} of ${of}` : noun;
}

/** A result's tile, on a card and in the grid (Task B9). */
export const STUDIO_TILE = {
  /** The tile itself, which opens the viewer: "Open picture 2 of 4: A lighthouse…". */
  open: (kind: MediaKind, n: number, of: number, prompt: string) =>
    prompt ? `Open ${resultName(kind, n, of)}: ${prompt}` : `Open ${resultName(kind, n, of)}`,
  /** Its box in the grid, to select it with others. */
  select: (kind: MediaKind, n: number, of: number, prompt: string) =>
    prompt ? `Select ${resultName(kind, n, of)}: ${prompt}` : `Select ${resultName(kind, n, of)}`,
  /** Its star on a card: "Star picture 2 of 4" (pressed while starred). */
  star: (kind: MediaKind, n: number, of: number) => `Star ${resultName(kind, n, of)}`,
} as const;

/** The gallery's grid (Task B9): its filters beside the top bar's search and Starred, Load
 *  more, and what several selected results can do together. */
export const STUDIO_GRID = {
  kinds: "Show",
  all: "All",
  /** The date filter. */
  when: "When",
  since: {
    any: "Any time",
    today: "Today",
    week: "Last 7 days",
    month: "Last 30 days",
  },
  loadMore: "Load more",
  empty: "Nothing here yet.",
  nothingMatches: "No results match.",
  selected: (n: number) => `${n} selected`,
  deleteSome: (n: number) => `Delete ${n}`,
  saveSome: (n: number) => `Save ${n}…`,
  clear: "Clear selection",
} as const;

/** Deleting results, in the viewer or several from the grid (Task B9): the file and its record
 *  go; the audit log keeps its line, and a cloud run made today still counts toward today's
 *  spending. */
export const STUDIO_DELETE = {
  title(count: number, kinds: readonly MediaKind[]): string {
    const pictures = kinds.every((k) => k === "image");
    const videos = kinds.every((k) => k === "video");
    if (count === 1) return videos ? "Delete this video?" : "Delete this picture?";
    return pictures ? `Delete ${count} pictures?` : videos ? `Delete ${count} videos?` : `Delete ${count} results?`;
  },
  removes: (count: number) =>
    count === 1 ? "The file and its record are removed from the gallery." : "The files and their records are removed from the gallery.",
  audit: "The audit log keeps a line saying a run happened (model and cost), but not your prompt.",
  /** Added after `audit` when a result of a cloud run made today (the local day the daily
   *  limit counts by) is among those deleted. */
  spending: "It still counts toward today's spending.",
  delete: "Delete",
  cancel: "Cancel",
} as const;

/** Reuse all and Reuse without seed when the run's model is no longer offered (Task B9). */
export const STUDIO_REUSE = {
  /** The model is gone, and `model` of its family is used in its place. */
  replaced: (endpoint: string, model: string) => `${endpoint} no longer offers this model, so ${model} is used.`,
  gone: (endpoint: string) => `${endpoint} no longer offers this model.`,
  goneHere: "This model is no longer on this computer.",
  endpointGone: "This run's endpoint was removed in Settings → Endpoints.",
} as const;

/** Animate and Edit when the tab's model cannot take the picture (Task B9): another model is
 *  offered, an installed local one first. */
export const STUDIO_OFFER = {
  animate: (model: string | null, offered: string) =>
    model ? `${model} takes no start frame. Use ${offered} instead?` : `Make the video with ${offered}?`,
  edit: (model: string | null, offered: string) =>
    model ? `${model} cannot change a picture. Use ${offered} instead?` : `Change the picture with ${offered}?`,
  noneAnimate: "No video model here takes a start frame.",
  noneEdit: "No picture model here can change a picture.",
  /** The offer's button ("use" would read as a React hook to the linter). */
  choose: (model: string) => `Use ${model}`,
} as const;

const pad2 = (n: number) => String(n).padStart(2, "0");

/** When a run was made, as the viewer says it: "17 Nov 2026 14:02". */
export function madeWords(at: number): string {
  const d = new Date(at);
  return `${d.getDate()} ${MONTHS[d.getMonth()]} ${d.getFullYear()} ${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}

/** Studio's viewer (Task B9, design Wireframe 4 and the local addendum). */
export const STUDIO_VIEWER = {
  /** The viewer's name for a screen reader. */
  title: (kind: MediaKind) => (kind === "video" ? "Video" : "Picture"),
  position: (n: number, of: number) => `${n} / ${of}`,
  previous: "Previous",
  next: "Next",
  close: "Close",
  /** A row's name before its value: "Cost: ". */
  row: (label: string) => `${label}: `,
  prompt: "Prompt",
  copyPrompt: "Copy prompt",
  model: "Model",
  /** Where OpenRouter ran it, when known: "OpenRouter (runs at OpenAI)". */
  runsAt: (company: string, upstream: string | null) => (upstream ? `${company} (runs at ${upstream})` : company),
  size: "Size",
  settings: "Settings",
  cost: "Cost",
  made: "Made",
  file: "File",
  notes: "Notes",
  /** After the device a local run ran on. */
  on: (device: string) => `on ${device}`,
  engineBuild: (build: number) => `engine build ${build}`,
  labelled: "Labelled as AI-made in the file",
  /** A setting and its value in the settings row: "Quality low". */
  setting: (label: string, value: string) => `${label} ${value}`,
  star: "Star",
  edit: "Edit",
  animate: "Animate",
  useAsReference: "Use as reference",
  showInFolder: "Show in folder",
  delete: "Delete",
  reuseAll: "Reuse all",
  reuseWithoutSeed: "Reuse without seed",
  /** Reuse filled the draft; the viewer stays open. */
  reused: "Filled in the prompt and settings. Nothing was started.",
  /** Use as reference added the picture to the tab's added pictures. */
  addedAsReference: "Added to the prompt's pictures.",
  saveAs: "Save as…",
  withSettings: "Also save the settings as a .json file",
  savedTo: (path: string) => `Saved to ${path}`,
  copy: "Copy",
  copied: "Copied",
  noCopy: "Copy is not available here. Use Save as… instead.",
  /** The file types the save dialog offers: "PNG picture", "MP4 video". */
  fileType: (ext: string, kind: MediaKind) => `${ext.toUpperCase()} ${kind === "video" ? "video" : "picture"}`,
} as const;

/** The shortcut sheet's keys for Studio's viewer (Task B9). */
export const STUDIO_VIEWER_KEYS = {
  title: "In Studio's viewer",
  rows: [
    ["← →", "Previous or next result"],
    ["F", "Star or unstar"],
    ["R", "Reuse all (Shift R: without the seed)"],
    ["A", "Animate a picture"],
    ["S", "Save as…"],
    ["C", "Copy a picture"],
    ["Del", "Delete"],
    ["Space", "Play or pause a video"],
    ["Esc", "Close the viewer (never stops a Studio job)"],
  ] as const satisfies readonly (readonly [string, string])[],
} as const;

/** Settings → Studio (Task B10): the local block first, then the gallery, cloud spending and
 *  the cloud model lists. */
export const STUDIO_SETTINGS = {
  local: LOCAL_MARK.name,
  /** The picture and video models installed here and the disk they take ("14 GB"). */
  models: (count: number, bytes: number) =>
    count === 0 ? "Picture and video models: none yet" : `Picture and video models: ${count} (${amountGb(bytes)})`,
  gpu: "When a local job needs the graphics card",
  gpuHelp: "When a local job fits beside the chat model, both run. When it does not:",
  gpuChoice: {
    unload: "Unload the chat model",
    ask: "Ask",
    processor: "Use the processor for local pictures",
  } satisfies Record<GpuPolicy, string>,
  reloadAfter: "Reload it after Studio",
  reloadHelp: "Loads the chat model again once, after the last local job.",
  /** Opens the note about local models, read only (Task B14). */
  whatLocalMeans: "What local models mean",
  results: "Results",
  /** What the gallery holds and where: "Studio uses 1.2 GB in ~/harnessUI/studio (312
   *  pictures, 14 videos)". Sizes in the same binary units as `amountGb`. */
  usage(bytes: number, folder: string, pictures: number, videos: number): string {
    const size = bytes > 0 ? downloadAmount(bytes) : "0 MB";
    const count = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;
    return `Studio uses ${size} in ${folder} (${count(pictures, "picture")}, ${count(videos, "video")})`;
  },
  openFolder: "Open folder",
  changeFolder: "Change…",
  folderChanged: "New results go to the new folder; existing ones stay where they are.",
  deleteUnfinished: "Delete stopped and failed runs",
  deleteTitle: "Delete stopped and failed runs?",
  deleteBody:
    "Every run that failed, was stopped or was refused goes from the gallery, with any files it kept. Cloud runs made today still count toward today's spending.",
  /** How many runs *Delete stopped and failed runs* deleted. */
  deleted: (n: number) =>
    n === 0 ? "There were no stopped or failed runs." : n === 1 ? "1 run was deleted." : `${n} runs were deleted.`,
  spending: "Cloud spending",
  confirmVideo: "Ask before every video",
  /** Before an amount box, which takes US dollars. */
  dollar: "$",
  /** Before the amount box; "or more" follows it. */
  confirmAt: "Ask before runs of",
  orMore: "or more",
  confirmAtRange: "Choose an amount from $0.25 to $100.",
  limit: "Daily limit",
  limitAmount: "Daily limit in US dollars",
  limitHelp:
    "A cloud run that would take today's spending past it is not sent. Runs on this computer are free and never count.",
  limitRange: "Choose an amount from $1 to $1,000.",
  lists: "Cloud model lists",
  listUpdated: (name: string, at: number) => `${name} · updated ${studioDateTime(at)}`,
  listNone: (name: string) => `${name} · no lists saved yet`,
  updateNow: "Update now",
  noEndpoint: "No endpoint makes pictures or video yet. Add OpenRouter or OpenAI in Endpoints.",
} as const;

/** What a model licence allows for what you make, as its badge says it (Task B13): green,
 *  amber with its condition ("$5M" from the revenue limit, `revenueLimit`), or red. */
export const LICENCE_BADGE = {
  yes: "Free for any use, including commercial",
  underRevenue: (limit: string | null) =>
    limit === null ? "Commercial use only under a revenue limit" : `Commercial use only under ${limit} revenue`,
  registration: "Commercial use only after registering with the publisher",
  unclear: "Check the licence",
  no: "Non-commercial",
} as const satisfies Record<Licence["commercial"], string | ((limit: string | null) => string)>;

/** A yearly revenue limit as a licence badge says it: "$5M", "$2.5M", "$500K". */
export function revenueLimit(usd: number): string {
  const units: readonly (readonly [number, string])[] = [
    [1e9, "B"],
    [1e6, "M"],
    [1e3, "K"],
  ];
  for (const [size, unit] of units) if (usd >= size) return `$${Math.round((usd / size) * 10) / 10}${unit}`;
  return `$${usd}`;
}

/** Get local models, Studio's own view (Task B13, local addendum Wireframe L2): the header,
 *  the filters, each model's row, Hugging Face search and the footer. */
export const LOCAL_MODELS = {
  title: STUDIO_ACTIONS.getLocalModels,
  back: "Back",
  /** The header's parts: "This computer: Radeon RX 7600, 8 GB graphics memory · 32 GB
   *  memory · 412 GB free"; amounts are `amountGb`'s. */
  thisComputer: (what: string) => `This computer: ${what}`,
  processorOnly: "processor only",
  graphicsMemory: (amount: string) => `${amount} graphics memory`,
  sharedMemory: (amount: string) => `${amount} shared memory`,
  memory: (amount: string) => `${amount} memory`,
  freeDisk: (amount: string) => `${amount} free`,
  onlyFits: "Only what fits",
  showNonCommercial: "Show non-commercial models",
  recommended: "Recommended for this computer",
  installed: "Installed",
  download: "Download",
  delete: "Delete",
  /** The folded list of models that do not fit this computer. */
  notHere: (n: number) => `Not for this computer (${n})`,
  /** The tab has no model that fits this computer. */
  noneFit: {
    image: "No picture model fits this computer.",
    video: "No video model fits this computer.",
  } satisfies Record<MediaKind, string>,
  videoNote: "Local video is slower and simpler than cloud video, but free and private.",
  /** A time line without an estimate (nothing measured here, no first guess). */
  noTime: STUDIO_TIME.notYet.charAt(0).toUpperCase() + STUDIO_TIME.notYet.slice(1),
  /** The bar of a model's download on its row, while it runs. */
  downloading: (name: string) => `Downloading ${name}`,
  /** After *Download again*: what happens now. */
  repairing: "Its files are being checked; anything missing or damaged downloads again (Models → Downloads).",
  deleteTitle: (name: string) => `Delete ${name}?`,
  deleteBody:
    "Its files are removed from this computer, except parts another model still uses. Pictures and videos you made stay in the gallery.",
  cancel: "Cancel",
  search: "Search Hugging Face",
  searchPlaceholder: "Search Hugging Face for picture and video models…",
  searchButton: "Search",
  searchSends: "Searching sends your words to huggingface.co.",
  searching: "Searching…",
  results: "Found on Hugging Face",
  noResults: "Hugging Face found no picture or video models for that.",
  /** A search hit's downloads, as `formatCount` gives them ("12.3k"). */
  downloads: (count: string) => `${count} downloads`,
  /** A search hit's licence, as its repository labels it. */
  labelled: (label: string) => `labelled ${label}`,
  opening: "Reading the repository on huggingface.co…",
  /** A searched file: "3 parts · 7.1 GB in all · 4.6 GB to download" (the last part only when
   *  some of it is already here). */
  fileLine(parts: number, total: number, toFetch: number): string {
    const line = `${parts === 1 ? "1 part" : `${parts} parts`} · ${amountGb(total)} in all`;
    return toFetch < total ? `${line} · ${amountGb(toFetch)} to download` : line;
  },
  /** One part of a searched file: "reads your prompt · 2.5 GB · already on this computer". */
  part: (label: string, bytes: number, here: boolean) =>
    `${label} · ${amountGb(bytes)}${here ? " · already on this computer" : ""}`,
  notSupportedYet: "Not supported yet",
  /** `reason` is the backend's clause ("its licence excludes some regions"). */
  notOffered: (reason: string | null) =>
    reason ? `Not offered in ${APP_NAME}: ${reason}.` : `Not offered in ${APP_NAME}.`,
  hiddenNonCommercial: "Its files show when Show non-commercial models is ticked.",
  notice: "Local models run only on this computer; no company checks prompts or results.",
  whatThatMeans: "What that means",
} as const;

/** "username and email", "username, email and company". */
function andList(items: readonly string[]): string {
  return items.length <= 1 ? (items[0] ?? "") : `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

/** The download confirmation of a local model (Task B14, local addendum Wireframe L3): every
 *  part with its size and licence, what you make may be used for, the Studio engine, a gated
 *  repository, an amber licence's condition, low disk space and the licences to read. */
export const DOWNLOAD_LOCAL = {
  title: (name: string) => `Download ${name}?`,
  /** "3 files, 6.2 GB in all, from Hugging Face (huggingface.co)"; with some of it here
   *  already, "…, 3.9 GB to download, from …". Amounts are `amountGb`'s. */
  summary(files: number, total: number, toFetch: number): string {
    const count = files === 1 ? "1 file" : `${files} files`;
    const fetch = toFetch < total ? `, ${amountGb(toFetch)} to download` : "";
    return `${count}, ${amountGb(total)} in all${fetch}, from Hugging Face (huggingface.co)`;
  },
  /** A part another model brought already: checked, not downloaded again. */
  alreadyHere: "already on this computer",
  /** What the pictures or videos you make may be used for, by the strictest licence
   *  (`verdict`): "Pictures you make: free for any use, including commercial.", "Videos you
   *  make: …" for a video model. */
  verdictLead: { image: "Pictures you make", video: "Videos you make" } as const satisfies Record<MediaKind, string>,
  verdict: {
    yes: "free for any use, including commercial.",
    underRevenue: (limit: string | null) =>
      limit === null ? "commercial use only under a revenue limit." : `commercial use only under ${limit} revenue.`,
    registration: "commercial use only after registering with the publisher.",
    unclear: "check the licence before any commercial use.",
    no: "non-commercial use only.",
  } as const satisfies Record<Licence["commercial"], string | ((limit: string | null) => string)>,
  /** The Studio engine comes with the model (Windows): "(29 MB, MIT licence)", from
   *  `engineSizeWords`. */
  engine: (size: string) => `Also downloads the Studio engine (${size} licence).`,
  /** Which form a gated repository's publisher shows on Hugging Face. */
  gate: {
    auto: "This model is gated: accept the publisher's terms on its Hugging Face page first, and access is given at once.",
    manual:
      "This model is gated: ask for access on its Hugging Face page first, and the publisher approves each request.",
  } as const,
  /** What the publisher of a gated repository receives: "username and email". */
  collects: (what: readonly string[]) => `The publisher receives your Hugging Face ${andList(what)}.`,
  openModelPage: "Open the model page",
  /** An amber licence's condition, then each duty it asks of you ("Review pictures before you
   *  share them." reads "The licence asks you to review pictures before you share them."). */
  amber(licence: Licence): string {
    const condition =
      licence.commercial === "registration"
        ? "Commercial use only after registering with the publisher."
        : licence.revenueLimitUsd === null
          ? "Commercial use only if your organisation earns less than the licence's limit."
          : `Commercial use only if your organisation earns less than ${revenueLimit(licence.revenueLimitUsd)} a year.`;
    const duties = licence.obligations.map((o) => `The licence asks you to ${SETTING_WORDS.midSentence(o)}`);
    return [condition, ...duties].join(" ");
  },
  lowDisk: "This leaves less than 10 GB free on this disk.",
  /** The link to a licence's text; with several licences, each says which it opens (by its
   *  name, the Studio engine's by its short word: "Read the licences (MIT)"). */
  readLicences: (name: string | null) => (name === null ? "Read the licences" : `Read the licences (${name})`),
  /** A licence or model page the browser did not open, and where to find it instead. */
  notOpened: "The page could not be opened in your browser.",
  goTo: (url: string) => `Go to ${url} in your browser.`,
  accepting: "Downloading means you accept these licences.",
  cancel: "Cancel",
  accept: "Accept and download",
} as const;

/** The one-time note about local models (Task B14, local addendum L1), word for word: shown
 *  before the first local download, and again from Get local models' footer and Settings →
 *  Studio. `lead` is its first sentence, in bold. */
export const LOCAL_NOTICE = {
  lead: "Local models run only on your computer.",
  body: "No company checks your prompts or pictures. Don't create sexual images of children, or sexual or intimate images of real people without their consent, and don't impersonate people. These are crimes in many countries. Each model's licence and use policy also applies.",
  understand: "I understand",
  cancel: "Cancel",
  close: "Close",
} as const;

/** What a download is doing, as Models → Downloads says it. [0.0.7 P6b] Checking a file
 *  already there, or stopping and deleting its files after Cancel, each take a while. */
function downloadStatusWords(g: DownloadGroup): string {
  if (g.status === "downloading" && g.checking === true) return "checking the file…";
  if (g.status === "cancelling") return "cancelling…";
  return g.status;
}

/** A download's progress as Models → Downloads says it, also on a picture or video model's row
 *  in Get local models (0.1.0 Task B14): "1.2 GB of 6.6 GB · downloading · 3 min left", with
 *  " · no published checksum" when a file has none. `amount` writes the sizes (Models:
 *  `formatBytes`; Studio: `amountGb`, as its model sizes are written). 0.0.7 (P6b review m5): no
 *  time left while a file is checked. */
export function downloadProgressWords(g: DownloadGroup, amount: (bytes: number) => string): string {
  const checking = g.status === "downloading" && g.checking === true;
  const eta = g.status === "downloading" && !checking ? formatEta(g.bytesTotal - g.bytesDone, g.bytesPerSecond) : null;
  return (
    `${amount(g.bytesDone)} of ${amount(g.bytesTotal)} · ${downloadStatusWords(g)}` +
    `${eta ? ` · ${eta}` : ""}${g.unverified ? " · no published checksum" : ""}`
  );
}

/** Models → Downloads: what a picture or video model's download is (Task B13). */
export const DOWNLOAD_KIND_WORDS: Record<"image" | "image-edit" | "video", string> = {
  image: "Picture model",
  "image-edit": "Picture model",
  video: "Video model",
};

/** Models → Installed: the one line for picture and video models, which open in Studio
 *  (Task B13): "Picture and video models: 2 (14 GB) · Manage in Studio". */
export const INSTALLED_STUDIO_LINE = {
  models: STUDIO_SETTINGS.models,
  manage: "Manage in Studio",
} as const;

/** What an endpoint card says it can do (Settings → Endpoints, 0.1.0): "Chat · Pictures ·
 *  Video" for OpenRouter, "Chat · Pictures" for OpenAI, "Chat" for the others. */
export function abilitiesWords(media: { image: boolean; video: boolean } | undefined): string {
  const parts = ["Chat"];
  if (media?.image) parts.push(STUDIO_TABS.image);
  if (media?.video) parts.push(STUDIO_TABS.video);
  return parts.join(" · ");
}

// Plain words for the app's technical values (0.0.6 D4, D8). Nothing on screen should show
// a raw identifier such as "vulkan", "Vulkan0" or "text_model". Every phrase of the model
// loading screens lives here, so the wording stays the same wherever it appears; the test
// walks every value of the contract's enums.
import type {
  CannotRunReason,
  Company,
  FileLabel,
  GpuPolicy,
  Licence,
  LocalFit,
  MediaKind,
  NotReadReason,
  PartRole,
  PartSource,
  RecipeKind,
  RefRole,
  SdVersion,
  SearchFilter,
  Verdict as StudioVerdict,
} from "../components/studio/types";
import { APP_NAME } from "./app";
import { formatBytes, formatCount, formatEta } from "./format";
import type {
  BackendKind,
  ContextSource,
  DeviceOption,
  DownloadGroup,
  DownloadStatus,
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
  source: ["picture to change", "pictures to change"],
  mask: ["mask", "masks"],
  control: ["control picture", "control pictures"],
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
  /** A list or an object as a value in a sentence (never its raw text). */
  items: (n: number) => (n === 1 ? "1 item" : `${n} items`),
  aValue: "a value",
  /** A LoRA attached that was made for another type of model than the new one. */
  loraNotFor: (name: string, type: string) => `The LoRA ${name} was made for ${type} models, so it was removed.`,
  /** The LoRAs attached, by name, for a model that takes none. */
  noLoras: (model: string, names: readonly string[]) =>
    `${model} takes no LoRAs, so ${andList(names)} ${names.length === 1 ? "was" : "were"} removed.`,
  noLorasCount: (model: string, n: number) =>
    `${model} takes no LoRAs, so ${n === 1 ? "1 LoRA was" : `${n} LoRAs were`} removed.`,
  /** The LoRAs past the new model's limit, by name. */
  lorasPastLimit: (model: string, max: number, names: readonly string[]) =>
    `${model} takes up to ${max === 1 ? "1 LoRA" : `${max} LoRAs`}, so ${andList(names)} ${names.length === 1 ? "was" : "were"} removed.`,
  lorasPastLimitCount: (model: string, max: number, n: number) =>
    `${model} takes up to ${max === 1 ? "1 LoRA" : `${max} LoRAs`}, so ${n === 1 ? "1 was" : `${n} were`} removed.`,
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

/** Studio's top bar and its menu (Task B2; the prompt box's words are `STUDIO_COMPOSER`'s). */
export const STUDIO_BAR = {
  /** The button beside Studio's name that shows or hides the chat list (Ctrl/⌘ B too). */
  chatList: "Show or hide the chat list",
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
  openEndpoints: "Open Endpoints",
  getTheList: "Get the list",
  notNow: "Not now",
  /** A local model's parts checked, and what is missing or damaged fetched again. */
  downloadAgain: "Download again",
} as const;

/** The Studio engine's download size and the short name of its licence: "30 MB, MIT". The
 *  size is `formatBytes`'s, as every size Studio shows (Task U7). */
export function engineSizeWords(bytes: number, licenseName: string): string {
  return `${formatBytes(bytes)}, ${licenceWord(licenseName)}`;
}

/** The Studio engine's download, where it is missing (Tasks B8, B10 and B13): its size and the
 *  short name of its licence, "Download the Studio engine (30 MB, MIT)". */
export function engineDownloadWords(bytes: number, licenseName: string): string {
  return `Download the Studio engine (${engineSizeWords(bytes, licenseName)})`;
}

/** The Studio engine's line (Settings → Studio and Get models' footer, Tasks B10 and
 *  B13): built in on a Mac; on Windows a pack that comes with the first model, is missing,
 *  downloads or is installed. */
export const ENGINE_LINE = {
  builtIn: "Studio engine: built in",
  /** No local model yet: the pack downloads with the first one ("30 MB, MIT"). */
  withFirstModel: (size: string) => `Studio engine: comes with your first model (${size})`,
  missing: "Studio engine: missing",
  partial: "Studio engine: partly downloaded",
  installed: (build: string) => `Studio engine: installed · build ${build}`,
  /** How much of the pack is here; amounts are `formatBytes`'s ("13 MB of 30 MB"). */
  downloading: (done: string | null, total: string | null) =>
    done !== null && total !== null ? `Studio engine: downloading · ${done} of ${total}` : "Studio engine: downloading…",
  unpacking: "Studio engine: unpacking…",
  /** Linux without a development engine. */
  unavailable: "Studio engine: not available on this computer",
  /** Between the installed line and *Remove*. */
  separator: "·",
  remove: "Remove",
  removeTitle: "Remove the Studio engine?",
  /** Under the question; `size` is its download ("30 MB"). */
  removeBody: (size: string) =>
    `Pictures and videos cannot be made on this computer until it is downloaded again (${size}). Your models and results stay.`,
  cancel: "Cancel",
} as const;

/** A recommended local model that fits, and its whole download: "FLUX.2 klein 4B fits this
 *  computer (5.3 GB)." */
export function fitsThisComputer(name: string, bytes: number): string {
  return `${name} fits this computer (${formatBytes(bytes)}).`;
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
  /** A saved endpoint makes the tab's kind but has no key, so none of its models can be used. */
  noKey: (name: string) => `${name} has no key, so Studio cannot use its models. Add a key in Settings → Endpoints.`,
  /** A saved endpoint makes the tab's kind, but its saved lists have no such model Studio can use. */
  noModel: (name: string, kind: MediaKind) =>
    `${name}'s saved lists have no ${kind === "image" ? "picture" : "video"} model Studio can use. Update model lists from the menu, or add another endpoint in Settings → Endpoints.`,
} as const;

/** The one-time suggestion while nothing is installed locally. */
export const STUDIO_SUGGESTION = {
  text: (name: string) => `Studio works on this computer. ${name} fits here: free and private.`,
  size: (bytes: number) => `One download of ${formatBytes(bytes)}.`,
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
 *  unloaded", or "May not fit" (a warning, never a refusal: `LOCAL_WARNING` says why). */
export const LOCAL_FIT_WORDS: Record<LocalFit["word"], string> = {
  fits: "Fits",
  slow: "Slow here",
  afterUnload: "Fits after the chat model is unloaded",
  notHere: "May not fit",
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

/** Studio's model picker (Task B3), as the model sheet and the side panel's price line keep
 *  it (Task U4). */
export const STUDIO_PICKER = {
  choose: "Choose a model",
  localTag: "free · private · nothing is sent",
  /** Where an endpoint's prompt goes, on its group in the model sheet. */
  goesTo: (host: string) => `Your prompt and added pictures go to ${host}.`,
  noKey: "Add a key in Settings → Endpoints",
  noLocalYet: (name: string, bytes: number) => `No local models yet. ${fitsThisComputer(name, bytes)}`,
  /** Nothing installed, and no local model of either kind known to fit this computer. */
  noLocalHere: "No local model is installed, and none is known to fit this computer; the cloud models below are the backup.",
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

/** The model sheet (Task U4, design 10.2): the composer's model chip opens it. */
export const MODEL_SHEET = {
  title: "Choose a model",
  search: "Search models",
  /** The name of the All · On this computer · Cloud chips. */
  filters: "Show",
  all: "All",
  local: "On this computer",
  cloud: "Cloud",
  recent: "Recent",
  priceUnknown: "price unknown",
  canEdit: "Can change a picture",
  transparent: "Transparent background",
  notes: "About this model",
  getMore: "Get more models",
  /** The LoRAs the draft attaches to the model: "1 LoRA", "2 LoRAs". */
  loraCount: (n: number) => (n === 1 ? "1 LoRA" : `${n} LoRAs`),
  needsSetup: "Needs setting up",
} as const;

/** Studio's right panel and its fields (Task B5). */
export const STUDIO_FIELDS = {
  /** An unset field: not sent, so the company decides. */
  automatic: "Automatic (company default)",
  /** An unset field of a model on this computer. */
  automaticHere: "Automatic",
  reset: (label: string) => `Reset ${label}`,
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
  /** A field's help button, whose tooltip holds the help (Task U8). */
  about: (label: string) => `About ${label}`,
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

/** The strip's and input zones' file chooser. */
const CHOOSE_PICTURE = "Choose a picture";

/** What a picture added to the prompt is for, and what the two files that need explaining
 *  are (Task S12): the role menu and the picture zones of Phase U's screens. */
export const INPUT_ROLE = {
  label: {
    reference: "Reference",
    startFrame: "Start frame",
    endFrame: "End frame",
    source: "Picture to change",
    mask: "Mask",
    control: "Control picture",
  } satisfies Record<RefRole, string>,
  hint: {
    mask: "A black-and-white picture: white areas are changed, black areas are kept.",
    control: "An edge map for Canny models, a depth map for Depth models.",
  },
} as const;

/** Added pictures: the composer's strip, its input zones and the role chips (Task B5; the
 *  composer's own words are `STUDIO_COMPOSER`'s). */
export const STUDIO_REFS = {
  choose: CHOOSE_PICTURE,
  /** The file chooser of an input zone, named by its field: "Start frame: Choose a picture". */
  chooseFor: (label: string) => `${label}: ${CHOOSE_PICTURE}`,
  role: INPUT_ROLE.label,
  roleMenu: "What this picture is for",
  picture: (n: number) => `Picture ${n}`,
  /** An input zone with its picture, which another replaces. */
  change: (label: string) => `${label}: change the picture`,
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

/** Create and what goes with it (the prompt dock of Task B7, the composer's since Task U3):
 *  the privacy line, the price or time on Create, the daily limit, and cloud as the backup. */
export const STUDIO_DOCK = {
  create: "Create",
  /** Under the prompt, judged on it as it is: the backend's own words for the same checks
   *  (`errors::invalid_sentences`, `errors::PROMPT_TOO_LONG`). */
  writePrompt: "Write a prompt first.",
  /** A prompt over its hidden bound (design 9.2: never shown as a limit), local or cloud. */
  promptTooLong: "This prompt is too long.",
  /** Create pressed while the extra settings waited for the model's price, which could not
   *  be read: the run went without them (they stay held for *Get the price*). */
  sentWithoutExtra: "Sent without the extra settings: their price couldn't be read.",
  /** A cloud run's estimate for the whole run, on Create: "~ $0.16". */
  estimate: (usd: string) => `~ ${usd}`,
  /** The privacy line under the composer, for a model on this computer. */
  here: "On this computer: your prompt and pictures stay here",
  /** After `here`, when the chat model is unloaded to make room for the run. */
  unloadChat: "the chat model will be unloaded to make room",
  /** Joins the parts of the privacy line. */
  joiner: " · ",
  /** Under Create, in amber, for a local run that takes more than 15 minutes. */
  over15: (time: string) =>
    `This will take ${time} and keep the graphics card busy. On a MacBook it will get warm.`,
  /** A local run that is very slow here, while an endpoint can make it. */
  tooSlow: "Too slow here? A cloud model makes this in minutes, for a price you see first.",
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
  /** Under the prompt while Create waits for an OpenRouter picture model's price, which says
   *  which extra settings it takes; `failed` once that request failed. */
  holdingExtra: (failed: boolean) =>
    failed
      ? "Your extra settings wait for this model's price, which could not be got. Press Get the price to ask again."
      : "Your extra settings wait for this model's price, which says which ones it takes.",
} as const;

/** The backend's names for added pictures in its refusals (`errors::pictures`): as
 *  `PICTURE_NOUNS`, but a reference is a "reference picture". */
const NEEDED_NOUNS: Record<RefRole, readonly [string, string]> = {
  ...PICTURE_NOUNS,
  reference: ["reference picture", "reference pictures"],
};

/** Studio's composer (0.2.0 Task U3, design 10.1): the prompt, the pictures in front of it, and
 *  the toolbar with the model chip, Shape, How many, a video's Length, Settings and Create. */
export const STUDIO_COMPOSER = {
  region: (kind: MediaKind) => (kind === "video" ? "Make a video" : "Make a picture"),
  prompt: "Prompt",
  placeholder: (kind: MediaKind) => (kind === "video" ? "Describe the video you want…" : "Describe the picture you want…"),
  toolbar: "Picture settings",
  shape: "Shape",
  count: "How many",
  length: "Length",
  settings: "Settings",
  addPicture: "Add picture",
  /** A picture's Remove button, named after it: "Remove picture 1", "Remove mask". */
  remove: (name: string) => `Remove ${SETTING_WORDS.midSentence(name)}`,
  required: "required",
  maskNeedsSource: "Add the picture to change before its mask.",
  /** An input picture's empty zone: "Add the picture to change", "Add the mask". */
  addInput: (label: string) => `Add the ${SETTING_WORDS.midSentence(label)}`,
  /** Why Create waits for a required picture, in the backend's words
   *  (`errors::invalid_sentences`): "This model needs at least 1 picture to change." */
  needsPictures(role: RefRole, min: number): string {
    const [one, many] = NEEDED_NOUNS[role];
    return `This model needs at least ${min} ${min === 1 ? one : many}.`;
  },
  /** The model chip's dot, whose tooltip says where the prompt and pictures go. */
  privacy: "Where your prompt goes",
  /** A menu's trigger, named with its value: "Shape: 1:1", "How many: 2". */
  menuValue: (name: string, value: string) => `${name}: ${value}`,
  /** How many, on its trigger: "×2". */
  times: (n: string) => `×${n}`,
} as const;

/** Studio's side panel (0.2.0 Task U8, design 10.1): its title and its sections. Shape and How
 *  many are never in it: they are the composer's. */
export const STUDIO_PANEL = {
  title: "Settings",
  setups: "My setups",
  advanced: "Advanced",
  loras: "LoRAs",
  files: "Files",
} as const;

/** Studio's top bar (0.2.0 Task U3). */
export const STUDIO_TOPBAR = {
  getModels: "Get models",
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
  /** `usd` is the amount asked from while that switch is on; `null` when it is off. */
  dontAskVideos: (usd: string | null) =>
    usd === null ? "Don't ask again before every video" : `Don't ask again for videos under ${usd}`,
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
  /** The LoRAs a local run attached: "LoRAs: pixel art ü 0.8, style 1" (each `loraUse`). */
  loras: (uses: readonly string[]) => `LoRAs: ${uses.join(", ")}`,
  loraUse: (name: string, weight: string) => `${name} ${weight}`,
  /** A LoRA whose row is gone, named by nothing else. */
  loraGone: "a removed LoRA",
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
  /** While *Load it again* loads the chat model (it waits while another local job holds the
   *  graphics card). */
  loadingChat: "Loading the chat model…",
  alongside: "Chat replies may be slower while this runs",
  onProcessor: "Running on the processor: slower, and chat will be slow while this runs.",
  retrying: "Ran out of graphics memory, trying again with memory saving (slower)",
  stalled: "No progress for 10 minutes",
  keepWaiting: "Keep waiting",
  /** After the engine's or a damaged part's download started from the card. */
  engineDownloading: "The Studio engine is downloading. Press Run again when it is done.",
  repairing: "The model's damaged parts are downloading again. Press Run again when it is done.",
  /** After *Download again* on a model whose files are all on this computer: checked at once. */
  repairedNow: "The model's files were checked and are whole. Press Run again.",
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

/** The most characters of a prompt a tile's label reads out. */
const TILE_PROMPT_CHARS = 72;

/** `prompt` cut for a tile's label: on one line, at most `TILE_PROMPT_CHARS` characters (never
 *  half of one), with "…" when it was longer. A prompt may hold 32,000 characters. */
function tilePrompt(prompt: string): string {
  const line = prompt.replace(/\s+/gu, " ").trim();
  const chars = Array.from(line);
  return chars.length <= TILE_PROMPT_CHARS ? line : `${chars.slice(0, TILE_PROMPT_CHARS).join("").trimEnd()}…`;
}

/** A result's tile, on a card and in the grid (Task B9). */
export const STUDIO_TILE = {
  /** The tile itself, which opens the viewer: "Open picture 2 of 4: A lighthouse…" (the prompt
   *  cut to its start). */
  open: (kind: MediaKind, n: number, of: number, prompt: string) =>
    tilePrompt(prompt) ? `Open ${resultName(kind, n, of)}: ${tilePrompt(prompt)}` : `Open ${resultName(kind, n, of)}`,
  /** Its box in the grid, to select it with others (the prompt cut the same way). */
  select: (kind: MediaKind, n: number, of: number, prompt: string) =>
    tilePrompt(prompt) ? `Select ${resultName(kind, n, of)}: ${tilePrompt(prompt)}` : `Select ${resultName(kind, n, of)}`,
  /** Its star, on hover and focus (always shown while starred): "Star picture 2 of 4"
   *  (pressed while starred). */
  star: (kind: MediaKind, n: number, of: number) => `Star ${resultName(kind, n, of)}`,
  /** Its Delete in the grid, on hover and focus: "Delete picture 2 of 4". */
  delete: (kind: MediaKind, n: number, of: number) => `Delete ${resultName(kind, n, of)}`,
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
  saveSome: (n: number) => `Save ${n}…`,
  clear: "Clear selection",
} as const;

/** Studio's results (0.2.0 Task U9, design 10.4): the grid's Select mode, which puts a box on
 *  every tile and a bar of actions over the selection, and the empty state without a model on
 *  this computer, with its one action. */
export const STUDIO_RESULTS = {
  select: "Select",
  selected: (n: number) => `${n} selected`,
  done: "Done",
  /** Only while nothing at all is installed on this computer. */
  noModels: "No models on this computer yet",
  /** Models are installed, but none of the tab's kind (U9 fix round 1). */
  noModelsOfKind: {
    image: "No picture models on this computer yet",
    video: "No video models on this computer yet",
  } satisfies Record<MediaKind, string>,
  getModels: "Get models",
  deleteSelected: (n: number) => `Delete ${n}`,
  /** The first-use card's button, which opens the model that fits in its detail panel:
   *  "Get FLUX.2 klein 4B". */
  getModel: (name: string) => `Get ${name}`,
  /** The bar of actions on the selection, for a screen reader. */
  selection: "Selection",
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
  /** Again, or Run again, on such a run: what to do instead. */
  endpointGoneNext: "Choose a model from the model chip under the prompt, then press Create.",
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
  /** Before the path a result was saved to (shown left to right, invisible characters as
   *  codes): "Saved to /home/me/Pictures". */
  savedTo: "Saved to",
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
  /** The picture and video models installed here and the disk they take ("14.2 GB"). */
  models: (count: number, bytes: number) =>
    count === 0 ? "Picture and video models: none yet" : `Picture and video models: ${count} (${formatBytes(bytes)})`,
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
  /** What the gallery holds and where: "Studio uses 1.3 GB in ~/harnessUI/studio (312
   *  pictures, 14 videos)". Sizes are `formatBytes`'s, as every size Studio shows. */
  usage(bytes: number, folder: string, pictures: number, videos: number): string {
    const size = bytes > 0 ? formatBytes(bytes) : "0 MB";
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
  /** Under the heading: all four are off until turned on. */
  spendingNote: "Each is off until you turn it on. Prices are always shown before a cloud run.",
  confirmVideo: "Ask before every video",
  /** Before an amount box, which takes US dollars. */
  dollar: "$",
  /** The switch that asks from an amount; its box (`confirmAtAmount`) shows while it is on. */
  confirmAt: "Ask before runs of a set amount or more",
  confirmAtAmount: "Ask before runs of this many US dollars or more",
  orMore: "or more",
  confirmAtRange: "Choose an amount from $0.25 to $100.",
  confirmUnknown: "Ask when the price is unknown",
  confirmUnknownHelp: "Some models do not say what a run costs before it is sent.",
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
  withUseRules: "Commercial use allowed, with use rules",
  unclear: "Check the licence",
  no: "Non-commercial",
} as const satisfies Record<Licence["commercial"], string | ((limit: string | null) => string)>;

/** Commercial-use wording for a licence with excluded regions. */
export const REGIONAL_USE = "Commercial use in permitted regions";

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

/** Get models' cards (Task B13; the view's own words are `GET_MODELS`, Task U5, and the detail
 *  panel's `MODEL_DETAIL`, Task U6): a model's time, its download and its deletion, and the
 *  footer's notice. */
export const LOCAL_MODELS = {
  download: "Download",
  /** A time line without an estimate (nothing measured here, no first guess). */
  noTime: STUDIO_TIME.notYet.charAt(0).toUpperCase() + STUDIO_TIME.notYet.slice(1),
  /** The bar of a model's download on its card, while it runs. */
  downloading: (name: string) => `Downloading ${name}`,
  /** After *Repair*: what happens now. */
  repairing: "Its files are being checked; anything missing or damaged downloads again, with its progress here.",
  /** After *Repair* on a model whose files are all on this computer: nothing downloads. */
  repairedNow: "Its files were checked: they are all on this computer and whole.",
  deleteTitle: (name: string) => `Delete ${name}?`,
  deleteBody:
    "Its files are removed from this computer, except parts another model still uses. Pictures and videos you made stay in the gallery.",
  /** A model that uses a file of the person's own where it is (*Use it where it is*, or a file
   *  chosen from this computer): Studio never deletes that file. */
  deleteBodyOwn:
    "Studio forgets this model and removes the files it downloaded or copied, except parts another model still uses. Your own file stays where it is. Pictures and videos you made stay in the gallery.",
  /** A model copied in with *Add a model file*: Studio's copy goes, the file it came from stays. */
  deleteBodyCopy:
    "Studio's copy of its file and the files it downloaded are removed, except parts another model still uses. The file you added it from stays where it is. Pictures and videos you made stay in the gallery.",
  cancel: "Cancel",
  showInFolder: "Show in folder",
  notice: "Local models run only on this computer; no company checks prompts or results.",
  whatThatMeans: "What that means",
} as const;

/** A model version's name as people know it (Task S6), never the engine's word for it. */
export const VERSION_NAME: Record<SdVersion, string> = {
  sd1: "SD 1.5",
  "sd1-inpaint": "SD 1.5 Inpainting",
  "sd1-pix2pix": "InstructPix2Pix",
  "sd1-tiny-unet": "SD 1.5 Tiny",
  sd2: "SD 2.1",
  "sd2-inpaint": "SD 2 Inpainting",
  "sd2-tiny-unet": "SD 2 Tiny",
  "sdxs-512-ds": "SDXS-512",
  "sdxs-09": "SDXS-09",
  sdxl: "SDXL",
  "sdxl-inpaint": "SDXL Inpainting",
  "sdxl-pix2pix": "CosXL Edit",
  "sdxl-vega": "Segmind Vega",
  "sdxl-ssd1b": "Segmind SSD-1B",
  svd: "Stable Video Diffusion",
  sd3: "SD 3.5",
  flux: "FLUX.1",
  "flux-fill": "FLUX.1 Fill",
  "flux-controls": "FLUX.1 Canny or Depth",
  "flex-2": "Flex.2",
  "chroma-radiance": "Chroma Radiance",
  wan2: "Wan 2.1",
  "wan2.2-i2v": "Wan 2.2 Image-to-Video",
  "wan2.2-ti2v": "Wan 2.2 5B",
  "wan2.2-s2v": "Wan 2.2 Speech-to-Video",
  "lingbot-video": "LingBot-Video",
  "qwen-image": "Qwen-Image",
  "qwen-image-layered": "Qwen-Image Layered",
  "qwen-image-2.1": "Qwen-Image 2.1",
  "hunyuan-video": "HunyuanVideo 1.5",
  anima: "Anima",
  flux2: "FLUX.2",
  "flux2-klein": "FLUX.2 klein",
  ltxav: "LTX-2.3",
  "minimax-h3": "MiniMax H3",
  "hidream-o1": "HiDream-O1",
  "z-image": "Z-Image",
  "boogu-image": "Boogu Image",
  "ovis-image": "Ovis-Image",
  "ernie-image": "ERNIE-Image",
  lens: "Lens",
  minit2i: "MiniT2I",
  longcat: "LongCat-Image",
  pid: "PiD",
  ideogram4: "Ideogram 4",
  "sefi-image": "SeFi-Image",
  krea2: "Krea 2",
  "mage-flow": "MageFlow",
  "sensenova-u1.5": "SenseNova U1.5",
  "llada-image": "LLaDA-Image",
  pixart: "PixArt",
  "ming-image": "Ming-Image",
  "z-image-l2p": "Z-Image L2P",
  iris: "Iris",
  flux3: "FLUX.3",
  "moss-image": "MOSS-Image",
  fibo2: "FIBO 2",
  chroma: "Chroma",
  "p-h3-image": "P-H3 Image",
  "minimax-h3-image": "MiniMax H3 Image",
};

/** The model type of the recipes whose type is not their version's name. */
export const RECIPE_TYPE_NAME: Record<string, string> = {
  "wan22-t2v": "Wan 2.2",
  "wan22-i2v": "Wan 2.2",
  "flux1-kontext": "FLUX.1 Kontext",
  "qwen-image-edit": "Qwen-Image Edit",
  "qwen-image-edit-2509": "Qwen-Image Edit",
  "qwen-image-edit-2511": "Qwen-Image Edit",
  "ltx25-dev": "LTX-2.5",
};

/** A model's type in words: its recipe's own type where it has one, else its version's name
 *  ("Wan 2.2" for the A14B recipe, though the engine reads it as Wan 2.1). A version this
 *  build has no name for (a newer backend's) reads "Unknown". */
export function typeName(version: SdVersion, recipe: string | null): string {
  return (recipe === null ? undefined : RECIPE_TYPE_NAME[recipe]) ?? VERSION_NAME[version] ?? "Unknown";
}

/** What a model makes, as a file's label says it ("SDXL picture model"). */
export const KIND_WORD: Record<RecipeKind, string> = {
  picture: "picture",
  pictureEdit: "picture edit",
  video: "video",
};

/** What a file does in a model's set-up, as its plan lists it (design 7.4). */
export const SLOT_LABEL: Record<PartRole, string> = {
  model: "Model",
  diffusion: "Diffusion model",
  "high-noise-diffusion": "Diffusion model (high-noise part)",
  "uncond-diffusion": "Diffusion model (unconditional)",
  "clip-l": "Text encoder (CLIP-L)",
  "clip-g": "Text encoder (CLIP-G)",
  t5xxl: "Text encoder (T5-XXL)",
  llm: "Text encoder (language model)",
  "llm-vision": "Picture encoder (language model)",
  "clip-vision": "Picture encoder (CLIP vision)",
  tokenizer: "Tokenizer",
  vae: "Decoder (VAE)",
  tae: "Tiny decoder (TAE)",
  "audio-vae": "Sound decoder",
  "audio-encoder": "Sound encoder",
  "embeddings-connectors": "Embeddings connectors",
};

/** A model's parameters as a label says them: "14B", "2.6B", "860M". */
export function parameterWords(parameters: number): string {
  if (parameters >= 1e9) {
    const billions = parameters / 1e9;
    return `${billions >= 10 ? Math.round(billions) : Math.round(billions * 10) / 10}B`;
  }
  if (parameters >= 1e6) return `${Math.round(parameters / 1e6)}M`;
  return String(parameters);
}

/** Why Studio can't run a file, as one sentence; `ext` names a pickle file's extension. */
const CANNOT_RUN_WORDS: Record<CannotRunReason, (ext: string) => string> = {
  lumina2: () => "This is a Lumina 2 model, which the Studio engine can't run",
  ltxVideo09: () => "This is an LTX-Video 0.9 model, which the Studio engine can't run",
  hunyuanVideo1: () => "This is a HunyuanVideo 1 model, which the Studio engine can't run",
  svd: () => "This is a Stable Video Diffusion model, which the Studio engine can't run",
  unknownWeights: () => "The Studio engine doesn't recognise this model",
  sharded: () => "This model is split into several files, which Studio can't put back together",
  pickle: (ext) => `Studio can't read ${ext} files`,
  noSha: () => "Hugging Face publishes no checksum for this file, so Studio can't check it",
  adapter: () => "This is an add-on file (such as a ControlNet or an IP-Adapter), which Studio doesn't use",
  needsControlFrames: () => "This model needs control video frames, which Studio can't give it",
  needsAudio: () => "This model needs a sound file, which Studio can't give it",
  upscaler: () => "This is an upscaler, not a picture or video model",
  folderModel: () => "This model comes as a folder of parts, which the Studio engine can't load",
  notThisEngine: () => "The Studio engine can't run this kind of model",
  ltx2: () => "This is an LTX-2 model, which the Studio engine can't run",
};

/** Why a file of a repository was not read, as one sentence. */
const NOT_READ_WORDS: Record<NotReadReason, string> = {
  needsToken: "Add a Hugging Face token in Settings to read this file",
  needsTerms: "Accept the model's terms on Hugging Face first",
  budget: "Not read: Studio reads 24 files per repository",
  failed: "Couldn't read this file",
};

/** What a file of a repository is, as its row says it (design 7.3). */
export const FILE_LABEL = {
  /** "Works in Studio · SDXL picture model", "Works in Studio · Wan 2.2 video model, 14B,
   *  high-noise part" (`parameters` 0 says no size). */
  works(type: string, kind: RecipeKind, part: "high" | "low" | null, parameters: number): string {
    const details = [parameters > 0 ? parameterWords(parameters) : null, part === null ? null : `${part}-noise part`];
    const more = details.filter((d) => d !== null).map((d) => `, ${d}`);
    return `Works in Studio · ${type} ${KIND_WORD[kind] ?? KIND_WORD.picture} model${more.join("")}`;
  },
  /** "LoRA for FLUX.1"; without a known base, "LoRA for an unknown model type". */
  lora: (type: string | null) => (type === null ? "LoRA for an unknown model type" : `LoRA for ${type}`),
  /** "Extra file · Decoder (VAE)". */
  extra: (role: PartRole) => `Extra file · ${SLOT_LABEL[role] ?? "another part"}`,
  /** "Can't run in Studio: Studio can't read .ckpt files" (`ext`, with its dot, for a pickle
   *  file). */
  cannotRun: (reason: CannotRunReason, ext = ".ckpt") =>
    `Can't run in Studio: ${(CANNOT_RUN_WORDS[reason] ?? CANNOT_RUN_WORDS.unknownWeights)(ext)}`,
  notRead: (reason: NotReadReason) => NOT_READ_WORDS[reason] ?? NOT_READ_WORDS.failed,
} as const;

/** A pickle file's extension, from its path ("model.CKPT" → ".ckpt"). */
function extensionOf(path: string): string {
  const name = path.split("/").pop() ?? path;
  const dot = name.lastIndexOf(".");
  return dot < 0 ? ".ckpt" : name.slice(dot).toLowerCase();
}

/** A file's label in words, for its row (`path` names a pickle file's extension). */
export function fileLabelWords(label: FileLabel, path: string): string {
  if ("works" in label) {
    const w = label.works;
    return FILE_LABEL.works(typeName(w.version, w.recipe), w.kind, w.part, w.parameters);
  }
  if ("lora" in label) return FILE_LABEL.lora(label.lora.base === null ? null : typeName(label.lora.base, null));
  if ("extra" in label) return FILE_LABEL.extra(label.extra.role);
  if ("cannotRun" in label) return FILE_LABEL.cannotRun(label.cannotRun.reason, extensionOf(path));
  return FILE_LABEL.notRead(label.notRead.reason);
}

/** What a search result offers, in words; `null` when its name says nothing Studio knows. */
export function verdictWords(verdict: StudioVerdict): string | null {
  if (verdict === "unknown") return null;
  if ("works" in verdict)
    return FILE_LABEL.works(typeName(verdict.works.version, verdict.works.recipe), verdict.works.kind, null, 0);
  if ("lora" in verdict) return FILE_LABEL.lora(verdict.lora.base === null ? null : typeName(verdict.lora.base, null));
  return FILE_LABEL.cannotRun(verdict.cannotRun.reason);
}

/** "username and email", "username, email and company". */
function andList(items: readonly string[]): string {
  return items.length <= 1 ? (items[0] ?? "") : `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

/** Whose files a download's licence verdict is about, by the model's kind. */
const VERDICT_LEAD = { image: "Pictures you make", video: "Videos you make" } as const satisfies Record<MediaKind, string>;

/** What the review before a local download says (0.1.0 Task B14's confirmation; since 0.2.0
 *  Task U6, the detail panel, whose own words are `MODEL_DETAIL`): a part already here, what
 *  you make may be used for, the Studio engine, a gated repository, an excluded region, low
 *  disk space, and a page the browser did not open. */
export const DOWNLOAD_LOCAL = {
  /** A part another model brought already: checked, not downloaded again. */
  alreadyHere: "already on this computer",
  /** The verdict as one line: "Pictures you make: you may use them commercially". */
  verdictLine: (kind: MediaKind, allowed: string) => `${VERDICT_LEAD[kind]}: ${allowed}`,
  /** What the pictures or videos you make may be used for, by the strictest licence (the
   *  detail panel's `verdict`): "Pictures you make: free for any use, including commercial." */
  verdict: {
    yes: "free for any use, including commercial.",
    underRevenue: (limit: string | null) =>
      limit === null ? "commercial use only under a revenue limit." : `commercial use only under ${limit} revenue.`,
    registration: "commercial use only after registering with the publisher.",
    withUseRules: "commercial use allowed, with the licence's use rules.",
    unclear: "check the licence before any commercial use.",
    no: "non-commercial use only.",
  } as const satisfies Record<Licence["commercial"], string | ((limit: string | null) => string)>,
  /** The Studio engine comes with the model (Windows): "(30 MB, MIT licence)", from
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
  lowDisk: "This leaves less than 10 GB free on this disk.",
  /** A licence or model page the browser did not open, and where to find it instead. */
  notOpened: "The page could not be opened in your browser.",
  goTo: (url: string) => `Go to ${url} in your browser.`,
  excludedRegions: (licence: string, regions: string[]) => `${licence} excludes ${andList(regions)}.`,
} as const;

/** LoRAs, in the composer's LoRA section and in Get models (Phase U). */
export const LORAS = {
  title: "LoRAs",
  add: "Add",
  remove: "Remove",
  strength: "Strength",
  highNoise: "High-noise part",
  mayNotFit: "may not fit this model",
  none: "No LoRAs yet",
  /** The heading over the LoRAs whose model type is not known: they are not judged. */
  unknownType: "LoRA for an unknown model type",
  /** An attached LoRA's strength slider, named after it (Task U8). */
  strengthOf: (name: string) => `Strength of ${name}`,
  /** An attached LoRA's Remove button, named after it. */
  removeOf: (name: string) => `Remove ${name}`,
  getLoras: "Get LoRAs",
  /** The composer's LoRAs button, with how many the draft attaches: "LoRAs (1)". */
  lorasButton: (n: number) => (n > 0 ? `LoRAs (${n})` : "LoRAs"),
  /** A LoRA whose file is gone: *Add* greys it out, and an attached one says so. */
  missing: "Its file is gone from this computer.",
} as const;

/** The model's files in Studio's side panel (Task U8, design 7.5): *Change* for a part, with a
 *  file on this computer, from disk or from a Hugging Face repository. *Change*, *Model type*
 *  and "untested set-up" are `SETUP`'s. */
export const FILES = {
  installedFile: "A file on this computer",
  diskFile: "A file from disk…",
  hubFile: "A file from Hugging Face…",
  repoLabel: "Repository",
  needsSetup: "Choose the model type and its files to use this model.",
  /** The Model type menu of a model that has none yet. */
  chooseType: "Choose a type",
  /** *A file on this computer* when no other installed model has a file for the part. */
  noInstalled: "No other file on this computer for this part",
  /** An installed file, by the model it belongs to: "from sd_xl_base_1.0". */
  fromModel: (model: string) => `from ${model}`,
  /** The Hugging Face dialog's title: "Decoder (VAE) from Hugging Face". */
  hubTitle: (part: string) => `${part} from Hugging Face`,
  repoHint: "owner/name",
  showFiles: "Show its files",
  readingRepo: "Reading the repository…",
  noFiles: "This repository has no file Studio can use for this part.",
  useFile: "Use this file",
  cancel: "Cancel",
  /** While the model's files are read (nothing is sent). */
  reading: "Reading the model's files…",
  /** While the model's own download runs: why its files cannot be changed meanwhile. */
  downloading: "Its files are downloading. You can change them again once the download is done.",
} as const;

/** Adding a model or LoRA file from this computer (Phase U's dialog, Task U4). */
export const IMPORT = {
  addModel: "Add a model file",
  addLora: "Add a LoRA file",
  copy: "Copy it into Studio's models folder",
  inPlace: "Use it where it is",
  reading: "Reading the file…",
  copying: "Copying…",
  title: "Add a model file",
  /** The extra files a model file's set-up fetches: "Also downloads 335 MB from Hugging Face". */
  alsoDownloads: (size: string) => `Also downloads ${size} from Hugging Face`,
  add: "Add",
  done: "Added",
  /** The button that closes the dialog once the file is added. */
  finish: "Done",
  /** Before the file chooser answers. */
  choose: "Choose a .safetensors, .sft or .gguf file.",
  chooseAnother: "Choose another file",
  /** The question of how a file from outside the models folder is added. */
  mode: "How to add it",
  /** The extra files of a model file's set-up, under this heading. */
  extraFiles: "Extra files",
  /** A LoRA file, whichever button chose it. */
  asLora: "It is added to your LoRAs.",
  /** How far adding it is: "1.2 GB of 6.9 GB". */
  amount: (done: string, total: string) => `${done} of ${total}`,
  /** Added, while the files its set-up fetches download. */
  fetching: "Its extra files are downloading; the model is ready when they finish.",
  cancel: "Cancel",
  /** While the plan of the model type chosen is made. */
  planning: "Updating the files…",
  /** A file Studio can't place (design 7.5): what *Set it up yourself* does. */
  byHand:
    "If you know what model this is, set it up yourself: choose its type, and Studio adds the files it needs. It runs as an untested set-up.",
} as const;

/** The codes of what can stop a set-up (`studio_setup_plan`, a download, *Add*): plain words
 *  for each, so none shows as its code. */
export type SetupProblem = "hf_needs_token" | "hf_needs_terms" | "studio_setup_incomplete";

/** A model's set-up: its labels and the problems it can meet (Phase U's set-up sheet). */
export const SETUP = {
  modelType: "Model type",
  files: "Files",
  change: "Change",
  untested: "untested set-up",
  yourself: "Set it up yourself",
  anyway: "Download file anyway",
  problem: {
    hf_needs_token: "This model is gated. Add a Hugging Face token in Settings → Hugging Face first.",
    hf_needs_terms: "Accept this model's terms on its Hugging Face page first.",
    studio_setup_incomplete: "Choose a file for each part first.",
  } satisfies Record<SetupProblem, string>,
  /** Where a slot's file comes from, in the set-up's list. */
  source: {
    main: "the model file",
    sibling: "next to the model file",
    installed: "from a model already installed",
    repo: "from the repository",
    companion: "the usual one for this model type",
    chosen: "chosen by you",
    embedded: "inside the model file",
    folder: "in the model's folder",
    missing: "no file yet",
  } satisfies Record<PartSource, string>,
} as const;

/** The set-up problem a refusal's `code` names, so it is said in `SETUP.problem`'s words; `null`
 *  for any other code. Only the group's own keys count ("toString" is not one). */
export function setupProblem(code: string | null | undefined): SetupProblem | null {
  return typeof code === "string" && Object.hasOwn(SETUP.problem, code) ? (code as SetupProblem) : null;
}

/** A model that may not run well here: a warning, never a refusal (Phase U's sheet, composer
 *  and cards). */
export const LOCAL_WARNING = {
  mayNotFit: (reason: string) => `May not fit this computer: ${reason}. It may fail or be very slow.`,
  videoOnProcessor: "Video on the processor is very slow",
  /** In the update box while a local job runs: installing stops it (design 9.2). */
  updateStopsIt: "Installing stops the picture being made",
} as const;

/** When a Hugging Face repository was last updated, as a result card says it: "today",
 *  "yesterday", "3 days ago", "2 weeks ago", "4 months ago", "a year ago". A time not yet come
 *  (a clock running behind) reads "today". */
export function updatedWhen(at: number, now: number = Date.now()): string {
  const days = Math.floor(Math.max(0, now - at) / 86_400_000);
  if (days < 1) return "today";
  if (days === 1) return "yesterday";
  if (days < 7) return `${days} days ago`;
  const ago = (n: number, unit: string) => (n === 1 ? `a ${unit} ago` : `${n} ${unit}s ago`);
  if (days < 30) return ago(Math.floor(days / 7), "week");
  if (days < 365) return ago(Math.floor(days / 30), "month");
  return ago(Math.floor(days / 365), "year");
}

/** Get models, Studio's full view (Task U5, design 10.3): its header with this computer's
 *  chips, the Hugging Face search with its filters, and the sections without a query. */
export const GET_MODELS = {
  title: STUDIO_TOPBAR.getModels,
  subtitle: "Picture and video models that run on this computer: free, and private.",
  back: "Back",
  /** The name of the header's chips. */
  thisComputer: "This computer",
  /** The graphics card and its memory: "Radeon RX 7600 · 8 GB graphics memory". */
  gpu: (name: string, gb: string) => `${name} · ${gb} graphics memory`,
  /** Apple silicon, whose memory is shared: "Apple M5 · 16 GB shared memory". */
  shared: (name: string, gb: string) => `${name} · ${gb} shared memory`,
  processorOnly: "Processor only",
  ram: (gb: string) => `${gb} memory`,
  freeDisk: (gb: string) => `${gb} free`,
  addModel: IMPORT.addModel,
  addLora: IMPORT.addLora,
  searchLabel: "Search Hugging Face",
  searchPlaceholder: "Search Hugging Face: SDXL, Flux, Wan 2.2, or a repo name",
  search: "Search",
  searchSends: "Searching sends your words to huggingface.co.",
  /** The name of the Pictures · Video · LoRAs chips. */
  filters: MODEL_SHEET.filters,
  pictures: STUDIO_TABS.image,
  video: STUDIO_TABS.video,
  loras: LORAS.title,
  fitsOnly: "Fits this computer",
  recommended: "Recommended for this computer",
  popular: "Popular on Hugging Face",
  here: "On this computer",
  results: "Found on Hugging Face",
  searching: "Searching…",
  noResults: {
    pictures: "Hugging Face found no picture models for that.",
    video: "Hugging Face found no video models for that.",
    loras: "Hugging Face found no LoRAs for that.",
  } satisfies Record<SearchFilter, string>,
  /** Without a query, no catalog model of the kind is left (*Fits this computer* hid them). */
  noneFit: {
    image: "No picture model in Studio's list fits this computer.",
    video: "No video model in Studio's list fits this computer.",
  } satisfies Record<MediaKind, string>,
  videoNote: "Local video is slower and simpler than cloud video, but free and private.",
  nothingHere: "No models on this computer yet",
  noLorasHere: LORAS.none,
  retry: "Try again",
  /** More of the Popular list than its first row or two. */
  showAll: (n: number) => `Show all ${n}`,
  showFewer: "Show fewer",
  /** The catalog's pick for this computer, on its card. */
  topPick: "Top pick",
  installed: "Installed",
  download: LOCAL_MODELS.download,
  repair: "Repair",
  repairHint: "Checks its files and downloads again anything missing or damaged.",
  delete: "Delete",
  /** A result card's figures: "48.2k downloads", "312 likes". */
  downloads: (n: number) => (n === 1 ? "1 download" : `${formatCount(n)} downloads`),
  likes: (n: number) => (n === 1 ? "1 like" : `${formatCount(n)} likes`),
  /** "updated 3 weeks ago", from `updatedWhen`. */
  updated: (date: string) => `updated ${date}`,
  /** A gated model's next step: Settings → Hugging Face, where the token goes. */
  openSettings: "Open Settings",
  /** Where a LoRA came from. */
  loraFrom: (repo: string) => `From ${repo}`,
  loraAdded: "Added from this computer",
  loraMissing: LORAS.missing,
  deleteLoraTitle: (name: string) => `Delete ${name}?`,
  deleteLoraBody: "Its file is removed from this computer. Pictures and videos you made with it stay in the gallery.",
} as const;

/** What Studio can do with a model it does not run, as a badge says it in short. */
const CANT_RUN_SHORT: Record<CannotRunReason, string> = {
  lumina2: "Lumina 2",
  ltxVideo09: "LTX-Video 0.9",
  hunyuanVideo1: "HunyuanVideo 1",
  svd: "Stable Video Diffusion",
  unknownWeights: "unknown model",
  sharded: "split files",
  pickle: "unreadable files",
  noSha: "no checksum",
  adapter: "add-on file",
  needsControlFrames: "needs control video",
  needsAudio: "needs sound",
  upscaler: "upscaler",
  folderModel: "folder of parts",
  notThisEngine: "unsupported type",
  ltx2: "LTX-2",
};

/** A Hugging Face result's Studio verdict, as its badge says it (Task U5, design 10.3): from
 *  its name until it is opened (`hint`), exact once its files were read. */
export const VERDICT = {
  /** "Works in Studio · SDXL". */
  works: (type: string) => `Works in Studio · ${type}`,
  /** "LoRA for FLUX.1"; without a known model type, "LoRA for an unknown model type". */
  lora: (type: string | null) => (type === null ? LORAS.unknownType : `LoRA for ${type}`),
  /** "Can't run: Lumina 2", in short; its badge's tooltip has the whole sentence. */
  cantRun: (reason: CannotRunReason) => `Can't run: ${CANT_RUN_SHORT[reason] ?? CANT_RUN_SHORT.unknownWeights}`,
  /** Its name says nothing Studio knows: opening it reads its files. */
  unknown: "Open to check",
  /** Opened, and none of its files is one Studio knows. */
  notRecognised: "Not recognised",
  /** A hint's tooltip. */
  hint: "From the name: opening it reads the files",
} as const;

/** A result's or a catalog model's detail panel in Get models (Task U6, design 10.3): the review
 *  before a download, with its main file (one per quantisation, the best for this computer
 *  marked), the extra files Studio adds, the licences, the total and one Download; then the
 *  files Studio can't run and the LoRAs. */
export const MODEL_DETAIL = {
  mainFile: "Main file",
  best: "Best for this computer",
  extras: "Extra files Studio adds",
  alreadyHere: "already on this computer",
  chooseFile: "Choose a file",
  /** "Total to download: 4.6 GB": the files not on this computer yet (and the Studio engine
   *  when it comes with the model). */
  total: (size: string) => `Total to download: ${size}`,
  download: "Download",
  /** Why Download is off while a part has no file. */
  missingWhy: SETUP.problem.studio_setup_incomplete,
  /** Why Download is off for a gated model while no Hugging Face token is saved. */
  needsToken: SETUP.problem.hf_needs_token,
  downloadLora: "Download LoRA",
  /** While the repository is read on opening. */
  reading: "Reading the repository on huggingface.co…",
  /** Who published a result: "by leejet". */
  by: (author: string) => `by ${author}`,
  licences: "Licences",
  /** A repository that names no licence at all. */
  noLicence: `This repository names no licence. ${LICENCE_BADGE.unclear} before you use what you make.`,
  /** A part with no file yet. */
  noFile: "No file yet",
  /** A part the person chose from this computer: "Chosen by you · t5xxl.safetensors". */
  chosen: (name: string) => `Chosen by you · ${name}`,
  /** A repository with no model Studio can set up (its files are listed below). */
  noSetup: "Studio can't set this repository up. Its files can still be downloaded one at a time, for another app.",
  /** A repository with a file Studio can't place but may run (design 7.5). */
  noSetupByHand:
    "Studio can't set this repository up by itself. Set one up yourself, choosing its type, or download a file anyway, for another app.",
  /** The *Set it up yourself* section's lead: which file it sets up. */
  byHandFor: (file: string) => `Set up ${file} as:`,
  otherFiles: "Other files in this repository",
  openFiles: "View files on Hugging Face",
  /** A file with no published checksum: the browser downloads it instead. */
  browserDownload: "Download file in browser",
  queued: "Queued in Models → Downloads.",
  installed: GET_MODELS.installed,
  /** An amber licence's duty: "Pass the use restrictions on." reads "The licence asks you to
   *  pass the use restrictions on." */
  duty: (obligation: string) => `The licence asks you to ${SETTING_WORDS.midSentence(obligation)}`,
  /** While a new plan is asked for, after a change of model type or a file chosen. */
  planning: "Updating the files…",
} as const;

/** Studio's downloads (Task U7, design 10.3): a model's or a LoRA's download on its card, in the
 *  detail panel and in the top bar's indicator, with Pause or Resume and Cancel, and *Use it*
 *  once a model is ready. Every size is `formatBytes`'s. */
export const STUDIO_DOWNLOADS = {
  /** The top bar's indicator while downloads run: "2 downloading". */
  indicator: (n: number) => `${n} downloading`,
  /** The indicator once none runs, while finished ones are listed: "1 ready". */
  indicatorReady: (n: number) => `${n} ready`,
  /** The indicator's list. */
  title: "Downloads",
  /** Takes the finished downloads off the indicator's list. */
  clear: "Clear finished",
  /** How far a download got: "2.4 GB of 6.7 GB · 2 min left"; `left` is the time left, or
   *  what the download is doing ("Paused"), or nothing. */
  progress: (done: string, total: string, left: string | null) =>
    left === null ? `${done} of ${total}` : `${done} of ${total} · ${left}`,
  /** What a download that is not simply downloading is doing, after its sizes. */
  status: {
    queued: "Waiting",
    downloading: null,
    paused: "Paused",
    verifying: "Checking the files…",
    done: "Ready",
    error: "Stopped",
    cancelled: "Cancelled",
    cancelling: "Cancelling…",
  } satisfies Record<DownloadStatus, string | null>,
  /** A file already on this computer, read through before the rest downloads. */
  checking: "Checking the file…",
  /** A file without a published checksum is part of it. */
  unverified: "no published checksum",
  /** The buttons, named for the model or LoRA. */
  pause: (name: string) => `Pause downloading ${name}`,
  resume: (name: string) => `Resume downloading ${name}`,
  cancel: (name: string) => `Cancel downloading ${name}`,
  /** Their tooltips. */
  tip: { pause: "Pause", resume: "Resume", cancel: "Cancel" },
  /** A finished model chosen in the composer: its tab, back at the results. */
  useIt: "Use it",
  ready: "Ready",
  /** What a download in the indicator's list is. */
  lora: "LoRA",
} as const;

/** A result's verdict badge in words: exact (`exact`) or its name's hint. */
export function verdictBadgeWords(verdict: StudioVerdict, exact: boolean): string {
  if (verdict === "unknown") return exact ? VERDICT.notRecognised : VERDICT.unknown;
  if ("works" in verdict) return VERDICT.works(typeName(verdict.works.version, verdict.works.recipe));
  if ("lora" in verdict) return VERDICT.lora(verdict.lora.base === null ? null : typeName(verdict.lora.base, null));
  return VERDICT.cantRun(verdict.cannotRun.reason);
}

/** The one-time note about local models (Task B14, local addendum L1), word for word: shown
 *  before the first local download, and again from Get models' footer and Settings →
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

/** A download's progress as Models → Downloads says it: "1.2 GB of 6.6 GB · downloading · 3 min
 *  left", with " · no published checksum" when a file has none. `amount` writes the sizes
 *  (`formatBytes`). 0.0.7 (P6b review m5): no time left while a file is checked. Studio says
 *  its downloads in its own words since 0.2.0 Task U7 (`STUDIO_DOWNLOADS`). */
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
  /** The line before its *Manage in Studio* button: "Picture and video models: 2 · 12 GB · ". */
  lead: (count: number, bytes: number) => `${STUDIO_SETTINGS.models(count, bytes)} · `,
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

/** The shared controls in `src/components/ui`. */
export const UI = {
  close: "Close",
} as const;

/** What the app says when part of a screen could not be drawn (0.2.1: the error boundaries,
 *  `src/components/ErrorBoundary.tsx` and `CrashNotice.tsx`). The whole window's notice first,
 *  then a panel's smaller one, which keeps the rest of the app working. */
export const CRASH = {
  title: "Something went wrong drawing this screen",
  safe: "Your saved chats, models and downloads are safe.",
  next: `Try again, or reload ${APP_NAME} if this keeps happening.`,
  /** After a Try again that failed. */
  nextAgain: `It happened again. Reload ${APP_NAME} to start this window afresh.`,
  tryAgain: "Try again",
  reload: `Reload ${APP_NAME}`,
  details: "Details",
  /** A panel's notice: "This model's page couldn't be shown." */
  panel: (what: string) => `${what} couldn't be shown.`,
  panelNext: "Something went wrong drawing it. The rest of the app still works.",
  /** A panel's notice after a Try again that failed (it then offers Reload too). */
  panelAgain: "It went wrong again. The rest of the app still works.",
  /** Studio's own way out: leaves Get models and a model's page, back at the results. */
  backToStudio: "Back to Studio",
  /** What each panel with its own notice is called in it. */
  where: {
    studio: "Studio",
    chat: "This chat",
    workbench: "The side panel",
    detail: "This model's page",
    sheet: "The model list",
    settings: "This page of Settings",
  },
} as const;

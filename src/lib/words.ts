// Plain words for the app's technical values (0.0.5 D4, D8). Nothing on screen should show
// a raw identifier such as "vulkan", "Vulkan0" or "text_model". Every phrase of the model
// loading screens lives here, so the wording stays the same wherever it appears; the test
// walks every value of the contract's enums.
import type {
  BackendKind,
  ContextSource,
  DeviceOption,
  GpuUse,
  LoadFailure,
  LoadStage,
  ModelBusy,
  ThinkingLimit,
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
      return "Trying again with part of the model on the processor";
    case "steppingDown":
      return `${capitalise(stepDownCause(stage.from, stage.reason))}, so trying ${place(stage.to)}`;
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
      return `${capitalise(place(stage.backend))} ran out of memory, so part of the model runs on the processor. Replies are slower.`;
    case "steppingDown": {
      const cause = `Running on ${place(stage.to)} because ${stepDownCause(stage.from, stage.reason)}.`;
      const action = stepDownAction(stage.reason);
      if (action === "reload") return `${cause} Reload to try ${place(stage.from)} again.`;
      if (action === "settings") return `${cause} Choose another in Model settings.`;
      return cause;
    }
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

/** Gigabytes from MiB, whole from 10 GB up, else with one decimal ("16 GB", "7.5 GB"). */
function gigabytes(mib: number): string {
  const gb = mib / 1024;
  const shown = gb >= 10 ? Math.round(gb) : Math.round(gb * 10) / 10;
  return `${shown} GB`;
}

/**
 * A graphics card by its own name and memory, e.g. "AMD Radeon RX 9070 XT · 16 GB
 * (recommended)"; never the backend's device id ("Vulkan0").
 */
export function deviceLabel(device: DeviceOption): string {
  const number = /(\d+)$/.exec(device.name);
  const name = device.description.trim() || `Graphics card ${number ? Number(number[1]) + 1 : ""}`.trim();
  const memory = device.totalMib > 0 ? ` · ${gigabytes(device.totalMib)}` : "";
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

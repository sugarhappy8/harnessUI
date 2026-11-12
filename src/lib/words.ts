// Plain words for the app's technical values (0.0.5 D4, D8). Nothing on screen should show
// a raw identifier such as "vulkan", "Vulkan0" or "text_model".
//
// WP0 skeleton: the names, types and the phrases the specification gives word for word.
// WP2 owns this file and finishes it (every LoadStage, step-down and failure messages, the
// slow states) and its test, which walks every exported value.
import type { BackendKind, GpuUse, LoadStage, ThinkingLimit } from "./types";

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

/**
 * About how many words fit in `tokens` tokens (three quarters of a token count), rounded
 * down to a whole thousand from 1,000 words up, else to a whole hundred.
 */
export function aboutWords(tokens: number): number {
  const words = Math.floor((Math.max(0, tokens) * 3) / 4);
  return words >= 1000 ? Math.floor(words / 1000) * 1000 : Math.floor(words / 100) * 100;
}

/** A token count with words, as the rules ask: "8,192 tokens, about 6,000 words". */
export function tokensWithWords(tokens: number): string {
  return `${tokens.toLocaleString("en-US")} tokens, about ${aboutWords(tokens).toLocaleString("en-US")} words`;
}

/** A load stage in plain words. TODO(WP2): final wording for every stage. */
export function loadStageWords(stage: LoadStage): string {
  switch (stage.kind) {
    case "starting":
      return "Starting";
    case "readingModel":
      return "Reading the model file";
    case "readingImageReader":
      return "Loading the image reader";
    case "loading":
      return "Loading";
    case "restarting":
      return `Restarting after a problem (${stage.attempt} of ${stage.max})`;
    case "retryingWithoutSaveMemory":
      return "Save memory doesn't work with this graphics card, so it was turned off for this load.";
    case "retryingWithLessGpu":
      return "Trying again with part of the model on the processor";
    case "steppingDown":
      return `Trying ${backendWords(stage.to)} instead`;
  }
}

// Plain words for what a model can do and how its file is made (0.0.5 item 4, D6): the
// catalog's `goodAt` keys, what a Hugging Face uploader lists (pipeline tag, tags), the
// quantisation in a file name, parameter counts and languages.
//
// A key with no phrase here is left out, never shown raw: uploader text is untrusted, and
// a raw key ("Q4_K_M", "text-generation") means nothing to the owner. No model name appears
// here; tags that name a model family have no phrase and so are never shown.

/** The catalog's `goodAt` vocabulary, in the order it is listed in the specification. */
export const GOOD_AT_KEYS = [
  "chat",
  "writing",
  "coding",
  "reasoning",
  "math",
  "tools",
  "images",
  "long-documents",
  "languages",
  "speed",
] as const;

export type GoodAtKey = (typeof GOOD_AT_KEYS)[number];

/** Each `goodAt` key as words that follow "Good at …". */
export const GOOD_AT_WORDS: Record<GoodAtKey, string> = {
  chat: "everyday chat",
  writing: "writing",
  coding: "coding",
  reasoning: "thinking through hard problems",
  math: "math",
  tools: "using tools",
  images: "reading images",
  "long-documents": "long documents",
  languages: "many languages",
  speed: "fast replies",
};

function isGoodAtKey(key: string): key is GoodAtKey {
  return (GOOD_AT_KEYS as readonly string[]).includes(key);
}

/** The known keys' phrases, once each, in the order given; unknown keys are dropped. */
export function goodAtWords(keys: readonly string[]): string[] {
  const out: string[] = [];
  for (const key of keys) {
    if (!isGoodAtKey(key)) continue;
    const phrase = GOOD_AT_WORDS[key];
    if (!out.includes(phrase)) out.push(phrase);
  }
  return out;
}

/** "a, b and c". */
export function joinWords(words: readonly string[]): string {
  if (words.length <= 1) return words.join("");
  return `${words.slice(0, -1).join(", ")} and ${words[words.length - 1]}`;
}

/** "Good at everyday chat and coding."; null when no key is known. */
export function goodAtSentence(keys: readonly string[]): string | null {
  const words = goodAtWords(keys);
  return words.length ? `Good at ${joinWords(words)}.` : null;
}

/**
 * Hugging Face pipeline tags (what the uploader says the model does), as lower-case
 * phrases that follow "The uploader lists:". Models that cannot chat say so.
 */
export const PIPELINE_WORDS: Record<string, string> = {
  "text-generation": "writes text",
  "image-text-to-text": "reads images and writes text",
  "audio-text-to-text": "listens to sound and writes text",
  "video-text-to-text": "watches video and writes text",
  "any-to-any": "works with text, images and sound",
  "text2text-generation": "rewrites text",
  "image-to-text": "describes images",
  "visual-question-answering": "answers questions about images",
  "question-answering": "answers questions about a text",
  summarization: "summarises text",
  translation: "translates text",
  "feature-extraction": "turns text into numbers for search (cannot chat)",
  "sentence-similarity": "compares sentences (cannot chat)",
  "fill-mask": "fills in missing words (cannot chat)",
  "text-classification": "sorts text into groups (cannot chat)",
  "token-classification": "finds names and terms in text (cannot chat)",
  "zero-shot-classification": "sorts text into groups (cannot chat)",
  "automatic-speech-recognition": "turns speech into text (cannot chat)",
  "text-to-speech": "turns text into speech (cannot chat)",
  "text-to-audio": "makes sound from text (cannot chat)",
  "text-to-image": "makes pictures (cannot chat)",
  "image-to-image": "changes pictures (cannot chat)",
  "text-to-video": "makes video (cannot chat)",
};

/** A pipeline tag in words, or null when it is unknown or missing. */
export function pipelineWords(tag: string | null | undefined): string | null {
  if (!tag) return null;
  return Object.prototype.hasOwnProperty.call(PIPELINE_WORDS, tag) ? (PIPELINE_WORDS[tag] ?? null) : null;
}

/**
 * Hugging Face tags that say what a model does, as lower-case phrases. Format tags
 * (gguf, imatrix, quantized, llama.cpp, region:…, base_model:…) have none on purpose:
 * when only those exist the column says there is no description (D6).
 */
export const TAG_WORDS: Record<string, string> = {
  conversational: "made for chat",
  chat: "made for chat",
  code: "coding",
  coding: "coding",
  math: "math",
  reasoning: "reasoning",
  thinking: "thinks before answering",
  "tool-use": "using tools",
  "function-calling": "using tools",
  tools: "using tools",
  agent: "working as an agent",
  vision: "reading images",
  multimodal: "works with images as well as text",
  "long-context": "long documents",
  multilingual: "many languages",
  translation: "translating",
  "creative-writing": "creative writing",
  roleplay: "role play",
  medical: "medical text",
  legal: "legal text",
  finance: "financial text",
  uncensored: "changed by the uploader to refuse less",
  abliterated: "changed by the uploader to refuse less",
};

/** The known tags' phrases, once each, in the order given. */
export function tagWords(tags: readonly string[]): string[] {
  const out: string[] = [];
  for (const tag of tags) {
    const phrase = Object.prototype.hasOwnProperty.call(TAG_WORDS, tag) ? TAG_WORDS[tag] : undefined;
    if (phrase && !out.includes(phrase)) out.push(phrase);
  }
  return out;
}

/** Approximate bits per weight of a quantisation (the same table as the models crate). */
function bitsPerWeight(label: string): number | null {
  const l = label.toUpperCase().replace(/^UD-/, "");
  switch (l) {
    case "F32":
      return 32;
    case "F16":
    case "BF16":
      return 16;
    case "Q8_0":
      return 8.5;
    case "Q6_K":
      return 6.56;
    case "Q5_K_M":
    case "Q5_K_L":
    case "Q5_K_XL":
      return 5.7;
    case "Q5_K_S":
    case "Q5_0":
    case "Q5_1":
      return 5.5;
    case "Q4_K_M":
    case "Q4_K_L":
    case "Q4_K_XL":
      return 4.85;
    case "Q4_K_S":
    case "Q4_0":
    case "Q4_1":
    case "IQ4_NL":
      return 4.5;
    case "MXFP4":
    case "MXFP4_MOE":
    case "IQ4_XS":
      return 4.25;
    case "Q3_K_L":
    case "Q3_K_XL":
      return 4.0;
    case "Q3_K_M":
      return 3.9;
    case "IQ3_M":
    case "IQ3_S":
      return 3.5;
    case "Q3_K_S":
      return 3.4;
    case "IQ3_XS":
    case "IQ3_XXS":
      return 3.1;
    case "Q2_K":
    case "Q2_K_L":
    case "Q2_K_XL":
      return 2.9;
    case "Q2_K_S":
    case "IQ2_M":
    case "IQ2_S":
      return 2.7;
    case "IQ2_XS":
    case "IQ2_XXS":
      return 2.3;
    case "IQ1_M":
    case "IQ1_S":
    case "TQ1_0":
    case "TQ2_0":
      return 1.8;
    default:
      return null;
  }
}

/** A quantisation name in words ("smaller file, good quality"); null when unknown. */
export function quantWords(label: string | null | undefined): string | null {
  if (!label) return null;
  const bits = bitsPerWeight(label);
  if (bits === null) return null;
  if (bits >= 16) return "full size, the original quality";
  if (bits >= 8) return "large file, almost the original quality";
  if (bits >= 6) return "large file, very close to the original quality";
  if (bits >= 5) return "medium file, very good quality";
  if (bits >= 4.2) return "smaller file, good quality";
  if (bits >= 3) return "small file, lower quality";
  if (bits >= 2) return "very small file, noticeably lower quality";
  return "tiny file, much lower quality";
}

/** "Q4_K_M: smaller file, good quality"; the name alone when unknown; null without one. */
export function fileWords(label: string | null | undefined): string | null {
  if (!label) return null;
  const words = quantWords(label);
  return words ? `${label}: ${words}` : label;
}

/** The quantisation name in a GGUF file name, as the models crate finds it (last match). */
export function quantFromFile(file: string | null | undefined): string | null {
  if (!file) return null;
  const name = file.split("/").pop() ?? file;
  const stem = name.replace(/-\d{5}-of-\d{5}\.gguf$/i, "").replace(/\.gguf$/i, "");
  const re = /(?:^|[-_.])((?:UD-)?(?:I?Q\d(?:_[0-9A-Z]+)*|BF16|F16|F32|FP16|FP32|MXFP4(?:_MOE)?|TQ\d_0))(?=$|[-_.])/gi;
  let last: string | null = null;
  for (const m of stem.matchAll(re)) last = (m[1] ?? "").toUpperCase();
  if (last === "FP16") return "F16";
  if (last === "FP32") return "F32";
  return last;
}

function oneDecimal(v: number): string {
  const r = Math.round(v * 10) / 10;
  return Number.isInteger(r) ? String(r) : r.toFixed(1);
}

/** A parameter count in words: "9 billion", "1.5 billion", "596 million". */
export function parameterWords(n: number): string {
  if (n >= 1e12) return `${oneDecimal(n / 1e12)} trillion`;
  if (n >= 1e9) {
    const v = n / 1e9;
    return `${v >= 10 ? Math.round(v) : oneDecimal(v)} billion`;
  }
  if (n >= 1e6) return `${Math.round(n / 1e6)} million`;
  return n.toLocaleString("en-US");
}

/**
 * The answer to "How big is it?". A mixture of experts (or known active parameters) says it
 * runs faster than its size suggests; null without a parameter count.
 */
export function sizeAnswer(parameters: number | null, active: number | null, moe: boolean): string | null {
  if (!parameters) return null;
  const total = parameterWords(parameters);
  if (active) return `${total}, using ${parameterWords(active)} at a time, so it runs faster than its size suggests.`;
  if (moe) return `${total} parameters, using only part of them at a time, so it runs faster than its size suggests.`;
  return `${total} parameters. Bigger models are usually smarter but slower.`;
}

/**
 * Common license ids (as Hugging Face and the catalog write them) as their names. Licenses
 * named after a model family have no entry (no model name appears here) and are shown as
 * the uploader or catalog writes them.
 */
export const LICENSE_WORDS: Record<string, string> = {
  "apache-2.0": "Apache 2.0",
  mit: "MIT",
  "bsd-2-clause": "BSD 2-Clause",
  "bsd-3-clause": "BSD 3-Clause",
  "gpl-2.0": "GPL 2.0",
  "gpl-3.0": "GPL 3.0",
  "lgpl-3.0": "LGPL 3.0",
  "agpl-3.0": "AGPL 3.0",
  "mpl-2.0": "Mozilla Public License 2.0",
  "cc-by-4.0": "Creative Commons Attribution 4.0",
  "cc-by-sa-4.0": "Creative Commons Attribution-ShareAlike 4.0",
  "cc-by-nc-4.0": "Creative Commons Attribution-NonCommercial 4.0 (no commercial use)",
  "cc-by-nc-sa-4.0": "Creative Commons Attribution-NonCommercial-ShareAlike 4.0 (no commercial use)",
  "cc0-1.0": "Public domain (CC0)",
  unlicense: "Public domain (Unlicense)",
  openrail: "OpenRAIL",
  other: "Another license (read the model card)",
};

/** A license id as its name ("apache-2.0" → "Apache 2.0"); anything else as it is written. */
export function licenseWords(name: string): string {
  const key = name.trim().toLowerCase();
  return Object.prototype.hasOwnProperty.call(LICENSE_WORDS, key) ? (LICENSE_WORDS[key] ?? name) : name;
}

/** Yes or No for "Can it use tools?" and its neighbours. */
export function capabilityAnswer(yes: boolean): string {
  return yes ? "Yes" : "No";
}

let languageNames: Intl.DisplayNames | null | undefined;

function languageName(code: string): string | null {
  if (languageNames === undefined) {
    try {
      languageNames = new Intl.DisplayNames(["en"], { type: "language", fallback: "none" });
    } catch {
      languageNames = null;
    }
  }
  if (!languageNames) return null;
  try {
    return languageNames.of(code) ?? null;
  } catch {
    return null;
  }
}

/** Language codes as names ("en" → "English"), once each; codes it cannot name are dropped. */
export function languageWords(codes: readonly string[]): string[] {
  const out: string[] = [];
  for (const raw of codes) {
    const code = raw.trim();
    const name = code.toLowerCase() === "multilingual" ? "Many languages" : languageName(code);
    if (name && !out.includes(name)) out.push(name);
  }
  return out;
}

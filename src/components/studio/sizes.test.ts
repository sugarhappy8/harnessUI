// One size format in Studio (0.2.0 Task U7, critique §2.7, §5 and §6): every size Studio shows is
// `formatBytes`'s (decimal gigabytes, as Hugging Face and the chat's Models dialog show them).
// 0.1.2 showed one model as "6.2 GB" (binary `amountGb`) and "6.7 GB" on the same card, and the
// engine's pack in binary megabytes (`downloadAmount`). No Studio source may name either, and
// the words Studio shows sizes with write them as `formatBytes` does.
import { describe, expect, it } from "vitest";
import { formatBytes } from "../../lib/format";
import {
  engineDownloadWords,
  engineSizeWords,
  fitsThisComputer,
  STUDIO_SETTINGS,
  STUDIO_SUGGESTION,
} from "../../lib/words";
import * as words from "../../lib/words";

/** Every Studio source, as text: this folder's (tests included) and the Studio store's. */
const SOURCES: Record<string, string> = {
  ...import.meta.glob<string>("./**/*.{ts,tsx}", { query: "?raw", import: "default", eager: true }),
  ...import.meta.glob<string>("../../store/studio*.ts", { query: "?raw", import: "default", eager: true }),
};

/** This file names them on purpose (Vite's glob leaves the file that asks out anyway). */
const SELF = "./sizes.test.ts";

/** The binary size helpers Studio no longer uses, their names built here so that this file
 *  never matches itself (preflight P38). */
const BINARY = [
  ["amount", "Gb"],
  ["download", "Amount"],
].map((parts) => parts.join(""));

describe("one size format in Studio", () => {
  it("reads every Studio source", () => {
    expect(Object.keys(SOURCES)).toContain("./ModelCard.tsx");
    expect(Object.keys(SOURCES)).toContain("./GetModels.tsx");
    expect(Object.keys(SOURCES)).toContain("../../store/studio.ts");
    expect(Object.keys(SOURCES)).toContain("../../store/studioLocal.ts");
    expect(Object.keys(SOURCES)).toContain("./GetModels.test.tsx");
  });

  it("no Studio source uses amountGb", () => {
    const found = Object.entries(SOURCES)
      .filter(([path]) => path !== SELF)
      .flatMap(([path, text]) => BINARY.filter((name) => new RegExp(`\\b${name}\\b`).test(text)).map((name) => `${path}: ${name}`));
    expect(found).toEqual([]);
  });

  it("the binary megabytes the engine's pack was written in are gone", () => {
    expect(BINARY[1]! in words).toBe(false);
  });

  it("the words Studio shows sizes with write them as formatBytes does", () => {
    // The Windows engine pack: 30,303,000 bytes read "30 MB" (binary megabytes said "29 MB").
    expect(engineSizeWords(30_303_000, "stable-diffusion.cpp MIT; ggml MIT")).toBe(`${formatBytes(30_303_000)}, MIT`);
    expect(engineSizeWords(30_303_000, "MIT")).toBe("30 MB, MIT");
    expect(engineDownloadWords(30_303_000, "MIT")).toBe("Download the Studio engine (30 MB, MIT)");
    // FLUX.2 klein 4B's download, 5,293,871,164 bytes: "5.3 GB" (binary said "4.9 GB").
    expect(fitsThisComputer("FLUX.2 klein 4B", 5_293_871_164)).toBe("FLUX.2 klein 4B fits this computer (5.3 GB).");
    expect(STUDIO_SUGGESTION.size(5_293_871_164)).toBe("One download of 5.3 GB.");
    expect(STUDIO_SETTINGS.models(2, 35_873_633_206)).toBe("Picture and video models: 2 (35.9 GB)");
    expect(STUDIO_SETTINGS.usage(1_300_000_000, "~/s", 1, 0)).toBe("Studio uses 1.3 GB in ~/s (1 picture, 0 videos)");
  });
});

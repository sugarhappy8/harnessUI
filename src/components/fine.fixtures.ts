// Test data for 0.0.6 item 6 (Fine control, the picture, the usage line), shared by the
// component tests. Not used by the app. The numbers are the worked example of
// item6-spec.md 6: a model of 14.0 GB with 41 layers, 30 on an RX 9070 XT (15.9 GB) with
// 0.9 GB used by other programs, and 32 GB of computer memory with 12.0 GB used.
import type {
  FineControl,
  FineFacts,
  FineLaunch,
  MemoryBar,
  OffloadPreview,
  Placement,
  UsageNow,
} from "../lib/types";

export const GB = 1024 * 1024 * 1024;

/** Gigabytes (binary, as Task Manager counts) to bytes. */
export function gb(n: number): number {
  return Math.round(n * GB);
}

export const FINE_AUTO: FineControl = {
  layers: "auto",
  experts: "auto",
  kv: "auto",
  threads: "auto",
  fastAttention: "auto",
  leaveFree: "auto",
};

export const FINE_LAUNCH_NONE: FineLaunch = {
  gpuLayers: null,
  expertsOnCpu: "none",
  kvInComputerMemory: false,
  threads: null,
  flashAttn: "auto",
  fitMarginMib: null,
};

export function fineFacts(patch: Partial<FineFacts> = {}): FineFacts {
  return { units: 41, expertLayers: null, threads: 16, graphics: true, apple: false, graphicsLimitBytes: null, ...patch };
}

export function graphicsBar(patch: Partial<MemoryBar> = {}): MemoryBar {
  return {
    name: "AMD Radeon RX 9070 XT",
    totalBytes: gb(15.9),
    freeBytes: gb(15.0),
    freeAfterUnload: false,
    otherBytes: gb(0.9),
    need: { model: gb(9.9), context: gb(1.2), compute: gb(0.6), imageReader: 0 },
    ...patch,
  };
}

export function computerBar(patch: Partial<MemoryBar> = {}): MemoryBar {
  return {
    name: "",
    totalBytes: gb(32),
    freeBytes: gb(19.9),
    freeAfterUnload: false,
    otherBytes: gb(12.0),
    need: { model: gb(4.1), context: gb(0.4), compute: gb(0.3), imageReader: 0 },
    ...patch,
  };
}

/** The worked example: 30 of 41 layers chosen in Fine control, measured. */
export function picture(patch: Partial<OffloadPreview> = {}): OffloadPreview {
  return {
    key: "k",
    source: { kind: "measured" },
    unified: false,
    units: 41,
    layersOnGraphics: 30,
    layersAbout: false,
    expertLayersOnProcessor: null,
    graphics: graphicsBar(),
    computer: computerBar(),
    graphicsLimitBytes: null,
    mark: "green",
    fits: "fits",
    custom: true,
    ...patch,
  };
}

/** The picture of a model on the processor only (this build machine). */
export function processorPicture(patch: Partial<OffloadPreview> = {}): OffloadPreview {
  return picture({
    source: { kind: "estimate", why: "quick" },
    units: 29,
    layersOnGraphics: 0,
    graphics: null,
    custom: false,
    computer: computerBar({ need: { model: gb(0.6), context: gb(0.4), compute: gb(0.3), imageReader: 0 } }),
    ...patch,
  });
}

/** Where the loaded model went: 30 of 41 layers on the card. */
export function placement(patch: Partial<Placement> = {}): Placement {
  return {
    graphics: { name: "AMD Radeon RX 9070 XT", use: { model: gb(9.9), context: gb(1.2), compute: gb(0.6), imageReader: 0 } },
    computer: { model: gb(4.1), context: gb(0.4), compute: gb(0.3), imageReader: 0 },
    units: 41,
    layersOnGraphics: 30,
    expertLayersOnProcessor: null,
    layersAbout: false,
    custom: true,
    ...patch,
  };
}

/** This computer's usage: the worked example of 6.1. */
export function usage(patch: Partial<UsageNow> = {}): UsageNow {
  return {
    graphics: {
      kind: "value",
      value: {
        name: "AMD Radeon RX 9070 XT",
        busyPercent: { kind: "value", value: 63 },
        memoryUsedBytes: { kind: "value", value: gb(12.6) },
        memoryTotalBytes: gb(15.9),
      },
    },
    processorPercent: { kind: "value", value: 12 },
    threads: 16,
    memory: { kind: "value", value: { usedBytes: gb(16.84), totalBytes: gb(32) } },
    unified: false,
    ...patch,
  };
}

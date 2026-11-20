import type { DownloadGroup } from "../../lib/types";
import { LICENCE_BADGE, REGIONAL_USE, revenueLimit } from "../../lib/words";
import type {
  Gate,
  Licence,
  LocalEntry,
  LocalFileChoice,
  LocalModels,
  LocalRepo,
  LocalSource,
  MediaKind,
  Part,
} from "./types";

// What Get local models offers to download, a licence's badge, and a model's download while it
// runs (0.1.0 Tasks B13 and B14), as plain functions with no React: the rows
// (LocalModelRow.tsx), Hugging Face search (LocalSearch.tsx), the download confirmation
// (DownloadLocalModel.tsx) and the store use them.
// An offer keeps names as they came (a searched file's path can hold any character); whatever
// shows one escapes it there (`visiblePath`).

/** A licence's badge: its colour (as the backend judged it) and what it allows for what you
 *  make, in plain words. A kind this version does not know reads "Check the licence". */
export function licenceBadge(l: Licence): { tone: "green" | "amber" | "red"; text: string } {
  if (l.commercial === "yes" && l.excludedRegions?.length) return { tone: "amber", text: REGIONAL_USE };
  let text: string;
  switch (l.commercial) {
    case "yes":
      text = LICENCE_BADGE.yes;
      break;
    case "underRevenue":
      text = LICENCE_BADGE.underRevenue(l.revenueLimitUsd === null ? null : revenueLimit(l.revenueLimitUsd));
      break;
    case "registration":
      text = LICENCE_BADGE.registration;
      break;
    case "unclear":
      text = LICENCE_BADGE.unclear;
      break;
    case "no":
      text = LICENCE_BADGE.no;
      break;
    default:
      // The backend sends it as a string: a kind this version does not know is not trusted.
      text = LICENCE_BADGE.unclear;
  }
  return { tone: l.badge, text };
}

/** The id of the Studio engine's licence among a download's licences (the backend's
 *  `engine_cmd::ENGINE_LICENCE_ID`): listed when the engine comes with the model. */
export const ENGINE_LICENCE_ID = "studio-engine";

/** What a Download button offers; the dialog shows the files and their licences. */
export interface DownloadOffer {
  /** The model's name, as the dialog's title says it (as it came: shown through
   *  `visiblePath`). */
  name: string;
  /** What it makes: the dialog says what the pictures, or the videos, you make may be used
   *  for. */
  kind: MediaKind;
  source: LocalSource;
  parts: Part[];
  /** Every distinct licence of its parts: all are shown. */
  licences: Licence[];
  totalBytes: number;
  /** What is not on this computer yet. */
  toFetchBytes: number;
  /** The Studio engine comes with it (Windows, before the first model). */
  needsEngine: boolean;
  /** The engine's download when it comes with it, else 0. */
  engineBytes: number;
  /** The download would leave less than 10 GB free. */
  lowDisk: boolean;
  gate: Gate;
}

/** A recommended entry's download, as its DTO plans it. */
export function offerForEntry(e: LocalEntry, local: LocalModels): DownloadOffer {
  return {
    name: e.name,
    kind: e.kind,
    source: { catalog: { entryId: e.id } },
    parts: e.parts,
    licences: e.licences,
    totalBytes: e.sizeBytes,
    toFetchBytes: e.toFetchBytes,
    needsEngine: e.needsEngine,
    engineBytes: e.needsEngine ? local.engine.downloadBytes : 0,
    lowDisk: e.lowDisk,
    gate: e.gate,
  };
}

/** A searched repository's file and the parts it needs beside it, at the commit it was read
 *  at (the backend recognises it there again before downloading), found on the `kind` tab (a
 *  search finds only that kind's repositories). Its name is the file's path as Hugging Face
 *  gave it. */
export function offerForFile(repo: LocalRepo, f: LocalFileChoice, local: LocalModels, kind: MediaKind): DownloadOffer {
  return {
    name: f.file,
    kind,
    source: { search: { repo: repo.repo, commit: repo.commit, file: f.file } },
    parts: f.parts,
    licences: f.licences,
    totalBytes: f.totalBytes,
    toFetchBytes: f.toFetchBytes,
    needsEngine: f.needsEngine,
    engineBytes: f.needsEngine ? local.engine.downloadBytes : 0,
    lowDisk: f.lowDisk,
    gate: repo.gate,
  };
}

/** The installed id a searched repository's file gets (the backend's `gen_search::entry_for`):
 *  its download group carries it while it downloads. */
export function searchedModelId(repo: string, file: string): string {
  return `hf:${repo}:${file}`;
}

/** The download of local model `genModelId` while it runs (queued, downloading, paused,
 *  checking, failed and waiting for Resume, or being cancelled), from the app's download
 *  groups; `null` when none is (never started, done or cancelled). */
export function activeDownload(groups: readonly DownloadGroup[], genModelId: string): DownloadGroup | null {
  return (
    groups.find((g) => g.genModelId === genModelId && g.status !== "done" && g.status !== "cancelled") ?? null
  );
}

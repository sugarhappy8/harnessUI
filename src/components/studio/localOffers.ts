import { LICENCE_BADGE, revenueLimit } from "../../lib/words";
import type { Gate, Licence, LocalEntry, LocalFileChoice, LocalModels, LocalRepo, LocalSource, Part } from "./types";

// What Get local models offers to download, and a licence's badge (0.1.0 Tasks B13 and B14), as
// plain functions with no React: the rows (LocalModelRow.tsx), Hugging Face search
// (LocalSearch.tsx), the download confirmation (DownloadLocalModel.tsx) and the store use them.
// An offer keeps names as they came (a searched file's path can hold any character); whatever
// shows one escapes it there (`visiblePath`).

/** A licence's badge: its colour (as the backend judged it) and what it allows for what you
 *  make, in plain words. A kind this version does not know reads "Check the licence". */
export function licenceBadge(l: Licence): { tone: "green" | "amber" | "red"; text: string } {
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

/** What a Download button offers; Task B14's dialog shows it and sends every licence's hash. */
export interface DownloadOffer {
  /** The model's name, as the dialog's title says it (as it came: shown through
   *  `visiblePath`). */
  name: string;
  source: LocalSource;
  parts: Part[];
  /** Every distinct licence of its parts: all are shown and accepted. */
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
 *  at (the backend recognises it there again before downloading). Its name is the file's path
 *  as Hugging Face gave it. */
export function offerForFile(repo: LocalRepo, f: LocalFileChoice, local: LocalModels): DownloadOffer {
  return {
    name: f.file,
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

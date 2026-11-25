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
  SetupChoice,
  SetupPlan,
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
    case "withUseRules":
      text = LICENCE_BADGE.withUseRules;
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
  /** The person's set-up choice the parts were planned with (a searched file only); `null`:
   *  the plan as it comes. The backend plans it again with this before downloading. */
  choice: SetupChoice | null;
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
    choice: null,
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

/** A searched repository's file and the files its set-up `plan` lists (its own plan unless
 *  another is given, with the `setup` choice that made it), at the commit it was read at (the
 *  backend plans it there again, with that choice, before downloading), found on the `kind`
 *  tab (a search finds only that kind's repositories). Its name is the file's path as Hugging
 *  Face gave it. */
export function offerForFile(
  repo: LocalRepo,
  choice: LocalFileChoice,
  local: LocalModels,
  kind: MediaKind,
  plan: SetupPlan = choice.plan,
  setup: SetupChoice | null = null,
): DownloadOffer {
  return {
    name: choice.file,
    kind,
    source: { search: { repo: repo.repo, commit: repo.commit, file: choice.file } },
    choice: setup,
    parts: planParts(plan),
    licences: plan.licences,
    totalBytes: plan.totalBytes,
    toFetchBytes: plan.toFetchBytes,
    needsEngine: plan.needsEngine,
    engineBytes: plan.needsEngine ? local.engine.downloadBytes : 0,
    lowDisk: plan.lowDisk,
    gate: repo.gate,
  };
}

/** The files a set-up plan lists, in its slots' order: none for a part the main file carries
 *  or that nothing fills. */
export function planParts(plan: SetupPlan): Part[] {
  return plan.slots.flatMap((s) => (s.part === null ? [] : [s.part]));
}

/** The installed id a searched repository's file gets (`hf:<repo>:<file>`, the backend's set-up
 *  plan's entry): its download group carries it while it downloads. */
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

// What Studio's runs cost, as Studio shows it (0.1.0 Tasks B2 and B7): dollars in one form
// everywhere (the top bar's spending, the picker's price hints, the Create button), the colour a
// price takes as a second cue, the label on Create (a price for a cloud run, the time or "free"
// for a local one), where the prompt goes, cloud as the backup for a local run, and the daily
// limit's line.
import { cloudGoesTo, STUDIO_DOCK, STUDIO_PRICE } from "../../lib/words";
import { cloudModelsFor, makesKind, type StudioTab } from "../../store/studio";
import { etaText } from "./eta";
import type { Estimate, Limit, RunTarget, StudioEndpoint } from "./types";

/** Dollars as Studio shows them: "$0.16"; more than nothing but less than a cent, "< $0.01". */
export function formatUsd(usd: number): string {
  if (usd > 0 && usd < 0.01) return "< $0.01";
  return `$${usd.toFixed(2)}`;
}

/** `n` with a comma between each group of three digits: 20000 → "20,000". */
function grouped(n: number): string {
  return String(n).replace(/\B(?=(\d{3})+$)/g, ",");
}

/** Dollars with cents and thousands commas, and no floor below a cent, as the backend writes
 *  them (`errors::usd`): 9.5 → "$9.50", 1234.5 → "$1,234.50", 0 → "$0.00". */
function usdWithCents(usd: number): string {
  const cents = Math.round(Math.max(0, usd) * 100);
  return `$${grouped(Math.floor(cents / 100))}.${String(cents % 100).padStart(2, "0")}`;
}

/** A dollar amount without cents when it is whole, as the backend writes a limit
 *  (`errors::usd_short`): 10 → "$10", 12.5 → "$12.50", 1000 → "$1,000". */
export function formatUsdShort(usd: number): string {
  const full = usdWithCents(usd);
  return full.endsWith(".00") ? full.slice(0, -3) : full;
}

/** The colour a price takes beside its text: under $1 plain, $1 to $2.50 amber, more red;
 *  "unknown" without a price. */
export function costTone(usd: number | null): "plain" | "amber" | "red" | "unknown" {
  if (usd === null) return "unknown";
  if (usd < 1) return "plain";
  return usd <= 2.5 ? "amber" : "red";
}

/** The label beside Create: a local run's time here ("about 4 min", "(first guess)" until it
 *  was measured), else "free" for a run that costs nothing; a cloud run's price for the whole
 *  run ("~ $0.16") or "price unknown". Nothing before there is an estimate. */
export function createLabel(estimate: Estimate | null): string {
  if (!estimate) return "";
  const time = etaText(estimate.eta);
  if (time) return time;
  if (estimate.usd === null) return STUDIO_PRICE.unknown;
  if (estimate.local || estimate.usd === 0) return STUDIO_PRICE.free;
  return STUDIO_DOCK.estimate(formatUsd(estimate.usd));
}

/** The line above Create: a local run's prompt and pictures stay here (and the chat model is
 *  unloaded to make room, when it is); a cloud run's go to its endpoint's `host`. */
export function placeLine(target: RunTarget, host: string, estimate: Estimate | null): string {
  if (target !== "thisComputer") return cloudGoesTo(host);
  return estimate?.local?.unloadChat ? `${STUDIO_DOCK.here}${STUDIO_DOCK.joiner}${STUDIO_DOCK.unloadChat}` : STUDIO_DOCK.here;
}

/**
 * Cloud as the backup for a local run that does not fit this computer or would be very slow
 * (more than 15 minutes): "useCloud" when a saved endpoint has a model of `tab`'s kind Studio
 * can use (OpenAI makes no video, whatever its lists hold), else "addEndpoint" (Settings →
 * Endpoints). `null` for any other run, or before there is an estimate. It only offers: Studio
 * never moves a draft to the cloud by itself.
 */
export function cloudBackup(
  tab: StudioTab,
  estimate: Estimate | null,
  endpoints: StudioEndpoint[],
): "useCloud" | "addEndpoint" | null {
  const local = estimate?.local;
  if (!local || (local.fit !== "notHere" && !local.verySlow)) return null;
  const usable = endpoints.some((e) => e.cloud && makesKind(e, tab) && cloudModelsFor(e, tab).length > 0);
  return usable ? "useCloud" : "addEndpoint";
}

/** In place of Create while a cloud run would pass the daily limit: what was spent today (the
 *  runs still going included) of the limit, in the backend's words (`errors::daily_limit`). */
export function limitLine(limit: Limit): string {
  return STUDIO_DOCK.limitReached(
    usdWithCents(limit.spentTodayUsd + limit.pendingUsd),
    formatUsdShort(limit.limitUsd ?? 0),
  );
}

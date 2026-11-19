// How long a local run takes, in words (0.1.0): the one place an `Eta` becomes text, so the
// picker, the panel's time chips, the Create button, the run card and Get local models all say
// the same ("about 4 min", "(first guess)" until it was measured on this computer).
import { STUDIO_TIME } from "../../lib/words";
import type { Eta } from "./types";

/** A time rounded as people read it, by the backend's rule (`gen_eta::round_for_display`): to
 *  the nearest 5 s under a minute, 30 s under 10 minutes, whole minutes above; at least 5 s. */
function roundForDisplay(ms: number): number {
  const step = ms < 60_000 ? 5_000 : ms < 600_000 ? 30_000 : 60_000;
  return Math.max(5_000, Math.floor((ms + step / 2) / step) * step);
}

/** A time in words, to the second: "25 s", "4 min", "1 min 30 s", "1 h 5 min" (a run card's
 *  "took 3 min 48 s"). */
export function timeWords(ms: number): string {
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return STUDIO_TIME.seconds(seconds);
  const minutes = Math.floor(seconds / 60);
  const rest = seconds % 60;
  if (minutes < 60) {
    const words = STUDIO_TIME.minutes(minutes);
    return rest > 0 ? `${words} ${STUDIO_TIME.seconds(rest)}` : words;
  }
  const hours = Math.floor(minutes / 60);
  const left = minutes % 60;
  return left > 0 ? `${STUDIO_TIME.hours(hours)} ${STUDIO_TIME.minutes(left)}` : STUDIO_TIME.hours(hours);
}

/** How long a run took, as the viewer says it: to a tenth of a second under a minute
 *  ("13.8 s"), else as `timeWords` ("3 min 48 s"). */
export function tookWords(ms: number): string {
  return ms < 59_950 ? STUDIO_TIME.seconds(Math.round(ms / 100) / 10) : timeWords(ms);
}

/**
 * How long a run takes, as one figure: the middle of the range, rounded ("about 4 min"), then
 * `per` when given ("a picture", "for a 2 s clip"), then "(first guess)" when it was not
 * measured on this computer. `null` when there is no estimate (nothing measured here and no
 * first guess): the caller says "time shown after your first run here".
 */
export function etaText(eta: Eta | null, per?: string): string | null {
  if (!eta) return null;
  const parts = [STUDIO_TIME.about(timeWords(roundForDisplay((eta.lowMs + eta.highMs) / 2)))];
  if (per) parts.push(per);
  if (!eta.measured) parts.push(STUDIO_TIME.firstGuess);
  return parts.join(" ");
}

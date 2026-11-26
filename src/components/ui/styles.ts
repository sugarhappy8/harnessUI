// The class strings the shared controls draw with. Every control that can take focus uses
// FOCUS_RING, so the app has one focus look (design 10.4).

/** The one focus ring: a soft 2 px ring in the accent, no outline. A utility, so it also
 *  beats the base `:focus-visible` outline in `styles.css`. The ring is a box-shadow, which
 *  forced colors (Windows High Contrast) remove, so a focused control also takes
 *  `outline-hidden`: no outline in normal themes, a transparent one there that forced colors
 *  draw. Only on focus, so a control at rest shows none. */
export const FOCUS_RING =
  "outline-none focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-accent/40";

/** A closed select, menu trigger or other field-like button. */
export const FIELD =
  "inline-flex min-h-8 items-center justify-between gap-2 rounded-lg border border-border bg-surface px-2.5 py-1 text-sm " +
  "hover:border-border-strong disabled:opacity-60 data-[placeholder]:text-fg-muted " +
  FOCUS_RING;

/** What a select, menu or tooltip floats in. */
export const POPUP = "z-50 rounded-xl border border-border bg-surface p-1 text-sm text-fg shadow-pop";

/** One row of a select or a menu. */
export const OPTION =
  "relative flex cursor-default items-center gap-2 rounded-lg px-2.5 py-1.5 text-sm select-none " +
  "data-[disabled]:text-fg-muted data-[highlighted]:bg-bg-muted " +
  FOCUS_RING;

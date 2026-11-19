// The class strings Studio's right panel and run cards share (0.1.0 Tasks B5, B6 and B8), in
// one place so the panel's fields, Advanced drawer and My setups draw their boxes and small
// buttons alike, and a card's buttons look the same wherever on the card they are drawn.

/** A text box, number box, select or text area. */
export const BOX =
  "rounded-md border border-border bg-bg px-2 py-1 text-sm outline-none focus:border-border-strong disabled:opacity-60";

/** A small text button: *Reset all*, *Edit JSON*, *Save as setup…*, *Cancel*. */
export const SMALL_BUTTON = "rounded-md px-1.5 py-0.5 text-xs text-fg-muted hover:bg-bg-muted hover:text-fg";

/** A run card's button: Cancel, Stop, *Get it anyway*, Again… */
export const CARD_BUTTON =
  "rounded-lg border border-border bg-bg px-2.5 py-1 text-sm hover:border-border-strong hover:bg-bg-muted disabled:opacity-60";

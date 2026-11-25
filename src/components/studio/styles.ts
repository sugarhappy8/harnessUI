// The class strings Studio's right panel and run cards share (0.1.0 Tasks B5, B6 and B8), in
// one place so the panel's fields, Advanced drawer and My setups draw their boxes and small
// buttons alike, and a card's buttons look the same wherever on the card they are drawn; and
// the one primary button Get local models' rows and search results share (Task B13).

import { FOCUS_RING } from "../ui/styles";

/** A text box, number box, select or text area. */
export const BOX = `rounded-md border border-border bg-bg px-2 py-1 text-sm focus:border-border-strong disabled:opacity-60 ${FOCUS_RING}`;

/** A small text button: *Reset all*, *Edit JSON*, *Save as setup…*, *Cancel*. */
export const SMALL_BUTTON = "rounded-md px-1.5 py-0.5 text-xs text-fg-muted hover:bg-bg-muted hover:text-fg";

/** A run card's button: Cancel, Stop, *Get it anyway*, Again… */
export const CARD_BUTTON =
  "rounded-lg border border-border bg-bg px-2.5 py-1 text-sm hover:border-border-strong hover:bg-bg-muted disabled:opacity-60";

/** The main action of a row: *Download* in Get local models' rows and search results. */
export const PRIMARY_BUTTON = "btn-primary rounded-lg px-3 py-1 text-sm font-medium";

/** A secondary button: *Get the list*, *Open Settings*, the model notices' actions. */
export const SECONDARY_BUTTON =
  "rounded-lg border border-border bg-bg px-2.5 py-1.5 text-sm hover:border-border-strong hover:bg-bg-muted disabled:opacity-60";

/** A run's card: a flex column. */
export const RUN_CARD = "enter flex flex-col gap-1.5 rounded-xl border border-border bg-surface p-3.5 shadow-soft";

/** A notice's card above the card stream. */
export const NOTICE_CARD = "enter rounded-xl border border-border bg-surface p-3.5 shadow-soft";

/** A chip that is checked or not (`aria-checked`): a setting's choices. */
export const CHIP_CHECKED =
  "rounded-full border border-border px-2 py-0.5 text-xs whitespace-nowrap hover:border-border-strong aria-checked:border-accent aria-checked:text-accent aria-expanded:border-border-strong";

/** A row of a menu. */
export const MENU_ITEM =
  "flex cursor-default items-center gap-2 rounded-lg px-2.5 py-1.5 text-sm outline-none data-[disabled]:text-fg-muted data-[highlighted]:bg-bg-muted";

/** A model's row card in Get local models. */
export const ROW = "flex flex-col gap-1.5 rounded-xl border border-border bg-surface p-3.5 shadow-soft";

/** A control in the composer's toolbar (Task U3): the model chip, Shape, How many and
 *  Settings, as soft pills on the composer's card. */
export const COMPOSER_CHIP =
  "inline-flex h-8 min-w-0 shrink-0 items-center gap-1.5 rounded-full bg-bg-muted/70 px-2.5 text-sm text-fg " +
  "hover:bg-bg-muted aria-expanded:bg-bg-muted data-[state=open]:bg-bg-muted disabled:opacity-60 " +
  FOCUS_RING;

/** A tile in the composer's picture strip (Task U3): an added picture, *Add picture*, or an
 *  input picture's zone. */
export const PICTURE_TILE = "flex h-12 shrink-0 items-center gap-2 rounded-xl border text-sm";

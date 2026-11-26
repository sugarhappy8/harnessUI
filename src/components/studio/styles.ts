// The class strings Studio's right panel and run cards share (0.1.0 Tasks B5, B6 and B8), in
// one place so the panel's fields, Advanced drawer and My setups draw their boxes and small
// buttons alike, and a card's buttons look the same wherever on the card they are drawn; and
// the one primary button Get models' cards and an opened result share (Task B13).

import { FOCUS_RING } from "../ui/styles";

/** A text box, number box, select or text area. */
export const BOX = `rounded-md border border-border bg-bg px-2 py-1 text-sm focus:border-border-strong disabled:opacity-60 ${FOCUS_RING}`;

/** A small text button: *Reset all*, *Edit JSON*, *Save as setup…*, *Cancel*. */
export const SMALL_BUTTON = `rounded-md px-1.5 py-0.5 text-xs text-fg-muted hover:bg-bg-muted hover:text-fg ${FOCUS_RING}`;

/** A run card's button: Cancel, Stop, *Check again*; the viewer's and the grid's buttons. With
 *  the one focus ring (Task U9), not the browser's outline. */
export const CARD_BUTTON = `rounded-lg border border-border bg-bg px-2.5 py-1 text-sm hover:border-border-strong hover:bg-bg-muted disabled:opacity-60 ${FOCUS_RING}`;

/** The main action of a card: *Download* in Get models' cards and an opened result. */
export const PRIMARY_BUTTON = "btn-primary rounded-lg px-3 py-1 text-sm font-medium";

/** A secondary button: *Get the list*, *Open Settings*, the model notices' actions. */
export const SECONDARY_BUTTON = `rounded-lg border border-border bg-bg px-2.5 py-1.5 text-sm hover:border-border-strong hover:bg-bg-muted disabled:opacity-60 ${FOCUS_RING}`;

/** A run's card: a flex column, its pictures first (Task U9). */
export const RUN_CARD = "enter flex flex-col gap-3 rounded-2xl border border-border bg-surface p-4 shadow-soft";

/** A notice's card above the card stream, and an empty state's (Task U9: an icon, one line and
 *  one primary action). */
export const NOTICE_CARD = "enter rounded-2xl border border-border bg-surface p-4 shadow-soft";

/** A row of a menu. */
export const MENU_ITEM =
  "flex cursor-default items-center gap-2 rounded-lg px-2.5 py-1.5 text-sm outline-none data-[disabled]:text-fg-muted data-[highlighted]:bg-bg-muted";

/** A card of Get models (Task U5): a catalog model, a Hugging Face result, a model or LoRA on
 *  this computer. */
export const MODEL_CARD = "flex min-w-0 flex-col gap-2.5 rounded-2xl border border-border bg-surface p-4 shadow-soft";

/** Get models' grid of cards: two or three columns in its width, one on the narrowest. */
export const MODEL_GRID = "grid grid-cols-[repeat(auto-fill,minmax(min(100%,17rem),1fr))] gap-3";

/** A control in the composer's toolbar (Task U3): the model chip, Shape, How many and
 *  Settings, as soft pills on the composer's card. */
export const COMPOSER_CHIP =
  "inline-flex h-8 min-w-0 shrink-0 items-center gap-1.5 rounded-full bg-bg-muted/70 px-2.5 text-sm text-fg " +
  "hover:bg-bg-muted aria-expanded:bg-bg-muted data-[state=open]:bg-bg-muted disabled:opacity-60 " +
  FOCUS_RING;

/** A tile in the composer's picture strip (Task U3): an added picture, *Add picture*, or an
 *  input picture's zone. */
export const PICTURE_TILE = "flex h-12 shrink-0 items-center gap-2 rounded-xl border text-sm";

/** A section of Studio's side panel (Task U8): My setups, Advanced, LoRAs and Files, each under
 *  a hairline, scrolled to with room above it. */
export const PANEL_SECTION = "flex scroll-mt-4 flex-col gap-4 border-t border-border pt-5";

/** A chip of a short list in the side panel, the chosen one in the accent (`data-state`). */
export const CHOICE_CHIP =
  "rounded-full border border-border bg-surface px-3 py-1 text-sm whitespace-nowrap hover:border-border-strong " +
  "data-[state=checked]:border-accent data-[state=checked]:bg-accent/10 data-[state=checked]:font-medium " +
  "data-[state=checked]:text-accent " +
  FOCUS_RING;

/** A small icon button: a field's reset or help, a LoRA's Remove. */
export const ICON_BUTTON =
  "inline-flex size-7 shrink-0 items-center justify-center rounded-md text-fg-muted hover:bg-bg-muted hover:text-fg " +
  "disabled:opacity-60 " +
  FOCUS_RING;

/** A quiet text action in the side panel: *Reset all*, *Cancel*. */
export const GHOST_BUTTON =
  "inline-flex items-center gap-1.5 rounded-md px-2 py-1 text-sm text-fg-muted hover:bg-bg-muted hover:text-fg " +
  "disabled:opacity-60 " +
  FOCUS_RING;

/** A secondary action in the side panel, with its icon: *Save as setup…*, *See the exact
 *  command*, *Add a LoRA file*, *Change*. */
export const PANEL_BUTTON =
  "inline-flex items-center gap-1.5 rounded-lg border border-border bg-surface px-2.5 py-1.5 text-sm " +
  "hover:border-border-strong hover:bg-bg-muted disabled:opacity-60 data-[state=open]:border-border-strong " +
  FOCUS_RING;

/** A card in the side panel that holds a list: the LoRAs attached, the model's parts. */
export const PANEL_CARD = "rounded-xl border border-border bg-bg-subtle/70";

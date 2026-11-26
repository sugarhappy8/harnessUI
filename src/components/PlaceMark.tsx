import type { LucideIcon } from "lucide-react";
import { placeMarkSpoken } from "../lib/words";
import { FOCUS_RING } from "./ui/styles";
import { Tip } from "./ui/Tip";

// The chip that says where a chat or a Studio run goes: the chat header's Cloud mark
// (CloudMark), Studio's mark for a model on this computer (LocalMark) and its chip for a
// cloud model, which names the endpoint's host.

/** An icon and a few words in a small round chip; `title` is the tooltip, which a screen
 *  reader hears after the words. `data-cloud-mark` or `data-local-mark` by `mark`. With `tip`
 *  the tooltip is the app's own (Radix, under `UiProviders`), on hover and on keyboard focus, in
 *  place of the browser's `title` (Studio's viewer, Task U9). */
export function PlaceMark(props: {
  mark: "cloud" | "local";
  icon: LucideIcon;
  label: string;
  title?: string;
  tip?: boolean;
  className?: string;
}) {
  const Icon = props.icon;
  const data = { [`data-${props.mark}-mark`]: true };
  const tip = props.tip === true && props.title !== undefined;
  const chip = (
    <span
      {...data}
      title={tip ? undefined : props.title}
      tabIndex={tip ? 0 : undefined}
      className={
        "flex shrink-0 items-center gap-1 rounded-full border border-border px-1.5 text-xs whitespace-nowrap text-fg-muted" +
        (tip ? ` ${FOCUS_RING}` : "") +
        (props.className ? ` ${props.className}` : "")
      }
    >
      <Icon size={12} aria-hidden />
      {props.label}
      {props.title && <span className="sr-only">{placeMarkSpoken(props.title)}</span>}
    </span>
  );
  return tip && props.title ? <Tip content={props.title}>{chip}</Tip> : chip;
}

import type { LucideIcon } from "lucide-react";
import { placeMarkSpoken } from "../lib/words";

// The chip that says where a chat or a Studio run goes: the chat header's Cloud mark
// (CloudMark), Studio's mark for a model on this computer (LocalMark) and its chip for a
// cloud model, which names the endpoint's host.

/** An icon and a few words in a small round chip; `title` is the tooltip, which a screen
 *  reader hears after the words. `data-cloud-mark` or `data-local-mark` by `mark`. */
export function PlaceMark(props: {
  mark: "cloud" | "local";
  icon: LucideIcon;
  label: string;
  title?: string;
  className?: string;
}) {
  const Icon = props.icon;
  const data = { [`data-${props.mark}-mark`]: true };
  return (
    <span
      {...data}
      title={props.title}
      className={
        "flex shrink-0 items-center gap-1 rounded-full border border-border px-1.5 text-xs whitespace-nowrap text-fg-muted" +
        (props.className ? ` ${props.className}` : "")
      }
    >
      <Icon size={12} aria-hidden />
      {props.label}
      {props.title && <span className="sr-only">{placeMarkSpoken(props.title)}</span>}
    </span>
  );
}

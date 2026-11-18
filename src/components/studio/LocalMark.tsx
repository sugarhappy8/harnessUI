import { Microchip } from "lucide-react";
import { LOCAL_MARK } from "../../lib/words";

// [0.1.0 Studio] The mark where a model runs on this computer, in the place the Cloud mark
// takes for a cloud model: nothing of the run leaves the computer.

/** "On this computer", with what that means in its tooltip. */
export function LocalMark() {
  return (
    <span
      data-local-mark
      title={LOCAL_MARK.title}
      className="flex shrink-0 items-center gap-1 rounded-full border border-border px-1.5 text-xs whitespace-nowrap text-fg-muted"
    >
      <Microchip size={12} aria-hidden />
      {LOCAL_MARK.name}
      <span className="sr-only">: {LOCAL_MARK.title}</span>
    </span>
  );
}

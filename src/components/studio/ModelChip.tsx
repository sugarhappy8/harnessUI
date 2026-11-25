import { ChevronDown } from "lucide-react";
import { STUDIO_COMPANY, STUDIO_COMPOSER, STUDIO_PICKER } from "../../lib/words";
import { draftModel, endpointOf, shownEstimate, useStudio, type StudioTab } from "../../store/studio";
import { FOCUS_RING } from "../ui/styles";
import { Tip } from "../ui/Tip";
import { placeLine } from "./cost";

// The composer's model chip (0.2.0 Task U3, design 10.1): the draft's model by name, where it
// runs (this computer, or the cloud endpoint's company), and a dot for that place whose tooltip
// is the privacy line the composer also shows under itself. Pressing the chip opens the model
// picker anchored to it (Task U4 replaces the picker with the model sheet).

/** The model chip of `tab`; `onOpen` opens the model picker. */
export function ModelChip(props: { tab: StudioTab; onOpen(): void }) {
  const { tab } = props;
  const target = useStudio((s) => s.drafts[tab].target);
  const model = useStudio((s) => draftModel(s, tab));
  const endpoint = useStudio((s) => endpointOf(s.drafts[tab].target, s.endpoints));
  const known = useStudio((s) => shownEstimate(s, tab)?.estimate ?? null);
  const here = target === "thisComputer";
  const place = target === null ? null : here ? STUDIO_COMPANY.local : (endpoint?.name ?? null);

  return (
    <div
      data-model-chip
      className="inline-flex h-8 min-w-0 items-center rounded-full bg-bg-muted/70 hover:bg-bg-muted"
    >
      {target !== null && (
        <Tip content={placeLine(target, endpoint?.host ?? "", known)}>
          {/* Focusable, so the tooltip is there from the keyboard too. */}
          <span
            role="img"
            tabIndex={0}
            aria-label={STUDIO_COMPOSER.privacy}
            className={`ml-1 grid size-6 shrink-0 place-items-center rounded-full ${FOCUS_RING}`}
          >
            <span aria-hidden className={`size-2 rounded-full ${here ? "bg-success" : "bg-accent"}`} />
          </span>
        </Tip>
      )}
      <button
        type="button"
        aria-haspopup="dialog"
        onClick={props.onOpen}
        className={`flex h-8 min-w-0 items-center gap-1.5 rounded-full pr-2.5 text-sm ${target !== null ? "pl-1" : "pl-3"} ${FOCUS_RING}`}
      >
        <span className="max-w-[16rem] min-w-0 truncate font-medium">{model?.displayName ?? STUDIO_PICKER.choose}</span>
        {/* On a narrow composer the place is left to the dot, and still read out. */}
        {place && <span className="shrink-0 text-fg-muted @max-2xl:sr-only">{place}</span>}
        <ChevronDown size={14} aria-hidden className="shrink-0 text-fg-muted" />
      </button>
    </div>
  );
}

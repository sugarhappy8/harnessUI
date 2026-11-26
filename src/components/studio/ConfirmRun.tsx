import { useState } from "react";
import { AlertDialog } from "radix-ui";
import { STUDIO_COMPANY, STUDIO_CONFIRM, STUDIO_DOCK, STUDIO_PRICE, studioDay } from "../../lib/words";
import { endpointOf, listedModel, useStudio, type StudioTab } from "../../store/studio";
import { valueWords, type Values } from "./canonical";
import { formatUsd } from "./cost";
import type { Canonical, Estimate, MediaKind, MediaModel, ParamSpec, StartRequest, StudioEndpoint } from "./types";

// Studio's confirmation before a cloud run (0.1.0 Task B7; design Wireframe 3): every cloud
// video, a run estimated at the amount set in Settings → Studio ($1 by default) or more, and a
// run whose price is unknown. It shows the press waiting to be sent (`pending`, never the draft,
// which may have changed or may not be what was pressed) with the estimate asked for it: the
// model and where it goes, its length, resolution, size, sound and count, the estimate with
// whose price list it comes from, where the prompt goes and, for a video, that a started video
// is charged. *Don't ask again for videos under $X* is offered only when a video is the only
// reason it asks. A run on this computer never gets here.

/** The settings the confirmation names, in its order. */
const NAMED: readonly Canonical[] = ["length", "resolution", "size"];

/** The value a request gives `field`: the one set, else what the company uses without one. */
function valueOf(field: ParamSpec, params: Values): unknown {
  return params[field.key] ?? field.companyDefault ?? null;
}

/** How many pictures or videos a request makes (1 when the model has no such field, or it
 *  is left to the company). */
function countOf(model: MediaModel | null, params: Values): number {
  const field = model?.params.find((p) => p.canonical === "count");
  const n = field ? valueOf(field, params) : null;
  return typeof n === "number" ? n : 1;
}

/** The request's settings in words: "8 s · 720p · sound on · 1 video". */
function settingsWords(model: MediaModel | null, params: Values, kind: MediaKind): string {
  const parts: string[] = [];
  for (const canonical of NAMED) {
    const field = model?.params.find((p) => p.canonical === canonical);
    const value = field ? valueOf(field, params) : null;
    if (field && value !== null) parts.push(valueWords(value, field));
  }
  const sound = model?.params.find((p) => p.canonical === "sound");
  const on = sound ? valueOf(sound, params) : null;
  if (typeof on === "boolean") parts.push(STUDIO_CONFIRM.sound(on));
  parts.push(STUDIO_CONFIRM.count(countOf(model, params), kind));
  return parts.join(STUDIO_CONFIRM.joiner);
}

/** Asks before the press waiting in `pending` is sent; `onConfirm` sends it, `onCancel` (also
 *  Esc) sends nothing. */
export function ConfirmRun(props: {
  open: boolean;
  tab: StudioTab;
  onCancel(): void;
  onConfirm(dontAskVideos: boolean): void;
}) {
  const pending = useStudio((s) => s.pending);
  const estimate = useStudio((s) => s.pendingEstimate);
  const local = useStudio((s) => s.local);
  const endpoints = useStudio((s) => s.endpoints);
  const shown = props.open && pending !== null && estimate !== null;
  return (
    <AlertDialog.Root
      open={shown}
      onOpenChange={(open) => {
        if (!open) props.onCancel();
      }}
    >
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="dialog-overlay fixed inset-0" />
        <AlertDialog.Content className="fixed top-1/2 left-1/2 w-[28rem] max-w-[92vw] -translate-x-1/2 -translate-y-1/2 rounded-2xl border border-border bg-surface p-5 text-sm shadow-pop">
          {shown && (
            // A new press starts with the box unticked.
            <Body
              key={pending.clientToken}
              kind={props.tab}
              req={pending}
              estimate={estimate}
              model={listedModel(pending.target, pending.model, local, endpoints)}
              endpoint={endpointOf(pending.target, endpoints)}
              onConfirm={props.onConfirm}
            />
          )}
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}

function Body(props: {
  kind: MediaKind;
  req: StartRequest;
  estimate: Estimate;
  model: MediaModel | null;
  endpoint: StudioEndpoint | null;
  onConfirm(dontAskVideos: boolean): void;
}) {
  const { kind, req, estimate, model, endpoint } = props;
  const [dontAsk, setDontAsk] = useState(false);
  const company = endpoint ? STUDIO_COMPANY[endpoint.company] : null;
  const fetchedAt = model?.priceFetchedAt ?? endpoint?.fetchedAt ?? null;
  const name = model?.displayName ?? req.model;
  // Only a video asks: the user may stop being asked for videos under the amount.
  const videoOnly = estimate.reasons.length === 1 && estimate.reasons[0] === "video";
  return (
    <>
      <AlertDialog.Title className="text-base font-semibold">
        {STUDIO_CONFIRM.title(kind, countOf(model, req.params))}
      </AlertDialog.Title>
      <AlertDialog.Description asChild>
        <div className="mt-2 flex flex-col gap-1.5">
          <p className="font-medium">{endpoint ? STUDIO_CONFIRM.through(name, endpoint.name) : name}</p>
          <p className="text-fg-muted">{settingsWords(model, req.params, kind)}</p>
          <p className="flex flex-wrap gap-x-2">
            <span className="text-fg-muted">{STUDIO_CONFIRM.estimate}</span>
            <span className="font-medium tabular-nums">
              {estimate.usd === null ? STUDIO_PRICE.unknown : formatUsd(estimate.usd)}
            </span>
            {company && estimate.usd !== null && (
              <span className="text-fg-muted">
                {STUDIO_CONFIRM.priceList(company, fetchedAt === null ? null : studioDay(fetchedAt))}
              </span>
            )}
          </p>
          {kind === "video" && company && <p>{STUDIO_CONFIRM.videoCharge(company)}</p>}
          {endpoint && (
            <p className="text-fg-muted">
              {STUDIO_CONFIRM.goesTo(endpoint.host, req.refs.length > 0, endpoint.company === "openrouter")}
            </p>
          )}
        </div>
      </AlertDialog.Description>
      {videoOnly && (
        <label className="mt-3 flex items-center gap-2">
          <input type="checkbox" checked={dontAsk} onChange={(e) => setDontAsk(e.target.checked)} />
          {STUDIO_CONFIRM.dontAskVideos(estimate.confirmAtUsd === null ? null : formatUsd(estimate.confirmAtUsd))}
        </label>
      )}
      <div className="mt-4 flex justify-end gap-2">
        <AlertDialog.Cancel className="rounded-md border border-border px-3 py-1.5 hover:bg-bg-muted">
          {STUDIO_CONFIRM.cancel}
        </AlertDialog.Cancel>
        {/* Not an AlertDialog.Action: closing the dialog would count as Cancel. */}
        <button
          type="button"
          onClick={() => props.onConfirm(videoOnly && dontAsk)}
          className="btn-primary rounded-md px-3 py-1.5 font-medium"
        >
          {kind === "video" ? STUDIO_CONFIRM.makeVideo : STUDIO_DOCK.create}
        </button>
      </div>
    </>
  );
}

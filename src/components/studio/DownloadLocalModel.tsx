import { useState } from "react";
import { AlertDialog } from "radix-ui";
import { BrowserLink, modelPage } from "./BrowserLink";
import type { ErrorInfo } from "../../lib/types";
import { amountGb, DOWNLOAD_LOCAL, REGIONAL_USE, engineSizeWords, licenceWord, revenueLimit } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { useStudio } from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import { visiblePath } from "../FolderChip";
import { ENGINE_LICENCE_ID, type DownloadOffer } from "./localOffers";
import type { Licence, MediaKind, Part } from "./types";

// The download confirmation of a local model (0.1.0 Task B14, local addendum Wireframe L3),
// shown for the store's `offer` before anything is fetched: every part with what it does, its
// size and its licence; what the pictures (or videos) you make may be used for (the strictest
// licence); the Studio engine when it comes with the model; a gated repository's form, what its
// publisher receives and *Open the model page*; an amber licence's condition; low disk space;
// Licence links are informational. Download confirms the file transfer.

/** How strict a licence is for what you make, from the least strict (the backend's
 *  `Commercial` order). */
const STRICTNESS: Record<Licence["commercial"], number> = {
  yes: 0,
  underRevenue: 1,
  registration: 2,
  unclear: 3,
  no: 4,
};

/** `l` is stricter than `than`: a stricter kind, or under a lower revenue limit (an unknown
 *  limit counts as none), as the backend's `gen_catalog::strictness` judges. */
function stricter(l: Licence, than: Licence): boolean {
  const a = STRICTNESS[l.commercial];
  const b = STRICTNESS[than.commercial];
  if (a !== b) return a > b;
  return l.commercial === "underRevenue" && (l.revenueLimitUsd ?? 0) < (than.revenueLimitUsd ?? 0);
}

/** What the pictures (a `kind` "video" model: the videos) you make may be used for, by the
 *  strictest of `licences` (the first of equals): "Pictures you make: free for any use,
 *  including commercial.", the amber line or the red line; "" without a licence. */
export function verdict(licences: Licence[], kind: MediaKind): string {
  const strictest = licences.reduce<Licence | null>((best, l) => (best === null || stricter(l, best) ? l : best), null);
  if (strictest === null) return "";
  if (strictest.commercial === "yes" && licences.some((l) => l.excludedRegions?.length)) {
    return DOWNLOAD_LOCAL.verdictLine(kind, REGIONAL_USE.toLowerCase() + ".");
  }
  const words = DOWNLOAD_LOCAL.verdict[strictest.commercial];
  const allowed =
    typeof words === "string"
      ? words
      : words(strictest.revenueLimitUsd === null ? null : revenueLimit(strictest.revenueLimitUsd));
  return DOWNLOAD_LOCAL.verdictLine(kind, allowed);
}

/** What a licence's link says it opens: its name; the Studio engine's by its short word, since
 *  its name is the engine's whole licence line ("stable-diffusion.cpp MIT; ggml MIT; …" reads
 *  "MIT", ruling B8-engine-words). */
function linkName(l: Licence): string {
  return l.id === ENGINE_LICENCE_ID ? licenceWord(l.name) : l.name;
}

/** The parts that make the picture: a gated model's page is its repository's. */
const MAKES_THE_PICTURE: ReadonlySet<Part["role"]> = new Set(["diffusion", "high-noise-diffusion", "model"]);

/** The confirmation for `offer`, while there is one; Cancel or Esc calls `onClose`, except
 *  while the download is starting. */
export function DownloadLocalModel(props: { offer: DownloadOffer | null; onClose(): void }) {
  const { offer, onClose } = props;
  const [busy, setBusy] = useState(false);
  return (
    <AlertDialog.Root open={offer !== null} onOpenChange={(open) => !open && !busy && onClose()}>
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="dialog-overlay fixed inset-0" />
        {offer && <Confirm offer={offer} busy={busy} setBusy={setBusy} />}
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}

const MUTED = "text-fg-muted";

/** The dialog's content: what downloads and under which licences, and Download (`busy` while
 *  it is on its way). */
function Confirm(props: { offer: DownloadOffer; busy: boolean; setBusy(busy: boolean): void }) {
  const { offer, busy, setBusy } = props;
  const downloadLocal = useStudio((s) => s.downloadLocal);
  const engineLicence = useStudio((s) => s.local?.engine.licenseName ?? "");
  const [error, setError] = useState<ErrorInfo | null>(null);

  const licenceOf = new Map(offer.licences.map((l) => [l.id, l]));
  // What you make answers to the parts' licences; the engine's is about the engine itself.
  const partLicences = offer.licences.filter((l) => l.id !== ENGINE_LICENCE_ID);
  const said = verdict(partLicences, offer.kind);
  const amber = offer.licences.filter((l) => l.badge === "amber");
  const gate = offer.gate.mode;
  const pageRepo = (offer.parts.find((p) => MAKES_THE_PICTURE.has(p.role)) ?? offer.parts[0])?.repo;
  const several = offer.licences.length > 1;

  function download() {
    setBusy(true);
    setError(null);
    downloadLocal(offer)
      .catch((e: unknown) => setError(toErrorInfo(e)))
      .finally(() => setBusy(false));
  }

  return (
    <AlertDialog.Content className="fixed top-1/2 left-1/2 flex max-h-[90vh] w-[32rem] max-w-[92vw] -translate-x-1/2 -translate-y-1/2 flex-col rounded-2xl border border-border bg-surface p-5 text-sm shadow-pop">
      <AlertDialog.Title className="text-base font-semibold">
        {/* A searched file's name can hold any character: shown with invisible ones as codes. */}
        <bdi dir="ltr">{DOWNLOAD_LOCAL.title(visiblePath(offer.name))}</bdi>
      </AlertDialog.Title>
      <AlertDialog.Description asChild>
        <div className="mt-2 flex min-h-0 flex-col gap-2 overflow-y-auto">
          {offer.parts.length > 0 && (
            <>
              <p className={MUTED}>
                {DOWNLOAD_LOCAL.summary(offer.parts.length, offer.totalBytes, offer.toFetchBytes)}
              </p>
              <ul className="flex flex-col gap-1">
                {offer.parts.map((part) => {
                  const licence = licenceOf.get(part.licenceId);
                  return (
                    <li key={`${part.repo}/${part.path}`} className="flex flex-wrap items-baseline gap-x-3">
                      <span className="min-w-0 flex-1">
                        <span>{part.label}</span>
                        {part.alreadyHere && (
                          <span className={MUTED}>
                            <span aria-hidden> · </span>
                            {DOWNLOAD_LOCAL.alreadyHere}
                          </span>
                        )}
                      </span>
                      <span className={`${MUTED} tabular-nums`}>{amountGb(part.sizeBytes)}</span>
                      {licence && <span title={licence.plainWords}>{licence.name}</span>}
                    </li>
                  );
                })}
              </ul>
            </>
          )}
          {said !== "" && <p className="font-medium">{said}</p>}
          {offer.needsEngine && (
            <p title={engineLicence}>{DOWNLOAD_LOCAL.engine(engineSizeWords(offer.engineBytes, engineLicence))}</p>
          )}
          {gate !== "none" && (
            <div className="flex flex-col gap-1">
              <p>{DOWNLOAD_LOCAL.gate[gate]}</p>
              {offer.gate.collects.length > 0 && <p>{DOWNLOAD_LOCAL.collects(offer.gate.collects)}</p>}
              {pageRepo && (
                <p>
                  <BrowserLink href={modelPage(pageRepo)} onError={setError}>
                    {DOWNLOAD_LOCAL.openModelPage}
                  </BrowserLink>
                </p>
              )}
            </div>
          )}
          {amber.map((l) => (
            <p key={l.id} className="text-warning">
              {DOWNLOAD_LOCAL.amber(l)}
            </p>
          ))}
          {offer.lowDisk && <p className="text-warning">{DOWNLOAD_LOCAL.lowDisk}</p>}
          <p className="flex flex-wrap gap-x-3 gap-y-1">
            {offer.licences.map((l) => (
              <BrowserLink key={l.id} href={l.url} title={l.name} onError={setError}>
                {DOWNLOAD_LOCAL.readLicences(several ? linkName(l) : null)}
              </BrowserLink>
            ))}
          </p>
          {offer.licences.map((l) =>
            l.excludedRegions?.length ? (
              <p key={l.id} className="text-warning">{DOWNLOAD_LOCAL.excludedRegions(l.name, l.excludedRegions)}</p>
            ) : null,
          )}
        </div>
      </AlertDialog.Description>
      {error && (
        <div className="mt-3">
          <ErrorBanner error={error} onDismiss={() => setError(null)} />
        </div>
      )}
      <div className="mt-4 flex justify-end gap-2">
        <AlertDialog.Cancel
          disabled={busy}
          className="rounded-md border border-border px-3 py-1.5 hover:bg-bg-muted disabled:opacity-60"
        >
          {DOWNLOAD_LOCAL.cancel}
        </AlertDialog.Cancel>
        <button
          type="button"
          disabled={busy}
          onClick={download}
          className="btn-primary rounded-md px-3 py-1.5 font-medium disabled:opacity-60"
        >
          {DOWNLOAD_LOCAL.download}
        </button>
      </div>
    </AlertDialog.Content>
  );
}

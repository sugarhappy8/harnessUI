import { useEffect, useId, useState } from "react";
import { Tabs } from "radix-ui";
import type { ErrorInfo } from "../../lib/types";
import { amountGb, LOCAL_MODELS } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { followDownloads } from "../../store/downloads";
import { useStudio, type StudioTab } from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import { DownloadLocalModel } from "./DownloadLocalModel";
import { EngineLine } from "./EngineLine";
import { KindTabs } from "./KindTabs";
import { EntryRow, InstalledRow } from "./LocalModelRow";
import type { DownloadOffer } from "./localOffers";
import { LocalNotice } from "./LocalNotice";
import { LocalSearch } from "./LocalSearch";
import { CARD_BUTTON } from "./styles";
import type { HardwareLine, LocalEntry, LocalModels as LocalModelsData } from "./types";

// Get local models (0.1.0 Task B13, local addendum Wireframe L2): Studio's own view, in place of
// the card stream while `localView` is on, with *Back*. The header says what this computer has,
// so the estimates can be checked; its own [Pictures | Video] tabs (KindTabs.tsx, as the top
// bar's, which give way to them), *Only what fits* (on);
// the recommended models of the tab, then the installed ones the list does not hold, then "Not
// for this computer (n)" folded away; Hugging Face search (LocalSearch.tsx); and the footer
// with the Studio engine's line (EngineLine.tsx) and the content notice. Opening it reads what
// is saved and sends nothing. *Download* sets the store's `offer` (localOffers.ts), which shows
// the download confirmation (DownloadLocalModel.tsx, Task B14), or the content notice first
// (LocalNotice.tsx) while it was never accepted; *What that means* shows the notice again, read
// only. While it is open it follows the app's downloads (store/downloads.ts), so a model that
// downloads shows how far it got on its row, and once it is done the local models are read
// again (it is installed now).

/** What this computer has, as the header says it: "This computer: Radeon RX 7600, 8 GB graphics
 *  memory · 32 GB memory · 412 GB free"; on shared memory the computer's memory only (never the
 *  graphics working set): "This computer: Apple M5, 16 GB shared memory · 180 GB free"; without
 *  a graphics card "This computer: processor only · …". A figure not known is left out. */
export function hardwareLine(h: HardwareLine): string {
  let what: string;
  const parts: string[] = [];
  if (h.sharedMemory) {
    what = h.ramBytes === null ? h.device : LOCAL_MODELS.deviceWith(h.device, LOCAL_MODELS.sharedMemory(amountGb(h.ramBytes)));
  } else {
    what =
      h.graphicsBytes === null
        ? LOCAL_MODELS.processorOnly
        : LOCAL_MODELS.deviceWith(h.device, LOCAL_MODELS.graphicsMemory(amountGb(h.graphicsBytes)));
    if (h.ramBytes !== null) parts.push(LOCAL_MODELS.memory(amountGb(h.ramBytes)));
  }
  if (h.freeDiskBytes !== null) parts.push(LOCAL_MODELS.freeDisk(amountGb(h.freeDiskBytes)));
  return [LOCAL_MODELS.thisComputer(what), ...parts].join(" · ");
}

/** The entries of `tab`'s kind: with *Only what fits* on, the recommended ones are shown and
 *  those that do not fit this computer are folded away (`notHere`); off, they are listed too. */
export function visibleEntries(
  local: LocalModelsData,
  tab: StudioTab,
  onlyFits: boolean,
): { shown: LocalEntry[]; notHere: LocalEntry[] } {
  const recommended = local.recommended.filter((e) => e.kind === tab);
  const notHere = local.notHere.filter((e) => e.kind === tab);
  return onlyFits ? { shown: recommended, notHere } : { shown: [...recommended, ...notHere], notHere: [] };
}

/** Get local models, with *Back* to the cards. */
export function LocalModels() {
  const local = useStudio((s) => s.local);
  const localTab = useStudio((s) => s.localTab);
  const setLocalTab = useStudio((s) => s.setLocalTab);
  const onlyFits = useStudio((s) => s.onlyFits);
  const setOnlyFits = useStudio((s) => s.setOnlyFits);
  const showLocalModels = useStudio((s) => s.showLocalModels);
  const noticeOpen = useStudio((s) => s.noticeOpen);
  const setNoticeOpen = useStudio((s) => s.setNoticeOpen);
  const offer = useStudio((s) => s.offer);
  const setOffer = useStudio((s) => s.setOffer);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const titleId = useId();
  const fail = (e: unknown) => setError(toErrorInfo(e));

  useEffect(
    () =>
      followDownloads((group) => {
        // A picture or video model that finished downloading is installed now.
        if (group.genModelId && group.status === "done") {
          useStudio
            .getState()
            .refreshLocal()
            .catch((e: unknown) => console.warn("Studio: the local models could not be read again", e));
        }
      }),
    [],
  );

  return (
    <section data-local-models aria-labelledby={titleId} className="flex flex-col gap-3">
      <div className="flex items-center gap-2">
        <h2 id={titleId} className="text-base font-semibold">
          {LOCAL_MODELS.title}
        </h2>
        <button type="button" className={`${CARD_BUTTON} ml-auto`} onClick={() => showLocalModels(false)}>
          {LOCAL_MODELS.back}
        </button>
      </div>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      {local && (
        <>
          <p className="text-sm text-fg-muted">{hardwareLine(local.hardware)}</p>
          <Tabs.Root
            value={localTab}
            onValueChange={(value) => setLocalTab(value as StudioTab)}
            className="flex flex-col gap-3"
          >
            <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
              <KindTabs onSelect={setLocalTab} />
              <label className="ml-auto flex items-center gap-1.5 text-sm">
                <input type="checkbox" checked={onlyFits} onChange={(e) => setOnlyFits(e.target.checked)} />
                {LOCAL_MODELS.onlyFits}
              </label>
            </div>
            <Tabs.Content value={localTab} className="flex flex-col gap-3 outline-none">
              {/* Keyed by tab: "Not for this computer" starts folded on each. */}
              <TabModels key={`models-${localTab}`} local={local} tab={localTab} onlyFits={onlyFits} onError={fail} />
              {/* Each tab searches for its own kind. The store keeps only the last search, so its
                  words and results show on the tab it was for, and a search on the other tab
                  takes their place. */}
              <LocalSearch key={`search-${localTab}`} tab={localTab} />
            </Tabs.Content>
          </Tabs.Root>
          <footer className="flex flex-col gap-2 border-t border-border pt-3">
            <EngineLine engine={local.engine} hasModel={local.installed.length > 0} removable />
            <p className="text-sm text-fg-muted">
              {LOCAL_MODELS.notice}{" "}
              <button type="button" className="underline hover:text-fg" onClick={() => setNoticeOpen("read")}>
                {LOCAL_MODELS.whatThatMeans}
              </button>
            </p>
          </footer>
          <Offered
            offer={offer}
            noticeAccepted={local.noticeAccepted}
            readingNotice={noticeOpen === "read"}
            onCloseOffer={() => setOffer(null)}
            onCloseNotice={() => setNoticeOpen(null)}
          />
        </>
      )}
    </section>
  );
}

/** The download confirmation for `offer`, and the content notice: in its place, to accept,
 *  while the notice was never accepted (accepting it shows the confirmation), and read only
 *  from the footer. */
function Offered(props: {
  offer: DownloadOffer | null;
  noticeAccepted: boolean;
  readingNotice: boolean;
  onCloseOffer: () => void;
  onCloseNotice: () => void;
}) {
  const { offer, onCloseOffer, onCloseNotice } = props;
  const accepting = offer !== null && !props.noticeAccepted;
  return (
    <>
      <LocalNotice
        open={accepting || props.readingNotice}
        readOnly={!accepting}
        // The store records it, and the confirmation follows from the offer.
        onAccepted={() => undefined}
        onClose={accepting ? onCloseOffer : onCloseNotice}
      />
      <DownloadLocalModel offer={accepting ? null : offer} onClose={onCloseOffer} />
    </>
  );
}

/** The models of `tab`: the recommended ones, the installed ones the list does not hold (from
 *  a search, an entry that left the catalog, one this version cannot read), whether or not
 *  *Only what fits* is on, and the ones that do not fit, folded away. */
function TabModels(props: {
  local: LocalModelsData;
  tab: StudioTab;
  onlyFits: boolean;
  onError: (e: unknown) => void;
}) {
  const { local, tab, onlyFits, onError } = props;
  const [unfolded, setUnfolded] = useState(false);
  const { shown, notHere } = visibleEntries(local, tab, onlyFits);
  const notForHere = new Set(local.notHere.map((e) => e.id));
  const listed = new Set([...local.recommended, ...local.notHere].map((e) => e.id));
  const others = local.installed.filter((m) => m.kind === tab && !listed.has(m.id));
  return (
    <div className="flex flex-col gap-3">
      {tab === "video" && <p className="text-sm text-fg-muted">{LOCAL_MODELS.videoNote}</p>}
      {shown.length === 0 && others.length === 0 && <p className="text-sm text-fg-muted">{LOCAL_MODELS.noneFit[tab]}</p>}
      {shown.map((entry) => (
        <EntryRow key={entry.id} entry={entry} local={local} notHere={notForHere.has(entry.id)} onError={onError} />
      ))}
      {others.map((row) => (
        <InstalledRow key={row.id} row={row} onError={onError} />
      ))}
      {notHere.length > 0 && (
        <div className="flex flex-col gap-2">
          <button
            type="button"
            aria-expanded={unfolded}
            onClick={() => setUnfolded((open) => !open)}
            className="self-start text-sm text-fg-muted hover:text-fg"
          >
            <span aria-hidden>{unfolded ? "▾ " : "▸ "}</span>
            {LOCAL_MODELS.notHere(notHere.length)}
          </button>
          {unfolded &&
            notHere.map((entry) => <EntryRow key={entry.id} entry={entry} local={local} notHere onError={onError} />)}
        </div>
      )}
    </div>
  );
}

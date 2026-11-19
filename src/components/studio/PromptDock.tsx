import { useEffect, useId, useMemo, useRef } from "react";
import { STUDIO_ACTIONS, STUDIO_BAR, STUDIO_DOCK } from "../../lib/words";
import {
  draftModel,
  endpointOf,
  requestKey,
  shownEstimate,
  startRequestOf,
  useStudio,
  type StudioTab,
} from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import { ChipsRow } from "./AdvancedDrawer";
import { ConfirmRun } from "./ConfirmRun";
import { cloudBackup, COST_TONE_CLASS, costTone, createLabel, limitLine, placeLine } from "./cost";
import { etaText } from "./eta";
import { RefTray } from "./RefTray";

// Studio's prompt dock (0.1.0 Task B7; design Wireframe 1, local addendum Wireframe L4): the
// added pictures (RefTray) with *+ Add picture*, the prompt (Enter creates, Shift+Enter adds a
// line), what the draft's model would refuse and why a press was refused, the changed settings
// as chips (ChipsRow), the line saying where the prompt goes, then the price of the whole run
// (a cloud model, its colour a second cue) or the time here (a local one) beside Create. The
// estimate is asked of the backend once the draft has stayed as it is for 150 ms (it sends
// nothing anywhere); Create does not wait for it, but stays greyed out while the answer shown
// (for this model and place) says why it cannot run. For a cloud model the daily limit takes
// Create's place, and the confirmation opens when the run needs one. A local model never asks
// and never meets the limit: one that does not fit here greys Create out with why and offers a
// cloud model instead, a run over 15 minutes says so in amber, and cloud is offered as the
// backup when it is very slow (Studio never moves the draft by itself). Other screens put the
// cursor in the prompt with the window event `app:focus-studio-prompt`.

const SECONDARY =
  "rounded-lg border border-border bg-bg px-2.5 py-1 text-sm hover:border-border-strong hover:bg-bg-muted";

/** Settings on one of its pages. */
function openSettings(page: "endpoints" | "studio") {
  window.dispatchEvent(new CustomEvent("app:open-settings", { detail: page }));
}

/** The prompt dock of `tab`. */
export function PromptDock(props: { tab: StudioTab }) {
  const { tab } = props;
  const draft = useStudio((s) => s.drafts[tab]);
  const model = useStudio((s) => draftModel(s, tab));
  const endpoints = useStudio((s) => s.endpoints);
  const endpoint = useStudio((s) => endpointOf(s.drafts[tab].target, s.endpoints));
  const prefs = useStudio((s) => s.prefs);
  const usage = useStudio((s) => s.usage);
  const shown = useStudio((s) => shownEstimate(s, tab));
  const starting = useStudio((s) => s.starting);
  const pending = useStudio((s) => s.pending);
  const pendingEstimate = useStudio((s) => s.pendingEstimate);
  const refusal = useStudio((s) => s.refusal[tab]);
  const setPrompt = useStudio((s) => s.setPrompt);
  const estimate = useStudio((s) => s.estimate);
  const start = useStudio((s) => s.start);
  const confirmStart = useStudio((s) => s.confirmStart);
  const cancelStart = useStudio((s) => s.cancelStart);
  const dismissRefusal = useStudio((s) => s.dismissRefusal);
  const openPicker = useStudio((s) => s.openPicker);
  const promptRef = useRef<HTMLTextAreaElement>(null);
  // What Create would cost or take, or why it cannot run here, read out with it.
  const aboutId = useId();

  // The estimate follows the draft as Create would send it, and what else it depends on: the
  // model as listed (its price may have come since), the preferences (the limit, what asks)
  // and today's spending.
  const key = useMemo(() => {
    const req = startRequestOf(tab, draft, "");
    return req ? requestKey(req) : null;
  }, [tab, draft]);
  useEffect(() => {
    if (key !== null) void estimate(tab);
  }, [tab, key, model, prefs, usage, estimate]);

  // Edit prompt, on a card or in the viewer, puts the cursor here.
  useEffect(() => {
    const focus = () => promptRef.current?.focus();
    window.addEventListener("app:focus-studio-prompt", focus);
    return () => window.removeEventListener("app:focus-studio-prompt", focus);
  }, []);

  const target = draft.target;
  const here = target === "thisComputer";
  const known = shown?.estimate ?? null;
  const local = here ? (known?.local ?? null) : null;
  const notHere = local?.fit === "notHere";
  // What the model would refuse, and a local model that does not fit here, as shown: an answer
  // for this model and place, until the one for the draft as it is now replaces it. Create
  // agrees with what is on screen, and an answer that greys nothing out is no reason to wait.
  const blocked = known !== null && (known.invalid.length > 0 || notHere);
  // The daily limit has taken Create's place (Enter included).
  const overLimit = !here && known !== null && !known.limit.wouldPass;
  const disabled = model === null || key === null || starting || blocked || overLimit;
  const longRun = local?.over15Min ? etaText(known?.eta ?? null) : null;
  const backup = here ? cloudBackup(tab, known, endpoints) : null;

  const create = () => {
    if (!disabled) void start(tab, false);
  };

  return (
    <div data-prompt-dock className="flex flex-col gap-2 border-t border-border px-4 py-3">
      <RefTray tab={tab} />
      <textarea
        ref={promptRef}
        aria-label={STUDIO_BAR.prompt}
        placeholder={STUDIO_BAR.promptPlaceholder[tab]}
        value={draft.prompt}
        onChange={(e) => setPrompt(tab, e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
            e.preventDefault();
            create();
          }
        }}
        rows={2}
        className="w-full resize-none rounded-xl border border-border bg-bg px-3 py-2 text-sm outline-none focus:border-border-strong"
      />
      {known?.invalid.map((sentence) => (
        <p key={sentence} className="text-xs text-danger">
          {sentence}
        </p>
      ))}
      {shown?.error && (
        <p className="text-xs text-fg-muted">
          {shown.error.message} {shown.error.nextStep}
        </p>
      )}
      {refusal && <ErrorBanner error={refusal} onDismiss={() => dismissRefusal(tab)} />}
      <ChipsRow tab={tab} />
      {target !== null && (
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
          <p className="min-w-0 flex-1 text-xs text-fg-muted">{placeLine(target, endpoint?.host ?? "", known)}</p>
          {overLimit ? (
            <div className="flex flex-wrap items-center gap-2">
              <p className="text-sm text-warning">{limitLine(known.limit)}</p>
              <button type="button" className={SECONDARY} onClick={() => openSettings("studio")}>
                {STUDIO_DOCK.raiseLimit}
              </button>
            </div>
          ) : (
            <div className="flex items-center gap-2">
              {notHere && (
                <span id={aboutId} className="text-xs text-fg-muted">
                  {STUDIO_DOCK.notHere(local?.reason ?? null)}
                </span>
              )}
              {known && !notHere && (
                <span
                  id={aboutId}
                  className={`text-sm tabular-nums ${here ? "text-fg-muted" : COST_TONE_CLASS[costTone(known.usd)]}`}
                >
                  {createLabel(known)}
                </span>
              )}
              <button
                type="button"
                aria-describedby={known ? aboutId : undefined}
                disabled={disabled}
                onClick={create}
                className="btn-primary rounded-lg px-4 py-1.5 text-sm font-medium disabled:opacity-60"
              >
                {STUDIO_DOCK.create}
              </button>
            </div>
          )}
        </div>
      )}
      {longRun && <p className="text-xs text-warning">{STUDIO_DOCK.over15(longRun)}</p>}
      {backup === "useCloud" && (
        <div className="flex flex-wrap items-center gap-2 text-xs text-fg-muted">
          <p>{notHere ? STUDIO_DOCK.cloudInstead : STUDIO_DOCK.tooSlow}</p>
          <button type="button" className={SECONDARY} onClick={() => openPicker("cloud")}>
            {STUDIO_DOCK.chooseCloud}
          </button>
        </div>
      )}
      {backup === "addEndpoint" && (
        <div className="flex flex-wrap items-center gap-2 text-xs text-fg-muted">
          <p>{STUDIO_DOCK.addEndpoint[tab]}</p>
          <button type="button" className={SECONDARY} onClick={() => openSettings("endpoints")}>
            {STUDIO_ACTIONS.openEndpoints}
          </button>
        </div>
      )}
      <ConfirmRun
        open={pending !== null && pendingEstimate !== null}
        tab={pending?.kind ?? tab}
        onCancel={cancelStart}
        onConfirm={(dontAskVideos) => void confirmStart(dontAskVideos)}
      />
    </div>
  );
}

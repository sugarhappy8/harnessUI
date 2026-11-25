import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { SlidersHorizontal, X } from "lucide-react";
import { STUDIO_ACTIONS, STUDIO_COMPOSER, STUDIO_DOCK, STUDIO_FIELDS } from "../../lib/words";
import {
  draftModel,
  endpointOf,
  promptProblems,
  requestKey,
  settingsProblems,
  shownEstimate,
  startRequestOf,
  useStudio,
  type StudioTab,
} from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import { FOCUS_RING } from "../ui/styles";
import { Tip } from "../ui/Tip";
import { ChipsRow } from "./AdvancedDrawer";
import { CountMenu, LengthMenu, ShapeMenu } from "./ComposerMenus";
import { ConfirmRun } from "./ConfirmRun";
import { cloudBackup, COST_TONE_CLASS, costTone, createLabel, limitLine, placeLine } from "./cost";
import { etaText } from "./eta";
import { InputZones } from "./InputZones";
import { ModelChip } from "./ModelChip";
import { RefTray } from "./RefTray";
import { COMPOSER_CHIP, SECONDARY_BUTTON } from "./styles";
import type { MediaModel, RefItem, RefRole } from "./types";

// Studio's composer (0.2.0 Task U3, design 10.1; it replaces 0.1.0's prompt dock, Task B7): one
// raised card at the foot of the results. Its first row holds the reference pictures with *Add
// picture* (RefTray.tsx), the input picture zones (InputZones.tsx) and the prompt (Enter creates,
// Shift+Enter adds a line). Under it, what holds Create back and why a press was refused, the
// note of what a model switch changed (while the side panel is closed: the panel shows it when
// open), and the changed settings as chips (ChipsRow). Then a toolbar: the model chip, which
// opens the model sheet (ModelChip.tsx), Shape, How many and a video's Length as menus
// (ComposerMenus.tsx), Settings, which opens the side panel, and at its end the price of the
// whole run (a cloud model, its colour a second cue) or the time here (a local one) beside
// Create. Under the card, one quiet line says where the prompt goes; the chip's dot says it too.
//
// The estimate is asked of the backend once the draft has stayed as it is for 150 ms (it sends
// nothing anywhere); Create never waits for it. Create is held back only for no model, the
// prompt's own checks (judged here on the prompt as it is), a required input picture missing,
// a mask without its picture to change, a setting the model refuses (from the answer shown, for
// this model and place, or a field holding what cannot be sent), extra settings held for the
// model's price, and a start on its way; for a cloud model the daily limit takes its place, and
// the confirmation opens when the run needs one. A model that may not fit this computer never
// holds it back (Task S11; Task G2 adds the warning line). A local run over 15 minutes says so
// in amber, and cloud is offered as the backup when it is very slow (Studio never moves the
// draft by itself). Other screens put the cursor in the prompt with `app:focus-studio-prompt`.

/** Settings on one of its pages. */
function openSettings(page: "endpoints" | "studio") {
  window.dispatchEvent(new CustomEvent("app:open-settings", { detail: page }));
}

/** What the draft's input pictures lack, in the backend's words: a mask without its picture to
 *  change, and each required picture field with fewer pictures than its least. */
function inputProblems(model: MediaModel, refs: RefItem[]): string[] {
  const count = (role: RefRole) => refs.filter((r) => (r.role ?? "reference") === role).length;
  const takes = (role: RefRole) => model.params.some((p) => p.kind.type === "images" && p.kind.role === role);
  const maskAlone = takes("source") && count("mask") > 0 && count("source") === 0;
  const problems: string[] = maskAlone ? [STUDIO_COMPOSER.maskNeedsSource] : [];
  for (const p of model.params) {
    if (p.kind.type !== "images" || p.kind.min < 1 || count(p.kind.role) >= p.kind.min) continue;
    // The mask's sentence already asks for the picture to change.
    if (!(maskAlone && p.kind.role === "source")) problems.push(STUDIO_COMPOSER.needsPictures(p.kind.role, p.kind.min));
  }
  return problems;
}

/** The arrow keys move along a toolbar's controls; Tab still reaches each of them. */
function toolbarArrows(e: KeyboardEvent<HTMLDivElement>) {
  if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
  // Not out of a text box or a menu inside it.
  if (!(e.target instanceof HTMLElement) || e.target.closest("input, textarea, [role=menu]")) return;
  const controls = Array.from(
    e.currentTarget.querySelectorAll<HTMLElement>("button:not(:disabled), [tabindex='0']"),
  ).filter((el) => !el.closest("[hidden]"));
  const at = controls.indexOf(e.target);
  if (at < 0) return;
  const next = controls[at + (e.key === "ArrowRight" ? 1 : -1)];
  if (next) {
    e.preventDefault();
    next.focus();
  }
}

/** The composer of `tab`. */
export function Composer(props: { tab: StudioTab }) {
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
  const holding = useStudio((s) => s.holding[tab]);
  // A field holding what cannot be sent (a custom size that does not fit): Create waits.
  const invalidFields = useStudio((s) => s.invalid[tab]);
  const priceFailed = useStudio((s) => s.priceError[tab] !== null);
  const settingsOpen = useStudio((s) => s.settingsOpen);
  const setSettingsOpen = useStudio((s) => s.setSettingsOpen);
  const setPrompt = useStudio((s) => s.setPrompt);
  const estimate = useStudio((s) => s.estimate);
  const start = useStudio((s) => s.start);
  const confirmStart = useStudio((s) => s.confirmStart);
  const cancelStart = useStudio((s) => s.cancelStart);
  const dismissRefusal = useStudio((s) => s.dismissRefusal);
  const dismissNote = useStudio((s) => s.dismissNote);
  const openSheet = useStudio((s) => s.openSheet);
  const promptRef = useRef<HTMLTextAreaElement>(null);
  // What Create would cost or take, and what holds it back, read out with it.
  const aboutId = useId();
  const whyId = useId();
  // What the prompt itself lacks, read out with the prompt box.
  const promptAboutId = useId();
  // Whether the prompt was typed in (so an empty one was emptied on purpose), and each press
  // refused for the prompt: until either, its sentences are for a screen reader only, not a
  // fault shown under every empty prompt (critique §6).
  const [typed, setTyped] = useState(false);
  const [refusedPresses, setRefusedPresses] = useState(0);

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
  // What Create would be refused for: the prompt's own checks, on the prompt as it is (no
  // answer about it is waited for); the input pictures, on the draft as it is; then the rest of
  // the answer shown, for this model and place (typing does not change it). A failed answer, or
  // one still on its way, holds nothing back once these pass; the backend checks every start.
  const asks = target !== null && key !== null;
  const promptRefused = asks ? promptProblems(draft.prompt, target) : [];
  const inputsRefused = asks && model ? inputProblems(model, draft.refs) : [];
  const settingsRefused = asks
    ? [...new Set([...Object.values(invalidFields), ...settingsProblems(known)])].filter((s) => !inputsRefused.includes(s))
    : [];
  const blocked = promptRefused.length > 0 || inputsRefused.length > 0 || settingsRefused.length > 0;
  const promptFault = typed || refusedPresses > 0;
  // The daily limit has taken Create's place (Enter included).
  const overLimit = !here && known !== null && !known.limit.wouldPass;
  // Extra settings held aside until the model's price says which it takes: Create waits for it.
  const disabled = model === null || key === null || starting || blocked || overLimit || holding;
  const longRun = local?.over15Min ? etaText(known?.eta ?? null) : null;
  // Cloud as the backup for a very slow local run (not for one that may not fit: Task G2).
  const backup = here && local?.verySlow ? cloudBackup(tab, known, endpoints) : null;
  const time = known ? createLabel(known) : "";
  // Create is described by its time or price, and by what holds it back.
  const describedBy = [
    time ? aboutId : "",
    promptRefused.length > 0 ? promptAboutId : "",
    inputsRefused.length > 0 || settingsRefused.length > 0 ? whyId : "",
  ]
    .filter(Boolean)
    .join(" ");

  const create = () => {
    if (!disabled) void start(tab, false);
    // A press the prompt holds back says why, aloud.
    else if (promptRefused.length > 0) setRefusedPresses((n) => n + 1);
  };

  return (
    <div className="mx-auto w-full max-w-4xl">
      <section
        aria-label={STUDIO_COMPOSER.region(tab)}
        data-composer
        className="composer @container flex flex-col gap-2 rounded-2xl p-2.5"
      >
        <div className="flex flex-wrap items-center gap-2">
          <RefTray tab={tab} />
          {model && <InputZones tab={tab} model={model} />}
          <textarea
            ref={promptRef}
            aria-label={STUDIO_COMPOSER.prompt}
            placeholder={STUDIO_COMPOSER.placeholder(tab)}
            value={draft.prompt}
            aria-describedby={promptRefused.length > 0 ? promptAboutId : undefined}
            onChange={(e) => {
              if (e.target.value !== "") setTyped(true);
              setPrompt(tab, e.target.value);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                // Enter held down repeats: one press is one run (the prompt stays after Create,
                // and each start is answered within moments).
                if (e.repeat) return;
                create();
              }
            }}
            rows={2}
            className="field-sizing-content max-h-48 min-h-14 min-w-[min(100%,16rem)] flex-1 resize-none bg-transparent px-1.5 py-1 text-[15px] leading-relaxed outline-none placeholder:text-fg-muted"
          />
        </div>
        <div className="flex flex-col gap-1.5 px-1.5 empty:hidden">
          {promptRefused.length > 0 && (
            // Each refused press puts it in again, so it is read out again.
            <div
              key={refusedPresses}
              id={promptAboutId}
              role={refusedPresses > 0 ? "alert" : undefined}
              className={promptFault ? "text-xs text-danger" : "sr-only text-xs text-fg-muted"}
            >
              {promptRefused.map((sentence) => (
                <p key={sentence}>{sentence}</p>
              ))}
            </div>
          )}
          {(inputsRefused.length > 0 || settingsRefused.length > 0) && (
            <div id={whyId} className="flex flex-col gap-0.5 text-xs">
              {inputsRefused.map((sentence) => (
                <p key={sentence} className="text-fg-muted">
                  {sentence}
                </p>
              ))}
              {settingsRefused.map((sentence) => (
                <p key={sentence} className="text-danger">
                  {sentence}
                </p>
              ))}
            </div>
          )}
          {holding && <p className="text-xs text-fg-muted">{STUDIO_DOCK.holdingExtra(priceFailed)}</p>}
          {shown?.error && (
            <p className="text-xs text-fg-muted">
              {shown.error.message} {shown.error.nextStep}
            </p>
          )}
          {refusal && <ErrorBanner error={refusal} onDismiss={() => dismissRefusal(tab)} />}
          {draft.note && !settingsOpen && (
            <div role="status" className="flex items-start gap-1.5 rounded-lg bg-bg-muted px-2.5 py-1.5 text-xs">
              <p className="min-w-0 flex-1">{draft.note}</p>
              <button
                type="button"
                aria-label={STUDIO_FIELDS.putNoteAway}
                onClick={() => dismissNote(tab)}
                className={`rounded p-0.5 text-fg-muted hover:text-fg ${FOCUS_RING}`}
              >
                <X size={12} aria-hidden />
              </button>
            </div>
          )}
          <ChipsRow tab={tab} />
          {longRun && <p className="text-xs text-warning">{STUDIO_DOCK.over15(longRun)}</p>}
          {backup === "useCloud" && (
            <div className="flex flex-wrap items-center gap-2 text-xs text-fg-muted">
              <p>{STUDIO_DOCK.tooSlow}</p>
              <button type="button" className={SECONDARY_BUTTON} onClick={() => openSheet(true, "cloud")}>
                {STUDIO_DOCK.chooseCloud}
              </button>
            </div>
          )}
          {backup === "addEndpoint" && (
            <div className="flex flex-wrap items-center gap-2 text-xs text-fg-muted">
              <p>{STUDIO_DOCK.addEndpoint[tab]}</p>
              <button type="button" className={SECONDARY_BUTTON} onClick={() => openSettings("endpoints")}>
                {STUDIO_ACTIONS.openEndpoints}
              </button>
            </div>
          )}
        </div>
        <div
          role="toolbar"
          aria-label={STUDIO_COMPOSER.toolbar}
          onKeyDown={toolbarArrows}
          className="flex flex-wrap items-center gap-1.5"
        >
          <ModelChip tab={tab} />
          <ShapeMenu tab={tab} />
          <CountMenu tab={tab} />
          <LengthMenu tab={tab} />
          <Tip content={STUDIO_COMPOSER.settings}>
            <button
              type="button"
              data-settings-button
              aria-expanded={settingsOpen}
              onClick={(e) => {
                // Focused first (WebKit does not focus a pressed button), so the panel gives
                // focus back here as it closes.
                e.currentTarget.focus();
                setSettingsOpen(!settingsOpen);
              }}
              className={COMPOSER_CHIP}
            >
              <SlidersHorizontal size={14} aria-hidden className="text-fg-muted" />
              {/* On a narrow composer, the icon and its tooltip. */}
              <span className="@max-2xl:sr-only">{STUDIO_COMPOSER.settings}</span>
            </button>
          </Tip>
          <div className="ml-auto flex min-w-0 flex-wrap items-center justify-end gap-x-3 gap-y-1.5 pl-1">
            {overLimit ? (
              <>
                <p className="text-sm text-warning">{limitLine(known.limit)}</p>
                <button type="button" className={SECONDARY_BUTTON} onClick={() => openSettings("studio")}>
                  {STUDIO_DOCK.raiseLimit}
                </button>
              </>
            ) : (
              <>
                {time && (
                  <span
                    id={aboutId}
                    className={`text-sm whitespace-nowrap tabular-nums ${here ? "text-fg-muted" : COST_TONE_CLASS[costTone(known?.usd ?? null)]}`}
                  >
                    {time}
                  </span>
                )}
                <button
                  type="button"
                  aria-describedby={describedBy || undefined}
                  disabled={disabled}
                  onClick={create}
                  className="btn-primary h-9 rounded-full px-5 text-sm font-semibold"
                >
                  {STUDIO_DOCK.create}
                </button>
              </>
            )}
          </div>
        </div>
      </section>
      {target !== null && (
        <p className="mt-1.5 truncate px-3 text-center text-meta">{placeLine(target, endpoint?.host ?? "", known)}</p>
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

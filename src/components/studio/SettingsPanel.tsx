import { useEffect, useId, useState } from "react";
import { X } from "lucide-react";
import { LOCAL_MARK, STUDIO_FIELDS, STUDIO_PICKER, STUDIO_PLACE, STUDIO_SETUPS } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { draftModel, endpointOf, priceNotAsked, useStudio, type Draft, type StudioTab } from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import { AdvancedDrawer } from "./AdvancedDrawer";
import { visibleFields } from "./canonical";
import { localHint, priceHint } from "./sheetModels";
import { clipAsks, SettingField } from "./SettingField";
import { BOX, SMALL_BUTTON } from "./styles";
import type { Setup } from "./types";

// Studio's right panel (0.1.0 Task B5; design Wireframe 1, local addendum "Right panel"): the
// model's name with where it runs ("On this computer · <device>" or "via <endpoint> · Cloud")
// and its price or time here, with *Get the price* (the model sheet, from the composer's model
// chip, switches models since Task U4), the note of what changed, the model's simple fields in its own
// order (a frame as a drop zone; reference pictures go in the tray), My setups and the Advanced
// drawer with its change count (Task B6, AdvancedDrawer.tsx). Each field, in either layer, is
// drawn by SettingField.tsx. On this computer, each Length and Resolution chip of a video shows
// its own time here, asked here once per chip and size (nothing is sent).

/** The right panel of `tab`. */
export function SettingsPanel(props: { tab: StudioTab }) {
  const { tab } = props;
  const draft = useStudio((s) => s.drafts[tab]);
  const model = useStudio((s) => draftModel(s, tab));
  const local = useStudio((s) => s.local);
  const endpoint = useStudio((s) => endpointOf(s.drafts[tab].target, s.endpoints));
  const localFits = useStudio((s) => s.localFits);
  const dismissNote = useStudio((s) => s.dismissNote);
  const askLocalFit = useStudio((s) => s.askLocalFit);
  // Per tab, the setup its draft was set from: kept here, so it outlasts My setups being drawn
  // afresh for another tab (or while the tab has no model).
  const [fromByTab, setFromByTab] = useState<Partial<Record<StudioTab, SetupFrom>>>({});
  const here = draft.target === "thisComputer";
  const localVideo = here && model?.kind === "video";

  // A local clip's chips each ask for their time, once per size and length, whichever layer
  // draws their field (SettingField shows the answers): the store keeps the answers (and which
  // could not be worked out) and asks only for what it has not got. It forgets them when the
  // local models are read again, so this runs again as `localFits` changes and asks for them
  // again.
  useEffect(() => {
    if (!model || !localVideo) return;
    for (const ask of clipAsks(model, draft.values)) askLocalFit(model.id, ask.width, ask.height, ask.seconds);
  }, [model, localVideo, draft.values, localFits, askLocalFit]);

  if (!model) {
    return (
      <div data-settings-panel className="flex flex-col gap-4 p-3">
        <p className="text-sm text-fg-muted">{STUDIO_PICKER.choose}</p>
      </div>
    );
  }

  const place = here
    ? local
      ? STUDIO_PLACE.here(local.hardware.device)
      : LOCAL_MARK.name
    : endpoint
      ? STUDIO_PLACE.cloud(endpoint.name)
      : null;

  return (
    <div data-settings-panel className="flex flex-col gap-4 p-3">
      <div className="flex flex-col gap-1">
        <p className="text-sm font-medium">{model.displayName}</p>
        {place && <p className="text-xs text-fg-muted">{place}</p>}
        {/* Its state (a request on its way) belongs to the model it was for. */}
        <ChosenLine key={`${here ? "here" : JSON.stringify(draft.target)} ${model.id}`} tab={tab} />
      </div>
      {draft.note && (
        <div role="status" className="flex items-start gap-1.5 rounded-lg bg-bg-muted px-2 py-1.5 text-xs">
          <p className="min-w-0 flex-1">{draft.note}</p>
          <button
            type="button"
            aria-label={STUDIO_FIELDS.putNoteAway}
            title={STUDIO_FIELDS.putNoteAway}
            onClick={() => dismissNote(tab)}
            className="rounded p-0.5 text-fg-muted hover:text-fg"
          >
            <X size={12} aria-hidden />
          </button>
        </div>
      )}
      {visibleFields(model.params, "simple", draft.values).map((spec) => (
        <SettingField key={spec.key} tab={tab} spec={spec} />
      ))}
      {/* A name being typed, or a refusal, is for one tab: another tab starts afresh. The setup
          each tab was set from is kept by the panel, so coming back to a tab still names it. */}
      <MySetups
        key={tab}
        tab={tab}
        from={fromByTab[tab] ?? null}
        onFrom={(of, from) => setFromByTab((all) => ({ ...all, [of]: from }))}
      />
      <AdvancedDrawer tab={tab} />
    </div>
  );
}

/** Under the model's name: its price hint, with *Get the price* while an OpenRouter picture's
 *  price was never asked for (and why the last request failed, kept by the store for the tab's
 *  model), or a local model's time here. */
function ChosenLine(props: { tab: StudioTab }) {
  const { tab } = props;
  const target = useStudio((s) => s.drafts[tab].target);
  const model = useStudio((s) => draftModel(s, tab));
  const local = useStudio((s) => s.local);
  const endpoint = useStudio((s) => endpointOf(s.drafts[tab].target, s.endpoints));
  const fetchPrice = useStudio((s) => s.fetchPrice);
  const error = useStudio((s) => s.priceError[tab]);
  const dismissPriceError = useStudio((s) => s.dismissPriceError);
  const [busy, setBusy] = useState(false);
  if (!model) return null;
  if (target === "thisComputer") {
    const installed = local?.installed.find((m) => m.model.id === model.id);
    return installed ? <p className="text-xs text-fg-muted">{localHint(installed, tab)}</p> : null;
  }
  if (!endpoint) return null;

  const getPrice = () => {
    setBusy(true);
    // A failure goes to the store's `priceError`.
    void fetchPrice(tab).finally(() => setBusy(false));
  };

  return (
    <>
      <p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-fg-muted">
        <span>{priceHint(model, endpoint.company)}</span>
        {priceNotAsked(model, endpoint.company) && (
          <button
            type="button"
            disabled={busy}
            onClick={getPrice}
            className="rounded-md border border-border bg-bg px-1.5 py-0.5 text-fg hover:border-border-strong hover:bg-bg-muted disabled:opacity-60"
          >
            {STUDIO_PICKER.getPrice}
          </button>
        )}
      </p>
      {error && <ErrorBanner error={error} onDismiss={() => dismissPriceError(tab)} />}
    </>
  );
}

/** While the preferences hold no setups. */
const NO_SETUPS: Setup[] = [];

/** The setup a tab's draft was set from (applied or saved), with the draft it gave. */
type SetupFrom = { name: string; draft: Draft };

/**
 * My setups: a menu of the tab's saved setups, which applies the one picked (or says why it
 * cannot), and *Save as setup…*, which keeps the model and every setting under a name. The menu
 * names the setup applied or saved last until the draft changes, then None. That setup is kept
 * by the panel (`from`, set through `onFrom` for the tab it was applied or saved on), so it is
 * still named after another tab was shown.
 */
function MySetups(props: {
  tab: StudioTab;
  from: SetupFrom | null;
  onFrom: (tab: StudioTab, from: SetupFrom | null) => void;
}) {
  const { tab, from, onFrom } = props;
  const all = useStudio((s) => s.prefs?.setups) ?? NO_SETUPS;
  const draft = useStudio((s) => s.drafts[tab]);
  const applySetup = useStudio((s) => s.applySetup);
  const saveSetup = useStudio((s) => s.saveSetup);
  const menuId = useId();
  // The name being typed (`null` while not saving), and why a pick or a save did not happen.
  const [name, setName] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  // What to do about a save that failed (the backend's next step), shown under its reason.
  const [nextStep, setNextStep] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const setups = all.filter((s) => s.kind === tab);
  const shown = from !== null && from.draft === draft ? setups.findIndex((s) => s.name === from.name) : -1;

  const pick = (value: string) => {
    const setup = value === "" ? undefined : setups[Number(value)];
    if (!setup) return;
    const refused = applySetup(setup);
    setMessage(refused);
    setNextStep(null);
    onFrom(tab, refused === null ? { name: setup.name, draft: useStudio.getState().drafts[tab] } : null);
  };
  const save = async () => {
    if (name === null || name.trim() === "" || saving) return;
    setSaving(true);
    setNextStep(null);
    try {
      const refused = await saveSetup(tab, name);
      setMessage(refused);
      if (refused === null) {
        onFrom(tab, { name: name.trim(), draft: useStudio.getState().drafts[tab] });
        setName(null);
      }
    } catch (e: unknown) {
      const error = toErrorInfo(e);
      setMessage(error.message);
      setNextStep(error.nextStep || null);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center gap-2">
        <label htmlFor={menuId} className="text-xs font-medium text-fg-muted">
          {STUDIO_SETUPS.label}
        </label>
        <select
          id={menuId}
          value={shown < 0 ? "" : String(shown)}
          onChange={(e) => pick(e.target.value)}
          className={`${BOX} min-w-0 flex-1`}
        >
          <option value="">{STUDIO_SETUPS.none}</option>
          {setups.map((setup, i) => (
            <option key={`${i} ${setup.name}`} value={String(i)}>
              {setup.name}
            </option>
          ))}
        </select>
      </div>
      {name === null ? (
        <button
          type="button"
          onClick={() => {
            setName("");
            setMessage(null);
          }}
          className={`${SMALL_BUTTON} self-start`}
        >
          {STUDIO_SETUPS.saveAs}
        </button>
      ) : (
        <div className="flex items-center gap-1.5">
          <input
            aria-label={STUDIO_SETUPS.name}
            placeholder={STUDIO_SETUPS.name}
            value={name}
            autoFocus
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void save();
              }
            }}
            className={`${BOX} min-w-0 flex-1`}
          />
          <button
            type="button"
            disabled={saving || name.trim() === ""}
            onClick={() => void save()}
            className="rounded-md border border-border px-2 py-0.5 text-xs whitespace-nowrap hover:border-border-strong disabled:opacity-60"
          >
            {STUDIO_SETUPS.save}
          </button>
          <button
            type="button"
            onClick={() => {
              setName(null);
              setMessage(null);
              setNextStep(null);
            }}
            className={SMALL_BUTTON}
          >
            {STUDIO_SETUPS.cancel}
          </button>
        </div>
      )}
      {message && <p className="text-xs text-warning">{message}</p>}
      {message && nextStep && <p className="text-xs text-fg-muted">{nextStep}</p>}
    </div>
  );
}

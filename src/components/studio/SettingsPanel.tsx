import { useEffect, useId, useRef, useState } from "react";
import { BookmarkPlus, X } from "lucide-react";
import { LOCAL_MARK, STUDIO_FIELDS, STUDIO_PANEL, STUDIO_PICKER, STUDIO_PLACE, STUDIO_SETUPS } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { draftModel, endpointOf, priceNotAsked, useStudio, type Draft, type StudioTab } from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import { SelectField } from "../ui/SelectField";
import { FOCUS_RING } from "../ui/styles";
import { AdvancedDrawer } from "./AdvancedDrawer";
import { visibleFields } from "./canonical";
import { LorasSection } from "./LorasSection";
import { ModelFilesSection } from "./ModelFilesSection";
import { localHint, priceHint } from "./sheetModels";
import { clipAsks, SettingField } from "./SettingField";
import { BOX, GHOST_BUTTON, PANEL_BUTTON, PANEL_SECTION } from "./styles";
import type { ParamSpec, Setup } from "./types";

// Studio's side panel, Settings (0.1.0 Task B5; design Wireframe 1, local addendum "Right
// panel"; rebuilt in 0.2.0 Task U8, design 10.1): the model's name with where it runs ("On this
// computer · <device>" or "via <endpoint> · Cloud") and its price or time here, with *Get the
// price* (the model sheet, from the composer's model chip, switches models since Task U4), the
// note of what changed, and the model's other simple fields in its own order (Quality,
// Resolution, a video's Length and Sound). Then its sections: My setups, Advanced (Task B6,
// AdvancedDrawer.tsx), and for a model on this computer its LoRAs (LorasSection.tsx) and its
// Files (ModelFilesSection.tsx). It never holds Shape or How many: they are the composer's
// menus. Each field, in either layer, is drawn by SettingField.tsx with the app's styled
// controls. Opened on a section (`settingsSection`: the composer's LoRAs button, *Set it up
// yourself*), it scrolls to it. On this computer, each Length and Resolution chip of a video
// shows its own time here, asked here once per chip and size (nothing is sent).

/** The composer's fields, never the panel's (design 10.1). */
function composerField(spec: ParamSpec): boolean {
  return spec.canonical === "shape" || spec.canonical === "count";
}

/** The side panel of `tab`. */
export function SettingsPanel(props: { tab: StudioTab }) {
  const { tab } = props;
  const draft = useStudio((s) => s.drafts[tab]);
  const model = useStudio((s) => draftModel(s, tab));
  const local = useStudio((s) => s.local);
  const endpoint = useStudio((s) => endpointOf(s.drafts[tab].target, s.endpoints));
  const localFits = useStudio((s) => s.localFits);
  const section = useStudio((s) => s.settingsSection);
  const dismissNote = useStudio((s) => s.dismissNote);
  const askLocalFit = useStudio((s) => s.askLocalFit);
  // Per tab, the setup its draft was set from: kept here, so it outlasts My setups being drawn
  // afresh for another tab (or while the tab has no model).
  const [fromByTab, setFromByTab] = useState<Partial<Record<StudioTab, SetupFrom>>>({});
  const root = useRef<HTMLDivElement>(null);
  const here = draft.target === "thisComputer";
  const localVideo = here && model?.kind === "video";
  // The model as this computer lists it: its LoRAs and its files.
  const installed = here && model ? (local?.installed.find((m) => m.model.id === model.id) ?? null) : null;
  const loras = installed !== null && installed.takesLoras && !installed.needsSetup;
  // Which sections are drawn: one opened on appears with its model.
  const drawn = `${model?.id ?? ""} ${loras} ${installed !== null}`;

  // A local clip's chips each ask for their time, once per size and length, whichever layer
  // draws their field (SettingField shows the answers): the store keeps the answers (and which
  // could not be worked out) and asks only for what it has not got. It forgets them when the
  // local models are read again, so this runs again as `localFits` changes and asks for them
  // again.
  useEffect(() => {
    if (!model || !localVideo) return;
    for (const ask of clipAsks(model, draft.values)) askLocalFit(model.id, ask.width, ask.height, ask.seconds);
  }, [model, localVideo, draft.values, localFits, askLocalFit]);

  // Opened on a section: brought into view (also once it is drawn, as its model comes).
  useEffect(() => {
    if (section === null) return;
    root.current?.querySelector(`[data-settings-section="${section}"]`)?.scrollIntoView?.({ block: "start" });
  }, [section, drawn]);

  if (!model) {
    return (
      <div data-settings-panel className="flex flex-col gap-4">
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
  const fields = visibleFields(model.params, "simple", draft.values).filter(
    (spec) => !composerField(spec) && spec.kind.type !== "images",
  );

  return (
    <div ref={root} data-settings-panel className="flex flex-col gap-5">
      <div className="flex flex-col gap-0.5">
        <p className="text-base leading-snug font-semibold">{model.displayName}</p>
        {place && <p className="text-meta">{place}</p>}
        {/* Its state (a request on its way) belongs to the model it was for. */}
        <ChosenLine key={`${here ? "here" : JSON.stringify(draft.target)} ${model.id}`} tab={tab} />
      </div>
      {draft.note && (
        <div role="status" className="flex items-start gap-1.5 rounded-lg bg-bg-muted px-3 py-2 text-sm">
          <p className="min-w-0 flex-1">{draft.note}</p>
          <button
            type="button"
            aria-label={STUDIO_FIELDS.putNoteAway}
            onClick={() => dismissNote(tab)}
            className={`rounded p-0.5 text-fg-muted hover:text-fg ${FOCUS_RING}`}
          >
            <X size={14} aria-hidden />
          </button>
        </div>
      )}
      {fields.length > 0 && (
        <div className="flex flex-col gap-5">
          {fields.map((spec) => (
            <SettingField key={spec.key} tab={tab} spec={spec} />
          ))}
        </div>
      )}
      {/* A name being typed, or a refusal, is for one tab: another tab starts afresh. The setup
          each tab was set from is kept by the panel, so coming back to a tab still names it. */}
      <MySetups
        key={tab}
        tab={tab}
        from={fromByTab[tab] ?? null}
        onFrom={(of, from) => setFromByTab((all) => ({ ...all, [of]: from }))}
      />
      {/* A model with no type yet has no settings to show: its Files come first. */}
      {!installed?.needsSetup && <AdvancedSection tab={tab} />}
      {loras && installed && <LorasSection tab={tab} model={installed} />}
      {installed && <ModelFilesSection key={installed.id} model={installed} />}
    </div>
  );
}

/** Advanced, as a section of the panel: the drawer, whose toggle is its heading. */
function AdvancedSection(props: { tab: StudioTab }) {
  return (
    <section aria-label={STUDIO_PANEL.advanced} data-settings-section="advanced" className={PANEL_SECTION}>
      <AdvancedDrawer tab={props.tab} />
    </section>
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
    return installed ? <p className="text-meta">{localHint(installed, tab)}</p> : null;
  }
  if (!endpoint) return null;

  const getPrice = () => {
    setBusy(true);
    // A failure goes to the store's `priceError`.
    void fetchPrice(tab).finally(() => setBusy(false));
  };

  return (
    <>
      <p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-meta">
        <span>{priceHint(model, endpoint.company)}</span>
        {priceNotAsked(model, endpoint.company) && (
          <button type="button" disabled={busy} onClick={getPrice} className={`${PANEL_BUTTON} px-2 py-0.5 text-xs text-fg`}>
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
 * My setups: a select of the tab's saved setups, which applies the one picked (or says why it
 * cannot), and *Save as setup…*, which keeps the model and every setting under a name. The
 * select names the setup applied or saved last until the draft changes, then None, its
 * placeholder. That setup is kept by the panel (`from`, set through `onFrom` for the tab it was
 * applied or saved on), so it is still named after another tab was shown.
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
  const headingId = useId();
  // The name being typed (`null` while not saving), and why a pick or a save did not happen.
  const [name, setName] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  // What to do about a save that failed (the backend's next step), shown under its reason.
  const [nextStep, setNextStep] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const setups = all.filter((s) => s.kind === tab);
  const shown = from !== null && from.draft === draft ? setups.findIndex((s) => s.name === from.name) : -1;

  const pick = (value: string) => {
    const setup = setups[Number(value)];
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
    <section aria-labelledby={headingId} data-settings-section="setups" className={PANEL_SECTION}>
      <h3 id={headingId} className="text-section">
        {STUDIO_PANEL.setups}
      </h3>
      <SelectField
        label={STUDIO_SETUPS.label}
        labelledBy={headingId}
        value={shown < 0 ? null : String(shown)}
        placeholder={STUDIO_SETUPS.none}
        options={setups.map((setup, i) => ({ value: String(i), label: setup.name }))}
        onChange={pick}
        disabled={setups.length === 0}
      />
      {name === null ? (
        <button
          type="button"
          onClick={() => {
            setName("");
            setMessage(null);
          }}
          className={`${PANEL_BUTTON} self-start`}
        >
          <BookmarkPlus size={14} aria-hidden className="text-fg-muted" />
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
            className={`${BOX} min-w-0 flex-1 py-1.5`}
          />
          <button
            type="button"
            disabled={saving || name.trim() === ""}
            onClick={() => void save()}
            className={`${PANEL_BUTTON} whitespace-nowrap`}
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
            className={GHOST_BUTTON}
          >
            {STUDIO_SETUPS.cancel}
          </button>
        </div>
      )}
      {message && <p className="text-sm text-warning">{message}</p>}
      {message && nextStep && <p className="text-sm text-fg-muted">{nextStep}</p>}
    </section>
  );
}

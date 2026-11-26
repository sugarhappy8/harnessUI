import { useEffect, useId, useMemo, useState } from "react";
import clsx from "clsx";
import { ChevronRight, Code2, Pencil, RotateCcw, X } from "lucide-react";
import type { ErrorInfo } from "../../lib/types";
import { SETTING_WORDS, STUDIO_ADVANCED, STUDIO_FIELDS } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { draftModel, endpointOf, priceNotAsked, startRequestOf, useStudio, type StudioTab } from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import { CheckboxField } from "../ui/CheckboxField";
import { SelectField } from "../ui/SelectField";
import { FOCUS_RING } from "../ui/styles";
import * as api from "./api";
import { changedCount, changedKeys, nameOf, valueWords, visibleFields, type Values } from "./canonical";
import { SettingField } from "./SettingField";
import { BOX, GHOST_BUTTON, PANEL_BUTTON } from "./styles";
import type { EndpointCompany, MediaModel, ParamSpec, RequestPreview, Routing, StartRequest } from "./types";

// Studio's Advanced drawer (0.1.0 Task B6; design Wireframe 2; a section of the side panel,
// with the app's styled controls, since 0.2.0 Task U8): its header counts the changes and offers
// *Reset all*; the model's Advanced fields follow in its order, each drawn as the simple ones are
// (SettingField: its reset, the seed with its dice and *Keep it*, a local clip's chips with their
// times; the sampler and scheduler as selects of the engine's full lists);
// then, for an OpenRouter picture, which company runs it; Extra fields (JSON), only the names
// the model lists; and *See the exact request*, what Create would send and to which host, with
// the key removed. For a model on this computer there is no routing and no Extra fields (they
// would become the engine's arguments), and the button is *See the exact command*. Whether the
// drawer is open is kept in the preferences. Each change also shows as a chip under the prompt
// (`ChipsRow`, which the composer mounts), and the chip removes it. The LoRAs attached are not
// Advanced's (`changedKeys`): the panel's LoRAs section has them.

// ---- the chips ----------------------------------------------------------------------------

/** The most characters of a text a chip shows. */
const CHIP_TEXT_MAX = 32;

/** `text` cut to `CHIP_TEXT_MAX` characters, with "…" when it was longer. */
function shortened(text: string): string {
  const chars = Array.from(text);
  return chars.length <= CHIP_TEXT_MAX ? text : `${chars.slice(0, CHIP_TEXT_MAX - 1).join("")}…`;
}

/** A field's value as its chip shows it: a switch on or off, a text cut short, else as the
 *  field shows it in the middle of a sentence ("transparent", "PNG", "80%"). */
function chipValue(field: ParamSpec, value: unknown): string {
  if (field.kind.type === "boolean") return value === true ? STUDIO_ADVANCED.on : STUDIO_ADVANCED.off;
  if (field.kind.type === "text") return shortened(String(value));
  return SETTING_WORDS.midSentence(valueWords(value, field));
}

/** An Extra field's value as its chip shows it: a text as it is, anything else as JSON. */
function extraValue(value: unknown): string {
  return shortened(typeof value === "string" ? value : String(JSON.stringify(value)));
}

/** A chip for each Advanced setting `values` and `extra` change (`changedKeys`, which Advanced's
 *  header counts), in the model's order and then the Extra fields: "seed 81723",
 *  "background: transparent", "moderation: low". */
export function chipsFor(model: MediaModel, values: Values, extra: Values): { key: string; text: string }[] {
  const changed = changedKeys(model, values, extra);
  const fields = changed.values.flatMap((key) => {
    const field = model.params.find((p) => p.key === key);
    if (!field) return [];
    const name = nameOf(field);
    const text =
      field.kind.type === "seed"
        ? STUDIO_ADVANCED.seedChip(name, String(values[key]))
        : STUDIO_ADVANCED.chip(name, chipValue(field, values[key]));
    return [{ key, text }];
  });
  return [...fields, ...changed.extra.map((key) => ({ key, text: STUDIO_ADVANCED.chip(key, extraValue(extra[key])) }))];
}

/** The chips of `tab`'s changed Advanced settings, each with the button that removes it (back to
 *  Automatic, or out of the Extra fields); nothing while none is changed. The composer shows
 *  them under the prompt. */
export function ChipsRow(props: { tab: StudioTab }) {
  const { tab } = props;
  const draft = useStudio((s) => s.drafts[tab]);
  const model = useStudio((s) => draftModel(s, tab));
  const resetValue = useStudio((s) => s.resetValue);
  const setExtra = useStudio((s) => s.setExtra);
  if (!model) return null;
  const chips = chipsFor(model, draft.values, draft.extra);
  if (chips.length === 0) return null;
  const fields = new Set(changedKeys(model, draft.values, draft.extra).values);
  const remove = (key: string) => {
    if (fields.has(key)) resetValue(tab, key);
    else setExtra(tab, Object.fromEntries(Object.entries(draft.extra).filter(([k]) => k !== key)));
  };
  return (
    <ul aria-label={STUDIO_ADVANCED.chips} className="flex flex-wrap gap-1.5">
      {chips.map((chip) => (
        <li
          key={`${fields.has(chip.key) ? "field" : "extra"} ${chip.key}`}
          className="flex items-center gap-1 rounded-full border border-border bg-bg-muted py-0.5 pr-1 pl-2 text-xs"
        >
          <span>{chip.text}</span>
          <button
            type="button"
            aria-label={STUDIO_ADVANCED.removeChip(chip.text)}
            title={STUDIO_ADVANCED.removeChip(chip.text)}
            onClick={() => remove(chip.key)}
            className="rounded-full p-0.5 text-fg-muted hover:bg-bg hover:text-fg"
          >
            <X size={12} aria-hidden />
          </button>
        </li>
      ))}
    </ul>
  );
}

// ---- Extra fields (JSON) ------------------------------------------------------------------

/**
 * Extra fields as typed: a JSON object whose names are all ones the model lists (`allowed`) and
 * none it takes through its own fields (`typed`); blank is none. Otherwise the first problem, in
 * plain words.
 */
export function parseExtra(
  text: string,
  allowed: string[],
  typed: string[],
): { ok: true; value: Values } | { ok: false; message: string } {
  if (text.trim() === "") return { ok: true, value: {} };
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return { ok: false, message: STUDIO_ADVANCED.notJson };
  }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    return { ok: false, message: STUDIO_ADVANCED.notObject };
  }
  const value = parsed as Values;
  for (const key of Object.keys(value)) {
    if (typed.includes(key)) return { ok: false, message: STUDIO_ADVANCED.typed(key) };
    if (!allowed.includes(key)) return { ok: false, message: STUDIO_ADVANCED.notAccepted(key) };
  }
  return { ok: true, value };
}

/** Extra fields (JSON): what is set, *Edit JSON* (checked on Save), and the names the model
 *  accepts. */
function ExtraFields(props: { tab: StudioTab; spec: ParamSpec; allowed: string[]; typed: string[]; extra: Values }) {
  const { tab, spec, allowed, typed, extra } = props;
  const setExtra = useStudio((s) => s.setExtra);
  const labelId = useId();
  const problemId = useId();
  // What is typed while editing (`null` while not), and why Save refused it.
  const [text, setText] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const names = Object.keys(extra);
  const close = () => {
    setText(null);
    setProblem(null);
  };
  const save = () => {
    const parsed = parseExtra(text ?? "", allowed, typed);
    if (!parsed.ok) {
      setProblem(parsed.message);
      return;
    }
    setExtra(tab, parsed.value);
    close();
  };
  return (
    <div role="group" aria-labelledby={labelId} className="flex flex-col gap-2">
      <div className="flex min-h-7 items-center gap-2">
        <span id={labelId} className="shrink-0 text-sm font-medium">
          {spec.label}
        </span>
        {text === null && (
          <>
            <span className="min-w-0 truncate text-sm text-fg-muted">
              {names.length === 0 ? STUDIO_ADVANCED.noneSet : names.join(", ")}
            </span>
            <button
              type="button"
              onClick={() => setText(names.length === 0 ? "" : JSON.stringify(extra, null, 2))}
              className={`${PANEL_BUTTON} ml-auto shrink-0 py-1`}
            >
              <Pencil size={13} aria-hidden className="text-fg-muted" />
              {STUDIO_ADVANCED.editJson}
            </button>
          </>
        )}
      </div>
      {text !== null && (
        <>
          <textarea
            aria-label={spec.label}
            aria-describedby={problem ? problemId : undefined}
            value={text}
            rows={4}
            spellCheck={false}
            onChange={(e) => setText(e.target.value)}
            className={`${BOX} resize-y font-mono text-xs`}
          />
          {problem && (
            <p id={problemId} role="alert" className="text-xs text-warning">
              {problem}
            </p>
          )}
          <div className="flex gap-1.5">
            <button type="button" onClick={save} className={`${PANEL_BUTTON} py-1`}>
              {STUDIO_ADVANCED.save}
            </button>
            <button type="button" onClick={close} className={GHOST_BUTTON}>
              {STUDIO_ADVANCED.cancel}
            </button>
          </div>
        </>
      )}
      {allowed.length > 0 && <p className="text-xs text-fg-muted">{STUDIO_ADVANCED.accepts(allowed)}</p>}
    </div>
  );
}

// ---- routing (an OpenRouter picture) ------------------------------------------------------

/** The routing for the company chosen (`null`: OpenRouter chooses, and another may always run
 *  it), whether another may run it when that one is busy, and whether only companies that keep
 *  no data may; `null` when nothing is asked of OpenRouter. */
function routingOf(tag: string | null, allowFallbacks: boolean, zdr: boolean): Routing | null {
  if (tag === null && !zdr) return null;
  return { order: tag === null ? [] : [tag], only: false, allowFallbacks: tag === null || allowFallbacks, zdr };
}

/** The company list's value for OpenRouter's own choice (no company's tag is empty). */
const OPENROUTER_CHOOSES = " openrouter";

/** Which company runs an OpenRouter picture: one of those the model's price answer listed (by
 *  name, sent as its tag), or OpenRouter's choice. *Allow another company* comes back as last
 *  chosen when a company is picked again after OpenRouter chose. */
function RoutingFields(props: { tab: StudioTab; title: string; companies: EndpointCompany[]; routing: Routing | null }) {
  const { tab, companies, routing } = props;
  const setRouting = useStudio((s) => s.setRouting);
  const companyId = useId();
  const tag = routing?.order[0] ?? null;
  const allow = routing?.allowFallbacks ?? true;
  const zdr = routing?.zdr ?? false;
  // The last choice of *Allow another company*, kept while OpenRouter chooses (when another
  // company may always run it) for the next company picked. It follows a company's routing set
  // elsewhere, by a setup say.
  const [chosenAllow, setChosenAllow] = useState(allow);
  if (tag !== null && allow !== chosenAllow) setChosenAllow(allow);
  // A company saved in a setup that the model's answer no longer lists keeps its tag, named in
  // words.
  const unlisted = tag !== null && !companies.some((c) => c.tag === tag);
  const options = [
    { value: OPENROUTER_CHOOSES, label: STUDIO_ADVANCED.openRouterChooses },
    ...companies.map((c) => ({ value: c.tag, label: c.name })),
    ...(unlisted ? [{ value: tag, label: STUDIO_ADVANCED.unlistedCompany }] : []),
  ];
  return (
    <div className="flex flex-col gap-3 border-t border-border pt-4">
      <p className="text-section">{props.title}</p>
      <div className="flex flex-col gap-2">
        <span id={companyId} className="text-sm font-medium">
          {STUDIO_ADVANCED.company}
        </span>
        <SelectField
          label={STUDIO_ADVANCED.company}
          labelledBy={companyId}
          value={tag ?? OPENROUTER_CHOOSES}
          options={options}
          onChange={(v) => setRouting(tab, routingOf(v === OPENROUTER_CHOOSES ? null : v, chosenAllow, zdr))}
        />
      </div>
      <CheckboxField
        label={STUDIO_ADVANCED.allowAnother}
        checked={allow}
        disabled={tag === null}
        onChange={(on) => setRouting(tab, routingOf(tag, on, zdr))}
      />
      <CheckboxField
        label={STUDIO_ADVANCED.zdr}
        checked={zdr}
        onChange={(on) => setRouting(tab, routingOf(tag, allow, on))}
      />
    </div>
  );
}

// ---- the exact request, or command --------------------------------------------------------

/** The press a preview is made for: `studio_preview` takes a whole request and sends nothing. */
const PREVIEW_TOKEN = "preview";

/** How long the draft stays unchanged before an open preview is asked again: not at every key
 *  of the prompt (on this computer each ask also works out the engine's command and the fit). */
const PREVIEW_DELAY_MS = 300;

/** *See the exact request*: the host it goes to, the method and address, and the body as JSON,
 *  read-only, with the key removed (added pictures named, not sent). */
export function ExactRequest(props: { preview: RequestPreview }) {
  const { preview } = props;
  return (
    <div className="flex flex-col gap-1.5">
      <p className="text-xs text-fg-muted">{STUDIO_ADVANCED.goesTo(preview.host)}</p>
      <p className="font-mono text-xs break-all">{`${preview.method} ${preview.url}`}</p>
      <pre className="max-h-72 overflow-auto rounded-md border border-border bg-bg-muted p-2 font-mono text-xs break-all whitespace-pre-wrap">
        {JSON.stringify(preview.body, null, 2)}
      </pre>
    </div>
  );
}

/** *See the exact command* (a run on this computer; the viewer shows a run's the same way): the
 *  engine's arguments one a line, then the prompt and the negative prompt, which it reads from
 *  files; or, when the command could not be worked out, why. */
export function ExactCommand(props: { preview: RequestPreview }) {
  const { body } = props.preview;
  const argv = Array.isArray(body.argv) ? body.argv.map(String) : [];
  const prompt = typeof body.prompt === "string" ? body.prompt : "";
  const negative = typeof body.negativePrompt === "string" && body.negativePrompt !== "" ? body.negativePrompt : null;
  return (
    <div className="flex flex-col gap-1.5">
      {body.error ? (
        <p className="text-xs text-warning">{body.error}</p>
      ) : (
        <>
          <ol className="max-h-72 overflow-auto rounded-md border border-border bg-bg-muted p-2 font-mono text-xs">
            {argv.map((arg, i) => (
              <li key={i} className="break-all">
                {arg}
              </li>
            ))}
          </ol>
          <p className="text-xs break-words">{STUDIO_ADVANCED.promptFile(prompt)}</p>
          {negative !== null && <p className="text-xs break-words">{STUDIO_ADVANCED.negativeFile(negative)}</p>}
        </>
      )}
      <p className="text-xs text-fg-muted">{STUDIO_ADVANCED.runsHere}</p>
    </div>
  );
}

/** The button that shows what Create would send for `tab`'s draft (asked of the backend, which
 *  sends nothing), kept up to date while it is shown; a request the model would refuse shows
 *  why instead. Mounted afresh for each tab, so one tab's request never shows on another. */
function RequestPreviewButton(props: { tab: StudioTab; here: boolean }) {
  const { tab } = props;
  const draft = useStudio((s) => s.drafts[tab]);
  const req = useMemo(() => startRequestOf(tab, draft, PREVIEW_TOKEN), [tab, draft]);
  const [open, setOpen] = useState(false);
  // The request when it was opened, which is asked at once.
  const [opened, setOpened] = useState<StartRequest | null>(null);
  const [answer, setAnswer] = useState<{ preview: RequestPreview } | { error: ErrorInfo } | null>(null);

  // Asked at once when opened; while open, a changed draft is asked once it has stayed as it is
  // for `PREVIEW_DELAY_MS`. An answer for a draft changed since is dropped.
  useEffect(() => {
    if (!open || req === null) return;
    let current = true;
    const ask = () => {
      api.studioPreview(req).then(
        (preview) => {
          if (current) setAnswer({ preview });
        },
        (e: unknown) => {
          if (current) setAnswer({ error: toErrorInfo(e) });
        },
      );
    };
    let timer: ReturnType<typeof setTimeout> | undefined;
    if (req === opened) ask();
    else timer = setTimeout(ask, PREVIEW_DELAY_MS);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [open, req, opened]);

  const toggle = () => {
    if (open) setAnswer(null);
    else setOpened(req);
    setOpen(!open);
  };
  return (
    <div className="flex flex-col gap-2.5 border-t border-border pt-4">
      <button type="button" aria-expanded={open} onClick={toggle} className={`${PANEL_BUTTON} self-start`}>
        <Code2 size={14} aria-hidden className="text-fg-muted" />
        {props.here ? STUDIO_ADVANCED.seeCommand : STUDIO_ADVANCED.seeRequest}
      </button>
      {open &&
        answer !== null &&
        ("error" in answer ? (
          <ErrorBanner error={answer.error} />
        ) : answer.preview.method === "run" ? (
          <ExactCommand preview={answer.preview} />
        ) : (
          <ExactRequest preview={answer.preview} />
        ))}
    </div>
  );
}

// ---- the drawer ---------------------------------------------------------------------------

/** The Advanced drawer of `tab`'s right panel; nothing while the tab has no model. */
export function AdvancedDrawer(props: { tab: StudioTab }) {
  const { tab } = props;
  const draft = useStudio((s) => s.drafts[tab]);
  const model = useStudio((s) => draftModel(s, tab));
  const endpoint = useStudio((s) => endpointOf(s.drafts[tab].target, s.endpoints));
  const savedOpen = useStudio((s) => s.prefs?.advancedOpen ?? false);
  const resetAdvanced = useStudio((s) => s.resetAdvanced);
  const setPrefs = useStudio((s) => s.setPrefs);
  // Counts *Reset all*: the routing's remembered choices start again with it.
  const [resets, setResets] = useState(0);
  // Whether it is open: what was saved, and the toggle's own choice even when saving it fails;
  // a choice saved elsewhere since is followed.
  const [open, setOpen] = useState(savedOpen);
  const [shownSaved, setShownSaved] = useState(savedOpen);
  if (shownSaved !== savedOpen) {
    setShownSaved(savedOpen);
    setOpen(savedOpen);
  }
  if (!model) return null;

  const here = draft.target === "thisComputer";
  const openRouterPicture = endpoint?.company === "openrouter" && model.kind === "image";
  const extraSpec = model.params.find((p) => p.kind.type === "extra");
  const changed = changedCount(model, draft.values, draft.extra);

  const toggle = () => {
    const next = !open;
    setOpen(next);
    setPrefs({ advancedOpen: next }).catch((e: unknown) =>
      // It opens or closes all the same, for now; the failure leaves a trace.
      console.warn("Studio: whether Advanced is open could not be saved", e),
    );
  };

  const resetAll = () => {
    resetAdvanced(tab);
    setResets((n) => n + 1);
  };

  return (
    <div data-advanced className="flex flex-col gap-5">
      <div className="flex min-h-7 items-center gap-2">
        <h3 className="min-w-0">
          <button
            type="button"
            aria-expanded={open}
            onClick={toggle}
            className={`-ml-1 flex items-center gap-1 rounded-md px-1 py-0.5 text-section hover:text-fg ${FOCUS_RING}`}
          >
            <ChevronRight
              size={16}
              aria-hidden
              className={clsx("shrink-0 text-fg-muted transition-transform", open && "rotate-90")}
            />
            {changed > 0 ? STUDIO_FIELDS.advancedChanged(changed) : STUDIO_FIELDS.advanced}
          </button>
        </h3>
        {open && (changed > 0 || draft.routing !== null) && (
          <button type="button" onClick={resetAll} className={`${GHOST_BUTTON} ml-auto`}>
            <RotateCcw size={13} aria-hidden />
            {STUDIO_ADVANCED.resetAll}
          </button>
        )}
      </div>
      {open && (
        <>
          {visibleFields(model.params, "advanced", draft.values).map((spec) => (
            <SettingField key={spec.key} tab={tab} spec={spec} />
          ))}
          {openRouterPicture && endpoint && (
            <RoutingFields
              key={`${model.id} ${resets}`}
              tab={tab}
              title={endpoint.name}
              companies={model.companies}
              routing={draft.routing}
            />
          )}
          {extraSpec?.kind.type === "extra" ? (
            <ExtraFields
              key={model.id}
              tab={tab}
              spec={extraSpec}
              allowed={extraSpec.kind.allowedKeys}
              typed={model.params.filter((p) => p.kind.type !== "extra").map((p) => p.key)}
              extra={draft.extra}
            />
          ) : (
            openRouterPicture &&
            endpoint &&
            priceNotAsked(model, endpoint.company) && <p className="text-xs text-fg-muted">{STUDIO_ADVANCED.pickToSee}</p>
          )}
          <RequestPreviewButton key={tab} tab={tab} here={here} />
        </>
      )}
    </div>
  );
}

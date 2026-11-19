import { useEffect, useId, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, X } from "lucide-react";
import type { ErrorInfo } from "../../lib/types";
import { SETTING_WORDS, STUDIO_ADVANCED, STUDIO_FIELDS } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { draftModel, priceNotAsked, startRequestOf, useStudio, type StudioTab } from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import * as api from "./api";
import { changedCount, changedKeys, nameOf, valueWords, visibleFields, type Values } from "./canonical";
import { ParamField } from "./ParamField";
import { FrameDrop } from "./RefTray";
import type { EndpointCompany, MediaModel, ParamSpec, RequestPreview, Routing } from "./types";

// Studio's Advanced drawer (0.1.0 Task B6; design Wireframe 2, opening inside the right panel):
// its header counts the changes and offers *Reset all*; the model's Advanced fields follow in
// its order, each through ParamField with its reset (the seed with its dice and *Keep it*);
// then, for an OpenRouter picture, which company runs it; Extra fields (JSON), only the names
// the model lists; and *See the exact request*, what Create would send and to which host, with
// the key removed. For a model on this computer there is no routing and no Extra fields (they
// would become the engine's arguments), and the button is *See the exact command*. Whether the
// drawer is open is kept in the preferences. Each change also shows as a chip under the prompt
// (`ChipsRow`, which the prompt dock mounts), and the chip removes it.

const BOX =
  "rounded-md border border-border bg-bg px-2 py-1 text-sm outline-none focus:border-border-strong disabled:opacity-60";
const SMALL_BUTTON = "rounded-md px-1.5 py-0.5 text-xs text-fg-muted hover:bg-bg-muted hover:text-fg";

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
 *  Automatic, or out of the Extra fields); nothing while none is changed. The prompt dock shows
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
    <div role="group" aria-labelledby={labelId} className="flex flex-col gap-1.5">
      <div className="flex min-h-5 items-center gap-1.5">
        <span id={labelId} className="text-xs font-medium text-fg-muted">
          {spec.label}
        </span>
        {text === null && (
          <>
            <span className="min-w-0 truncate text-xs text-fg-muted">
              {names.length === 0 ? STUDIO_ADVANCED.noneSet : names.join(", ")}
            </span>
            <button
              type="button"
              onClick={() => setText(names.length === 0 ? "" : JSON.stringify(extra, null, 2))}
              className={`${SMALL_BUTTON} ml-auto`}
            >
              {STUDIO_ADVANCED.editJson}
            </button>
          </>
        )}
      </div>
      {text !== null && (
        <>
          <textarea
            aria-label={spec.label}
            value={text}
            rows={4}
            spellCheck={false}
            onChange={(e) => setText(e.target.value)}
            className={`${BOX} resize-y font-mono text-xs`}
          />
          {problem && <p className="text-xs text-warning">{problem}</p>}
          <div className="flex gap-1.5">
            <button
              type="button"
              onClick={save}
              className="rounded-md border border-border px-2 py-0.5 text-xs hover:border-border-strong"
            >
              {STUDIO_ADVANCED.save}
            </button>
            <button type="button" onClick={close} className={SMALL_BUTTON}>
              {STUDIO_ADVANCED.cancel}
            </button>
          </div>
        </>
      )}
      <p className="text-xs text-fg-muted">{STUDIO_ADVANCED.accepts(allowed)}</p>
    </div>
  );
}

// ---- routing (an OpenRouter picture) ------------------------------------------------------

/** The routing for the company chosen (`null`: OpenRouter chooses), whether another may run it
 *  when that one is busy, and whether only companies that keep no data may; `null` when
 *  nothing is asked of OpenRouter. */
function routingOf(tag: string | null, allowFallbacks: boolean, zdr: boolean): Routing | null {
  if (tag === null && !zdr) return null;
  return { order: tag === null ? [] : [tag], only: false, allowFallbacks: tag === null || allowFallbacks, zdr };
}

/** Which company runs an OpenRouter picture: one of those the model's price answer listed (by
 *  name, sent as its tag), or OpenRouter's choice. */
function RoutingFields(props: { tab: StudioTab; title: string; companies: EndpointCompany[]; routing: Routing | null }) {
  const { tab, companies, routing } = props;
  const setRouting = useStudio((s) => s.setRouting);
  const selectId = useId();
  const tag = routing?.order[0] ?? null;
  const allow = routing?.allowFallbacks ?? true;
  const zdr = routing?.zdr ?? false;
  // A company saved in a setup that the model's answer no longer lists keeps its tag.
  const unlisted = tag !== null && !companies.some((c) => c.tag === tag);
  return (
    <div className="flex flex-col gap-1.5">
      <p className="border-t border-border pt-3 text-xs font-medium">{props.title}</p>
      <label htmlFor={selectId} className="text-xs font-medium text-fg-muted">
        {STUDIO_ADVANCED.company}
      </label>
      <select
        id={selectId}
        value={tag ?? ""}
        onChange={(e) => setRouting(tab, routingOf(e.target.value === "" ? null : e.target.value, allow, zdr))}
        className={BOX}
      >
        <option value="">{STUDIO_ADVANCED.openRouterChooses}</option>
        {companies.map((c) => (
          <option key={c.tag} value={c.tag}>
            {c.name}
          </option>
        ))}
        {unlisted && <option value={tag}>{tag}</option>}
      </select>
      <label className="flex items-center gap-1.5 text-xs has-disabled:opacity-60">
        <input
          type="checkbox"
          checked={allow}
          disabled={tag === null}
          onChange={(e) => setRouting(tab, routingOf(tag, e.target.checked, zdr))}
          className="accent-[var(--accent)]"
        />
        {STUDIO_ADVANCED.allowAnother}
      </label>
      <label className="flex items-center gap-1.5 text-xs">
        <input
          type="checkbox"
          checked={zdr}
          onChange={(e) => setRouting(tab, routingOf(tag, allow, e.target.checked))}
          className="accent-[var(--accent)]"
        />
        {STUDIO_ADVANCED.zdr}
      </label>
    </div>
  );
}

// ---- the exact request, or command --------------------------------------------------------

/** The press a preview is made for: `studio_preview` takes a whole request and sends nothing. */
const PREVIEW_TOKEN = "preview";

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
 *  why instead. */
function RequestPreviewButton(props: { tab: StudioTab; here: boolean }) {
  const { tab } = props;
  const draft = useStudio((s) => s.drafts[tab]);
  const req = useMemo(() => startRequestOf(tab, draft, PREVIEW_TOKEN), [tab, draft]);
  const [open, setOpen] = useState(false);
  const [answer, setAnswer] = useState<{ preview: RequestPreview } | { error: ErrorInfo } | null>(null);

  // Asked when opened and again whenever the draft changes while open; an answer for a draft
  // changed since is dropped.
  useEffect(() => {
    if (!open || req === null) return;
    let current = true;
    api.studioPreview(req).then(
      (preview) => {
        if (current) setAnswer({ preview });
      },
      (e: unknown) => {
        if (current) setAnswer({ error: toErrorInfo(e) });
      },
    );
    return () => {
      current = false;
    };
  }, [open, req]);

  const toggle = () => {
    if (open) setAnswer(null);
    setOpen(!open);
  };
  return (
    <div className="flex flex-col gap-2 border-t border-border pt-3">
      <button
        type="button"
        aria-expanded={open}
        onClick={toggle}
        className="self-start rounded-lg border border-border px-2.5 py-1 text-xs hover:border-border-strong"
      >
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
  const endpoints = useStudio((s) => s.endpoints);
  const savedOpen = useStudio((s) => s.prefs?.advancedOpen ?? false);
  const setValue = useStudio((s) => s.setValue);
  const resetValue = useStudio((s) => s.resetValue);
  const resetAdvanced = useStudio((s) => s.resetAdvanced);
  const newSeed = useStudio((s) => s.newSeed);
  const lockSeed = useStudio((s) => s.lockSeed);
  const setPrefs = useStudio((s) => s.setPrefs);
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
  const target = draft.target;
  const endpoint =
    target !== null && target !== "thisComputer" ? endpoints.find((e) => e.providerId === target.endpoint) : undefined;
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

  const draw = (spec: ParamSpec) => {
    if (spec.kind.type === "images") {
      // Reference pictures are added in the tray beside the prompt.
      return spec.kind.role === "reference" ? null : <FrameDrop key={spec.key} tab={tab} spec={spec} />;
    }
    return (
      <ParamField
        key={spec.key}
        spec={spec}
        value={draft.values[spec.key]}
        onChange={(v) => setValue(tab, spec.key, v)}
        onReset={() => resetValue(tab, spec.key)}
        local={here}
        seed={
          spec.kind.type === "seed"
            ? { locked: draft.seedLocked, onNew: () => newSeed(tab), onLock: (locked) => lockSeed(tab, locked) }
            : undefined
        }
      />
    );
  };

  return (
    <div data-advanced className="flex flex-col gap-4 border-t border-border pt-3">
      <div className="flex items-center gap-2">
        <button
          type="button"
          aria-expanded={open}
          onClick={toggle}
          className="flex items-center gap-1 self-start text-sm font-medium hover:text-fg"
        >
          {open ? <ChevronDown size={14} aria-hidden /> : <ChevronRight size={14} aria-hidden />}
          {changed > 0 ? STUDIO_FIELDS.advancedChanged(changed) : STUDIO_FIELDS.advanced}
        </button>
        {open && (changed > 0 || draft.routing !== null) && (
          <button type="button" onClick={() => resetAdvanced(tab)} className={`${SMALL_BUTTON} ml-auto`}>
            {STUDIO_ADVANCED.resetAll}
          </button>
        )}
      </div>
      {open && (
        <>
          {visibleFields(model.params, "advanced", draft.values).map(draw)}
          {openRouterPicture && endpoint && (
            <RoutingFields tab={tab} title={endpoint.name} companies={model.companies} routing={draft.routing} />
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
          <RequestPreviewButton tab={tab} here={here} />
        </>
      )}
    </div>
  );
}

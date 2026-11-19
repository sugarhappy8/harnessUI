import { useCallback, useEffect, useState } from "react";
import { Cloud, Pencil, Plus, Trash2 } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import * as ipc from "../../lib/ipc";
import type { ErrorInfo, Provider, ProviderKind } from "../../lib/types";
import { toErrorInfo, useApp } from "../../store/app";
import { ErrorBanner } from "../ErrorBanner";
import {
  COMPANY_CHOICES,
  CONTEXT_CHOICES,
  ENDPOINT_CHOICES,
  SERVER_CHOICES,
  abilitiesOf,
  choiceFor,
  hostOf,
  originOf,
  switchChoice,
} from "./endpointPresets";

// Settings → Endpoints: the servers and cloud AI companies the app can chat through. [0.0.7
// P5] Ready-made choices for Anthropic, OpenAI, OpenRouter and abliteration.ai (pick one,
// paste the key), a context size for each endpoint, Edit, and a "Cloud" mark on an endpoint
// on the internet. The backend checks every value again when it is saved: it refuses an API
// key over plain http:// to a server on the internet, and a saved key for a new address.
// [0.1.0] Each card says what its endpoint can do ("Chat · Pictures · Video"): OpenRouter and
// OpenAI also make pictures (and OpenRouter video) in Studio.

interface Form {
  /** The endpoint being edited; `null` for a new one. */
  id: number | null;
  kind: ProviderKind;
  name: string;
  baseUrl: string;
  apiKey: string;
  /** The endpoint has a key saved. An empty box keeps it while the address keeps its
   *  scheme, host and port; a key is never sent to a new address. */
  keySaved: boolean;
  /** The address the saved key is for. */
  savedUrl: string | null;
  /** "Remove the saved key" is ticked. */
  removeKey: boolean;
  /** `null`: Automatic. */
  contextLength: number | null;
}

/** The saved key stays with the endpoint as the form now stands. */
function keyStays(form: Form): boolean {
  if (!form.keySaved || form.removeKey || form.savedUrl === null) return false;
  const saved = originOf(form.savedUrl);
  return saved !== null && saved === originOf(form.baseUrl);
}

function newForm(): Form {
  const first = ENDPOINT_CHOICES[0]!;
  return {
    id: null,
    kind: first.kind,
    name: first.name,
    baseUrl: first.baseUrl,
    apiKey: "",
    keySaved: false,
    savedUrl: null,
    removeKey: false,
    contextLength: null,
  };
}

function editForm(p: Provider): Form {
  return {
    id: p.id,
    kind: p.kind,
    name: p.name,
    baseUrl: p.baseUrl,
    apiKey: "",
    keySaved: p.hasApiKey,
    savedUrl: p.baseUrl,
    removeKey: false,
    contextLength: p.contextLength ?? null,
  };
}

/** The tooltip of an endpoint's "Cloud" mark. */
export function cloudTitle(baseUrl: string): string {
  return `Chats with this endpoint's models go to ${hostOf(baseUrl)}.`;
}

export function Endpoints() {
  const [providers, setProviders] = useState<Provider[]>([]);
  const [form, setForm] = useState<Form | null>(null);
  const [status, setStatus] = useState<Record<number, string>>({});
  const [error, setError] = useState<ErrorInfo | null>(null);
  const refreshModelOptions = useApp((s) => s.refreshModelOptions);
  const activeChatId = useApp((s) => s.activeChatId);
  const openChat = useApp((s) => s.openChat);

  const load = useCallback(async () => {
    try {
      setProviders(await ipc.listProviders());
    } catch (err) {
      setError(toErrorInfo(err));
    }
  }, []);
  useEffect(() => {
    let alive = true;
    ipc
      .listProviders()
      .then((p) => alive && setProviders(p))
      .catch((err) => alive && setError(toErrorInfo(err)));
    return () => {
      alive = false;
    };
  }, []);

  async function test(id: number) {
    setStatus((s) => ({ ...s, [id]: "Checking…" }));
    try {
      const models = await ipc.testProvider(id);
      setStatus((s) => ({ ...s, [id]: `Connected · ${models.length} model(s)` }));
      await refreshModelOptions();
    } catch (err) {
      setStatus((s) => ({ ...s, [id]: toErrorInfo(err).message }));
    }
  }

  async function save() {
    if (!form) return;
    setError(null);
    try {
      const saved = await ipc.saveProvider({
        id: form.id,
        name: form.name.trim() || choiceFor(form.kind).name,
        kind: form.kind,
        baseUrl: form.baseUrl.trim(),
        apiKey: form.apiKey.trim() || null,
        // Removed on request, or left behind when the address moved without a new key.
        clearApiKey: form.keySaved && form.apiKey.trim() === "" && !keyStays(form),
        contextLength: form.contextLength,
      });
      setForm(null);
      await load();
      // Lists its models for the model picker at once, and says whether the key works.
      await test(saved.id);
      // The open chat's "Cloud" mark and Thinking switch follow the edited endpoint.
      if (activeChatId != null && openChat) await openChat(activeChatId);
    } catch (err) {
      setError(toErrorInfo(err));
    }
  }

  function openPage(url: string) {
    openUrl(url).catch((err) =>
      setError({
        ...toErrorInfo(err),
        message: "The page could not be opened in your browser.",
        nextStep: `Go to ${url} in your browser.`,
      }),
    );
  }

  const choice = form ? choiceFor(form.kind) : null;
  const company = choice?.company;
  const stays = form ? keyStays(form) : false;
  const keyMoved = !!form && form.keySaved && !form.removeKey && !stays;
  const automatic = company?.defaultContext ?? null;
  const contextOptions =
    form?.contextLength != null && !CONTEXT_CHOICES.includes(form.contextLength)
      ? [...CONTEXT_CHOICES, form.contextLength].sort((a, b) => a - b)
      : CONTEXT_CHOICES;

  return (
    <div className="space-y-4">
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <p className="text-sm text-fg-muted">
        Use a cloud AI company with your API key, or connect a server on this computer or your
        network: Ollama, LM Studio, vLLM or any OpenAI-compatible server. API keys are kept in
        your system keychain and sent only to their endpoint.
      </p>
      <ul className="space-y-2">
        {providers.map((p) => (
          <li key={p.id} className="flex items-center gap-2 rounded-md border border-border px-3 py-2 text-sm">
            <div className="min-w-0 flex-1">
              <p className="flex items-center gap-1.5 font-medium">
                {p.name}
                {p.cloud && (
                  <span
                    data-cloud-mark
                    title={cloudTitle(p.baseUrl)}
                    className="flex items-center gap-1 rounded-full border border-border px-1.5 text-xs font-normal text-fg-muted"
                  >
                    <Cloud size={12} aria-hidden />
                    Cloud
                    <span className="sr-only">: {cloudTitle(p.baseUrl)}</span>
                  </span>
                )}
                <span data-abilities className="text-xs font-normal text-fg-muted">
                  {abilitiesOf(p.kind)}
                </span>
              </p>
              <p className="truncate text-xs text-fg-muted">
                {p.baseUrl}
                {p.hasApiKey ? " · API key saved" : ""}
                {p.contextLength != null ? ` · at most ${p.contextLength.toLocaleString()} tokens` : ""}
              </p>
              {status[p.id] && <p className="text-xs">{status[p.id]}</p>}
            </div>
            <button type="button" className="text-xs underline" onClick={() => void test(p.id)}>
              Test
            </button>
            <button
              type="button"
              aria-label={`Edit ${p.name}`}
              title="Edit"
              onClick={() => setForm(editForm(p))}
              className="text-fg-muted hover:text-fg"
            >
              <Pencil size={14} aria-hidden />
            </button>
            <button
              type="button"
              aria-label={`Remove ${p.name}`}
              onClick={() => void ipc.deleteProvider(p.id).then(load).then(refreshModelOptions)}
              className="text-fg-muted hover:text-danger"
            >
              <Trash2 size={14} aria-hidden />
            </button>
          </li>
        ))}
      </ul>
      {form && choice ? (
        <form
          aria-label={form.id === null ? "New endpoint" : `Edit ${form.name}`}
          className="grid grid-cols-[7rem_1fr] items-center gap-2 rounded-md border border-border p-3 text-sm"
          onSubmit={(e) => {
            e.preventDefault();
            void save();
          }}
        >
          <label htmlFor="p-kind">Company or server</label>
          <select
            id="p-kind"
            value={form.kind}
            onChange={(e) => {
              const kind = e.target.value as ProviderKind;
              setForm({ ...form, kind, ...switchChoice(form, form.kind, kind) });
            }}
            className="rounded-md border border-border bg-bg px-2 py-1"
          >
            <optgroup label="AI companies (with your API key)">
              {COMPANY_CHOICES.map((c) => (
                <option key={c.kind} value={c.kind}>
                  {c.label}
                </option>
              ))}
            </optgroup>
            <optgroup label="Servers">
              {SERVER_CHOICES.map((c) => (
                <option key={c.kind} value={c.kind}>
                  {c.label}
                </option>
              ))}
            </optgroup>
          </select>
          {company && (
            <p data-company-note className="col-span-2 text-xs text-fg-muted">
              Chats that use its models go to {company.name}: your messages, the files you attach
              and what tools return. Get an API key at{" "}
              <button type="button" className="underline" onClick={() => openPage(company.keyUrl)}>
                {company.keyPage}
              </button>
              .
            </p>
          )}
          <label htmlFor="p-name">Name</label>
          <input
            id="p-name"
            value={form.name}
            onChange={(e) => setForm({ ...form, name: e.target.value })}
            className="rounded-md border border-border bg-bg px-2 py-1"
          />
          <label htmlFor="p-url">Address</label>
          <input
            id="p-url"
            required
            value={form.baseUrl}
            onChange={(e) => setForm({ ...form, baseUrl: e.target.value })}
            className="rounded-md border border-border bg-bg px-2 py-1 font-mono text-xs"
          />
          <label htmlFor="p-key">API key</label>
          <input
            id="p-key"
            type="password"
            autoComplete="off"
            required={!!company && !stays}
            value={form.apiKey}
            placeholder={stays ? "saved (leave empty to keep it)" : company ? "paste your key" : "optional"}
            aria-describedby={keyMoved ? "p-key-moved" : undefined}
            onChange={(e) => setForm({ ...form, apiKey: e.target.value })}
            className="rounded-md border border-border bg-bg px-2 py-1"
          />
          {keyMoved && (
            <p id="p-key-moved" className="col-span-2 text-xs text-fg-muted">
              The saved key is for {hostOf(form.savedUrl ?? "")} and is not sent to a new address.
              {company ? " Paste the key for this address." : " Paste a key if this server needs one."}
            </p>
          )}
          {form.keySaved && (
            <label className="col-span-2 flex items-center gap-2 text-xs">
              <input
                type="checkbox"
                checked={form.removeKey}
                onChange={(e) => setForm({ ...form, removeKey: e.target.checked })}
              />
              Remove the saved key
            </label>
          )}
          <label htmlFor="p-ctx">Context size</label>
          <select
            id="p-ctx"
            aria-describedby="p-ctx-help"
            value={form.contextLength ?? ""}
            onChange={(e) => setForm({ ...form, contextLength: e.target.value === "" ? null : Number(e.target.value) })}
            className="rounded-md border border-border bg-bg px-2 py-1"
          >
            <option value="">Automatic</option>
            {contextOptions.map((n) => (
              <option key={n} value={n}>
                at most {n.toLocaleString()} tokens
              </option>
            ))}
          </select>
          <p id="p-ctx-help" className="col-span-2 text-xs text-fg-muted">
            How much of a chat the app sends with each message at most.{" "}
            {company && automatic !== null
              ? `Automatic: what the server reports for each model, at most ${automatic.toLocaleString()} tokens${
                  company.contextNote ? ` (${company.contextNote})` : ""
                }. A long chat sends up to this much with every message, and the company charges for each token.`
              : "Automatic: what the server reports for each model, or else the size chosen below for servers that report none."}
            {company && automatic !== null && form.contextLength !== null && form.contextLength > automatic
              ? " A larger size uses much more of your credit with every message of a long chat."
              : ""}
          </p>
          <div className="col-span-2 flex justify-end gap-2">
            <button type="button" onClick={() => setForm(null)} className="px-2 py-1">
              Cancel
            </button>
            <button type="submit" className="rounded-md bg-accent px-3 py-1 text-accent-fg">
              Save
            </button>
          </div>
        </form>
      ) : (
        <button
          type="button"
          onClick={() => setForm(newForm())}
          className="flex items-center gap-1 text-sm text-accent"
        >
          <Plus size={14} aria-hidden /> Add an endpoint
        </button>
      )}
      <RemoteContext onError={setError} />
    </div>
  );
}

const REMOTE_CTX_OPTIONS = [2048, 4096, 8192, 16384, 32768];
const REMOTE_CTX_DEFAULT = 4096;

/** Context size for an endpoint whose model reports none (8.4; setting "remoteContextLength"). */
function RemoteContext(props: { onError: (e: ErrorInfo) => void }) {
  const [ctx, setCtx] = useState<number | null>(null);
  const { onError } = props;
  useEffect(() => {
    let alive = true;
    ipc
      .getSetting<unknown>("remoteContextLength")
      .then((v) => {
        if (!alive) return;
        // Missing or invalid means the default, as on the Rust side.
        setCtx(typeof v === "number" && Number.isInteger(v) && v > 0 ? v : REMOTE_CTX_DEFAULT);
      })
      .catch((err) => {
        if (!alive) return;
        setCtx(REMOTE_CTX_DEFAULT);
        onError(toErrorInfo(err));
      });
    return () => {
      alive = false;
    };
  }, [onError]);
  if (ctx === null) return null;
  const options = REMOTE_CTX_OPTIONS.includes(ctx)
    ? REMOTE_CTX_OPTIONS
    : [...REMOTE_CTX_OPTIONS, ctx].sort((a, b) => a - b);
  async function save(n: number) {
    const before = ctx;
    setCtx(n);
    try {
      await ipc.setSetting("remoteContextLength", n);
    } catch (err) {
      setCtx(before);
      onError(toErrorInfo(err));
    }
  }
  return (
    <section className="space-y-1 border-t border-border pt-4 text-sm">
      <label htmlFor="remote-ctx" className="block font-medium">
        Context size when a server does not report one
      </label>
      <p id="remote-ctx-help" className="text-xs text-fg-muted">
        Most servers say how much context their model uses. For one that does not, the app plans
        with this size. If it is larger than what the server really uses, older messages can be
        dropped without warning. This size is not used for an AI company&apos;s endpoint, which
        plans with what its server reports up to a size of its own, nor for an endpoint given a
        size with Edit.
      </p>
      <select
        id="remote-ctx"
        aria-describedby="remote-ctx-help"
        value={ctx}
        onChange={(e) => void save(Number(e.target.value))}
        className="rounded-md border border-border bg-bg px-2 py-1"
      >
        {options.map((n) => (
          <option key={n} value={n}>
            {n.toLocaleString()} tokens{n === REMOTE_CTX_DEFAULT ? " (default)" : ""}
          </option>
        ))}
      </select>
    </section>
  );
}

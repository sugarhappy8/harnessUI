import { useCallback, useEffect, useState } from "react";
import type { ErrorInfo } from "../lib/types";
import { toErrorInfo } from "../store/app";
import { ErrorBanner } from "./ErrorBanner";
import * as api from "./sources/api";
import type { SearchProvider, SearchSettings as Settings, TestSearch } from "./sources/types";

const PROVIDERS: { id: SearchProvider; name: string; about: string }[] = [
  { id: "duckduckgo", name: "DuckDuckGo", about: "No account and no key needed." },
  { id: "brave", name: "Brave Search", about: "Needs an API key from Brave." },
  { id: "tavily", name: "Tavily", about: "Needs an API key from Tavily." },
  { id: "searxng", name: "SearXNG", about: "Your own SearXNG instance, by its address." },
];

/** One API key: whether it is set, a field to enter a new one, and a way to remove it.
 *  The key itself never comes back from the app, so it is never shown. */
function KeyField(props: {
  provider: "brave" | "tavily";
  name: string;
  isSet: boolean;
  onChanged: (s: Settings) => void;
  onError: (e: ErrorInfo) => void;
}) {
  const [key, setKey] = useState("");
  const id = `search-key-${props.provider}`;
  return (
    <div className="space-y-1">
      <label htmlFor={id} className="block text-xs text-fg-muted">
        {props.name} API key: <span className={props.isSet ? "text-success" : ""}>{props.isSet ? "set" : "not set"}</span>
      </label>
      <div className="flex flex-wrap gap-2">
        <input
          id={id}
          type="password"
          autoComplete="off"
          spellCheck={false}
          placeholder={props.isSet ? "Enter a new key to replace it" : "Paste the key"}
          value={key}
          onChange={(e) => setKey(e.target.value)}
          className="w-72 rounded-md border border-border bg-bg px-2 py-1 font-mono text-xs"
        />
        <button
          type="button"
          disabled={key.trim() === ""}
          onClick={() =>
            void api
              .setSearchKey(props.provider, key)
              .then((s) => {
                setKey("");
                props.onChanged(s);
              })
              .catch((e) => props.onError(toErrorInfo(e)))
          }
          className="rounded-md border border-border px-3 py-1 disabled:opacity-40"
        >
          Set key
        </button>
        {props.isSet && (
          <button
            type="button"
            onClick={() =>
              void api
                .clearSearchKey(props.provider)
                .then(props.onChanged)
                .catch((e) => props.onError(toErrorInfo(e)))
            }
            className="rounded-md border border-border px-3 py-1"
          >
            Remove key
          </button>
        )}
      </div>
    </div>
  );
}

/** Settings → Search: which search service the model's web searches go to, its key or
 *  address, how many pages are read, and matching to the project's library versions. */
export function SearchSettings() {
  const [settings, setSettings] = useState<Settings | null>(null);
  /** The service chosen on screen. SearXNG is saved only together with its address. */
  const [provider, setProvider] = useState<SearchProvider>("duckduckgo");
  const [address, setAddress] = useState("");
  const [error, setError] = useState<ErrorInfo | null>(null);
  const [testing, setTesting] = useState(false);
  const [tested, setTested] = useState<TestSearch | null>(null);

  const apply = useCallback((s: Settings) => {
    setSettings(s);
    setProvider(s.provider);
    setAddress(s.searxngUrl);
    setTested(null);
  }, []);
  useEffect(() => {
    void api
      .getSearchSettings()
      .then(apply)
      .catch((e) => setError(toErrorInfo(e)));
  }, [apply]);

  if (!settings) return error ? <ErrorBanner error={error} onDismiss={() => setError(null)} /> : null;

  function save(change: Partial<Pick<Settings, "provider" | "searxngUrl" | "pages" | "versionMatching">>) {
    if (!settings) return;
    setError(null);
    void api
      .setSearchSettings({
        provider: settings.provider,
        searxngUrl: settings.searxngUrl,
        pages: settings.pages,
        versionMatching: settings.versionMatching,
        ...change,
      })
      .then(apply)
      .catch((e) => setError(toErrorInfo(e)));
  }

  function choose(next: SearchProvider) {
    setProvider(next);
    setTested(null);
    // Its address is asked for first; "Set address" then saves both.
    if (next === "searxng" && settings?.searxngUrl === "") return;
    save({ provider: next });
  }

  async function test() {
    setTesting(true);
    setTested(null);
    setError(null);
    try {
      setTested(await api.testSearch());
    } catch (e) {
      setError(toErrorInfo(e));
    } finally {
      setTesting(false);
    }
  }

  const unsaved = provider !== settings.provider;
  return (
    <div className="space-y-4 text-sm">
      <h2 className="text-lg font-medium">Search</h2>
      <p className="text-fg-muted">
        When the model searches the web, the search is sent to the service chosen here, and the pages it finds are
        then fetched from their own sites. Every search asks for your approval first, like other tools.
      </p>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}

      <section className="space-y-2">
        <h3 className="font-medium">Search service</h3>
        <div role="radiogroup" aria-label="Search service" className="space-y-1">
          {PROVIDERS.map((p) => (
            <button
              key={p.id}
              type="button"
              role="radio"
              aria-checked={provider === p.id}
              onClick={() => choose(p.id)}
              className={
                "flex w-full items-baseline gap-2 rounded-lg border px-3 py-1.5 text-left " +
                (provider === p.id ? "border-accent bg-accent/10" : "border-border hover:bg-bg-muted")
              }
            >
              <span className="font-medium">{p.name}</span>
              <span className="text-xs text-fg-muted">{p.about}</span>
            </button>
          ))}
        </div>
        {provider === "brave" && (
          <KeyField provider="brave" name="Brave Search" isSet={settings.braveKeySet} onChanged={apply} onError={setError} />
        )}
        {provider === "tavily" && (
          <KeyField provider="tavily" name="Tavily" isSet={settings.tavilyKeySet} onChanged={apply} onError={setError} />
        )}
        {(provider === "brave" || provider === "tavily") && (
          <p className="text-xs text-fg-muted">
            The key is kept in the system keychain and sent only to that service. It is never shown again here.
          </p>
        )}
        {provider === "searxng" && (
          <div className="space-y-1">
            <label htmlFor="searxng-address" className="block text-xs text-fg-muted">
              Address of your SearXNG instance (it must allow results in the JSON format)
            </label>
            <div className="flex flex-wrap gap-2">
              <input
                id="searxng-address"
                type="url"
                spellCheck={false}
                placeholder="http://127.0.0.1:8888"
                value={address}
                onChange={(e) => setAddress(e.target.value)}
                className="w-80 rounded-md border border-border bg-bg px-2 py-1 font-mono text-xs"
              />
              <button
                type="button"
                disabled={address.trim() === "" || (address.trim() === settings.searxngUrl && !unsaved)}
                onClick={() => save({ provider: "searxng", searxngUrl: address })}
                className="rounded-md border border-border px-3 py-1 disabled:opacity-40"
              >
                Set address
              </button>
            </div>
            {unsaved && <p className="text-xs text-warning">Enter the address to switch to SearXNG.</p>}
          </div>
        )}
      </section>

      <section className="space-y-2">
        <h3 className="font-medium">Reading pages</h3>
        <label className="flex items-center gap-2">
          <span>Pages read per search</span>
          <select
            aria-label="Pages read per search"
            value={settings.pages}
            onChange={(e) => save({ pages: Number(e.target.value) })}
            className="rounded-md border border-border bg-bg px-2 py-1"
          >
            {[0, 1, 2, 3, 4, 5, 6, 7, 8].map((n) => (
              <option key={n} value={n}>
                {n === 0 ? "None (summaries only)" : n}
              </option>
            ))}
          </select>
        </label>
        <p className="text-xs text-fg-muted">
          Pages are never read from this computer or your local network, whatever a search result or a redirect points to.
        </p>
      </section>

      <section className="space-y-1">
        <label className="flex items-start gap-2">
          <input
            type="checkbox"
            className="mt-1"
            checked={settings.versionMatching}
            onChange={(e) => save({ versionMatching: e.target.checked })}
          />
          <span>Match results to the library versions this project uses</span>
        </label>
        <p className="pl-6 text-xs text-fg-muted">
          The versions are read from the project files in the chat&apos;s working folder (package.json, Cargo.toml,
          pyproject.toml, requirements.txt, go.mod). When a question is about one of those libraries, its version is
          added to the search, and its package index (npm, PyPI or crates.io) is asked once where its documentation
          is. Official documentation is listed first either way.
        </p>
      </section>

      <section className="space-y-2">
        <button
          type="button"
          disabled={testing || unsaved}
          onClick={() => void test()}
          className="rounded-md border border-border px-3 py-1 disabled:opacity-40"
        >
          {testing ? "Searching…" : "Test search"}
        </button>
        {tested && (
          <div role="status" className="rounded-lg border border-border bg-bg-subtle p-2 text-xs">
            {tested.results.length === 0 ? (
              <p>{tested.provider} answered, but with no results.</p>
            ) : (
              <>
                <p className="text-success">
                  {tested.provider} answered with {tested.results.length === 1 ? "one result" : `${tested.results.length} results`}.
                </p>
                <ul className="mt-1 space-y-0.5 text-fg-muted">
                  {tested.results.slice(0, 3).map((r, i) => (
                    <li key={i} className="truncate">
                      {r.title} · {r.site}
                    </li>
                  ))}
                </ul>
              </>
            )}
          </div>
        )}
      </section>
    </div>
  );
}

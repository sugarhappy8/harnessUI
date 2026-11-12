import { useCallback, useEffect, useRef, useState } from "react";
import { Lock, Search } from "lucide-react";
import * as ipc from "../../lib/ipc";
import type { ErrorInfo, HubSort, RepoSummary } from "../../lib/types";
import { formatCount } from "../../lib/format";
import { toErrorInfo } from "../../store/app";
import { ErrorBanner } from "../ErrorBanner";
import { Badge } from "./Badge";
import { RepoDetail } from "./RepoDetail";

/** Models → Browse: Hugging Face search, and a repository's page. */
export function Browse(props: { onStarted: () => void }) {
  const [text, setText] = useState("");
  const [sort, setSort] = useState<HubSort>("downloads");
  const [allGguf, setAllGguf] = useState(false);
  const [items, setItems] = useState<RepoSummary[]>([]);
  const [next, setNext] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const timer = useRef<number | undefined>(undefined);

  const run = useCallback(async (q: string, s: HubSort, all: boolean) => {
    // "owner/repo" opens that repository directly.
    if (/^[\w.-]+\/[\w.-]+$/.test(q.trim())) {
      setSelected(q.trim());
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const page = await ipc.hubSearch(q, s, all);
      setItems(page.items);
      setNext(page.next);
    } catch (err) {
      setError(toErrorInfo(err));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    // Debounced so typing does not spend the anonymous rate limit.
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => void run(text, sort, allGguf), text ? 400 : 0);
    return () => window.clearTimeout(timer.current);
  }, [text, sort, allGguf, run]);

  async function more() {
    if (!next) return;
    setBusy(true);
    try {
      const page = await ipc.hubSearchNext(next);
      setItems((old) => [...old, ...page.items]);
      setNext(page.next);
    } catch (err) {
      setError(toErrorInfo(err));
    } finally {
      setBusy(false);
    }
  }

  if (selected) {
    return <RepoDetail repoId={selected} onBack={() => setSelected(null)} onStarted={props.onStarted} />;
  }
  return (
    <div className="space-y-3">
      <div className="flex gap-2">
        <label className="relative flex-1">
          <span className="sr-only">Search Hugging Face</span>
          <Search size={14} className="absolute top-2 left-2 text-fg-muted" aria-hidden />
          <input
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="Search models, or paste owner/repo"
            className="w-full rounded-md border border-border bg-bg py-1.5 pr-2 pl-7 text-sm"
          />
        </label>
        <select
          aria-label="Sort by"
          value={sort}
          onChange={(e) => setSort(e.target.value as HubSort)}
          className="rounded-md border border-border bg-bg px-2 text-sm"
        >
          <option value="downloads">Most downloaded</option>
          <option value="trendingScore">Trending</option>
          <option value="likes">Most liked</option>
          <option value="lastModified">Recently updated</option>
        </select>
      </div>
      <label className="flex items-center gap-2 text-xs text-fg-muted">
        <input type="checkbox" checked={allGguf} onChange={(e) => setAllGguf(e.target.checked)} />
        Show all GGUF repositories (including speech, image and video models that cannot chat)
      </label>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <ul className="space-y-1">
        {items.map((r) => (
          <li key={r.id}>
            <button
              type="button"
              onClick={() => setSelected(r.id)}
              className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-bg-muted"
            >
              <span className="min-w-0 flex-1 truncate">{r.id}</span>
              {r.gated !== "no" && <Lock size={12} className="text-fg-muted" aria-label="gated" />}
              {r.license && <Badge>{r.license}</Badge>}
              <span className="w-16 text-right text-xs text-fg-muted" title="Downloads">
                {formatCount(r.downloads)}
              </span>
            </button>
          </li>
        ))}
      </ul>
      {!busy && items.length === 0 && !error && <p className="text-sm text-fg-muted">No matching models.</p>}
      {next && (
        <button type="button" disabled={busy} onClick={() => void more()} className="text-sm text-accent disabled:opacity-40">
          {busy ? "Loading…" : "Show more"}
        </button>
      )}
      <p className="text-xs text-fg-muted">
        Results come from huggingface.co. Model cards are shown as plain text, without images.
      </p>
    </div>
  );
}

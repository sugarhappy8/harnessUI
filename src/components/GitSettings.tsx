import { useCallback, useEffect, useState } from "react";
import { Trash2 } from "lucide-react";
import * as ipc from "../lib/ipc";
import type { ErrorInfo, GitInfo } from "../lib/types";
import { toErrorInfo } from "../store/app";
import { ErrorBanner } from "./ErrorBanner";

/** Settings → Git: which git the tools use, tokens per host, and the commit identity. */
export function GitSettings() {
  const [info, setInfo] = useState<GitInfo | null>(null);
  const [host, setHost] = useState("");
  const [token, setToken] = useState("");
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);

  const refresh = useCallback(() => {
    void ipc
      .gitInfo()
      .then((i) => {
        setInfo(i);
        setName(i.identity.name);
        setEmail(i.identity.email);
      })
      .catch((e) => setError(toErrorInfo(e)));
  }, []);
  useEffect(refresh, [refresh]);

  async function addToken() {
    try {
      await ipc.setGitToken(host, token);
      setHost("");
      setToken("");
      refresh();
    } catch (e) {
      setError(toErrorInfo(e));
    }
  }

  async function saveIdentity() {
    try {
      await ipc.setGitIdentity({ name, email });
      setSaved(true);
      setTimeout(() => setSaved(false), 1500);
    } catch (e) {
      setError(toErrorInfo(e));
    }
  }

  if (!info) return error ? <ErrorBanner error={error} onDismiss={() => setError(null)} /> : null;
  return (
    <div className="space-y-4 text-sm">
      <h2 className="text-lg font-medium">Git</h2>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <p className={info.notice ? "text-warning" : "text-fg-muted"}>
        {info.notice ??
          "The git tools use the Git installed on this computer, with hooks, external diff tools and the pager switched off. Push always asks and shows where it pushes."}
      </p>
      <section className="space-y-2">
        <h3 className="font-medium">Access tokens</h3>
        <p className="text-xs text-fg-muted">
          {info.backend === "libgit2"
            ? "Private repositories over https need a token for their host (for GitHub, a fine-grained personal access token). Tokens are kept in the system keychain and only sent to that host."
            : "Your installed Git uses its own credential helper; tokens here are only used if Git is uninstalled later."}
        </p>
        <ul className="space-y-1">
          {info.tokenHosts.map((h) => (
            <li key={h} className="flex items-center gap-2">
              <span className="font-mono text-xs">{h}</span>
              <button
                type="button"
                aria-label={`Remove the token for ${h}`}
                onClick={() =>
                  void ipc
                    .removeGitToken(h)
                    .then(refresh)
                    .catch((e) => setError(toErrorInfo(e)))
                }
              >
                <Trash2 size={13} aria-hidden />
              </button>
            </li>
          ))}
        </ul>
        <div className="flex flex-wrap gap-2">
          <input
            aria-label="Host"
            placeholder="github.com"
            value={host}
            onChange={(e) => setHost(e.target.value)}
            className="w-48 rounded-md border border-border bg-bg px-2 py-1 font-mono text-xs"
          />
          <input
            aria-label="Token"
            type="password"
            placeholder="token"
            autoComplete="off"
            value={token}
            onChange={(e) => setToken(e.target.value)}
            className="w-64 rounded-md border border-border bg-bg px-2 py-1 font-mono text-xs"
          />
          <button
            type="button"
            disabled={!host.trim() || !token.trim()}
            onClick={() => void addToken()}
            className="rounded-md border border-border px-3 py-1 disabled:opacity-40"
          >
            Save token
          </button>
        </div>
      </section>
      <section className="space-y-2">
        <h3 className="font-medium">Name and email for commits</h3>
        <p className="text-xs text-fg-muted">Used when the model commits. Leave empty to use your Git configuration.</p>
        <div className="flex flex-wrap gap-2">
          <input
            aria-label="Name"
            placeholder="Your Name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            className="w-48 rounded-md border border-border bg-bg px-2 py-1"
          />
          <input
            aria-label="Email"
            placeholder="you@example.com"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            className="w-64 rounded-md border border-border bg-bg px-2 py-1"
          />
          <button type="button" onClick={() => void saveIdentity()} className="rounded-md border border-border px-3 py-1">
            {saved ? "Saved" : "Save"}
          </button>
        </div>
      </section>
    </div>
  );
}

import { useCallback, useEffect, useMemo, useState } from "react";
import { Popover } from "radix-ui";
import { AlertTriangle, Wrench } from "lucide-react";
import * as ipc from "../lib/ipc";
import type { ToolDto } from "../lib/types";
import { useApp, toErrorInfo } from "../store/app";

const SERVER_LABEL: Record<string, string> = {
  files: "Files",
  terminal: "Terminal",
  git: "Git",
  skills: "Skills",
};

/**
 * The tools switched on for this chat (9.3). Only switched-on tools are offered to the
 * model; each costs context, shown as an estimate.
 */
export function ToolsPanel(props: { chatId: number; nCtx: number | null }) {
  const [tools, setTools] = useState<ToolDto[]>([]);
  const [open, setOpen] = useState(false);
  const showError = useApp((s) => s.showError);

  const refresh = useCallback(() => {
    ipc
      .listChatTools(props.chatId)
      .then(setTools)
      .catch((e) => showError(toErrorInfo(e)));
  }, [props.chatId, showError]);

  useEffect(() => {
    refresh();
    let un: (() => void) | undefined;
    void ipc.onMcpChanged(refresh).then((u) => (un = u));
    return () => un?.();
  }, [refresh]);

  const enabled = tools.filter((t) => t.enabledInChat && t.available && !t.schemaError);
  const cost = enabled.reduce((n, t) => n + t.tokenEstimate, 0);
  const groups = useMemo(() => {
    const m = new Map<string, ToolDto[]>();
    for (const t of tools) m.set(t.server, [...(m.get(t.server) ?? []), t]);
    return [...m.entries()];
  }, [tools]);

  async function save(next: string[]) {
    try {
      await ipc.setChatTools(props.chatId, next);
      refresh();
    } catch (e) {
      showError(toErrorInfo(e));
    }
  }

  function toggle(names: string[], on: boolean) {
    const current = new Set(tools.filter((t) => t.enabledInChat).map((t) => t.exposed));
    for (const n of names) {
      if (on) current.add(n);
      else current.delete(n);
    }
    void save([...current]);
  }

  return (
    <Popover.Root open={open} onOpenChange={(o) => (setOpen(o), o && refresh())}>
      <Popover.Trigger asChild>
        <button
          type="button"
          className="flex items-center gap-1 rounded-md px-2 py-1 text-sm text-fg-muted hover:bg-bg-muted hover:text-fg"
          title="Tools for this chat"
        >
          <Wrench size={14} aria-hidden /> Tools ({enabled.length})
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          align="end"
          sideOffset={6}
          className="z-50 max-h-[70vh] w-96 overflow-y-auto rounded-lg border border-border bg-bg p-3 text-sm shadow-lg"
        >
          <div className="mb-2 flex items-baseline justify-between">
            <h2 className="font-medium">Tools for this chat</h2>
            <span className="text-xs text-fg-muted" title="Context the tool descriptions take on every request">
              ~{cost.toLocaleString()} tokens
              {props.nCtx ? ` (${Math.round((cost / props.nCtx) * 100)}% of context)` : ""}
            </span>
          </div>
          {props.nCtx && cost > props.nCtx * 0.25 && (
            <p className="mb-2 flex items-start gap-1 text-xs text-warning">
              <AlertTriangle size={12} className="mt-0.5 shrink-0" aria-hidden />
              The switched-on tools take a large part of the context. Switch off the ones this chat does not need.
            </p>
          )}
          {groups.map(([server, list]) => {
            const names = list.filter((t) => t.available && !t.schemaError).map((t) => t.exposed);
            const allOn = names.length > 0 && names.every((n) => list.find((t) => t.exposed === n)?.enabledInChat);
            return (
              <section key={server} className="mb-3">
                <label className="flex items-center gap-2 font-medium">
                  <input
                    type="checkbox"
                    checked={allOn}
                    disabled={names.length === 0}
                    onChange={(e) => toggle(names, e.target.checked)}
                  />
                  {SERVER_LABEL[server] ?? server}
                  {server === "terminal" && (
                    <span className="text-xs font-normal text-fg-muted">— runs commands as you; not a sandbox</span>
                  )}
                </label>
                <ul className="mt-1 space-y-1 pl-6">
                  {list.map((t) => (
                    <li key={t.exposed}>
                      <label className="flex items-start gap-2" title={t.description}>
                        <input
                          type="checkbox"
                          className="mt-0.5"
                          checked={t.enabledInChat && t.available}
                          disabled={!t.available || t.schemaError !== null}
                          onChange={(e) => toggle([t.exposed], e.target.checked)}
                        />
                        <span className="min-w-0 flex-1">
                          <span className="font-mono text-xs">{t.exposed}</span>
                          {!t.ready && t.available && <span className="ml-1 text-xs text-warning">not running</span>}
                          {t.changed && (
                            <span className="ml-1 text-xs text-warning">changed — review it in Settings → MCP servers</span>
                          )}
                          {!t.available && !t.changed && !t.builtin && (
                            <span className="ml-1 text-xs text-fg-muted">off in Settings → MCP servers</span>
                          )}
                          {t.schemaError && <span className="ml-1 text-xs text-danger">{t.schemaError}</span>}
                        </span>
                        <span className="text-xs text-fg-muted">~{t.tokenEstimate}</span>
                      </label>
                    </li>
                  ))}
                </ul>
              </section>
            );
          })}
          <button
            type="button"
            className="mt-1 text-xs text-fg-muted underline hover:text-fg"
            onClick={() =>
              void ipc
                .clearChatGrants(props.chatId)
                .then(() => setOpen(false))
                .catch((e) => showError(toErrorInfo(e)))
            }
          >
            Forget everything I allowed in this chat
          </button>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

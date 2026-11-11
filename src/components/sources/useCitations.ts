import { useEffect, useMemo } from "react";
import { useApp } from "../../store/app";
import type { Citations } from "../Markdown";
import { startSources, useSources } from "./state";

// Message items are part of the app from its first render, so this is where the
// `sources-changed` event starts being listened to (once, and only inside the app).
startSources();

/** Opens the panel beside the chat on source `n`. */
export function openSource(chatId: number, n: number): void {
  useSources.getState().requestFocus(chatId, n);
  useApp.getState().openWorkbench("sources");
}

/**
 * What the answers of the chat on screen check their `[n]` against. `undefined` until the
 * chat's sources are known (and for anything that is not an answer): citations are then
 * left as the plain text they are, never marked as missing by mistake.
 */
export function useCitations(enabled: boolean): Citations | undefined {
  const chatId = useApp((s) => s.activeChatId);
  const sources = useSources((s) => (chatId === null ? undefined : s.byChat[chatId]));
  useEffect(() => {
    if (enabled && chatId !== null) useSources.getState().ensure(chatId);
  }, [enabled, chatId]);
  return useMemo(() => {
    if (!enabled || chatId === null || !sources) return undefined;
    return {
      sources: new Map(sources.map((s) => [s.n, { title: s.title, site: s.site }])),
      onOpen: (n: number) => openSource(chatId, n),
    };
  }, [enabled, chatId, sources]);
}

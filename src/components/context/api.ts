// The pinned-instruction commands (src-tauri/src/pinned.rs), as typed calls. Every change
// returns the chat's pins as they now are.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { PinList } from "./types";

export const listPins = (chatId: number) => invoke<PinList>("list_pins", { chatId });

/** Pins what the user typed, for this chat or for every chat. */
export const addPin = (chatId: number, text: string, everyChat: boolean) =>
  invoke<PinList>("add_pin", { chatId, text, everyChat });

export const editPin = (chatId: number, id: number, text: string) =>
  invoke<PinList>("edit_pin", { chatId, id, text });

export const removePin = (chatId: number, id: number) => invoke<PinList>("remove_pin", { chatId, id });

/** One place up or down among the pins of the same kind. */
export const movePin = (chatId: number, id: number, up: boolean) =>
  invoke<PinList>("move_pin", { chatId, id, up });

export const setPinScope = (chatId: number, id: number, everyChat: boolean) =>
  invoke<PinList>("set_pin_scope", { chatId, id, everyChat });

/** Pins changed: those of one chat (its id), or those for every chat (null). */
export function onPinsChanged(handler: (chatId: number | null) => void): Promise<UnlistenFn> {
  return listen<number | null>("pinned-changed", (e) => handler(e.payload));
}

// What the pinned-instruction commands (src-tauri/src/pinned.rs) return.

/** One pinned instruction. The user's own text, shown as plain text. */
export interface Pin {
  id: number;
  text: string;
  /** For every chat, or for this chat only. */
  everyChat: boolean;
}

/** The pins that apply to a chat, in the order the model gets them: those for every chat,
 *  then the chat's own. */
export interface PinList {
  pins: Pin[];
  /** The most pins that may apply to one chat. */
  max: number;
  /** The most characters a pin may have. */
  maxChars: number;
}

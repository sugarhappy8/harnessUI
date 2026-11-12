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

/** One section of the next request (src-tauri/src/context_cmd.rs). All text is plain text. */
export interface ContextSection {
  id: string;
  title: string;
  /** One line under the title. */
  detail: string;
  /** About how many tokens; null when no numbers can be given. */
  tokens: number | null;
  /** The exact text. Untrusted in part (tool results, file text): never rendered as markup. */
  text: string;
}

export interface ContextShare {
  used: number;
  allowed: number;
}

export interface ContextTotals {
  tokens: number;
  /** Counted by the model server; otherwise an estimate. */
  exact: boolean;
  nCtx: number;
  budget: number;
  trigger: number;
  tools: ContextShare | null;
  pinned: ContextShare;
}

/** Exactly what the next request of a chat would contain. */
export interface ContextView {
  model: string;
  totals: ContextTotals | null;
  sections: ContextSection[];
  nextDrop: string;
  refusal: string | null;
  notes: string[];
}

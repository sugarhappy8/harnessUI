import { useEffect, useState } from "react";
import * as ipc from "../../lib/ipc";
import type { ErrorInfo } from "../../lib/types";
import { toErrorInfo } from "../../store/app";
import { ErrorBanner } from "../ErrorBanner";

// [0.0.7] Settings → General: how many times in a row the model may make the same call (the
// same tool with the same arguments) before the turn stops. The app warns the model two calls
// before the limit (one call before at 3; at 2 there is no room for a warning). The backend
// checks the number when it is saved and again when a turn reads it.

/** The setting's key ("sameCallLimit" in `src-tauri/src/settings.rs`). */
export const SAME_CALL_LIMIT = "sameCallLimit";
export const DEFAULT_SAME_CALL_LIMIT = 5;
export const MIN_SAME_CALL_LIMIT = 2;
export const MAX_SAME_CALL_LIMIT = 100;

/** A typed number held to 2..100; `null` when the box holds no whole number. */
export function sameCallLimitFrom(text: string): number | null {
  const n = Number(text.trim());
  if (text.trim() === "" || !Number.isInteger(n)) return null;
  return Math.min(MAX_SAME_CALL_LIMIT, Math.max(MIN_SAME_CALL_LIMIT, n));
}

/**
 * The limit a turn uses for a stored value, as `settings::same_call_limit` reads it: a whole
 * number from 0 up, held to 2..100; anything else (a negative or broken number, text) is 5.
 */
export function storedSameCallLimit(value: unknown): number {
  if (typeof value !== "number" || !Number.isInteger(value) || value < 0) {
    return DEFAULT_SAME_CALL_LIMIT;
  }
  return Math.min(MAX_SAME_CALL_LIMIT, Math.max(MIN_SAME_CALL_LIMIT, value));
}

/** "Stop when the model makes the same call this many times in a row" (setting "sameCallLimit"). */
export function SameCallLimit() {
  const [saved, setSaved] = useState<number>(DEFAULT_SAME_CALL_LIMIT);
  const [text, setText] = useState<string>(String(DEFAULT_SAME_CALL_LIMIT));
  const [error, setError] = useState<ErrorInfo | null>(null);
  useEffect(() => {
    let alive = true;
    ipc
      .getSetting<unknown>(SAME_CALL_LIMIT)
      .then((v) => {
        if (!alive) return;
        // A number saved some other way shows as a turn uses it.
        const value = storedSameCallLimit(v);
        setSaved(value);
        setText(String(value));
      })
      .catch((err) => alive && setError(toErrorInfo(err)));
    return () => {
      alive = false;
    };
  }, []);

  async function save() {
    const n = sameCallLimitFrom(text);
    if (n === null) {
      setText(String(saved));
      return;
    }
    setText(String(n));
    if (n === saved) return;
    setError(null);
    try {
      await ipc.setSetting(SAME_CALL_LIMIT, n);
      setSaved(n);
    } catch (err) {
      setText(String(saved));
      setError(toErrorInfo(err));
    }
  }

  return (
    <fieldset>
      <legend className="mb-1 text-sm font-medium">Same call in a row</legend>
      <label className="flex items-center gap-2 text-sm">
        <span>Stop when the model makes the same call this many times in a row</span>
        <input
          type="number"
          inputMode="numeric"
          min={MIN_SAME_CALL_LIMIT}
          max={MAX_SAME_CALL_LIMIT}
          step={1}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onBlur={() => void save()}
          onKeyDown={(e) => {
            if (e.key === "Enter") void save();
          }}
          className="w-20 rounded-md border border-border bg-bg px-2 py-1 text-sm"
        />
      </label>
      <p className="mt-1 text-xs text-fg-muted">
        The model is warned two calls before the limit (one call before at 3, no warning at 2).
        From 2 to 100; 5 unless you change it.
      </p>
      {error && (
        <div className="mt-2">
          <ErrorBanner error={error} onDismiss={() => setError(null)} />
        </div>
      )}
    </fieldset>
  );
}

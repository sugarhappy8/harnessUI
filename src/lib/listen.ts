import type { UnlistenFn } from "@tauri-apps/api/event";

/**
 * [0.0.7 P6] Starts listening for an app event and returns what stops it, for an effect's
 * cleanup. Listening starts asynchronously, so the cleanup can run before the listener
 * exists: a component that unmounts at once (React's development mode mounts every effect
 * twice), or a chat switched quickly. Then the listener is removed as soon as `listen`
 * resolves, instead of staying behind with the old chat for good.
 *
 * [0.0.7 P6a review] Until then the old handler can still be called: Tauri registers it
 * before `listen` resolves, so an event sent in that short time reaches it after the cleanup
 * ran. The callers allow for that: on an event they read again what is on screen, or apply
 * what the newer listener applies too (a card by its id, the runtime's status), and a handler
 * that must not act once its effect is gone checks for itself (`followRunningTurn`).
 */
export function listenInEffect(start: () => Promise<UnlistenFn>): () => void {
  let stop: UnlistenFn | undefined;
  let gone = false;
  start()
    .then((unlisten) => {
      if (gone) unlisten();
      else stop = unlisten;
    })
    .catch(() => undefined);
  return () => {
    gone = true;
    stop?.();
    stop = undefined;
  };
}

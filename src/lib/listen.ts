import type { UnlistenFn } from "@tauri-apps/api/event";

/**
 * [0.0.7 P6] Starts listening for an app event and returns what stops it, for an effect's
 * cleanup. Listening starts asynchronously, so the cleanup can run before the listener
 * exists: a component that unmounts at once (React's development mode mounts every effect
 * twice), or a chat switched quickly. Then the listener is stopped as soon as it exists,
 * instead of staying behind with the old chat and calling its handler later.
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

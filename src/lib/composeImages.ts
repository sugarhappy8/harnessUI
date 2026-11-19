// The window event that puts pictures in the chat's message box (0.1.0 Task B11). Studio's
// *Send to chat* (src/store/studioResults.ts) sends it; the composer (src/components/Composer.tsx)
// attaches the pictures, up to its 8, and checks each one first, since any code can send it.
import type { ImageInput } from "./types";

/** The event's name. */
export const COMPOSE_IMAGES_EVENT = "app:compose-images";

/** The event's detail: the pictures to attach, as the composer takes them. */
export type ComposeImagesDetail = ImageInput[];

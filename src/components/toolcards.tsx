import type { ReactNode } from "react";
import type { ToolCallView } from "./ToolCallCard";

/**
 * What a feature adds to the card of its own tools, by tool name: a button that opens the
 * artifact a call made, the sources a search found, a link to a full log. The card shows
 * it under its header. Arguments and results come from the model and from tools: render
 * them as plain text only.
 *
 * One entry per tool. Each feature adds its own lines and nothing else.
 */
export const TOOL_CARD_EXTRAS: Record<string, (call: ToolCallView) => ReactNode> = {};

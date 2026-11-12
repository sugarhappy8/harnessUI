import type { ReactNode } from "react";
import type { ToolCallView } from "./ToolCallCard";

/** The tool that runs another tool by name (src-tauri/src/tool_select/finder.rs). */
const CALL_TOOL = "call_tool";

/**
 * The call a `call_tool` call stands for: the tool it names, with that tool's arguments
 * and the same result. `null` for any other call, and when it names nothing usable.
 * The name and the arguments were written by the model: they are data.
 */
export function calledTool(call: ToolCallView): ToolCallView | null {
  if (call.name !== CALL_TOOL) return null;
  const name = typeof call.arguments.name === "string" ? call.arguments.name.trim() : "";
  if (name === "" || name === CALL_TOOL) return null;
  const inner = call.arguments.arguments;
  const args =
    inner !== null && typeof inner === "object" && !Array.isArray(inner) ? (inner as Record<string, unknown>) : {};
  return { ...call, name, arguments: args };
}

/**
 * What the card of a `call_tool` call shows under its header: whatever the tool it ran
 * shows on its own cards (an artifact's Open button, the sources of a search), so a tool
 * reached this way looks the same as one the model called directly.
 */
export function calledToolExtra(
  call: ToolCallView,
  extras: Record<string, (call: ToolCallView) => ReactNode>,
): ReactNode {
  const target = calledTool(call);
  // Own entries only: the name is the model's text, and every object has a "constructor".
  if (!target || !Object.hasOwn(extras, target.name)) return null;
  return extras[target.name]?.(target) ?? null;
}

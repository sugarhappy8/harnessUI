import type { ReactNode } from "react";
import type { ToolCallView } from "./ToolCallCard";
import { calledToolExtra } from "./CalledTool";
import { artifactCardExtra } from "./artifacts/ArtifactCardExtra";
import { readSourceExtra, searchHitsExtra } from "./sources/SearchHits";

/**
 * What a feature adds to the card of its own tools, by tool name: a button that opens the
 * artifact a call made, the sources a search found, a link to a full log. The card shows
 * it under its header. Arguments and results come from the model and from tools: render
 * them as plain text only.
 *
 * One entry per tool. Each feature adds its own lines and nothing else.
 */
export const TOOL_CARD_EXTRAS: Record<string, (call: ToolCallView) => ReactNode> = {
  // A tool run through `call_tool` shows what it shows when it is called directly.
  call_tool: (call) => calledToolExtra(call, TOOL_CARD_EXTRAS),
  // An artifact the call saved: "Open artifact" shows it in the panel beside the chat.
  create_artifact: artifactCardExtra,
  update_artifact: artifactCardExtra,
  // The sources a search found (number, title, site); a click shows one in the panel.
  web_search: searchHitsExtra,
  read_source: readSourceExtra,
};

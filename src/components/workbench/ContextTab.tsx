import { ContextInspector } from "../context/ContextInspector";
import { PinnedList } from "../context/PinnedList";

/** The "Context" tab of the panel beside the chat: the chat's pinned instructions, then
 *  exactly what the next request would contain. */
export function ContextTab(props: { chatId: number }) {
  return (
    <div data-chat={props.chatId}>
      <PinnedList chatId={props.chatId} />
      <ContextInspector chatId={props.chatId} />
    </div>
  );
}

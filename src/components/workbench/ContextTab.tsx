import { PinnedList } from "../context/PinnedList";

/** The "Context" tab of the panel beside the chat: the chat's pinned instructions. */
export function ContextTab(props: { chatId: number }) {
  return (
    <div data-chat={props.chatId}>
      <PinnedList chatId={props.chatId} />
    </div>
  );
}

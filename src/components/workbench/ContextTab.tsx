/** The "Context" tab of the panel beside the chat. Owned by its feature; this is the
 *  empty state the panel shows until that feature lands. */
export function ContextTab(props: { chatId: number }) {
  return (
    <p className="p-6 text-sm text-fg-muted" data-chat={props.chatId}>
      Pinned instructions, and exactly what will be sent to the model next, appear here.
    </p>
  );
}

/** The "Sources" tab of the panel beside the chat. Owned by its feature; this is the
 *  empty state the panel shows until that feature lands. */
export function SourcesTab(props: { chatId: number }) {
  return (
    <p className="p-6 text-sm text-fg-muted" data-chat={props.chatId}>
      Pages a web search retrieved for this chat appear here, with the text the answer relied on.
    </p>
  );
}

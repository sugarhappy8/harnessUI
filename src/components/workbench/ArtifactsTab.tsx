/** The "Artifacts" tab of the panel beside the chat. Owned by its feature; this is the
 *  empty state the panel shows until that feature lands. */
export function ArtifactsTab(props: { chatId: number }) {
  return (
    <p className="p-6 text-sm text-fg-muted" data-chat={props.chatId}>
      Documents, pages and diagrams the model makes in this chat appear here.
    </p>
  );
}

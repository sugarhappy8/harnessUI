/** The "Task" tab of the panel beside the chat. Owned by its feature; this is the
 *  empty state the panel shows until that feature lands. */
export function TaskTab(props: { chatId: number }) {
  return (
    <p className="p-6 text-sm text-fg-muted" data-chat={props.chatId}>
      The goal, steps, changed files and checks of the work in this chat appear here.
    </p>
  );
}

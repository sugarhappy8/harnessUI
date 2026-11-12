// A small label used by the Models dialog's sections. Shared by Installed, Browse and
// RepoDetail; change it only together with all three.

export function Badge(props: { children: React.ReactNode; title?: string }) {
  return (
    <span title={props.title} className="rounded border border-border px-1.5 py-0.5 text-[11px] text-fg-muted">
      {props.children}
    </span>
  );
}

// A small label used by the Models dialog's sections. Shared by Installed, Browse and
// RepoDetail; change it only together with all three.
import clsx from "clsx";

/** `tone="danger"`: the label names what is wrong (the kind of model this version cannot open). */
export function Badge(props: { children: React.ReactNode; title?: string; tone?: "danger" }) {
  return (
    <span
      title={props.title}
      data-tone={props.tone}
      className={clsx(
        "rounded border px-1.5 py-0.5 text-[11px]",
        props.tone === "danger" ? "border-danger/40 bg-danger/10 text-danger" : "border-border text-fg-muted",
      )}
    >
      {props.children}
    </span>
  );
}

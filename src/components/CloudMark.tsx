import { Cloud } from "lucide-react";

// [0.0.7 P5] The chat header's mark beside the model's name when the chat's messages go to a
// server on the internet: a cloud AI company, or any endpoint that is not on this computer or
// the local network (the backend decides, `agent::endpoint_place`).

/** The mark's tooltip. */
export function cloudMarkTitle(host: string): string {
  return `Messages and files in this chat go to ${host}.`;
}

/** "Cloud", with where the chat goes in its tooltip; nothing when `host` is empty. */
export function CloudMark(props: { host: string | null | undefined }) {
  if (!props.host) return null;
  const title = cloudMarkTitle(props.host);
  return (
    <span
      data-cloud-mark
      title={title}
      className="flex shrink-0 items-center gap-1 rounded-full border border-border px-1.5 text-xs whitespace-nowrap text-fg-muted"
    >
      <Cloud size={12} aria-hidden />
      Cloud
      <span className="sr-only">: {title}</span>
    </span>
  );
}

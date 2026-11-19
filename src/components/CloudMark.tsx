import { Cloud } from "lucide-react";
import { PlaceMark } from "./PlaceMark";

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
  return <PlaceMark mark="cloud" icon={Cloud} label="Cloud" title={cloudMarkTitle(props.host)} />;
}

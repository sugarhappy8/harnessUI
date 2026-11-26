import type { ReactElement } from "react";
import { Tooltip } from "radix-ui";
import { POPUP } from "./styles";

/** A short hint on hover or keyboard focus. Needs the one `Tooltip.Provider` at the app's root. */
export function Tip(props: { content: string; side?: "top" | "right" | "bottom" | "left"; children: ReactElement }) {
  return (
    <Tooltip.Root>
      <Tooltip.Trigger asChild>{props.children}</Tooltip.Trigger>
      <Tooltip.Portal>
        <Tooltip.Content
          side={props.side ?? "top"}
          sideOffset={6}
          className={`${POPUP} max-w-72 px-2.5 py-1.5 text-xs`}
        >
          {props.content}
        </Tooltip.Content>
      </Tooltip.Portal>
    </Tooltip.Root>
  );
}

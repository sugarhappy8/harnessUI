import type { ReactNode } from "react";
import { DropdownMenu } from "radix-ui";
import { Check } from "lucide-react";
import { FIELD, OPTION, POPUP } from "./styles";

/** A button that opens a menu (Radix DropdownMenu); `trigger` is what the button shows. */
export function MenuButton(props: { label: string; trigger: ReactNode; align?: "start" | "end"; children: ReactNode }) {
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger aria-label={props.label} className={FIELD}>
        {props.trigger}
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content align={props.align ?? "start"} sideOffset={4} className={`${POPUP} min-w-40`}>
          {props.children}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

/** One action in a menu. */
export function MenuItem(props: { onSelect(): void; disabled?: boolean; children: ReactNode }) {
  return (
    <DropdownMenu.Item className={OPTION} disabled={props.disabled} onSelect={props.onSelect}>
      {props.children}
    </DropdownMenu.Item>
  );
}

/** A choice of one among several in a menu, the chosen one ticked. */
export function MenuRadio<T extends string>(props: {
  value: T;
  options: { value: T; label: string }[];
  onChange(v: T): void;
}) {
  return (
    <DropdownMenu.RadioGroup value={props.value} onValueChange={(v) => props.onChange(v as T)}>
      {props.options.map((o) => (
        <DropdownMenu.RadioItem key={o.value} value={o.value} className={OPTION}>
          {o.label}
          <DropdownMenu.ItemIndicator className="ml-auto">
            <Check className="size-4 text-accent" aria-hidden />
          </DropdownMenu.ItemIndicator>
        </DropdownMenu.RadioItem>
      ))}
    </DropdownMenu.RadioGroup>
  );
}

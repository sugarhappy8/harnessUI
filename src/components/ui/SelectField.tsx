import { useId } from "react";
import { Select } from "radix-ui";
import { Check, ChevronDown } from "lucide-react";
import clsx from "clsx";
import { FIELD, OPTION, POPUP } from "./styles";
import { installTrustedTypes } from "../../lib/trustedTypes";

/** A closed list of options that opens as a styled popup (Radix Select), named by its label.
 *  `labelledBy` names it by a label its owner draws instead (a Studio field's row), and then it
 *  draws none of its own.
 *
 *  Radix draws the list's viewport with a fixed `<style>` written as HTML, even while closed,
 *  which the window's Trusted Types rule refuses without the app's policy (the blank window of
 *  0.2.0). The app makes that policy before it draws anything (main.tsx); a SelectField makes
 *  sure of it too (once, and nothing where the browser has no Trusted Types), so it can never be
 *  drawn without it. */
export function SelectField<T extends string>(props: {
  label: string;
  hideLabel?: boolean;
  labelledBy?: string;
  value: T | null;
  placeholder?: string;
  options: { value: T; label: string; hint?: string }[];
  onChange(v: T): void;
  disabled?: boolean;
}) {
  installTrustedTypes();
  const id = useId();
  return (
    <div className="flex flex-col gap-1">
      {props.labelledBy === undefined && (
        <label id={id} className={clsx("text-xs text-fg-muted", props.hideLabel && "sr-only")}>
          {props.label}
        </label>
      )}
      {/* Always controlled: "" (no value) shows the placeholder, also after a value was chosen. */}
      <Select.Root
        value={props.value ?? ""}
        onValueChange={(v) => props.onChange(v as T)}
        disabled={props.disabled}
      >
        <Select.Trigger aria-labelledby={props.labelledBy ?? id} className={FIELD}>
          <Select.Value placeholder={props.placeholder} />
          <Select.Icon>
            <ChevronDown className="size-4 text-fg-muted" aria-hidden />
          </Select.Icon>
        </Select.Trigger>
        <Select.Portal>
          <Select.Content
            position="popper"
            sideOffset={4}
            className={clsx(POPUP, "min-w-[var(--radix-select-trigger-width)]")}
          >
            <Select.Viewport>
              {props.options.map((o) => (
                <Select.Item key={o.value} value={o.value} className={OPTION}>
                  <Select.ItemText>{o.label}</Select.ItemText>
                  {o.hint && <span className="ml-auto pl-3 text-xs text-fg-muted">{o.hint}</span>}
                  <Select.ItemIndicator className={clsx(!o.hint && "ml-auto")}>
                    <Check className="size-4 text-accent" aria-hidden />
                  </Select.ItemIndicator>
                </Select.Item>
              ))}
            </Select.Viewport>
          </Select.Content>
        </Select.Portal>
      </Select.Root>
    </div>
  );
}

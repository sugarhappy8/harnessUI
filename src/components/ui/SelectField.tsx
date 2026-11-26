import { useId } from "react";
import { Select } from "radix-ui";
import { Check, ChevronDown } from "lucide-react";
import clsx from "clsx";
import { FIELD, OPTION, POPUP } from "./styles";

/** A closed list of options that opens as a styled popup (Radix Select), named by its label.
 *  `labelledBy` names it by a label its owner draws instead (a Studio field's row), and then it
 *  draws none of its own. */
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

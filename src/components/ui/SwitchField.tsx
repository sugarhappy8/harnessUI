import { useId } from "react";
import { Switch } from "radix-ui";
import { FOCUS_RING } from "./styles";

/** An on/off switch with its label, and an optional line of help under it. */
export function SwitchField(props: {
  label: string;
  description?: string;
  checked: boolean;
  onChange(v: boolean): void;
  disabled?: boolean;
}) {
  const id = useId();
  const desc = `${id}-d`;
  return (
    <div className="flex items-start justify-between gap-3">
      <div className="flex min-w-0 flex-col">
        <label htmlFor={id} className="text-sm">
          {props.label}
        </label>
        {props.description && (
          <span id={desc} className="text-xs text-fg-muted">
            {props.description}
          </span>
        )}
      </div>
      <Switch.Root
        id={id}
        checked={props.checked}
        onCheckedChange={props.onChange}
        disabled={props.disabled}
        aria-describedby={props.description ? desc : undefined}
        className={`relative mt-0.5 h-5 w-9 shrink-0 rounded-full bg-bg-muted transition-colors data-[state=checked]:bg-accent disabled:opacity-60 ${FOCUS_RING}`}
      >
        <Switch.Thumb className="block size-4 translate-x-0.5 rounded-full bg-surface shadow-soft transition-transform data-[state=checked]:translate-x-[18px]" />
      </Switch.Root>
    </div>
  );
}

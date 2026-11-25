import { useId } from "react";
import { Checkbox } from "radix-ui";
import { Check } from "lucide-react";
import { FOCUS_RING } from "./styles";

/** A tick box with its label. */
export function CheckboxField(props: { label: string; checked: boolean; onChange(v: boolean): void }) {
  const id = useId();
  return (
    <div className="flex items-center gap-2">
      <Checkbox.Root
        id={id}
        checked={props.checked}
        onCheckedChange={(v) => props.onChange(v === true)}
        className={`flex size-4 shrink-0 items-center justify-center rounded border border-border-strong bg-surface data-[state=checked]:border-accent data-[state=checked]:bg-accent ${FOCUS_RING}`}
      >
        <Checkbox.Indicator>
          <Check className="size-3 text-accent-fg" strokeWidth={3} aria-hidden />
        </Checkbox.Indicator>
      </Checkbox.Root>
      <label htmlFor={id} className="text-sm">
        {props.label}
      </label>
    </div>
  );
}

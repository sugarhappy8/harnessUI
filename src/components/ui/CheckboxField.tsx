import { useId } from "react";
import { Checkbox } from "radix-ui";
import { Check } from "lucide-react";
import { FOCUS_RING } from "./styles";

/** The box as it is drawn: in a form, or over a picture (round, on a dark veil, so it shows on
 *  any picture: a grid tile in Select mode, Task U9). */
const BOX = {
  form: "size-4 rounded border border-border-strong bg-surface",
  overlay: "size-6 rounded-full border-2 border-white/90 bg-black/35 shadow-soft backdrop-blur-sm",
} as const;

/** A tick box with its label. `hideLabel` keeps the label for a screen reader only (a grid
 *  tile's box, named for its picture); `tone` draws the box over a picture. */
export function CheckboxField(props: {
  label: string;
  checked: boolean;
  onChange(v: boolean): void;
  disabled?: boolean;
  hideLabel?: boolean;
  tone?: keyof typeof BOX;
}) {
  const id = useId();
  const overlay = props.tone === "overlay";
  return (
    <div className="flex items-center gap-2 has-disabled:opacity-60">
      <Checkbox.Root
        id={id}
        checked={props.checked}
        disabled={props.disabled}
        onCheckedChange={(v) => props.onChange(v === true)}
        className={`flex shrink-0 items-center justify-center ${BOX[props.tone ?? "form"]} data-[state=checked]:border-accent data-[state=checked]:bg-accent ${FOCUS_RING}`}
      >
        <Checkbox.Indicator>
          <Check className={`${overlay ? "size-4" : "size-3"} text-accent-fg`} strokeWidth={3} aria-hidden />
        </Checkbox.Indicator>
      </Checkbox.Root>
      <label htmlFor={id} className={props.hideLabel ? "sr-only" : "text-sm"}>
        {props.label}
      </label>
    </div>
  );
}

import { ToggleGroup } from "radix-ui";
import { FOCUS_RING } from "./styles";

/** A row of chips of which one is pressed at a time. Pressing the pressed one keeps it.
 *  `size` "md" draws them at body size, for a page's main filter (Get models). */
export function ChipGroup<T extends string>(props: {
  label: string;
  value: T;
  options: { value: T; label: string }[];
  onChange(v: T): void;
  size?: "sm" | "md";
}) {
  return (
    <ToggleGroup.Root
      type="single"
      role="group"
      aria-label={props.label}
      value={props.value}
      onValueChange={(v) => {
        if (v) props.onChange(v as T);
      }}
      className="flex flex-wrap gap-1.5"
    >
      {props.options.map((o) => (
        <ToggleGroup.Item
          role="button"
          aria-pressed={o.value === props.value}
          aria-checked={undefined}
          key={o.value}
          value={o.value}
          className={`rounded-full border border-border whitespace-nowrap hover:border-border-strong aria-pressed:border-accent aria-pressed:bg-accent/10 aria-pressed:text-accent ${
            props.size === "md" ? "bg-surface px-3.5 py-1 text-sm aria-pressed:font-medium" : "px-2.5 py-0.5 text-xs"
          } ${FOCUS_RING}`}
        >
          {o.label}
        </ToggleGroup.Item>
      ))}
    </ToggleGroup.Root>
  );
}

import { Slider } from "radix-ui";
import { FOCUS_RING } from "./styles";

/** A slider with its label and its value as text beside it. */
export function SliderField(props: {
  label: string;
  min: number;
  max: number;
  step: number;
  value: number;
  onChange(v: number): void;
  format?(v: number): string;
}) {
  const text = props.format ? props.format(props.value) : String(props.value);
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-baseline justify-between gap-3 text-sm">
        <span>{props.label}</span>
        <span className="text-xs text-fg-muted tabular-nums">{text}</span>
      </div>
      <Slider.Root
        min={props.min}
        max={props.max}
        step={props.step}
        value={[props.value]}
        onValueChange={(v) => props.onChange(v[0] ?? props.value)}
        className="relative flex h-5 w-full touch-none items-center select-none"
      >
        <Slider.Track className="relative h-1.5 grow rounded-full bg-bg-muted">
          <Slider.Range className="absolute h-full rounded-full bg-accent" />
        </Slider.Track>
        <Slider.Thumb
          aria-label={props.label}
          aria-valuetext={text}
          className={`block size-4 rounded-full border border-border-strong bg-surface shadow-soft ${FOCUS_RING}`}
        />
      </Slider.Root>
    </div>
  );
}

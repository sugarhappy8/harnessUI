import { Slider } from "radix-ui";
import { FOCUS_RING } from "./styles";

/** A slider with its label and its value as text beside it. `name` is its accessible name when
 *  that says more than the label ("Strength of pixel art" under a LoRA's name). `hideLabel`
 *  leaves the label and value to its owner, which draws them in its own row (a Studio field's).
 *  `unset`: nothing is chosen yet, so the thumb is grey and the track unfilled wherever it rests
 *  (a Studio field left on Automatic, Task U9), until it is moved. */
export function SliderField(props: {
  label: string;
  name?: string;
  hideLabel?: boolean;
  min: number;
  max: number;
  step: number;
  value: number;
  onChange(v: number): void;
  format?(v: number): string;
  unset?: boolean;
}) {
  const unset = props.unset ? "" : undefined;
  const text = props.format ? props.format(props.value) : String(props.value);
  return (
    <div className="flex flex-col gap-1.5">
      {!props.hideLabel && (
        <div className="flex items-baseline justify-between gap-3 text-sm">
          <span>{props.label}</span>
          <span className="text-xs text-fg-muted tabular-nums">{text}</span>
        </div>
      )}
      <Slider.Root
        min={props.min}
        max={props.max}
        step={props.step}
        value={[props.value]}
        onValueChange={(v) => props.onChange(v[0] ?? props.value)}
        className="relative flex h-5 w-full touch-none items-center select-none"
      >
        <Slider.Track className="relative h-1.5 grow rounded-full bg-border-strong/70">
          <Slider.Range
            data-slider-range
            data-unset={unset}
            className="absolute h-full rounded-full bg-accent data-[unset]:bg-transparent"
          />
        </Slider.Track>
        <Slider.Thumb
          aria-label={props.name ?? props.label}
          aria-valuetext={text}
          data-unset={unset}
          className={`block size-4 rounded-full border-2 border-accent bg-surface shadow-soft data-[unset]:border-fg-muted/70 data-[unset]:bg-bg-muted data-[unset]:shadow-none ${FOCUS_RING}`}
        />
      </Slider.Root>
    </div>
  );
}

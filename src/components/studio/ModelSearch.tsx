import { useId, useState } from "react";
import { Search } from "lucide-react";
import { GET_MODELS } from "../../lib/words";
import { ChipGroup } from "../ui/ChipGroup";
import { SwitchField } from "../ui/SwitchField";
import { FOCUS_RING } from "../ui/styles";
import type { SearchFilter } from "./types";

// Get models' search (Task U5, design 10.3): one large field, "Search Hugging Face: SDXL, Flux,
// Wan 2.2, or a repo name", which asks Hugging Face only on Enter or Search, never as typing
// goes, and says first that the words go to huggingface.co; under it the Pictures · Video ·
// LoRAs chips and *Fits this computer*. Emptying the field goes back to the lists shown without
// a query, and asks nothing.

/** The Pictures · Video · LoRAs chips, in order. */
const FILTERS: { value: SearchFilter; label: string }[] = [
  { value: "pictures", label: GET_MODELS.pictures },
  { value: "video", label: GET_MODELS.video },
  { value: "loras", label: GET_MODELS.loras },
];

/** The search field, the filter chips and *Fits this computer*. `onSearch` gets the words to
 *  ask for, or "" when the field was emptied. */
export function ModelSearch(props: {
  filter: SearchFilter;
  onFilter(filter: SearchFilter): void;
  fitsOnly: boolean;
  onFitsOnly(on: boolean): void;
  onSearch(text: string): void;
}) {
  const [text, setText] = useState("");
  const sendsId = useId();
  const ask = () => {
    const words = text.trim();
    if (words !== "") props.onSearch(words);
  };
  return (
    <section data-model-search className="flex flex-col gap-3">
      <form
        role="search"
        onSubmit={(e) => {
          e.preventDefault();
          ask();
        }}
        className="flex h-14 items-center gap-3 rounded-2xl border border-border bg-surface pr-2 pl-4 shadow-soft focus-within:border-border-strong focus-within:ring-2 focus-within:ring-accent/40"
      >
        <Search size={20} aria-hidden className="shrink-0 text-fg-muted" />
        <input
          type="search"
          aria-label={GET_MODELS.searchLabel}
          aria-describedby={sendsId}
          placeholder={GET_MODELS.searchPlaceholder}
          value={text}
          onChange={(e) => {
            const value = e.target.value;
            setText(value);
            if (value.trim() === "") props.onSearch("");
          }}
          className="min-w-0 flex-1 bg-transparent text-base outline-none placeholder:text-fg-muted [&::-webkit-search-cancel-button]:hidden"
        />
        <button
          type="submit"
          className={`btn-primary inline-flex h-10 shrink-0 items-center rounded-xl px-4 text-sm font-medium ${FOCUS_RING}`}
        >
          {GET_MODELS.search}
        </button>
      </form>
      <div className="flex flex-wrap items-center justify-between gap-x-6 gap-y-2">
        <ChipGroup label={GET_MODELS.filters} value={props.filter} options={FILTERS} onChange={props.onFilter} size="md" />
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1">
          <p id={sendsId} className="text-meta">
            {GET_MODELS.searchSends}
          </p>
          <SwitchField label={GET_MODELS.fitsOnly} checked={props.fitsOnly} onChange={props.onFitsOnly} />
        </div>
      </div>
    </section>
  );
}

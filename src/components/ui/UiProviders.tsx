import type { ReactNode } from "react";
import { Tooltip } from "radix-ui";

/** The one provider the shared controls need (`Tip` throws without a `Tooltip.Provider`).
 *  It sits above `<App />` in `main.tsx` and in the preview harness, so it covers every
 *  branch of App, first run included. */
export function UiProviders(props: { children: ReactNode }) {
  return <Tooltip.Provider delayDuration={300}>{props.children}</Tooltip.Provider>;
}

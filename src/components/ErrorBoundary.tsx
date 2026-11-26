import { Component, type ReactNode } from "react";
import { asError, reportCrash } from "../lib/crash";

/** What a fallback is given: the error; `reset`, which draws the children again (Try again);
 *  and `again`, true when the error came from a Try again that failed. */
export type FallbackProps = { error: Error; reset: () => void; again: boolean };

/** Why the children are drawn again: Try again (`reset`), or a change of `resetKeys`. */
export type ResetCause = "user" | "keys";

type Props = {
  /** The part of the app it guards, as the console report names it ("ModelDetail"). */
  name: string;
  /** Drawn in the children's place once they throw while being drawn. */
  fallback: (props: FallbackProps) => ReactNode;
  /** Values that, when any of them changes, draw the children again (a different model
   *  chosen, another chat opened), compared one by one with Object.is. */
  resetKeys?: readonly unknown[];
  /** Called just before the children are drawn again, with why. */
  onReset?: (cause: ResetCause) => void;
  children?: ReactNode;
};

/** `retried`: the children are being drawn again by Try again, and have not drawn since. */
type State = { error: Error | null; retried: boolean };

function changed(a: readonly unknown[] = [], b: readonly unknown[] = []): boolean {
  return a.length !== b.length || a.some((v, i) => !Object.is(v, b[i]));
}

/** Keeps an error thrown while its children are drawn from unmounting the whole app (React
 *  removes every component up to the nearest boundary): it shows `fallback` in their place,
 *  reports the error's name and cleaned message to the console (`src/lib/crash.ts`), and draws
 *  the children again on `reset` or when `resetKeys` change. Errors in event handlers and
 *  promises never reach it; they are handled where they happen. */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null, retried: false };

  static getDerivedStateFromError(thrown: unknown): Partial<State> {
    return { error: asError(thrown) };
  }

  // The component stack React passes as a second argument is left out: it names the app's files.
  componentDidCatch(thrown: unknown): void {
    reportCrash(this.props.name, thrown);
  }

  componentDidUpdate(prev: Props, prevState: State): void {
    // Only for an error already shown: the update that caught it may itself carry new keys.
    if (prevState.error !== null && this.state.error !== null && changed(prev.resetKeys, this.props.resetKeys)) {
      this.restart("keys");
    } else if (this.state.error === null && this.state.retried) {
      // The children drew after Try again: a later error is a first one again.
      this.setState({ retried: false });
    }
  }

  private restart(cause: ResetCause): void {
    this.props.onReset?.(cause);
    this.setState({ error: null, retried: cause === "user" });
  }

  reset = (): void => this.restart("user");

  render(): ReactNode {
    const { error, retried } = this.state;
    if (error !== null) return this.props.fallback({ error, reset: this.reset, again: retried });
    return this.props.children;
  }
}

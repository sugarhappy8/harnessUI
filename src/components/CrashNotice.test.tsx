// What the error boundaries show (0.2.1): the whole window's notice and a panel's.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { AppCrash, AppRoot, PanelBoundary, PanelCrash } from "./CrashNotice";
import { CRASH } from "../lib/words";
import { APP_NAME } from "../lib/app";

let quiet: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  quiet = vi.spyOn(console, "error").mockImplementation(() => {});
});
afterEach(() => {
  cleanup();
  quiet.mockRestore();
});

const ERROR = new TypeError("Cannot read 'x' of '/home/alice/secret.json'");

describe("the whole window's notice", () => {
  it("says what happened and that nothing is lost, in plain words, as an alert", () => {
    render(<AppCrash error={ERROR} reset={() => {}} />);
    const alert = screen.getByRole("alert", { name: CRASH.title });
    expect(alert.textContent).toContain("Your saved chats, models and downloads are safe.");
    expect(screen.getByRole("heading", { name: "Something went wrong drawing this screen" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Try again" })).toBeTruthy();
    expect(screen.getByRole("button", { name: `Reload ${APP_NAME}` })).toBeTruthy();
  });

  it("after a Try again that failed, says it happened again", () => {
    render(<AppCrash error={ERROR} reset={() => {}} again />);
    expect(screen.getByRole("alert", { name: CRASH.title }).textContent).toContain(
      `It happened again. Reload ${APP_NAME} to start this window afresh.`,
    );
  });

  it("puts the keyboard on Try again, and Try again resets", () => {
    const reset = vi.fn();
    render(<AppCrash error={ERROR} reset={reset} />);
    const again = screen.getByRole("button", { name: "Try again" });
    expect(document.activeElement).toBe(again);
    fireEvent.click(again);
    expect(reset).toHaveBeenCalledTimes(1);
  });

  it("keeps the details collapsed: the error's name and message, with no path", () => {
    const { container } = render(<AppCrash error={ERROR} reset={() => {}} />);
    const details = container.querySelector("details")!;
    expect(details.open).toBe(false);
    expect(details.querySelector("summary")!.textContent).toBe("Details");
    expect(container.querySelector("[data-crash-details]")!.textContent).toBe("TypeError: Cannot read 'x' of '…'");
    expect(container.textContent).not.toMatch(/alice|secret/);
  });
});

describe("a panel's notice", () => {
  it("names the panel, says the rest still works, and has Try again and the details", () => {
    const reset = vi.fn();
    render(<PanelCrash where="detail" error={ERROR} reset={reset} />);
    const alert = screen.getByRole("alert", { name: "This model's page couldn't be shown." });
    expect(alert.textContent).toContain("The rest of the app still works.");
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(reset).toHaveBeenCalledTimes(1);
    expect(alert.querySelector("details")!.open).toBe(false);
  });

  it("puts the keyboard on Try again when the panel took the focus with it, and leaves it alone otherwise", () => {
    (document.activeElement as HTMLElement | null)?.blur();
    render(<PanelCrash where="chat" error={ERROR} reset={() => {}} />);
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Try again" }));
    cleanup();

    const elsewhere = document.body.appendChild(document.createElement("input"));
    elsewhere.focus();
    render(<PanelCrash where="chat" error={ERROR} reset={() => {}} />);
    expect(document.activeElement).toBe(elsewhere);
    elsewhere.remove();
  });

  it("after a Try again that failed, says so and offers Reload", () => {
    render(<PanelCrash where="studio" error={ERROR} reset={() => {}} again />);
    const alert = screen.getByRole("alert", { name: "Studio couldn't be shown." });
    expect(alert.textContent).toContain("It went wrong again. The rest of the app still works.");
    expect(screen.getByRole("button", { name: `Reload ${APP_NAME}` })).toBeTruthy();
    cleanup();
    render(<PanelCrash where="studio" error={ERROR} reset={() => {}} />);
    expect(screen.queryByRole("button", { name: `Reload ${APP_NAME}` })).toBeNull();
  });

  it("shows a panel's own way out next to Try again", () => {
    const home = vi.fn();
    render(<PanelCrash where="studio" error={ERROR} reset={() => {}} action={{ label: "Back to Studio", run: home }} />);
    fireEvent.click(screen.getByRole("button", { name: "Back to Studio" }));
    expect(home).toHaveBeenCalledTimes(1);
  });

  it("has words for every panel", () => {
    for (const where of Object.keys(CRASH.where) as (keyof typeof CRASH.where)[]) {
      cleanup();
      render(<PanelCrash where={where} error={ERROR} reset={() => {}} />);
      expect(screen.getByRole("alert", { name: CRASH.panel(CRASH.where[where]) })).toBeTruthy();
    }
  });
});

describe("the boundaries", () => {
  function Broken(): never {
    throw new Error("drawing failed");
  }

  it("PanelBoundary keeps the rest of the window drawn and reports by component name", () => {
    render(
      <AppRoot>
        <nav>sidebar</nav>
        <PanelBoundary where="studio" fill>
          <Broken />
        </PanelBoundary>
      </AppRoot>,
    );
    expect(screen.getByText("sidebar")).toBeTruthy();
    expect(screen.getByRole("alert", { name: "Studio couldn't be shown." })).toBeTruthy();
    expect(quiet.mock.calls.map((c: unknown[]) => String(c[0]))).toContain("[crash] StudioView could not be drawn: Error: drawing failed");
  });

  it("AppRoot shows the whole window's notice for an error nothing else caught", () => {
    render(
      <AppRoot>
        <nav>sidebar</nav>
        <Broken />
      </AppRoot>,
    );
    expect(screen.queryByText("sidebar")).toBeNull();
    expect(screen.getByRole("alert", { name: CRASH.title })).toBeTruthy();
  });
});

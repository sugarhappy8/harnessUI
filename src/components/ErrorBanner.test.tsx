import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { ErrorBanner } from "./ErrorBanner";

const error = { code: "stepCap", message: "The model reached the step limit for one turn.", nextStep: "Choose Continue.", details: "" };

describe("ErrorBanner", () => {
  afterEach(cleanup);

  it("is an alert by default, with no action", () => {
    render(<ErrorBanner error={{ ...error, code: "http_500" }} />);
    expect(screen.getByRole("alert")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Continue" })).toBeNull();
  });

  // A stop at the step limit is not a failure: it is a notice with the way forward.
  it("shows a notice with its action", () => {
    const onClick = vi.fn();
    render(<ErrorBanner error={error} tone="notice" action={{ label: "Continue", onClick }} />);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByRole("status").textContent).toContain("step limit");
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});

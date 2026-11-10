import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { ApprovalCard, CARD_INPUT_DELAY_MS } from "./ApprovalCard";
import type { ApprovalRequest } from "../lib/types";

const base: ApprovalRequest = {
  id: "req-1",
  chatId: 1,
  callId: "call_1",
  server: "files",
  tool: "write_file",
  summary: "Create /home/u/proj/a.txt (5 bytes)",
  preview: "+hello",
  card: "grantable",
  reason: "normal",
};

describe("ApprovalCard safety", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("ignores clicks for a short time after it appears", () => {
    const onDecide = vi.fn();
    render(<ApprovalCard request={base} onDecide={onDecide} />);
    fireEvent.click(screen.getByText("Allow once"));
    expect(onDecide).not.toHaveBeenCalled();
    act(() => {
      vi.advanceTimersByTime(CARD_INPUT_DELAY_MS + 10);
    });
    fireEvent.click(screen.getByText("Allow once"));
    expect(onDecide).toHaveBeenCalledWith("allowOnce", null);
  });

  it("never gives Allow default focus and Enter does not approve", () => {
    const onDecide = vi.fn();
    render(<ApprovalCard request={base} onDecide={onDecide} />);
    act(() => {
      vi.advanceTimersByTime(CARD_INPUT_DELAY_MS + 10);
    });
    const allow = screen.getByText("Allow once");
    expect(document.activeElement).not.toBe(allow);
    expect(document.activeElement).not.toBe(screen.getByText("Allow for this chat"));
    fireEvent.keyDown(allow, { key: "Enter" });
    expect(onDecide).not.toHaveBeenCalled();
  });

  it("always-ask cards offer only Allow once and Deny", () => {
    render(<ApprovalCard request={{ ...base, card: "onceOnly", reason: "alwaysAsk" }} onDecide={() => {}} />);
    expect(screen.queryByText("Allow for this chat")).toBeNull();
    expect(screen.getByText(/always asks/)).toBeTruthy();
  });

  it("shows model text as text, including escaped controls, never as HTML", () => {
    const evil: ApprovalRequest = {
      ...base,
      summary: '<img src=x onerror="alert(1)">',
      preview: "evil\\u{202E}txt.exe <script>alert(2)</script>",
      reason: "recoveredFromText",
    };
    const { container } = render(<ApprovalCard request={evil} onDecide={() => {}} />);
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("script")).toBeNull();
    expect(container.textContent).toContain("\\u{202E}");
    expect(container.textContent).toContain("recovered from text");
  });

  it("passes a deny note back", () => {
    const onDecide = vi.fn();
    render(<ApprovalCard request={base} onDecide={onDecide} />);
    act(() => {
      vi.advanceTimersByTime(CARD_INPUT_DELAY_MS + 10);
    });
    fireEvent.click(screen.getByText("Add a note"));
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "use the other folder" } });
    fireEvent.click(screen.getByText("Deny"));
    expect(onDecide).toHaveBeenCalledWith("deny", "use the other folder");
  });
});

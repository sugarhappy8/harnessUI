import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

const apiMock = vi.hoisted(() => ({ pinMessage: vi.fn() }));
vi.mock("./api", () => apiMock);

const { PinMessageButton } = await import("./PinMessageButton");
const { useApp } = await import("../../store/app");

describe("PinMessageButton", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useApp.setState({ activeChatId: 7, error: null, workbench: { open: false, tab: "artifacts" } });
  });
  afterEach(() => cleanup());

  it("sends the chat and the message id only, then opens the Context tab", async () => {
    apiMock.pinMessage.mockResolvedValue({ pins: [], max: 20, maxChars: 1000 });
    render(<PinMessageButton messageId={42} />);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Pin for this chat" }));
    });
    expect(apiMock.pinMessage).toHaveBeenCalledWith(7, 42);
    expect(apiMock.pinMessage.mock.calls[0]).toHaveLength(2);
    expect(useApp.getState().workbench).toEqual({ open: true, tab: "context" });
    expect(screen.getByRole("button", { name: "Pinned for this chat" })).toBeTruthy();
  });

  it("shows the app's refusal", async () => {
    apiMock.pinMessage.mockRejectedValue({
      code: "pinned_refused",
      message: "This message has 1,001 characters, and a pin can have at most 1,000.",
      nextStep: "",
      details: "",
    });
    render(<PinMessageButton messageId={42} />);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Pin for this chat" }));
    });
    expect(useApp.getState().error?.message).toMatch(/1,001 characters/);
    expect(useApp.getState().workbench.open).toBe(false);
  });
});

// The one-time content notice (0.1.0 Task B14, local addendum L1): shown before the first
// local download, word for word, and accepted with *I understand* (recorded by the backend);
// read again, read only, from Get models' footer and from Settings → Studio.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { LOCAL } from "./fixtures/models";

// Get models follows the app's downloads: none here.
vi.mock("../../lib/ipc", () => ({
  isTauri: () => true,
  listDownloads: () => Promise.resolve([]),
  onDownloadProgress: () => Promise.resolve(() => undefined),
}));
vi.mock("./api");

const api = await import("./api");
const { LocalNotice } = await import("./LocalNotice");
const { PREFS, click, flush, showLocal } = await import("./testing/local");
const { useStudio } = await import("../../store/studio");

const INITIAL = useStudio.getState();

/** The notice, addendum L1, word for word after its bold first sentence. */
const BODY =
  "No company checks your prompts or pictures. Don't create sexual images of children, or sexual or intimate images of real people without their consent, and don't impersonate people. These are crimes in many countries. Each model's licence and use policy also applies.";

const onAccepted = vi.fn<() => void>();
const onClose = vi.fn<() => void>();

function renderNotice(readOnly?: boolean) {
  useStudio.setState({ local: { ...LOCAL, noticeAccepted: false } });
  render(<LocalNotice open readOnly={readOnly} onAccepted={onAccepted} onClose={onClose} />);
  return screen.getByRole("alertdialog");
}

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
  vi.mocked(api.studioAcceptLocalNotice).mockResolvedValue({ ...PREFS, localNoticeAcceptedAt: 1_700_000_000_000 });
});

afterEach(cleanup);

describe("the content notice", () => {
  it("says what local models mean, word for word", () => {
    renderNotice();
    screen.getByText("Local models run only on your computer.");
    screen.getByText(BODY);
  });

  it("I understand records it with the backend, then says so", async () => {
    renderNotice();
    fireEvent.click(screen.getByRole("button", { name: "I understand" }));
    await waitFor(() => expect(onAccepted).toHaveBeenCalledTimes(1));
    expect(api.studioAcceptLocalNotice).toHaveBeenCalledTimes(1);
    expect(useStudio.getState().local?.noticeAccepted).toBe(true);
    expect(useStudio.getState().prefs?.localNoticeAcceptedAt).toBe(1_700_000_000_000);
    expect(onClose).not.toHaveBeenCalled();
  });

  it("a refusal to record it says why, and the notice stays", async () => {
    vi.mocked(api.studioAcceptLocalNotice).mockRejectedValue({
      code: "studio_prefs",
      message: "Studio's settings could not be saved.",
      nextStep: "Try again.",
      details: "",
    });
    renderNotice();
    await click(screen.getByRole("button", { name: "I understand" }));
    screen.getByText("Studio's settings could not be saved.");
    screen.getByText("Local models run only on your computer.");
    expect(onAccepted).not.toHaveBeenCalled();
    expect(useStudio.getState().local?.noticeAccepted).toBe(false);
  });

  it("while I understand is on its way, Cancel and Esc wait for it", async () => {
    let fail: (e: unknown) => void = () => undefined;
    vi.mocked(api.studioAcceptLocalNotice).mockReturnValue(
      new Promise((_, reject) => {
        fail = reject;
      }),
    );
    const dialog = renderNotice();
    await click(screen.getByRole("button", { name: "I understand" }));
    const cancel = within(dialog).getByRole<HTMLButtonElement>("button", { name: "Cancel" });
    expect(cancel.disabled).toBe(true);
    await click(cancel);
    fireEvent.keyDown(dialog, { key: "Escape" });
    await flush();
    expect(onClose).not.toHaveBeenCalled();
    // The refusal still shows where it can be read.
    await act(async () => {
      fail({ code: "studio_prefs", message: "Studio's settings could not be saved.", nextStep: "Try again.", details: "" });
    });
    await flush();
    within(dialog).getByText("Studio's settings could not be saved.");
    expect(onAccepted).not.toHaveBeenCalled();
    expect(cancel.disabled).toBe(false);
    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("Cancel before a first download accepts nothing", async () => {
    renderNotice();
    await click(screen.getByRole("button", { name: "Cancel" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(api.studioAcceptLocalNotice).not.toHaveBeenCalled();
  });

  it("read only, it has Close and nothing to accept", async () => {
    renderNotice(true);
    screen.getByText("Local models run only on your computer.");
    screen.getByText(BODY);
    expect(screen.queryByRole("button", { name: "I understand" })).toBeNull();
    await click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(api.studioAcceptLocalNotice).not.toHaveBeenCalled();
  });

  it("the footer shows the notice again, read only", async () => {
    await showLocal();
    await click(screen.getByRole("button", { name: "What that means" }));
    screen.getByText("Local models run only on your computer.");
    expect(screen.queryByRole("button", { name: "I understand" })).toBeNull();
    await click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByText("Local models run only on your computer.")).toBeNull();
    expect(useStudio.getState().noticeOpen).toBeNull();
    expect(api.studioAcceptLocalNotice).not.toHaveBeenCalled();
  });

  it("Cancel on the notice before a first download downloads nothing", async () => {
    await showLocal({ ...LOCAL, noticeAccepted: false });
    // Since 0.2.0 (Task U6) the card's Download opens the detail panel, whose Download shows
    // the notice first.
    await click(within(screen.getByRole("article", { name: "Z-Image Turbo" })).getByRole("button", { name: "Download" }));
    await click(within(screen.getByRole("dialog", { name: "Z-Image Turbo" })).getByRole("button", { name: "Download" }));
    screen.getByText("Local models run only on your computer.");
    await click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByText("Local models run only on your computer.")).toBeNull();
    expect(api.studioLocalDownload).not.toHaveBeenCalled();
    expect(api.studioAcceptLocalNotice).not.toHaveBeenCalled();
  });
});

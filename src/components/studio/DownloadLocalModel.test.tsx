// The download confirmation (0.1.0 Task B14, local addendum Wireframe L3): every part with its
// size and licence, what the pictures you make may be used for (the strictest licence), the
// Studio engine when it comes with the model, a gated repository's form, an amber licence's
// condition, low disk space, a link to each licence, and Accept and download, which sends the
// hash of every licence it showed. Before the first local download, the content notice comes
// first.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { amountGb } from "../../lib/words";
import { LOCAL } from "./fixtures/models";
import type { Licence } from "./types";

// Get local models follows the app's downloads: none here.
vi.mock("../../lib/ipc", () => ({
  isTauri: () => true,
  listDownloads: () => Promise.resolve([]),
  onDownloadProgress: () => Promise.resolve(() => undefined),
}));
vi.mock("./api");
const openUrl = vi.hoisted(() => vi.fn<(url: string) => Promise<void>>());
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl }));

const api = await import("./api");
const { DownloadLocalModel, verdict } = await import("./DownloadLocalModel");
const { PREFS, click, flush, showLocal } = await import("./testing/local");
const { offerForEntry } = await import("./localOffers");
const { useStudio } = await import("../../store/studio");

type Offer = NonNullable<ReturnType<typeof useStudio.getState>["offer"]>;

const INITIAL = useStudio.getState();

const zimage = LOCAL.recommended.find((e) => e.id === "z-image-turbo-q4")!;
const apache = zimage.licence;

/** Commercial use under a $5M yearly revenue, with a duty for what you share. */
const kommunity: Licence = {
  ...apache,
  id: "kommunity",
  name: "Example Community Licence",
  url: "https://example.com/kommunity",
  sha256: "cd".repeat(32),
  badge: "amber",
  commercial: "underRevenue",
  revenueLimitUsd: 5_000_000,
  plainWords: "Commercial use only under $5M revenue.",
  obligations: ["Review pictures before you share them."],
};

const nonCommercial: Licence = {
  ...apache,
  id: "nc",
  name: "Non-Commercial Licence",
  sha256: "b".repeat(64),
  badge: "red",
  commercial: "no",
  revenueLimitUsd: null,
  plainWords: "Only for non-commercial use.",
};

function offer(patch: Partial<Offer> = {}): Offer {
  return { ...offerForEntry(zimage, LOCAL), ...patch };
}

const onClose = vi.fn<() => void>();

/** The dialog alone, for `o`, with the fixture's local models in the store. */
function renderDialog(o: Offer) {
  useStudio.setState({ local: LOCAL, offer: o });
  render(<DownloadLocalModel offer={o} onClose={onClose} />);
  return screen.getByRole("alertdialog");
}

/** *Download* on the row of model `name`. */
async function clickDownload(name: string) {
  await click(within(screen.getByRole("article", { name })).getByRole("button", { name: "Download" }));
}

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
  vi.mocked(api.studioLocalDownload).mockResolvedValue("group-1");
  vi.mocked(api.studioLocalModels).mockResolvedValue(LOCAL);
  vi.mocked(api.studioAcceptLocalNotice).mockResolvedValue({ ...PREFS, localNoticeAcceptedAt: 1_700_000_000_000 });
  openUrl.mockResolvedValue(undefined);
});

afterEach(cleanup);

describe("verdict", () => {
  it("says what the pictures you make may be used for, by the strictest licence", () => {
    expect(verdict([apache], "image")).toBe("Pictures you make: free for any use, including commercial.");
    expect(verdict([apache, kommunity], "image")).toBe("Pictures you make: commercial use only under $5M revenue.");
    expect(verdict([kommunity, { ...kommunity, id: "tiny", revenueLimitUsd: 1_000_000 }], "image")).toBe(
      "Pictures you make: commercial use only under $1M revenue.",
    );
    expect(verdict([{ ...kommunity, commercial: "registration", revenueLimitUsd: null }, kommunity], "image")).toBe(
      "Pictures you make: commercial use only after registering with the publisher.",
    );
    expect(verdict([apache, { ...nonCommercial, commercial: "unclear" }], "image")).toBe(
      "Pictures you make: check the licence before any commercial use.",
    );
    expect(verdict([kommunity, nonCommercial, { ...nonCommercial, commercial: "unclear" }], "image")).toBe(
      "Pictures you make: non-commercial use only.",
    );
    expect(verdict([], "image")).toBe("");
  });

  it("a video model's says what the videos you make may be used for", () => {
    expect(verdict([apache], "video")).toBe("Videos you make: free for any use, including commercial.");
    expect(verdict([apache, kommunity], "video")).toBe("Videos you make: commercial use only under $5M revenue.");
    expect(verdict([{ ...kommunity, revenueLimitUsd: null }], "video")).toBe(
      "Videos you make: commercial use only under a revenue limit.",
    );
    expect(verdict([{ ...kommunity, commercial: "registration" }], "video")).toBe(
      "Videos you make: commercial use only after registering with the publisher.",
    );
    expect(verdict([{ ...nonCommercial, commercial: "unclear" }], "video")).toBe(
      "Videos you make: check the licence before any commercial use.",
    );
    expect(verdict([nonCommercial], "video")).toBe("Videos you make: non-commercial use only.");
  });
});

describe("the download confirmation", () => {
  it("the first local download shows the content notice first", async () => {
    await showLocal({ ...LOCAL, noticeAccepted: false });
    await clickDownload("Z-Image Turbo");
    screen.getByText("Local models run only on your computer.");
    screen.getByText(
      /Don't create sexual images of children, or sexual or intimate images of real people without their consent/,
    );
    expect(screen.queryByText("Download Z-Image Turbo?")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "I understand" }));
    await waitFor(() => expect(api.studioAcceptLocalNotice).toHaveBeenCalledTimes(1));
    // then the dialog
    await screen.findByText("Download Z-Image Turbo?");
    expect(screen.queryByText("Local models run only on your computer.")).toBeNull();
    expect(useStudio.getState().local?.noticeAccepted).toBe(true);
    expect(useStudio.getState().prefs?.localNoticeAcceptedAt).toBe(1_700_000_000_000);
    expect(api.studioLocalDownload).not.toHaveBeenCalled();
  });

  it("with the notice accepted before, Download opens the dialog at once", async () => {
    await showLocal();
    await clickDownload("Z-Image Turbo");
    screen.getByText("Download Z-Image Turbo?");
    expect(screen.queryByText("Local models run only on your computer.")).toBeNull();
    expect(api.studioAcceptLocalNotice).not.toHaveBeenCalled();
  });

  it("the dialog lists every part with its size and licence, then the verdict", () => {
    const dialog = renderDialog(offer());
    within(dialog).getByText("Download Z-Image Turbo?");
    within(dialog).getByText(`3 files, ${amountGb(zimage.sizeBytes)} in all, from Hugging Face (huggingface.co)`);
    within(dialog).getByText("makes the picture");
    within(dialog).getByText("reads your prompt");
    within(dialog).getByText("picture decoder");
    for (const part of zimage.parts) within(dialog).getByText(amountGb(part.sizeBytes));
    expect(within(dialog).getAllByText("Apache License 2.0")).toHaveLength(3);
    within(dialog).getByText("Pictures you make: free for any use, including commercial.");
    within(dialog).getByText("Downloading means you accept these licences.");
    const link = within(dialog).getByRole("link", { name: "Read the licences" });
    expect(link.getAttribute("href")).toBe(apache.url);
    // Nothing else to say about this one.
    expect(within(dialog).queryByText(/Studio engine/)).toBeNull();
    expect(within(dialog).queryByText(/less than 10 GB/)).toBeNull();
    expect(within(dialog).queryByText(/Hugging Face username/)).toBeNull();
    expect(within(dialog).queryByText("already on this computer")).toBeNull();
  });

  it("a video model's dialog says what the videos you make may be used for", () => {
    const wan = LOCAL.recommended.find((e) => e.kind === "video")!;
    const dialog = renderDialog(offerForEntry(wan, LOCAL));
    within(dialog).getByText("Videos you make: free for any use, including commercial.");
    expect(within(dialog).queryByText(/^Pictures you make/)).toBeNull();
  });

  it("Read the licences opens the licence in the browser", async () => {
    const dialog = renderDialog(offer());
    await click(within(dialog).getByRole("link", { name: "Read the licences" }));
    expect(openUrl).toHaveBeenCalledWith(apache.url);
  });

  it("Accept and download sends the licence hashes it showed", async () => {
    const dialog = renderDialog(offer());
    fireEvent.click(within(dialog).getByRole("button", { name: "Accept and download" }));
    expect(api.studioLocalDownload).toHaveBeenCalledWith(
      { catalog: { entryId: "z-image-turbo-q4" } },
      zimage.licences.map((l) => l.sha256),
    );
    await flush();
    // The offer is put away and the local models are read again.
    expect(useStudio.getState().offer).toBeNull();
    expect(api.studioLocalModels).toHaveBeenCalledTimes(1);
  });

  it("from Get local models, Accept and download closes the dialog", async () => {
    await showLocal();
    await clickDownload("Z-Image Turbo");
    const reads = vi.mocked(api.studioLocalModels).mock.calls.length;
    await click(screen.getByRole("button", { name: "Accept and download" }));
    expect(api.studioLocalDownload).toHaveBeenCalledWith(
      { catalog: { entryId: "z-image-turbo-q4" } },
      zimage.licences.map((l) => l.sha256),
    );
    expect(screen.queryByText("Download Z-Image Turbo?")).toBeNull();
    expect(useStudio.getState().offer).toBeNull();
    expect(vi.mocked(api.studioLocalModels).mock.calls.length).toBe(reads + 1);
  });

  it("a model with two licences shows and sends both", async () => {
    const parts = zimage.parts.map((p, i) => (i === 1 ? { ...p, licenceId: "kommunity" } : p));
    const dialog = renderDialog(offer({ parts, licences: [apache, kommunity] }));
    expect(within(dialog).getAllByText("Apache License 2.0")).toHaveLength(2);
    within(dialog).getByText("Example Community Licence");
    const links = within(dialog).getAllByRole("link", { name: /^Read the licences/ });
    expect(links.map((a) => a.getAttribute("href"))).toEqual([apache.url, kommunity.url]);
    // Each link says which licence it opens.
    expect(links.map((a) => a.textContent)).toEqual([
      "Read the licences (Apache License 2.0)",
      "Read the licences (Example Community Licence)",
    ]);
    within(dialog).getByText("Pictures you make: commercial use only under $5M revenue.");
    await click(within(dialog).getByRole("button", { name: "Accept and download" }));
    expect(api.studioLocalDownload).toHaveBeenCalledWith({ catalog: { entryId: "z-image-turbo-q4" } }, [
      apache.sha256,
      kommunity.sha256,
    ]);
  });

  it("Cancel downloads nothing", async () => {
    const dialog = renderDialog(offer());
    await click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(api.studioLocalDownload).not.toHaveBeenCalled();
  });

  it("Cancel from Get local models puts the offer away", async () => {
    await showLocal();
    await clickDownload("Z-Image Turbo");
    await click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByText("Download Z-Image Turbo?")).toBeNull();
    expect(useStudio.getState().offer).toBeNull();
    expect(api.studioLocalDownload).not.toHaveBeenCalled();
  });

  it("a download that could not start says why and keeps the dialog", async () => {
    vi.mocked(api.studioLocalDownload).mockRejectedValue({
      code: "studio_license_not_accepted",
      message: "A licence changed since this list was read.",
      nextStep: "Close this and press Download again.",
      details: "",
    });
    const o = offer();
    const dialog = renderDialog(o);
    await click(within(dialog).getByRole("button", { name: "Accept and download" }));
    within(dialog).getByText("A licence changed since this list was read.");
    within(dialog).getByText("Close this and press Download again.");
    expect(useStudio.getState().offer).toBe(o);
    expect(api.studioLocalModels).not.toHaveBeenCalled();
  });

  it("while Accept is on its way, Cancel and Esc wait for it", async () => {
    let fail: (e: unknown) => void = () => undefined;
    vi.mocked(api.studioLocalDownload).mockReturnValue(
      new Promise<string>((_, reject) => {
        fail = reject;
      }),
    );
    const o = offer();
    const dialog = renderDialog(o);
    await click(within(dialog).getByRole("button", { name: "Accept and download" }));
    const cancel = within(dialog).getByRole<HTMLButtonElement>("button", { name: "Cancel" });
    expect(cancel.disabled).toBe(true);
    await click(cancel);
    fireEvent.keyDown(dialog, { key: "Escape" });
    await flush();
    expect(onClose).not.toHaveBeenCalled();
    // The refusal still shows where it can be read.
    await act(async () => {
      fail({ code: "download_busy", message: "The download could not start.", nextStep: "Try again.", details: "" });
    });
    await flush();
    within(dialog).getByText("The download could not start.");
    expect(useStudio.getState().offer).toBe(o);
    expect(cancel.disabled).toBe(false);
    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("the engine comes with the first model on Windows, and says so", () => {
    const dialog = renderDialog(offer({ needsEngine: true, engineBytes: 30_303_000 }));
    const line = within(dialog).getByText("Also downloads the Studio engine (29 MB, MIT licence).");
    expect(line.getAttribute("title")).toBe(LOCAL.engine.licenseName);
  });

  it("the engine's licence is linked and accepted with the model's, and leaves the verdict alone", async () => {
    const engine: Licence = {
      ...apache,
      id: "studio-engine",
      name: LOCAL.engine.licenseName,
      url: LOCAL.engine.licenseUrl,
      sha256: "ef".repeat(32),
      plainWords: "The Studio engine: free software.",
    };
    const dialog = renderDialog(
      offer({ licences: [kommunity, engine], needsEngine: true, engineBytes: 30_303_000, parts: [] }),
    );
    within(dialog).getByText("Pictures you make: commercial use only under $5M revenue.");
    const links = within(dialog).getAllByRole("link", { name: /^Read the licences/ });
    expect(links.map((a) => a.getAttribute("href"))).toEqual([kommunity.url, LOCAL.engine.licenseUrl]);
    // The engine's licence by its short word; each link's whole name is its tooltip.
    expect(links.map((a) => a.textContent)).toEqual([
      "Read the licences (Example Community Licence)",
      "Read the licences (MIT)",
    ]);
    expect(links.map((a) => a.getAttribute("title"))).toEqual([kommunity.name, LOCAL.engine.licenseName]);
    await click(within(dialog).getByRole("button", { name: "Accept and download" }));
    expect(api.studioLocalDownload).toHaveBeenCalledWith(expect.anything(), [kommunity.sha256, engine.sha256]);
  });

  it("a part already here, a gated part, an amber licence and low disk space each say so", async () => {
    const parts = zimage.parts.map((p, i) =>
      i === 1 ? { ...p, alreadyHere: true } : i === 0 ? { ...p, licenceId: "kommunity" } : p,
    );
    const toFetch = parts[0]!.sizeBytes + parts[2]!.sizeBytes;
    const dialog = renderDialog(
      offer({
        parts,
        licences: [apache, kommunity],
        toFetchBytes: toFetch,
        lowDisk: true,
        gate: { mode: "auto", collects: ["username", "email"] },
      }),
    );
    within(dialog).getByText(
      `3 files, ${amountGb(zimage.sizeBytes)} in all, ${amountGb(toFetch)} to download, from Hugging Face (huggingface.co)`,
    );
    within(dialog).getByText("already on this computer");
    within(dialog).getByText(
      "This model is gated: accept the publisher's terms on its Hugging Face page first, and access is given at once.",
    );
    within(dialog).getByText("The publisher receives your Hugging Face username and email.");
    const page = within(dialog).getByRole("link", { name: "Open the model page" });
    expect(page.getAttribute("href")).toBe("https://huggingface.co/leejet/Z-Image-Turbo-GGUF");
    await click(page);
    expect(openUrl).toHaveBeenCalledWith("https://huggingface.co/leejet/Z-Image-Turbo-GGUF");
    within(dialog).getByText(
      "Commercial use only if your organisation earns less than $5M a year. The licence asks you to review pictures before you share them.",
    );
    within(dialog).getByText("This leaves less than 10 GB free on this disk.");
  });

  it("a repository whose publisher approves each request says so", () => {
    const dialog = renderDialog(offer({ gate: { mode: "manual", collects: ["username", "email", "company"] } }));
    within(dialog).getByText(
      "This model is gated: ask for access on its Hugging Face page first, and the publisher approves each request.",
    );
    within(dialog).getByText("The publisher receives your Hugging Face username, email and company.");
    within(dialog).getByRole("link", { name: "Open the model page" });
  });

  it("an amber licence that asks for registration says so", () => {
    const signUp: Licence = { ...kommunity, id: "sign-up", commercial: "registration", revenueLimitUsd: null, obligations: [] };
    const dialog = renderDialog(offer({ licences: [apache, signUp] }));
    within(dialog).getByText("Commercial use only after registering with the publisher.");
  });
});

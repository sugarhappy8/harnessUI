// What Studio's runs cost, as Studio shows it (0.1.0 Tasks B2 and B7): dollars, their colour,
// the label on Create (a price for a cloud run, the time or "free" for a local one), where the
// prompt goes, cloud as the backup for a local run, and the daily limit's line.
import { describe, expect, it, vi } from "vitest";
import { MODELS } from "./fixtures/models";
import type { Estimate, Limit, LocalEstimate, StudioEndpoint } from "./types";

vi.mock("./api");

const { cloudBackup, costTone, createLabel, formatUsd, formatUsdShort, limitLine, placeLine } = await import("./cost");

function est(patch: Partial<Estimate> = {}): Estimate {
  return {
    usd: 0.16,
    reasons: [],
    confirmAtUsd: 1,
    limit: { limitUsd: 10, spentTodayUsd: 0, pendingUsd: 0, wouldPass: true, refusal: null, reserveUsd: 5 },
    invalid: [],
    eta: null,
    local: null,
    ...patch,
  };
}

function localEst(patch: Partial<LocalEstimate> = {}): LocalEstimate {
  return { fit: "fits", unloadChat: false, verySlow: false, over15Min: false, reason: null, ...patch };
}

/** About 4 minutes on this computer: 3 to 5. */
const FOUR_MIN = { lowMs: 180_000, highMs: 300_000, measured: true, leftMs: null };

/** OpenRouter with the fixture's models (pictures and video), or OpenAI (pictures only). */
function endpoint(patch: Partial<StudioEndpoint> = {}): StudioEndpoint {
  const openai = patch.company === "openai";
  return {
    providerId: 1,
    name: openai ? "OpenAI" : "OpenRouter",
    company: "openrouter",
    host: openai ? "api.openai.com" : "openrouter.ai",
    cloud: true,
    hasKey: true,
    fetchedAt: new Date(2026, 10, 17, 9).getTime(),
    models: openai ? [{ ...MODELS[0]!, id: "gpt-image-2.6-flare" }] : MODELS,
    ...patch,
  };
}

describe("formatUsd", () => {
  it("shows cents, and less than a cent as such", () => {
    expect(formatUsd(0.16)).toBe("$0.16");
    expect(formatUsd(0.84)).toBe("$0.84");
    expect(formatUsd(12.5)).toBe("$12.50");
    expect(formatUsd(0.004)).toBe("< $0.01");
    expect(formatUsd(0)).toBe("$0.00");
  });

  it("formats and colours prices", () => {
    expect(formatUsd(0.16)).toBe("$0.16");
    expect(formatUsd(0.004)).toBe("< $0.01");
    expect(costTone(0.99)).toBe("plain");
    expect(costTone(1)).toBe("amber");
    expect(costTone(2.5)).toBe("amber");
    expect(costTone(2.51)).toBe("red");
    expect(costTone(null)).toBe("unknown");
    expect(createLabel(est({ usd: null }))).toBe("price unknown");
  });
});

describe("createLabel", () => {
  it("a cloud run shows its price for the whole run", () => {
    expect(createLabel(est())).toBe("~ $0.16");
    expect(createLabel(est({ usd: 2.4 }))).toBe("~ $2.40");
  });

  it("a local run shows its time, or free when nothing was measured", () => {
    expect(createLabel(est({ usd: 0, eta: FOUR_MIN, local: localEst() }))).toBe("about 4 min");
    expect(createLabel(est({ usd: 0, eta: { ...FOUR_MIN, measured: false }, local: localEst() }))).toBe(
      "about 4 min (first guess)",
    );
    expect(createLabel(est({ usd: 0, eta: null, local: localEst({}) }))).toBe("free");
  });

  it("says nothing before there is an estimate", () => {
    expect(createLabel(null)).toBe("");
  });
});

describe("placeLine", () => {
  it("a local run's prompt stays here, and says when the chat model will be unloaded", () => {
    expect(placeLine("thisComputer", "", est({ usd: 0, local: localEst() }))).toBe(
      "On this computer: your prompt and pictures stay here",
    );
    expect(placeLine("thisComputer", "", est({ usd: 0, local: localEst({ unloadChat: true }) }))).toBe(
      "On this computer: your prompt and pictures stay here · the chat model will be unloaded to make room",
    );
    expect(placeLine("thisComputer", "", null)).toBe("On this computer: your prompt and pictures stay here");
  });

  it("a cloud run's prompt goes to its endpoint's host", () => {
    expect(placeLine({ endpoint: 1 }, "openrouter.ai", est())).toBe(
      "Cloud: your prompt and added pictures go to openrouter.ai",
    );
  });
});

describe("cloudBackup", () => {
  const openrouter = endpoint();
  const openai = endpoint({ providerId: 2, company: "openai" });

  it("offers a cloud model only for a local run that does not fit here or is very slow", () => {
    expect(cloudBackup("image", est({ usd: 0, local: localEst() }), [openrouter])).toBeNull();
    expect(cloudBackup("image", est({ usd: 0, local: localEst({ fit: "slow" }) }), [openrouter])).toBeNull();
    expect(cloudBackup("image", est({ usd: 0, local: localEst({ verySlow: true }) }), [openrouter])).toBe("useCloud");
    expect(cloudBackup("image", est({ usd: 0, local: localEst({ fit: "notHere" }) }), [openrouter])).toBe("useCloud");
    // A cloud run, or none estimated yet, needs no backup.
    expect(cloudBackup("image", est(), [openrouter])).toBeNull();
    expect(cloudBackup("image", null, [openrouter])).toBeNull();
  });

  it("points to Settings → Endpoints when no saved endpoint has a model of the tab's kind", () => {
    const slow = est({ usd: 0, local: localEst({ verySlow: true }) });
    expect(cloudBackup("image", slow, [])).toBe("addEndpoint");
    expect(cloudBackup("image", slow, [openai])).toBe("useCloud");
    // OpenAI makes no video, whatever its saved lists hold.
    expect(cloudBackup("video", slow, [endpoint({ company: "openai", models: MODELS })])).toBe("addEndpoint");
    expect(cloudBackup("video", slow, [openrouter])).toBe("useCloud");
    expect(cloudBackup("video", slow, [endpoint({ models: MODELS.filter((m) => m.kind === "image") })])).toBe(
      "addEndpoint",
    );
  });

  it("an endpoint already saved for this kind is the backup even without its key or its lists", () => {
    // The picker then says what is missing (no key; Get the list): Settings → Endpoints would ask
    // to add what is already there.
    const slow = est({ usd: 0, local: localEst({ verySlow: true }) });
    const notHere = est({ usd: 0, local: localEst({ fit: "notHere" }) });
    expect(cloudBackup("image", slow, [endpoint({ hasKey: false })])).toBe("useCloud");
    expect(cloudBackup("video", notHere, [endpoint({ hasKey: false })])).toBe("useCloud");
    expect(cloudBackup("image", slow, [endpoint({ fetchedAt: null, models: [] })])).toBe("useCloud");
    expect(cloudBackup("video", slow, [endpoint({ hasKey: false, fetchedAt: null, models: [] })])).toBe("useCloud");
    // Still never OpenAI for video.
    expect(cloudBackup("video", slow, [endpoint({ company: "openai", hasKey: false, fetchedAt: null })])).toBe(
      "addEndpoint",
    );
  });
});

describe("the daily limit", () => {
  const limit = (patch: Partial<Limit>): Limit => ({
    limitUsd: 10,
    spentTodayUsd: 9,
    pendingUsd: 0.5,
    wouldPass: false,
    refusal: "over",
    reserveUsd: 5,
    ...patch,
  });

  it("says what was spent today, the runs still going included, of the limit", () => {
    expect(limitLine(limit({}))).toBe("Daily limit reached: $9.50 of $10 spent today");
    expect(limitLine(limit({ limitUsd: 12.5 }))).toBe("Daily limit reached: $9.50 of $12.50 spent today");
    expect(limitLine(limit({ limitUsd: 1000 }))).toBe("Daily limit reached: $9.50 of $1,000 spent today");
    expect(limitLine(limit({ limitUsd: 1, spentTodayUsd: 0, pendingUsd: 0 }))).toBe(
      "Daily limit reached: $0.00 of $1 spent today",
    );
    expect(limitLine(limit({ limitUsd: 1000, spentTodayUsd: 1234.5, pendingUsd: 0 }))).toBe(
      "Daily limit reached: $1,234.50 of $1,000 spent today",
    );
  });

  it("says why a run whose price is unknown is refused for the reserve", () => {
    const reserve = limit({ refusal: "unknownNeedsReserve", limitUsd: 3, spentTodayUsd: 0, pendingUsd: 0 });
    expect(limitLine(reserve)).toBe(
      "This model's price is unknown, so Studio needs $5 of today's limit left ($3.00 left).",
    );
    expect(limitLine(limit({ refusal: "unknownNeedsReserve" }))).toBe(
      "This model's price is unknown, so Studio needs $5 of today's limit left ($0.50 left).",
    );
    // Spent past the limit: nothing is left.
    expect(limitLine(limit({ refusal: "unknownNeedsReserve", spentTodayUsd: 11 }))).toBe(
      "This model's price is unknown, so Studio needs $5 of today's limit left ($0.00 left).",
    );
  });

  it("writes a whole amount without cents", () => {
    expect(formatUsdShort(10)).toBe("$10");
    expect(formatUsdShort(12.5)).toBe("$12.50");
    expect(formatUsdShort(1000)).toBe("$1,000");
    expect(formatUsdShort(0.004)).toBe("$0");
  });
});

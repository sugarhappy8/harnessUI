// Takes the Studio screenshots with Playwright's Chromium, in the light and dark themes.
//
//   node zz-preview/shoot.cjs [--out DIR] [--base URL] [--only a,b2,...] [--themes light,dark] [--scale 2]
//
// The preview dev server must be running (see README.md). Each screen starts from a fresh page,
// so one screen's clicks never leak into the next. Every screen is driven through the UI the
// way a person would (clicks, typing), except where a comment says otherwise.
const path = require("node:path");
const fs = require("node:fs");

let playwright;
try {
  playwright = require("playwright");
} catch {
  playwright = require(process.env.PLAYWRIGHT_PATH || "/opt/node22/lib/node_modules/playwright");
}

const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : fallback;
};
const OUT = path.resolve(opt("out", path.join(__dirname, "shots")));
const BASE = opt("base", `http://127.0.0.1:${process.env.PREVIEW_PORT || 5199}/zz-preview/index.html`);
const ONLY = opt("only", "") ? opt("only", "").split(",") : null;
const THEMES = opt("themes", "light,dark").split(",");
const SCALE = Number(opt("scale", "2"));
const PROMPT = "A paper boat on a rain puddle, macro photo, shallow depth of field";

fs.mkdirSync(OUT, { recursive: true });

const settle = (page, ms = 700) => page.waitForTimeout(ms);

async function open(page, query = "view=studio") {
  await page.goto(`${BASE}?${query}`, { waitUntil: "networkidle" });
  await page.waitForSelector("[data-studio-view], nav[aria-label='Chat history']");
  // Tiles read their pictures once near the screen; the pictures are drawn on a canvas.
  await settle(page, 1200);
}

/** Studio's middle column scrolled to its top (the card stream opens at its bottom). */
const mainToTop = (page) =>
  page.locator("[data-studio-view] main").evaluate((el) => {
    el.scrollTop = 0;
  });

const typePrompt = (page) => page.getByRole("textbox", { name: "Prompt" }).fill(PROMPT);

async function openLocalModels(page) {
  await page.getByRole("button", { name: "Studio menu" }).click();
  await settle(page, 300);
  await page.getByRole("menuitem", { name: "Get local models" }).click();
  await settle(page, 700);
}

async function search(page, words) {
  const box = page.getByRole("searchbox", { name: "Search Hugging Face" });
  await box.fill(words);
  await box.press("Enter");
  await settle(page, 700);
  // The search sits under the recommended list: bring it to the top of the column.
  await box.evaluate((el) => el.closest("section").scrollIntoView({ block: "start" }));
  await settle(page, 300);
}

async function openResult(page, repo) {
  await page.getByRole("button", { name: repo, exact: true }).click();
  await settle(page, 800);
  await page
    .getByRole("button", { name: repo, exact: true })
    .evaluate((el) => el.closest("li").scrollIntoView({ block: "start" }));
  await settle(page, 300);
}

const SCREENS = [
  {
    id: "a",
    name: "a-main-view",
    run: async (page) => {
      await open(page);
      await typePrompt(page);
      await settle(page, 600);
    },
  },
  {
    id: "b",
    name: "b-model-picker-open",
    run: async (page) => {
      await open(page);
      await page.locator("[data-model-picker] button").first().click();
      await settle(page, 600);
    },
  },
  {
    id: "b2",
    name: "b2-after-switching-model",
    run: async (page) => {
      await open(page);
      await typePrompt(page);
      // A setting changed first, so the switch has something to say about it.
      await page.getByRole("radiogroup", { name: "How many" }).getByRole("radio", { name: "2" }).click();
      await page.locator("[data-model-picker] button").first().click();
      await settle(page, 500);
      await page
        .getByRole("listbox", { name: "via OpenRouter" })
        .getByRole("option", { name: /^Nano Banana 2\.1\s*Edit/ })
        .click();
      await settle(page, 900);
    },
  },
  {
    id: "c0",
    name: "c0-studio-menu",
    run: async (page) => {
      await open(page);
      await page.getByRole("button", { name: "Studio menu" }).click();
      await settle(page, 500);
    },
  },
  {
    id: "c",
    name: "c-get-local-models",
    run: async (page) => {
      await open(page);
      await openLocalModels(page);
      await mainToTop(page);
      await settle(page, 300);
    },
  },
  {
    id: "c2",
    name: "c2-get-local-models-as-opened",
    run: async (page) => {
      await open(page);
      await openLocalModels(page);
    },
  },
  {
    id: "d",
    name: "d-hf-search-results",
    run: async (page) => {
      await open(page);
      await openLocalModels(page);
      await search(page, "z-image");
    },
  },
  {
    id: "d2",
    name: "d2-hf-result-supported",
    run: async (page) => {
      await open(page);
      await openLocalModels(page);
      await search(page, "z-image");
      await openResult(page, "leejet/Z-Image-Turbo-GGUF");
    },
  },
  {
    id: "d3",
    name: "d3-hf-result-not-supported",
    run: async (page) => {
      await open(page);
      await openLocalModels(page);
      await search(page, "z-image");
      await openResult(page, "Tongyi-MAI/Z-Image-Turbo");
    },
  },
  {
    id: "e0",
    name: "e0-local-notice-before-first-download",
    run: async (page) => {
      await open(page, "view=studio&notice=0");
      await openLocalModels(page);
      await mainToTop(page);
      await page.getByRole("article", { name: "Z-Image Turbo" }).getByRole("button", { name: "Download" }).click();
      await settle(page, 700);
    },
  },
  {
    id: "e",
    name: "e-download-dialog",
    run: async (page) => {
      await open(page);
      await openLocalModels(page);
      await mainToTop(page);
      await page.getByRole("article", { name: "Z-Image Turbo" }).getByRole("button", { name: "Download" }).click();
      await settle(page, 700);
    },
  },
  {
    id: "e2",
    name: "e2-downloading-row",
    run: async (page) => {
      await open(page);
      await openLocalModels(page);
      await mainToTop(page);
      await page.getByRole("article", { name: "Z-Image Turbo" }).getByRole("button", { name: "Download" }).click();
      await settle(page, 600);
      await page.getByRole("alertdialog").getByRole("button", { name: "Download" }).click();
      await settle(page, 900);
      await mainToTop(page);
      await settle(page, 300);
    },
  },
  {
    id: "f",
    name: "f-advanced-panel",
    run: async (page) => {
      await open(page);
      await page.getByRole("button", { name: /^Advanced/ }).click();
      await settle(page, 500);
    },
  },
  {
    id: "f2",
    name: "f2-advanced-panel-full-height",
    viewport: { width: 1280, height: 1640 },
    element: "[data-studio-panel]",
    run: async (page) => {
      await open(page);
      await page.getByRole("button", { name: /^Advanced/ }).click();
      await settle(page, 500);
    },
  },
  {
    id: "g",
    name: "g-gallery-grid",
    run: async (page) => {
      await open(page);
      await page.getByRole("button", { name: "Grid" }).click();
      await settle(page, 600);
      await mainToTop(page);
      await settle(page, 900);
    },
  },
  {
    id: "g2",
    name: "g2-gallery-grid-as-opened",
    run: async (page) => {
      await open(page);
      await page.getByRole("button", { name: "Grid" }).click();
      await settle(page, 1200);
    },
  },
  {
    id: "h",
    name: "h-viewer",
    run: async (page) => {
      await open(page);
      await page.getByRole("button", { name: "Grid" }).click();
      await settle(page, 600);
      await mainToTop(page);
      await settle(page, 600);
      await page.locator('[data-output-tile="coast-o2"] button').first().click();
      await settle(page, 1200);
    },
  },
  {
    id: "i",
    name: "i-settings-studio",
    run: async (page) => {
      await open(page);
      await page.getByRole("button", { name: "Settings", exact: true }).click();
      await settle(page, 500);
      await page.getByRole("tab", { name: "Studio" }).click();
      await settle(page, 700);
    },
  },
  {
    id: "i2",
    name: "i2-settings-studio-lower",
    run: async (page) => {
      await open(page);
      await page.getByRole("button", { name: "Settings", exact: true }).click();
      await settle(page, 500);
      await page.getByRole("tab", { name: "Studio" }).click();
      await settle(page, 600);
      await page.locator("[role=dialog] div.overflow-y-auto.p-5").evaluate((el) => {
        el.scrollTop = el.scrollHeight;
      });
      await settle(page, 300);
    },
  },
  {
    id: "j",
    name: "j-main-view-narrow-900x650",
    viewport: { width: 900, height: 650 },
    run: async (page) => {
      await open(page);
      await typePrompt(page);
      await settle(page, 600);
    },
  },
  {
    id: "k",
    name: "k-video-tab",
    run: async (page) => {
      await open(page);
      await page.getByRole("tab", { name: "Video" }).click();
      await settle(page, 1200);
    },
  },
  {
    id: "z1",
    name: "z1-first-use-empty",
    run: async (page) => {
      await open(page, "view=studio&empty=1");
    },
  },
  {
    id: "z2",
    name: "z2-first-use-model-picker",
    run: async (page) => {
      await open(page, "view=studio&empty=1");
      await page.locator("[data-model-picker] button").first().click();
      await settle(page, 600);
    },
  },
  {
    id: "x1",
    name: "x1-chat-screen-for-comparison",
    run: async (page) => {
      await open(page, "");
      await page.getByRole("button", { name: /Refactor the download queue/ }).click();
      await settle(page, 1000);
    },
  },
  {
    id: "x2",
    name: "x2-chat-model-menu-for-comparison",
    run: async (page) => {
      await open(page, "");
      await page.getByRole("button", { name: /Refactor the download queue/ }).click();
      await settle(page, 800);
      await page.getByRole("button", { name: "Model for this chat" }).click();
      await settle(page, 600);
    },
  },
  {
    id: "x3",
    name: "x3-models-dialog-for-comparison",
    run: async (page) => {
      await open(page, "");
      await page.getByRole("button", { name: /Refactor the download queue/ }).click();
      await settle(page, 600);
      await page.getByRole("button", { name: "Models" }).click();
      await settle(page, 900);
    },
  },
];

(async () => {
  const browser = await playwright.chromium.launch();
  const written = [];
  const problems = [];
  for (const theme of THEMES) {
    for (const screen of SCREENS) {
      if (ONLY && !ONLY.includes(screen.id)) continue;
      const context = await browser.newContext({
        viewport: screen.viewport ?? { width: 1280, height: 800 },
        deviceScaleFactor: SCALE,
        colorScheme: theme,
        reducedMotion: "reduce",
      });
      const page = await context.newPage();
      page.on("pageerror", (e) => problems.push(`${screen.name} (${theme}): ${e.message}`));
      try {
        await screen.run(page);
        const file = path.join(OUT, `${screen.name}-${theme}.png`);
        if (screen.element) await page.locator(screen.element).screenshot({ path: file });
        else await page.screenshot({ path: file });
        written.push(file);
        const unhandled = await page.evaluate(() => [...window.__preview.unhandled]);
        if (unhandled.length) problems.push(`${screen.name} (${theme}): unhandled ${unhandled.join(", ")}`);
      } catch (e) {
        problems.push(`${screen.name} (${theme}): ${e.message.split("\n")[0]}`);
      }
      await context.close();
    }
  }
  await browser.close();
  for (const f of written) console.log(f);
  if (problems.length) console.log(`\n${problems.join("\n")}`);
})();

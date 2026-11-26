// Writes one page with every screen's before and after screenshot side by side, in both
// themes, for the owner's review (design 10.5): first the five pairs the design names, then
// every screen.
//
//   node e2e/preview/pairs.mjs <before-dir> <after-dir> [--out .cache/preview/pairs.html]
//
// The images are linked by relative path, not embedded: keep the page next to the two folders
// (the default, .cache/preview/, does).
import fs from "node:fs";
import path from "node:path";

const argv = process.argv.slice(2);
const outAt = argv.indexOf("--out");
const OUT = path.resolve(outAt >= 0 ? argv[outAt + 1] : ".cache/preview/pairs.html");
const [before, after] = argv.filter((a, i) => !a.startsWith("--") && argv[i - 1] !== "--out").map((d) => path.resolve(d));
if (!before || !after) {
  console.error("usage: node e2e/preview/pairs.mjs <before-dir> <after-dir> [--out FILE]");
  process.exit(2);
}

const THEMES = ["light", "dark"];
const png = (dir) => (fs.existsSync(dir) ? fs.readdirSync(dir).filter((f) => f.endsWith(".png")) : []);
const screenOf = (file) => file.replace(/-(light|dark)\.png$/, "");
const names = [...new Set([...png(before), ...png(after)].map(screenOf))];
// "a-main-view", "b2-after-switching-model": the file names sort by the screen's letter and number.
const order = (name) => {
  const m = /^([a-z])(\d*)-/.exec(name);
  return m ? `${m[1]}${(m[2] || "0").padStart(3, "0")}` : name;
};
names.sort((x, y) => order(x).localeCompare(order(y)));

/** The pairs design 10.5 names for the owner, shown first: the main view, the model sheet, Get
 *  models, a search result's detail panel and the grid. */
const FOR_REVIEW = [
  ["a-main-view", "The main view"],
  ["b-model-picker-open", "The model sheet"],
  ["c-get-local-models", "Get models"],
  ["d2-hf-result-supported", "A search result's detail panel"],
  ["g-gallery-grid", "The grid"],
].filter(([name]) => names.includes(name));

const esc = (t) => t.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
const rel = (dir, file) => encodeURI(path.relative(path.dirname(OUT), path.join(dir, file)).split(path.sep).join("/"));

const cell = (dir, label, name, theme) => {
  const file = `${name}-${theme}.png`;
  const has = fs.existsSync(path.join(dir, file));
  return `<figure><figcaption>${label}, ${theme}</figcaption>${
    has ? `<a href="${rel(dir, file)}"><img loading="lazy" src="${rel(dir, file)}" alt="${esc(name)} ${label} ${theme}"></a>` : '<div class="missing">no screenshot</div>'
  }</figure>`;
};

const section = (name, id, title) =>
  `<section id="${esc(id)}"><h2>${esc(title)}</h2>${THEMES.map(
    (theme) => `<div class="row">${cell(before, "before", name, theme)}${cell(after, "after", name, theme)}</div>`,
  ).join("")}</section>`;
const review = FOR_REVIEW.map(([name, title]) => section(name, `review-${name}`, `${title} (${name})`)).join("\n");
const sections = names.map((name) => section(name, name, name)).join("\n");

const html = `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Studio before and after</title>
<style>
  :root { color-scheme: light dark; font-family: system-ui, sans-serif; }
  body { margin: 0 auto; padding: 16px; max-width: 2200px; }
  h1 { font-size: 1.25rem; }
  h2 { font-size: 1rem; margin: 24px 0 8px; }
  nav { display: flex; flex-wrap: wrap; gap: 6px 12px; font-size: 0.85rem; }
  .row { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; margin-bottom: 12px; }
  figure { margin: 0; min-width: 0; }
  figcaption { font-size: 0.8rem; opacity: 0.7; margin-bottom: 4px; }
  img { width: 100%; height: auto; display: block; border: 1px solid #8884; }
  .missing { padding: 40px; text-align: center; border: 1px dashed #8886; opacity: 0.7; }
</style></head>
<body>
<h1>Studio, before (${esc(path.basename(before))}) and after (${esc(path.basename(after))})</h1>
<nav>${names.map((n) => `<a href="#${esc(n)}">${esc(n.split("-")[0])}</a>`).join("")}</nav>
${review ? `<h1>For review (design 10.5)</h1>\n${review}\n<h1>Every screen</h1>` : ""}
${sections}
</body></html>
`;
fs.mkdirSync(path.dirname(OUT), { recursive: true });
fs.writeFileSync(OUT, html);
console.log(`${OUT}: ${names.length} screens`);

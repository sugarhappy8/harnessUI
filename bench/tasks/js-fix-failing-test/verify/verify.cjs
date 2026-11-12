const fs = require("node:fs");
const path = require("node:path");
const assert = require("node:assert");

const nonce = fs.readFileSync(0, "utf8").split("\n")[0].trim();
const load = (name) => require(path.join(process.cwd(), name));

try {
  const { slugify } = load("slugify.js");
  assert.strictEqual(slugify("Hello World"), "hello-world");
  assert.strictEqual(slugify("  Many   spaces  "), "many-spaces");
  assert.strictEqual(slugify("Ready? Go!"), "ready-go");
  assert.strictEqual(slugify("--Already-Slugged--"), "already-slugged");
  assert.strictEqual(slugify("plain"), "plain");
} catch (e) {
  console.error(e && e.message ? e.message : String(e));
  process.exit(1);
}
console.log("VERIFIED " + nonce);

const fs = require("node:fs");
const path = require("node:path");
const assert = require("node:assert");

const nonce = fs.readFileSync(0, "utf8").split("\n")[0].trim();
const load = (name) => require(path.join(process.cwd(), name));

try {
  const { displayName } = load("names.js");
  assert.strictEqual(displayName({ profile: { name: " Ada " } }), "Ada");
  assert.strictEqual(displayName({}), "(no name)");
  assert.strictEqual(displayName({ profile: {} }), "(no name)");
  assert.strictEqual(displayName({ profile: { name: "Grace" } }), "Grace");
} catch (e) {
  console.error(e && e.message ? e.message : String(e));
  process.exit(1);
}
console.log("VERIFIED " + nonce);

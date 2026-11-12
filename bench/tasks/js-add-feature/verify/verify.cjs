const fs = require("node:fs");
const path = require("node:path");
const assert = require("node:assert");

const nonce = fs.readFileSync(0, "utf8").split("\n")[0].trim();
const load = (name) => require(path.join(process.cwd(), name));

try {
  const { Cart } = load("cart.js");
  const cart = new Cart();
  cart.add("a", 10, 2);
  cart.add("b", 5.5);
  assert.strictEqual(cart.total(), 25.5);
  assert.strictEqual(cart.total(10), 22.95);
  assert.strictEqual(cart.total(100), 0);
  assert.strictEqual(new Cart().total(), 0);
  assert.strictEqual(cart.count(), 3);
} catch (e) {
  console.error(e && e.message ? e.message : String(e));
  process.exit(1);
}
console.log("VERIFIED " + nonce);

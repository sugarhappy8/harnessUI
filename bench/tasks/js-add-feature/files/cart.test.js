const test = require("node:test");
const assert = require("node:assert");
const { Cart } = require("./cart");

test("count adds up the quantities", () => {
  const cart = new Cart();
  cart.add("pen", 1.5, 2);
  cart.add("pad", 3);
  assert.strictEqual(cart.count(), 3);
});

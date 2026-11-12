const assert = require("node:assert");
const { slugify } = require("./slugify");

assert.strictEqual(slugify("Hello World"), "hello-world");
assert.strictEqual(slugify("  Many   spaces  "), "many-spaces");
assert.strictEqual(slugify("Ready? Go!"), "ready-go");
console.log("all tests passed");

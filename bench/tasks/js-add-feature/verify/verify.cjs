// Prints the raw results of the project's code for fixed inputs, as one line
// "RESULT <json>". It decides nothing: the app compares the values with what it
// expects. It runs in its own process; the app never loads the project's code.
const path = require("node:path");
const load = (name) => require(path.join(process.cwd(), name));
const caseOf = (fn) => {
  try {
    return { value: fn() };
  } catch (e) {
    return { error: (e && e.name) || "Error" };
  }
};

let results;
try {
  const { Cart } = load("cart.js");
  const cart = new Cart();
  cart.add("a", 10, 2);
  cart.add("b", 5.5);
  results = {
    total: caseOf(() => cart.total()),
    total10: caseOf(() => cart.total(10)),
    total100: caseOf(() => cart.total(100)),
    empty: caseOf(() => new Cart().total()),
    count: caseOf(() => cart.count()),
  };
} catch (e) {
  results = { failedToLoad: String((e && e.name) || e) };
}
console.log("RESULT " + JSON.stringify(results));

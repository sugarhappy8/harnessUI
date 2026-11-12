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
  const { slugify } = load("slugify.js");
  results = {
    hello: caseOf(() => slugify("Hello World")),
    spaces: caseOf(() => slugify("  Many   spaces  ")),
    ready: caseOf(() => slugify("Ready? Go!")),
    slugged: caseOf(() => slugify("--Already-Slugged--")),
    plain: caseOf(() => slugify("plain")),
  };
} catch (e) {
  results = { failedToLoad: String((e && e.name) || e) };
}
console.log("RESULT " + JSON.stringify(results));

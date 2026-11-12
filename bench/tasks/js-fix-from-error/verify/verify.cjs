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
  const { displayName } = load("names.js");
  results = {
    ada: caseOf(() => displayName({ profile: { name: " Ada " } })),
    noProfile: caseOf(() => displayName({})),
    noName: caseOf(() => displayName({ profile: {} })),
    grace: caseOf(() => displayName({ profile: { name: "Grace" } })),
  };
} catch (e) {
  results = { failedToLoad: String((e && e.name) || e) };
}
console.log("RESULT " + JSON.stringify(results));

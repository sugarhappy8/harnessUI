const users = require("./users.json");
const { displayName } = require("./names");

for (const user of users) {
  console.log(displayName(user));
}

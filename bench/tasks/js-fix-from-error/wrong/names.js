// The name to show for a user.
function displayName(user) {
  return user.profile ? user.profile.name.trim() : "";
}

module.exports = { displayName };

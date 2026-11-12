// The name to show for a user.
function displayName(user) {
  const name = user.profile && user.profile.name;
  return name ? name.trim() : "(no name)";
}

module.exports = { displayName };

// Turns a title into a lower-case slug: words joined by single hyphens.
function slugify(title) {
  return title
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-");
}

module.exports = { slugify };

/* Shows an SVG artifact (protocol.rs, svg_page). The picture arrives as text. It is read as
   XML, so that what does not parse is reported, and what does parse is shown.

   This file is put on one line: see the note at the top of bootstrap.js. */
(function () {
  "use strict";
  var preview = window.__preview;

  var notice = function (message) {
    var box = document.createElement("div");
    box.id = "notice";
    box.textContent = message;
    document.body.appendChild(box);
  };

  var parsed = new DOMParser().parseFromString(String(preview.source), "image/svg+xml");
  var failure = parsed.getElementsByTagName("parsererror")[0];
  var root = parsed.documentElement;

  if (failure) {
    var said = String(failure.textContent || "");
    var found = /error on line (\d+) at column (\d+): ([^\n]*)/.exec(said);
    var message = found ? found[3] : said.replace(/\s+/g, " ").trim();
    preview.problem(
      "svg",
      "The SVG does not parse: " + message,
      found ? Number(found[1]) : undefined,
      found ? Number(found[2]) : undefined
    );
    notice("This SVG does not parse: " + message);
  } else if (!root || root.localName !== "svg") {
    preview.problem("svg", "The content is not an SVG picture: its outermost element must be an svg element.");
    notice("This is not an SVG picture.");
  } else {
    /* A link element of the HTML kind inside the picture (in a foreignObject) could hint
       a connection to a server. Nothing else in a picture can reach one, so it goes. */
    var links = root.getElementsByTagNameNS("http://www.w3.org/1999/xhtml", "link");
    while (links.length) { links[0].parentNode.removeChild(links[0]); }
    document.body.appendChild(document.importNode(root, true));
  }
  preview.ready();
})();

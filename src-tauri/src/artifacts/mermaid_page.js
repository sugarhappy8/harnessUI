/* Draws a Mermaid artifact with the library bundled in the app (protocol.rs, mermaid_page).
   The library's strict setting keeps scripts and links out of what it draws; the sandbox
   around the page is what the app relies on.

   This file is put on one line: see the note at the top of bootstrap.js. */
(function () {
  "use strict";
  var preview = window.__preview;
  var target = document.getElementById("diagram");
  var library = window.mermaid;

  if (!library || typeof library.render !== "function") {
    target.id = "notice";
    target.textContent = "The diagram could not be drawn on this system.";
    preview.unavailable("the diagram library could not start in the preview on this system");
    return;
  }

  var failed = function (error) {
    var message = String((error && (error.message || error.str)) || error);
    var found = /line (\d+)/.exec(message);
    preview.problem("diagram", message, found ? Number(found[1]) : undefined);
    target.id = "notice";
    target.textContent = message;
    preview.ready();
  };

  try {
    library.initialize({ startOnLoad: false, securityLevel: "strict", theme: "default", suppressErrorRendering: true });
    library.render("artifact-diagram", String(preview.source)).then(function (drawn) {
      target.innerHTML = drawn.svg;
      preview.ready();
    }, failed);
  } catch (error) {
    failed(error);
  }
})();

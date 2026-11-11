/* The first script of every preview page (protocol.rs puts it there). It runs before
   anything the model wrote.

   - It stops the page when the page is not inside the app's frame.
   - It reports what goes wrong on the page to the frame's parent, which is the only place
     a preview can send anything to: uncaught errors, unhandled promise rejections,
     console.error lines and requests that the page's rules stopped.
   - On WebKit it makes those reports complete (the last part of this file).

   Everything it sends is text the page controls, and the app treats it as that.

   This file is put on one line: end every statement with a semicolon, write comments
   like this one, and keep the two characters that open a comment out of strings. */
(function () {
  "use strict";
  var config = __CONFIG__;
  var own = document.currentScript;

  /* A preview is only ever shown inside the app's frame. As the page of a whole window it
     would be handed the app's own script interface, so it stops before anything else. */
  if (window.top === window.self) {
    try { window.stop(); } catch (ignore) {}
    try { document.documentElement.textContent = ""; } catch (ignore) {}
    throw new Error("A preview is only shown inside the app.");
  }

  /* The page's first line starts with "<!doctype html><script>", this script and its end
     tag. Positions on that line are counted from where the model's text starts. */
  var prefix = 23 + (own ? own.textContent.length : 0) + 9;

  var sent = 0;
  function post(message) {
    if (sent >= 60) { return; }
    sent += 1;
    message.source = "artifact";
    message.id = config.id;
    message.version = config.version;
    try { window.parent.postMessage(message, "*"); } catch (ignore) {}
  }

  function text(value) {
    try { return String(value).slice(0, 600); } catch (ignore) { return "(a value that cannot be shown)"; }
  }

  function describe(value) {
    try {
      if (value && typeof value.message === "string") {
        return (typeof value.name === "string" && value.name ? value.name + ": " : "") + value.message;
      }
      if (value && typeof value === "object") { return text(JSON.stringify(value)); }
    } catch (ignore) {}
    return text(value);
  }

  function problem(type, message, line, col) {
    var out = { kind: "problem", type: type, message: text(message) };
    if (typeof line === "number" && line >= 1 && line < 1e7) {
      out.line = Math.floor(line);
      if (typeof col === "number" && col >= 1 && col < 1e7) { out.col = Math.floor(col); }
    }
    post(out);
  }

  function place(line, col, url) {
    var ownFile = typeof url === "string" && url.indexOf("/__s/") >= 0;
    if (line === 1 && !ownFile && typeof col === "number") { col = col > prefix ? col - prefix : 0; }
    return { line: line, col: col };
  }

  function whereThrown(error) {
    var line;
    var col;
    var url;
    try {
      if (typeof error.line === "number") {
        line = error.line;
        col = error.column;
        url = error.sourceURL;
      } else if (typeof error.stack === "string") {
        var found = /(\S*?):(\d+):(\d+)\)?[ \t]*(\n|$)/.exec(error.stack);
        if (found) {
          url = found[1];
          line = Number(found[2]);
          col = Number(found[3]);
        }
      }
    } catch (ignore) {}
    return place(line, col, url);
  }

  /* Set while an error this script caught itself (with its details) is on its way to the
     window, where WebKit announces it a second time without them. */
  var detailed = false;

  window.addEventListener("error", function (event) {
    if (!event || typeof event.message !== "string") { return; }
    if (event.message === "Script error." && !event.lineno) {
      if (!detailed) { problem("error", "A script on the page failed. The preview could not see where or why."); }
      return;
    }
    var at = place(event.lineno, event.colno, event.filename);
    problem("error", event.message, at.line, at.col);
  });

  window.addEventListener("unhandledrejection", function (event) {
    var at = whereThrown(event.reason || {});
    problem("rejection", describe(event.reason), at.line, at.col);
  });

  document.addEventListener("securitypolicyviolation", function (event) {
    problem(
      "blocked",
      text(event.effectiveDirective || event.violatedDirective) + " " + text(event.blockedURI),
      event.lineNumber,
      event.columnNumber
    );
  });

  var pageConsole = window.console;
  if (pageConsole && typeof pageConsole.error === "function") {
    var consoleError = pageConsole.error;
    pageConsole.error = function () {
      var parts = [];
      for (var i = 0; i < arguments.length; i += 1) { parts.push(describe(arguments[i])); }
      problem("console", parts.join(" "));
      return consoleError.apply(pageConsole, arguments);
    };
  }

  function ready() { post({ kind: "ready" }); }

  if (config.kind === "html") {
    window.addEventListener("load", ready);
  } else {
    /* The app's own pages for pictures and diagrams (svg_page.js, mermaid_page.js). */
    window.__preview = {
      source: config.source,
      problem: problem,
      ready: ready,
      unavailable: function (why) { post({ kind: "unavailable", message: text(why) }); }
    };
  }

  /* WebKit tells a sandboxed page nothing about an error in its own script: only "Script
     error.", with no message and no line. Two things restore the details there.

     1. A script the page has in its text is asked for as a file instead, with permission
        to read it, before it runs (protocol.rs, "A page's own scripts, as files"). It is
        the same text at the same place, runs at the same moment, and an error while it
        runs is then reported in full.
     2. Code that runs later (event listeners, timers, handlers written as attributes) is
        called through a wrapper that looks at what it throws and throws it on unchanged. */
  var isWebKit = /AppleWebKit\//.test(navigator.userAgent) && !/(Chrome|Chromium|Edg)\//.test(navigator.userAgent);
  if (isWebKit && config.kind === "html" && typeof MutationObserver === "function" && typeof WeakMap === "function") {
    var nativeTimeout = window.setTimeout;
    var guards = new WeakMap();

    var caught = function (error) {
      var at = whereThrown(error || {});
      detailed = true;
      nativeTimeout.call(window, function () { detailed = false; }, 0);
      problem("error", describe(error), at.line, at.col);
    };

    var guard = function (callback) {
      if (typeof callback !== "function") { return callback; }
      var guarded = guards.get(callback);
      if (!guarded) {
        guarded = function () {
          try { return callback.apply(this, arguments); } catch (error) { caught(error); throw error; }
        };
        guards.set(callback, guarded);
      }
      return guarded;
    };

    var add = EventTarget.prototype.addEventListener;
    var remove = EventTarget.prototype.removeEventListener;
    EventTarget.prototype.addEventListener = function (type, listener, options) {
      return add.call(this, type, guard(listener), options);
    };
    EventTarget.prototype.removeEventListener = function (type, listener, options) {
      return remove.call(this, type, (typeof listener === "function" && guards.get(listener)) || listener, options);
    };
    ["setTimeout", "setInterval", "requestAnimationFrame", "queueMicrotask"].forEach(function (name) {
      var native = window[name];
      if (typeof native !== "function") { return; }
      window[name] = function (callback) {
        var args = Array.prototype.slice.call(arguments);
        args[0] = guard(callback);
        return native.apply(window, args);
      };
    });

    var source = typeof config.source === "string" ? config.source : "";
    var cursor = 0;
    var javascript = /^(|text\/javascript|application\/javascript|text\/ecmascript|application\/ecmascript|application\/x-javascript|text\/x-javascript|text\/jscript)$/i;

    var asFile = function (script) {
      if (document.readyState !== "loading" || !config.token) { return; }
      if (script.namespaceURI !== "http://www.w3.org/1999/xhtml") { return; }
      if (script.hasAttribute("src") || script.hasAttribute("nomodule") || script.hasAttribute("language") || script.hasAttribute("for")) { return; }
      if (!javascript.test((script.getAttribute("type") || "").trim())) { return; }
      var code = script.textContent;
      if (!code || !code.trim()) { return; }
      var at = source.indexOf(code, cursor);
      if (at < 0) { return; }
      cursor = at + code.length;
      script.removeAttribute("async");
      script.removeAttribute("defer");
      script.setAttribute("crossorigin", "anonymous");
      script.setAttribute("src", config.origin + "/__s/" + config.token + "/" + at + "-" + (at + code.length) + ".js");
    };

    var guardHandlers = function (element) {
      var attributes = element.attributes;
      for (var i = 0; attributes && i < attributes.length; i += 1) {
        var name = attributes[i].name;
        if (name.length > 2 && name.slice(0, 2) === "on") {
          try {
            var handler = element[name];
            if (typeof handler === "function") { element[name] = guard(handler); }
          } catch (ignore) {}
        }
      }
    };

    new MutationObserver(function (records) {
      for (var i = 0; i < records.length; i += 1) {
        var added = records[i].addedNodes;
        for (var j = 0; j < added.length; j += 1) {
          var node = added[j];
          if (node.nodeType !== 1) { continue; }
          if (node.localName === "script") { asFile(node); }
          guardHandlers(node);
          if (node.firstElementChild) {
            var inner = node.querySelectorAll("*");
            for (var k = 0; k < inner.length; k += 1) { guardHandlers(inner[k]); }
          }
        }
      }
    }).observe(document, { childList: true, subtree: true });
  }

  /* Leave no element of the app's in the page the model's scripts look at. */
  if (own && own.parentNode) { own.parentNode.removeChild(own); }
})();

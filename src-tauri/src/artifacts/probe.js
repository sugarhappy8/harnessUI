/* The page that checks the preview sandbox on this system (isolation.rs). It tries what a
   hostile preview would try first, and says what it saw. The app does not rely on what it
   says alone: the call and the requests below are tripwires, noticed on the app's side
   whatever this page reports.

   This file is put on one line: see the note at the top of bootstrap.js. */
(function () {
  "use strict";
  var config = __CONFIG__;
  var report = {
    origin: String(window.origin),
    storage: "blocked",
    parent: "blocked",
    cookie: "blocked",
    internals: "absent",
    ipc: "none"
  };

  try {
    window.localStorage.setItem("preview-check", "1");
    report.storage = "available";
  } catch (ignore) {}
  try {
    if (window.parent.document) { report.parent = "reachable"; }
  } catch (ignore) {}
  try {
    document.cookie = "previewcheck=1";
    if (String(document.cookie).indexOf("previewcheck=1") >= 0) { report.cookie = "available"; }
  } catch (ignore) {}

  /* The app's own script interface, where the webview puts it into frames (WebView2). */
  try {
    var internals = window.__TAURI_INTERNALS__;
    if (internals) {
      report.internals = "present";
      if (typeof internals.invoke === "function") {
        report.ipc = "sent";
        internals.invoke("artifact_canary", { nonce: config.nonce }).then(
          function () { report.ipc = "answered"; },
          function () { report.ipc = "refused"; }
        );
      }
    }
  } catch (ignore) {
    report.ipc = "threw";
  }

  /* Requests the page's rules must stop. One that arrives is noticed by the app. */
  var hit = config.origin + "/__probe/" + config.nonce + "/hit/";
  try { window.fetch(hit + "fetch").then(function () {}, function () {}); } catch (ignore) {}
  try {
    var request = new XMLHttpRequest();
    request.open("GET", hit + "xhr");
    request.send();
  } catch (ignore) {}
  try { new Image().src = hit + "img"; } catch (ignore) {}

  /* A frame inside the frame gets the same treatment. */
  try {
    var child = document.createElement("iframe");
    child.srcdoc = "<script>try{window.__TAURI_INTERNALS__.invoke('artifact_canary',{nonce:'" + config.nonce + "'})}catch(e){}<\/script>";
    document.documentElement.appendChild(child);
  } catch (ignore) {}

  window.setTimeout(function () {
    try {
      window.parent.postMessage({ source: "artifact-probe", nonce: config.nonce, report: report }, "*");
    } catch (ignore) {}
  }, 500);
})();

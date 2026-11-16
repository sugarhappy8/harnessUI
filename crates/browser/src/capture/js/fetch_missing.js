async function (opts) {
  // browser_capture_site: asks for pictures the page's style sheets name but the page never
  // loaded (a background for hover, for another screen width), so the copy has them too.
  // Plain GET requests without cookies, made by the page itself, so the browser's request
  // checks apply to each one exactly as to the page's own. Runs in an isolated world.
  'use strict';
  const urls = (opts && opts.urls) || [];
  const deadline = Date.now() + ((opts && opts.ms) || 8000);
  let done = 0;
  let i = 0;
  async function worker() {
    while (i < urls.length && Date.now() < deadline) {
      const u = urls[i++];
      try {
        const ctrl = new AbortController();
        const timer = setTimeout(() => ctrl.abort(), Math.max(500, deadline - Date.now()));
        const res = await fetch(u, { mode: 'no-cors', credentials: 'omit', cache: 'no-store', redirect: 'follow', referrerPolicy: 'no-referrer', signal: ctrl.signal });
        try {
          await res.arrayBuffer();
        } catch (e) {
          /* an opaque body still reaches the browser's record of the response */
        }
        clearTimeout(timer);
        done++;
      } catch (e) {
        /* blocked or failed: the copy keeps the address */
      }
    }
  }
  const workers = [];
  for (let k = 0; k < 6; k++) workers.push(worker());
  await Promise.all(workers);
  return { asked: Math.min(i, urls.length), done: done };
}

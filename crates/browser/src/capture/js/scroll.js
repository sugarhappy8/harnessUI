function (opts) {
  // browser_capture_site: scrolls the page (or, when the page scrolls inside one element,
  // that element) to `opts.y` at once and reports where it is. Runs in an isolated world.
  'use strict';
  const se = document.scrollingElement || document.documentElement;
  const vh = window.innerHeight || 800;
  let inner = null;
  if (se.scrollHeight <= vh + 40) {
    // A page whose document does not scroll but one large element does (smooth-scroll
    // libraries often work like this).
    let best = null;
    let bestArea = 0;
    const all = document.body ? document.body.getElementsByTagName('*') : [];
    for (let i = 0; i < all.length && i < 5000; i++) {
      const el = all[i];
      if (el.scrollHeight <= el.clientHeight + 100) continue;
      const oy = getComputedStyle(el).overflowY;
      if (oy !== 'auto' && oy !== 'scroll' && oy !== 'overlay') continue;
      const area = el.clientWidth * el.clientHeight;
      if (area > bestArea) {
        best = el;
        bestArea = area;
      }
    }
    if (best && bestArea >= window.innerWidth * vh * 0.4) inner = best;
  }
  const y = Math.max(0, Number(opts && opts.y) || 0);
  if (inner) {
    inner.scrollTop = y;
    return { y: Math.round(inner.scrollTop), height: inner.scrollHeight, viewport: inner.clientHeight, inner: true };
  }
  window.scrollTo({ top: y, left: 0, behavior: 'instant' });
  return {
    y: Math.round(window.scrollY),
    height: Math.max(se.scrollHeight, document.body ? document.body.scrollHeight : 0),
    viewport: vh,
    inner: false,
  };
}

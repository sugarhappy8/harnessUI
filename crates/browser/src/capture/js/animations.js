function (opts) {
  // browser_capture_site: every animation running on the page right now, from the Web
  // Animations API (CSS animations, CSS transitions and script animations alike), with its
  // keyframes and timing. Runs in an isolated world; reads only. Values are page data.
  'use strict';
  const max = (opts && opts.max) || 300;
  function sel(el) {
    if (!el || el.nodeType !== 1) return '';
    const parts = [];
    let e = el;
    let depth = 0;
    while (e && e.nodeType === 1 && depth < 4) {
      let p = e.localName;
      if (e.id && /^[A-Za-z][\w-]{0,40}$/.test(e.id)) {
        parts.unshift(p + '#' + e.id);
        break;
      }
      const cls = Array.from(e.classList || []).filter((c) => /^[A-Za-z_-][\w-]{0,40}$/.test(c)).slice(0, 2);
      if (cls.length) p += '.' + cls.join('.');
      const parent = e.parentElement;
      if (parent) {
        const same = Array.from(parent.children).filter((c) => c.localName === e.localName);
        if (same.length > 1) p += ':nth-of-type(' + (same.indexOf(e) + 1) + ')';
      }
      parts.unshift(p);
      e = parent;
      depth++;
    }
    return parts.join(' > ').slice(0, 300);
  }
  function num(v) {
    if (v === Infinity) return 'infinite';
    if (typeof v === 'number') return Math.round(v * 1000) / 1000;
    if (v == null) return null;
    return String(v).slice(0, 60);
  }
  function range(r) {
    if (r == null) return null;
    if (typeof r === 'string') return r.slice(0, 60);
    try {
      return ((r.rangeName || '') + ' ' + (r.offset ? String(r.offset) : '')).trim().slice(0, 60);
    } catch (e) {
      return null;
    }
  }
  let list = [];
  try {
    list = document.getAnimations();
  } catch (e) {
    list = [];
  }
  const res = [];
  for (let i = 0; i < list.length && res.length < max; i++) {
    const a = list[i];
    try {
      const e = a.effect;
      if (!e) continue;
      const t = e.getTiming ? e.getTiming() : {};
      let kind = 'script';
      let name = a.id || '';
      if (typeof CSSAnimation !== 'undefined' && a instanceof CSSAnimation) {
        kind = 'css-animation';
        name = a.animationName;
      } else if (typeof CSSTransition !== 'undefined' && a instanceof CSSTransition) {
        kind = 'css-transition';
        name = a.transitionProperty;
      }
      let kfs = [];
      try {
        kfs = e.getKeyframes ? e.getKeyframes() : [];
      } catch (x) {
        kfs = [];
      }
      const frames = kfs.slice(0, 40).map((k) => {
        const o = {};
        let n = 0;
        for (const p in k) {
          if (p === 'computedOffset' || n >= 30) continue;
          const v = k[p];
          o[p] = v == null ? null : typeof v === 'number' ? Math.round(v * 10000) / 10000 : String(v).slice(0, 240);
          n++;
        }
        return o;
      });
      let timeline = { kind: 'document' };
      const tl = a.timeline;
      if (!tl) timeline = { kind: 'none' };
      else if (typeof ViewTimeline !== 'undefined' && tl instanceof ViewTimeline) timeline = { kind: 'view', axis: String(tl.axis || ''), subject: sel(tl.subject) };
      else if (typeof ScrollTimeline !== 'undefined' && tl instanceof ScrollTimeline) timeline = { kind: 'scroll', axis: String(tl.axis || ''), source: sel(tl.source) };
      res.push({
        kind: kind,
        name: String(name || '').slice(0, 120),
        target: sel(e.target),
        pseudo: e.pseudoElement ? String(e.pseudoElement).slice(0, 40) : null,
        keyframes: frames,
        timing: {
          duration: num(t.duration),
          delay: num(t.delay),
          endDelay: num(t.endDelay),
          easing: String(t.easing || 'linear').slice(0, 120),
          iterations: num(t.iterations),
          direction: String(t.direction || 'normal'),
          fill: String(t.fill || 'auto'),
          iterationStart: num(t.iterationStart),
        },
        timeline: timeline,
        rangeStart: range(a.rangeStart),
        rangeEnd: range(a.rangeEnd),
        playState: String(a.playState || ''),
        playbackRate: typeof a.playbackRate === 'number' ? a.playbackRate : 1,
      });
    } catch (err) {
      /* one animation that cannot be read */
    }
  }
  return { y: Math.round(window.scrollY), total: list.length, list: res };
}

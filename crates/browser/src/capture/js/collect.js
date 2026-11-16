async function (opts) {
  // browser_capture_site (ARCHITECTURE.md 9.3): runs in an isolated world of the page's main
  // frame, so the page's own scripts can neither see it nor change the built-ins it uses. It
  // reads the live page and never changes it: the copy is built in an inert document (one
  // with no window, so nothing in it loads or runs), node by node.
  //
  // Returns the page serialised from that copy, with scripts, event handlers, frames'
  // contents, `javascript:` addresses and the page's own policies removed, and every address
  // the app may replace with a local file written as a token `hcap-<nonce>-<kind><n>`:
  //   u<n>: urls[n], an absolute http(s) address (a picture, an icon, a font, a video...)
  //   s<n>: styles[n], a style sheet (linked, inline or adopted)
  //   x<n>: srcsets[n], the candidates of a srcset attribute
  //   c<n>: canvases[n], a still picture of a canvas the app may save
  // and the facts the report needs: outline, computed-style counts, library markers.
  // Everything returned is data from the page; the app treats it as untrusted.
  'use strict';
  const NONCE = String(opts.nonce || 'x');
  const MAX_NODES = opts.maxNodes || 60000;
  const MAX_DEPTH = 400;
  const MAX_STAT = opts.maxStatElements || 4000;
  const MAX_STYLE_CHARS = opts.maxStyleChars || 8000000;
  const MAX_INLINE_SCRIPT = opts.maxInlineScriptChars || 1000000;
  const MAX_SCRIPTS = 400;
  const MAX_CANVASES = opts.maxCanvases || 8;
  const HTML_NS = 'http://www.w3.org/1999/xhtml';
  const BASE = document.baseURI;
  const pre = 'hcap-' + NONCE + '-';
  const out = {
    url: String(location.href),
    base: String(document.baseURI || location.href),
    title: String(document.title || ''),
    lang: String(document.documentElement.getAttribute('lang') || ''),
    quirks: document.compatMode === 'BackCompat',
    urls: [],
    styles: [],
    srcsets: [],
    canvases: [],
    scripts: [],
    frames: [],
    media: [],
    outline: [],
    headings: [],
    stats: {},
    markers: {},
    nodes: 0,
    cut: false,
    shadowRoots: 0,
    signIn: false,
    html: '',
  };

  // ---- addresses ------------------------------------------------------------------------
  function absUrl(v, base) {
    if (v == null) return null;
    const t = String(v).trim();
    if (!t) return null;
    try {
      return new URL(t, base || BASE).href;
    } catch (e) {
      return null;
    }
  }
  function isHttp(u) {
    return /^https?:\/\//i.test(u);
  }
  // An address that runs code or shows a document of its own when followed.
  function squeeze(v) {
    let s = '';
    for (const ch of String(v)) {
      const c = ch.codePointAt(0);
      if (c <= 0x20 || (c >= 0x7f && c <= 0xa0) || c === 0x1680 || c === 0x180e || (c >= 0x2000 && c <= 0x200f)) continue;
      if (c === 0x2028 || c === 0x2029 || c === 0x202f || c === 0x205f || c === 0x3000 || c === 0xfeff) continue;
      s += ch;
    }
    return s.toLowerCase();
  }
  function isScriptUrl(v) {
    const t = squeeze(v);
    return (
      t.startsWith('javascript:') ||
      t.startsWith('vbscript:') ||
      t.startsWith('data:text/html') ||
      t.startsWith('data:application/xhtml') ||
      t.startsWith('data:text/xml') ||
      t.startsWith('data:application/xml')
    );
  }
  const urlIdx = new Map();
  // `kind` is 'u', or 'v' for a shape another SVG file holds (`<use href="icons.svg#x">`),
  // which the app may copy into the page: a page opened from disk cannot use another file's.
  function urlToken(v, base, kind) {
    const a = absUrl(v, base);
    if (!a || !isHttp(a)) return null;
    const hash = a.indexOf('#');
    const file = hash >= 0 ? a.slice(0, hash) : a;
    const frag = hash >= 0 ? a.slice(hash) : '';
    let i = urlIdx.get(file);
    if (i === undefined) {
      i = out.urls.length;
      out.urls.push(file);
      urlIdx.set(file, i);
    }
    return pre + (kind || 'u') + i + frag;
  }
  function srcsetToken(v, base) {
    const list = [];
    // Candidates are separated by commas that follow a descriptor or a URL; a URL itself may
    // hold commas (data: URLs), which are left as they are.
    const parts = String(v).split(/,(?=\s*\S)/);
    for (const part of parts) {
      const m = part.trim().match(/^(\S+)(\s+.*)?$/);
      if (!m) continue;
      const a = absUrl(m[1], base);
      if (!a || !isHttp(a)) continue;
      list.push([a.split('#')[0], (m[2] || '').trim().slice(0, 40)]);
      if (list.length >= 30) break;
    }
    if (!list.length) return null;
    const i = out.srcsets.length;
    out.srcsets.push(list);
    return pre + 'x' + i;
  }

  // CSS addresses inside a style attribute or an SVG style element.
  const CSS_URL_RE = /url\(\s*(?:"((?:[^"\\\n]|\\[\s\S])*)"|'((?:[^'\\\n]|\\[\s\S])*)'|((?:[^)\s"'\\]|\\[\s\S])*))\s*\)/gi;
  function cssUnescape(s) {
    return s.replace(/\\([0-9a-fA-F]{1,6}\s?|[\s\S])/g, function (m, e) {
      if (/^[0-9a-fA-F]/.test(e)) {
        const cp = parseInt(e, 16);
        return String.fromCodePoint(cp > 0 && cp <= 0x10ffff ? cp : 0xfffd);
      }
      return e === '\n' ? '' : e;
    });
  }
  function rewriteInlineCss(text) {
    return String(text).replace(CSS_URL_RE, function (m, d, s, u) {
      const raw = cssUnescape(d != null ? d : s != null ? s : u || '');
      if (/^\s*(data:|#)/i.test(raw)) return m;
      if (isScriptUrl(raw)) return 'none';
      const t = urlToken(raw, BASE);
      return t ? 'url("' + t + '")' : m;
    });
  }

  // ---- style sheets ---------------------------------------------------------------------
  function rulesText(sheet) {
    let rules = null;
    try {
      rules = sheet && sheet.cssRules;
    } catch (e) {
      return null;
    }
    if (!rules) return null;
    const parts = [];
    for (let i = 0; i < rules.length; i++) parts.push(rules[i].cssText);
    return parts.join('\n');
  }
  // The text of a <style> element. CSS-in-JS libraries add rules with insertRule() and leave
  // the element's text empty, or add more rules than its text holds: then the rules are what
  // the page shows. Otherwise the text is kept as written (with what the browser dropped).
  function styleElementText(el) {
    const raw = el.textContent || '';
    let rules = null;
    try {
      rules = el.sheet && el.sheet.cssRules;
    } catch (e) {
      rules = null;
    }
    if (!rules || rules.length === 0) return { text: raw, cssom: false };
    let fromText = -1;
    try {
      const probe = new CSSStyleSheet();
      probe.replaceSync(raw);
      fromText = probe.cssRules.length;
    } catch (e) {
      fromText = -1;
    }
    if (raw.trim() === '' || (fromText >= 0 && rules.length > fromText)) {
      return { text: rulesText(el.sheet) || raw, cssom: true };
    }
    return { text: raw, cssom: false };
  }
  function addStyle(entry) {
    const text = entry.text || '';
    entry.cut = text.length > MAX_STYLE_CHARS;
    entry.text = entry.cut ? text.slice(0, MAX_STYLE_CHARS) : text;
    const i = out.styles.length;
    out.styles.push(entry);
    return pre + 's' + i;
  }
  function linkTo(doc, token, media) {
    const l = doc.createElement('link');
    l.setAttribute('rel', 'stylesheet');
    l.setAttribute('href', token);
    if (media) l.setAttribute('media', media);
    return l;
  }
  function adoptedLinks(doc, owner, where) {
    const list = [];
    let sheets = [];
    try {
      sheets = Array.from(owner.adoptedStyleSheets || []);
    } catch (e) {
      sheets = [];
    }
    for (const sheet of sheets) {
      const text = rulesText(sheet);
      if (text == null) continue;
      let media = '';
      try {
        media = sheet.media ? sheet.media.mediaText : '';
      } catch (e) {
        media = '';
      }
      list.push(linkTo(doc, addStyle({ kind: 'adopted', where: where, text: text, media: media }), media));
    }
    return list;
  }

  // ---- element description, for selectors and the outline -------------------------------
  function shortName(el) {
    if (el.id && /^[A-Za-z][\w-]{0,40}$/.test(el.id)) return '#' + el.id;
    const cls = Array.from(el.classList || []).filter((c) => /^[A-Za-z_-][\w-]{0,40}$/.test(c)).slice(0, 2);
    return cls.length ? '.' + cls.join('.') : '';
  }
  function text(el, max) {
    let t = '';
    try {
      t = el.innerText || el.textContent || '';
    } catch (e) {
      t = '';
    }
    t = t.replace(/\s+/g, ' ').trim();
    return t.length > max ? t.slice(0, max) : t;
  }

  // ---- the inert copy -------------------------------------------------------------------
  const doc = document.implementation.createHTMLDocument('');
  // Elements that run code or load other documents, and the legacy raw-text elements whose
  // text the serialiser writes unescaped (`<xmp>` holding `</xmp><script>` would become a
  // script in the copy).
  const DROP = new Set(['script', 'noscript', 'base', 'object', 'embed', 'applet', 'frame', 'frameset', 'portal', 'fencedframe', 'param', 'xmp', 'noembed', 'noframes', 'plaintext']);
  const SKIP_ATTRS = new Set(['nonce', 'integrity', 'srcdoc', 'crossorigin', 'ping', 'http-equiv', 'formaction', 'shadowrootmode', 'shadowroot', 'shadowrootdelegatesfocus', 'shadowrootclonable', 'shadowrootserializable']);
  const URL_ATTRS = new Set(['src', 'href', 'poster', 'action', 'data', 'background', 'xlink:href', 'codebase', 'cite', 'longdesc', 'manifest', 'icon', 'lowsrc', 'dynsrc', 'to', 'from', 'values', 'by']);

  function recordScript(el) {
    if (out.scripts.length >= MAX_SCRIPTS) return;
    const src = el.getAttribute('src');
    const type = String(el.getAttribute('type') || '').toLowerCase().slice(0, 60);
    const inline = src ? '' : el.textContent || '';
    out.scripts.push({
      src: src ? absUrl(src) || '' : '',
      type: type,
      inline: inline.length > MAX_INLINE_SCRIPT ? inline.slice(0, MAX_INLINE_SCRIPT) : inline,
      length: inline.length,
    });
  }

  function setAttr(c, a, value) {
    try {
      if (a.namespaceURI) c.setAttributeNS(a.namespaceURI, a.name, value);
      else c.setAttribute(a.name, value);
    } catch (e) {
      /* a name the copy cannot hold */
    }
  }

  function copyAttributes(el, c, lower, isHtml) {
    for (const a of Array.from(el.attributes)) {
      const name = a.name;
      const lname = name.toLowerCase();
      const v = a.value;
      if (lname.startsWith('on')) continue;
      if (SKIP_ATTRS.has(lname)) continue;
      // An address that runs code is dropped from any attribute, not only the known ones.
      if (isScriptUrl(v)) continue;
      let nv = v;
      if (lname === 'style') {
        nv = rewriteInlineCss(v);
      } else if (isHtml) {
        if (lname === 'action' && lower === 'form') continue;
        if (lname === 'target' && lower === 'form') continue;
        if (lname === 'src' && (lower === 'img' || lower === 'source' || lower === 'input' || lower === 'track' || lower === 'audio' || lower === 'video')) {
          let pick = v;
          if ((lower === 'img' || lower === 'video' || lower === 'audio') && el.currentSrc) pick = el.currentSrc;
          if (/^\s*blob:/i.test(pick)) {
            nv = v;
          } else {
            const t = urlToken(pick);
            if (t) nv = t;
            else if (!/^\s*data:/i.test(v)) {
              const ab = absUrl(v);
              if (ab) nv = ab;
            }
          }
        } else if (lname === 'srcset' && (lower === 'img' || lower === 'source')) {
          const t = srcsetToken(v);
          if (!t) continue;
          nv = t;
        } else if (lname === 'poster' && lower === 'video') {
          const t = urlToken(v);
          if (t) nv = t;
        } else if (lname === 'background') {
          const t = urlToken(v);
          if (t) nv = t;
        } else if (lname === 'href' && (lower === 'a' || lower === 'area')) {
          if (!/^\s*#/.test(v)) {
            const ab = absUrl(v);
            if (ab && /^(https?|mailto|tel):/i.test(ab)) nv = ab;
            else continue;
          }
        }
      } else {
        // SVG and MathML: addresses of pictures and of shapes kept in other files.
        if (lname === 'href' || lname === 'xlink:href') {
          if (/^\s*#/.test(v)) {
            nv = v;
          } else if (lower === 'a') {
            const ab = absUrl(v);
            if (ab && /^https?:/i.test(ab)) nv = ab;
            else continue;
          } else if (!/^\s*data:/i.test(v)) {
            const t = urlToken(v, undefined, lower === 'use' ? 'v' : 'u');
            if (t) nv = t;
            else continue;
          }
        }
      }
      setAttr(c, a, nv);
    }
  }

  // A lazily loaded picture whose real address is still only in a data attribute: the copy
  // shows it when that address was saved.
  function promoteLazy(el, c) {
    const cur = el.getAttribute('src') || '';
    const placeholder = !cur.trim() || /^\s*data:/i.test(cur);
    if (!placeholder) return;
    for (const name of ['data-src', 'data-lazy-src', 'data-original']) {
      const v = el.getAttribute(name);
      if (v) {
        const t = urlToken(v);
        if (t) c.setAttribute('src', t);
        break;
      }
    }
    const ss = el.getAttribute('data-srcset');
    if (ss && !el.getAttribute('srcset')) {
      const t = srcsetToken(ss);
      if (t) c.setAttribute('srcset', t);
    }
  }

  function cloneChildren(src, dst, depth) {
    for (let n = src.firstChild; n; n = n.nextSibling) {
      if (out.nodes >= MAX_NODES) {
        out.cut = true;
        return;
      }
      let c = null;
      if (n.nodeType === 1) c = cloneElement(n, depth);
      else if (n.nodeType === 3 || n.nodeType === 4) c = doc.createTextNode(n.data);
      if (c) dst.appendChild(c);
    }
  }

  function cloneElement(el, depth) {
    out.nodes++;
    if (depth > MAX_DEPTH) {
      out.cut = true;
      return null;
    }
    const ns = el.namespaceURI;
    const isHtml = ns === HTML_NS || ns == null;
    const tag = el.localName;
    const lower = tag.toLowerCase();
    if (lower === 'script') {
      recordScript(el);
      return null;
    }
    if (isHtml && DROP.has(lower)) return null;
    if (isHtml && lower === 'style') {
      if (el.disabled) return null;
      const st = styleElementText(el);
      const media = el.getAttribute('media') || '';
      return linkTo(doc, addStyle({ kind: 'inline', text: st.text, cssom: st.cssom, media: media }), media);
    }
    if (isHtml && lower === 'link') {
      const rel = String(el.getAttribute('rel') || '').toLowerCase().split(/\s+/);
      if (rel.includes('stylesheet') && !rel.includes('alternate')) {
        if (el.disabled) return null;
        const href = absUrl(el.getAttribute('href'));
        if (!href || !isHttp(href)) return null;
        const media = el.getAttribute('media') || '';
        const cssom = rulesText(el.sheet);
        return linkTo(doc, addStyle({ kind: 'link', href: href.split('#')[0], text: cssom || '', media: media }), media);
      }
      if (rel.some((r) => r === 'icon' || r === 'apple-touch-icon' || r === 'mask-icon')) {
        const t = urlToken(el.getAttribute('href'));
        if (!t) return null;
        const l = doc.createElement('link');
        l.setAttribute('rel', rel.filter((r) => /^[a-z-]{1,20}$/.test(r)).join(' '));
        l.setAttribute('href', t);
        for (const k of ['sizes', 'type', 'color', 'media']) {
          const v = el.getAttribute(k);
          if (v) l.setAttribute(k, v);
        }
        return l;
      }
      return null;
    }
    if (isHtml && lower === 'meta') {
      if (el.hasAttribute('http-equiv') || el.hasAttribute('charset')) return null;
      const name = String(el.getAttribute('name') || '').toLowerCase();
      if (!['viewport', 'theme-color', 'color-scheme', 'description'].includes(name)) return null;
      const m = doc.createElement('meta');
      m.setAttribute('name', name);
      m.setAttribute('content', String(el.getAttribute('content') || ''));
      if (el.getAttribute('media')) m.setAttribute('media', el.getAttribute('media'));
      return m;
    }
    if (isHtml && lower === 'iframe') {
      const src = absUrl(el.getAttribute('src')) || '';
      if (out.frames.length < 50) {
        out.frames.push({ src: src.slice(0, 600), w: el.clientWidth || 0, h: el.clientHeight || 0 });
      }
      const f = doc.createElement('iframe');
      for (const k of ['id', 'class', 'width', 'height', 'title', 'name', 'loading']) {
        const v = el.getAttribute(k);
        if (v != null) f.setAttribute(k, v);
      }
      const st = el.getAttribute('style');
      if (st) f.setAttribute('style', rewriteInlineCss(st));
      if (src && isHttp(src)) f.setAttribute('data-captured-src', src);
      return f;
    }
    if (!isHtml && /^(animate|set|animatetransform|animatemotion)$/.test(lower)) {
      const an = String(el.getAttribute('attributeName') || '').toLowerCase();
      if (an === 'href' || an === 'xlink:href') return null;
    }
    let c;
    try {
      c = isHtml ? doc.createElement(tag) : doc.createElementNS(ns, el.prefix ? el.prefix + ':' + tag : tag);
    } catch (e) {
      return null;
    }
    copyAttributes(el, c, lower, isHtml);
    if (isHtml && lower === 'img') promoteLazy(el, c);
    if (isHtml && (lower === 'video' || lower === 'audio') && out.media.length < 50) {
      const cur = String(el.currentSrc || '');
      out.media.push({ kind: lower, src: cur.slice(0, 600), poster: String(el.getAttribute('poster') || '').slice(0, 600), stream: /^blob:/i.test(cur) });
    }
    if (isHtml && lower === 'canvas') {
      const r = el.getBoundingClientRect();
      if (out.canvases.length < MAX_CANVASES && r.width >= 32 && r.height >= 32) {
        const i = out.canvases.length;
        out.canvases.push({
          x: Math.round(r.left + window.scrollX),
          y: Math.round(r.top + window.scrollY),
          w: Math.round(r.width),
          h: Math.round(r.height),
          engine: String(el.getAttribute('data-engine') || '').slice(0, 60),
        });
        const st = c.getAttribute('style') || '';
        c.setAttribute('style', (st ? st.replace(/;?\s*$/, ';') : '') + 'background:url("' + pre + 'c' + i + '") center / 100% 100% no-repeat');
      }
    }
    if (!isHtml && lower === 'style') {
      c.textContent = rewriteInlineCss(el.textContent || '');
      return c;
    }
    // An open shadow root becomes a declarative one, so the copy shows it without scripts.
    let root = null;
    try {
      root = el.shadowRoot;
    } catch (e) {
      root = null;
    }
    if (root) {
      out.shadowRoots++;
      const t = doc.createElement('template');
      t.setAttribute('shadowrootmode', 'open');
      cloneChildren(root, t.content, depth + 1);
      for (const l of adoptedLinks(doc, root, 'shadow root of ' + lower + shortName(el))) t.content.appendChild(l);
      c.appendChild(t);
    }
    if (isHtml && lower === 'template') cloneChildren(el.content, c.content, depth + 1);
    else cloneChildren(el, c, depth + 1);
    return c;
  }

  const rootClone = cloneElement(document.documentElement, 0);
  if (rootClone) {
    doc.replaceChild(rootClone, doc.documentElement);
    let head = null;
    let body = null;
    for (const ch of Array.from(rootClone.children)) {
      if (ch.localName === 'head' && !head) head = ch;
      if (ch.localName === 'body' && !body) body = ch;
    }
    if (!head) head = doc.createElement('head');
    // The head comes first, so the policy below is in force for everything after it.
    rootClone.insertBefore(head, rootClone.firstChild);
    if (!body) {
      body = doc.createElement('body');
      rootClone.appendChild(body);
    }
    for (const l of adoptedLinks(doc, document, 'document')) body.appendChild(l);
    const csp = doc.createElement('meta');
    csp.setAttribute('http-equiv', 'Content-Security-Policy');
    csp.setAttribute('content', String(opts.csp || "script-src 'none'"));
    const charset = doc.createElement('meta');
    charset.setAttribute('charset', 'utf-8');
    head.insertBefore(csp, head.firstChild);
    head.insertBefore(charset, csp);
    head.insertBefore(doc.createComment(' Saved by harnessUI for studying the design. The page\'s scripts are switched off in this copy. '), csp.nextSibling);
    let doctype = '';
    const dt = document.doctype;
    if (dt) {
      const safe = (s) => /^[A-Za-z0-9 .:\/_+()-]*$/.test(s);
      doctype = '<!DOCTYPE html>';
      if (dt.publicId && safe(dt.publicId) && safe(dt.systemId || '')) {
        doctype = '<!DOCTYPE html PUBLIC "' + dt.publicId + '"' + (dt.systemId ? ' "' + dt.systemId + '"' : '') + '>';
      }
    }
    out.html = (doctype ? doctype + '\n' : '') + rootClone.outerHTML;
  }

  // ---- what the report needs --------------------------------------------------------------
  function hex(c) {
    const m = String(c).match(/^rgba?\(\s*([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)(?:\s*[,/]\s*([\d.]+%?))?\s*\)$/);
    if (!m) return String(c).slice(0, 60);
    let a = m[4] == null ? 1 : m[4].endsWith('%') ? parseFloat(m[4]) / 100 : parseFloat(m[4]);
    if (a === 0) return null;
    const h = (n) => Math.max(0, Math.min(255, Math.round(parseFloat(n)))).toString(16).padStart(2, '0');
    return '#' + h(m[1]) + h(m[2]) + h(m[3]) + (a < 1 ? h(a * 255) : '');
  }
  function inc(map, key, extra) {
    if (key == null || key === '') return;
    const e = map.get(key) || { n: 0, tags: new Set() };
    e.n++;
    if (extra && e.tags.size < 6) e.tags.add(extra);
    map.set(key, e);
  }
  function top(map, n) {
    return Array.from(map.entries())
      .sort((a, b) => b[1].n - a[1].n)
      .slice(0, n)
      .map(([k, v]) => [k, v.n, Array.from(v.tags)]);
  }
  // Splits a computed list at its top-level commas ("ease, cubic-bezier(0, 0, 1, 1)").
  function splitList(s) {
    const out = [];
    let depth = 0, start = 0;
    for (let i = 0; i < s.length; i++) {
      const c = s[i];
      if (c === '(') depth++;
      else if (c === ')') depth = Math.max(0, depth - 1);
      else if (c === ',' && depth === 0) {
        out.push(s.slice(start, i).trim());
        start = i + 1;
      }
    }
    out.push(s.slice(start).trim());
    return out.filter((x) => x);
  }
  // "opacity 0.3s ease, transform 0.3s ease 0.1s": each property with its own timing (the
  // shorter lists repeat, as in CSS).
  function transitionText(cs) {
    const props = splitList(String(cs.transitionProperty || 'all'));
    const durs = splitList(String(cs.transitionDuration || '0s'));
    const fns = splitList(String(cs.transitionTimingFunction || 'ease'));
    const dels = splitList(String(cs.transitionDelay || '0s'));
    if (!props.length || !durs.length || (props.length === 1 && props[0] === 'none')) return '';
    const parts = [];
    for (let i = 0; i < props.length && i < 8; i++) {
      const d = durs[i % durs.length];
      if (!(parseFloat(d) > 0)) continue;
      const f = fns.length ? fns[i % fns.length] : 'ease';
      const del = dels.length ? dels[i % dels.length] : '0s';
      parts.push(props[i] + ' ' + d + ' ' + f + (parseFloat(del) ? ' ' + del : ''));
    }
    return parts.join(', ');
  }
  function stats() {
    const colors = new Map(), bgs = new Map(), borders = new Map(), fonts = new Map(), sizes = new Map();
    const spacing = new Map(), radii = new Map(), shadows = new Map(), widths = new Map(), transitions = new Map();
    const SKIP = new Set(['script', 'style', 'link', 'meta', 'head', 'title', 'noscript', 'template', 'br']);
    let counted = 0;
    const all = document.body ? document.body.getElementsByTagName('*') : [];
    // Hidden elements are skipped without counting, so the walk itself is bounded too.
    for (let i = 0; i < all.length && i < MAX_STAT * 10 && counted < MAX_STAT; i++) {
      const el = all[i];
      if (SKIP.has(el.localName)) continue;
      const r = el.getBoundingClientRect();
      if (r.width === 0 && r.height === 0) continue;
      const cs = getComputedStyle(el);
      if (cs.display === 'none' || cs.visibility === 'hidden') continue;
      counted++;
      let hasText = false;
      for (let n = el.firstChild; n; n = n.nextSibling) {
        if (n.nodeType === 3 && n.data.trim()) {
          hasText = true;
          break;
        }
      }
      if (hasText) {
        inc(colors, hex(cs.color));
        const fam = String(cs.fontFamily || '').split(',')[0].replace(/["']/g, '').trim();
        inc(fonts, fam + ' ' + cs.fontWeight + (cs.fontStyle === 'italic' ? ' italic' : ''));
        inc(sizes, cs.fontSize + ' / ' + cs.lineHeight + ' / ' + cs.fontWeight + (cs.letterSpacing !== 'normal' ? ' / ' + cs.letterSpacing : '') + (cs.textTransform !== 'none' ? ' / ' + cs.textTransform : ''), el.localName);
      }
      const bg = hex(cs.backgroundColor);
      if (bg) inc(bgs, bg);
      if (parseFloat(cs.borderTopWidth) > 0 && cs.borderTopStyle !== 'none') inc(borders, hex(cs.borderTopColor));
      for (const p of ['paddingTop', 'paddingRight', 'paddingBottom', 'paddingLeft', 'marginTop', 'marginBottom', 'rowGap', 'columnGap']) {
        const v = cs[p];
        if (v && v !== '0px' && v !== 'normal' && /px$/.test(v)) inc(spacing, v);
      }
      if (cs.borderTopLeftRadius && cs.borderTopLeftRadius !== '0px') inc(radii, cs.borderTopLeftRadius);
      if (cs.boxShadow && cs.boxShadow !== 'none') inc(shadows, cs.boxShadow.slice(0, 120));
      if (cs.maxWidth && /px$/.test(cs.maxWidth) && parseFloat(cs.maxWidth) >= 320) inc(widths, cs.maxWidth);
      const tt = transitionText(cs);
      if (tt) inc(transitions, tt.slice(0, 200), el.localName + shortName(el));
    }
    return {
      counted: counted,
      colors: top(colors, 12),
      backgrounds: top(bgs, 12),
      borders: top(borders, 6),
      fonts: top(fonts, 12),
      sizes: top(sizes, 16),
      spacing: top(spacing, 14),
      radii: top(radii, 8),
      shadows: top(shadows, 5),
      widths: top(widths, 6),
      transitions: top(transitions, 10),
    };
  }
  function outline() {
    const SEL = 'header, nav, main, section, article, aside, footer, form, [role="banner"], [role="navigation"], [role="main"], [role="contentinfo"], [role="region"], [role="search"]';
    const items = [];
    let all = [];
    try {
      all = Array.from(document.querySelectorAll(SEL));
    } catch (e) {
      all = [];
    }
    const describe = (el, depth) => {
      const r = el.getBoundingClientRect();
      return {
        tag: el.localName,
        role: String(el.getAttribute('role') || '').slice(0, 30),
        depth: depth,
        name: shortName(el),
        label: String(el.getAttribute('aria-label') || '').slice(0, 80),
        top: Math.round(r.top + window.scrollY),
        height: Math.round(r.height),
        headings: Array.from(el.querySelectorAll('h1, h2, h3')).slice(0, 3).map((h) => ({ level: h.localName, text: text(h, 100) })),
        links: el.querySelectorAll('a[href]').length,
        buttons: el.querySelectorAll('button, [role="button"], input[type="submit"], input[type="button"]').length,
        images: el.querySelectorAll('img, picture, svg, video, canvas').length,
        fields: el.querySelectorAll('input:not([type="hidden"]), textarea, select').length,
        password: !!el.querySelector('input[type="password"]'),
      };
    };
    for (const el of all.slice(0, 2000)) {
      let d = 0;
      for (let p = el.parentElement; p; p = p.parentElement) {
        try {
          if (p.matches(SEL)) d++;
        } catch (e) {
          /* ignore */
        }
      }
      if (d > 2) continue;
      const r = el.getBoundingClientRect();
      if (r.height < 8 && r.width < 8) continue;
      items.push(describe(el, d));
      if (items.length >= 40) break;
    }
    if (items.length < 2 && document.body) {
      for (const el of Array.from(document.body.children)) {
        const r = el.getBoundingClientRect();
        if (r.height < 80) continue;
        items.push(describe(el, 0));
        if (items.length >= 25) break;
      }
    }
    return items;
  }
  const q = (s) => {
    try {
      return document.querySelectorAll(s).length;
    } catch (e) {
      return 0;
    }
  };
  try {
    out.stats = stats();
  } catch (e) {
    out.stats = {};
  }
  try {
    out.outline = outline();
  } catch (e) {
    out.outline = [];
  }
  try {
    out.headings = Array.from(document.querySelectorAll('h1, h2, h3')).slice(0, 30).map((h) => ({ level: h.localName, text: text(h, 100) }));
  } catch (e) {
    out.headings = [];
  }
  out.signIn = q('input[type="password"]') > 0;
  const html = document.documentElement;
  out.markers = {
    aos: q('[data-aos]'),
    lenis: html.classList.contains('lenis') || q('.lenis, .lenis-smooth') > 0 ? 1 : 0,
    locomotive: q('[data-scroll-container], .has-scroll-smooth, .c-scrollbar'),
    pinSpacer: q('.pin-spacer'),
    framer: q('[data-framer-name], [data-framer-component-type], [data-framer-appear-id]'),
    projection: q('[data-projection-id]'),
    lottie: q('lottie-player, dotlottie-player, dotlottie-wc'),
    threeCanvas: q('canvas[data-engine*="three"]'),
    spline: q('spline-viewer'),
    barba: q('[data-barba]'),
    swiper: q('.swiper, .swiper-container'),
    splide: q('.splide'),
    slick: q('.slick-slider'),
    webflow: html.hasAttribute('data-wf-page') ? 1 : 0,
    webflowInteractions: q('[data-w-id]'),
    modelViewer: q('model-viewer'),
    rellax: q('.rellax, [data-rellax-speed]'),
    speed: q('[data-speed], [data-scroll-speed]'),
    canvases: q('canvas'),
    videos: q('video'),
    svgSmil: q('animate, animateTransform, animateMotion, set'),
  };
  out.stats.scrollHeight = Math.max(html.scrollHeight, document.body ? document.body.scrollHeight : 0);
  out.stats.viewport = [window.innerWidth, window.innerHeight];
  return out;
}

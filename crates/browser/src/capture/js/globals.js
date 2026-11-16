(() => {
  // browser_capture_site: which animation libraries the page's own scripts put on `window`.
  // Evaluated in the page's main world, so it reads own data properties only and never calls
  // anything of the page's; what it returns is page data and may be untrue.
  const names = ['gsap', 'ScrollTrigger', 'TweenMax', 'TweenLite', 'Lenis', 'lenis', 'lottie', 'bodymovin', 'AOS', 'THREE', '__THREE__', 'anime', 'Swiper', 'LocomotiveScroll', 'barba', 'ScrollMagic', 'Splide', 'rive', 'Webflow', 'Motion', 'Velocity', 'PIXI', 'p5', 'Rellax', 'Flickity', 'Glide', 'Swup', 'Typed', 'Granim', 'Vivus', 'Matter'];
  const own = Object.getOwnPropertyDescriptor;
  const r = {};
  for (const n of names) {
    try {
      const d = own(window, n);
      if (!d || !('value' in d) || d.value == null) continue;
      const v = d.value;
      if (typeof v === 'string' || typeof v === 'number') {
        r[n] = String(v).slice(0, 24);
        continue;
      }
      let version = '';
      for (const k of ['version', 'VERSION', 'REVISION']) {
        const vd = typeof v === 'object' || typeof v === 'function' ? own(v, k) : null;
        if (vd && 'value' in vd && (typeof vd.value === 'string' || typeof vd.value === 'number')) {
          version = String(vd.value).slice(0, 24);
          break;
        }
      }
      r[n] = version || true;
    } catch (e) {
      /* a property that cannot be read */
    }
  }
  return r;
})()

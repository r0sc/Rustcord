// The scroll stage: the window rises out of its tilt, sticks and changes screens. It only reads scrollY (no scroll-jacking).

// p: 0 (tilted in the hero) .. 1 (flat, stuck); i: the screen to show. Pure, so node tests it.
export function stageAt(y, start, step, n) {
  const p = start > 0 ? Math.min(1, Math.max(0, y / start)) : 1;
  const i = Math.min(n - 1, Math.max(0, Math.floor((y - start) / step)));
  return { p, i };
}

const STEP = 0.64, DWELL = 340; // scroll per screen in viewport heights (matches 64vh in styles.css); least ms between two swaps

export function initStage(tour, view) {
  const d = document.documentElement, win = tour.querySelector('.win'), stack = tour.querySelector('.stack');
  const shots = [...tour.querySelectorAll('.shot')], dots = [...tour.querySelectorAll('.pills button')];
  let start = 1, step = 1, cur = -1, raf = 0, on = false, swapAt = -1e9;
  const measure = () => {
    on = d.classList.contains('stage');
    // the rise takes at least 55vh of scroll
    start = Math.max(1, innerHeight * 0.55, tour.getBoundingClientRect().top + scrollY - parseFloat(getComputedStyle(stack).top));
    step = innerHeight * STEP;
  };
  const show = i => {
    if (i === cur) return;
    shots.forEach((s, k) => { s.classList.toggle('is-on', k === i); s.classList.toggle('is-prev', k === cur); });
    dots.forEach((b, k) => b.setAttribute('aria-current', String(k === i)));
    cur = i; win._i = i;
    win.dispatchEvent(new Event('screen')); // fx.js retints the room light
  };
  const frame = () => {
    raf = 0;
    const t0 = performance.now();
    if (!on) { win._p = 1; stack.style.removeProperty('--p'); show(0); return; }
    const s = stageAt(scrollY, start, step, shots.length);
    if (s.p !== win._p) { win._p = s.p; stack.style.setProperty('--p', s.p.toFixed(4)); } // the window, captions and dots read it
    // a fast scroll can't flash screens: at most one swap per DWELL ms
    if (s.i !== cur && t0 - swapAt < DWELL) kick(); else if (s.i !== cur) { show(s.i); swapAt = t0; }
    if (window.__fx) { window.__fx.stage += performance.now() - t0; window.__fx.stageN++; }
  };
  const kick = () => { if (!raf) raf = requestAnimationFrame(frame); };
  addEventListener('scroll', kick, { passive: true });
  addEventListener('resize', () => { measure(); kick(); });
  matchMedia('(prefers-reduced-motion: no-preference)').addEventListener('change', () => { measure(); kick(); }); // the head script toggles .stage without a resize
  new ResizeObserver(() => { measure(); kick(); }).observe(document.querySelector('.hero')); // the font swap moves the window
  dots.forEach((b, k) => b.addEventListener('click', () =>
    scrollTo({ top: start + k * step + 2, behavior: d.classList.contains('motion') ? 'smooth' : 'auto' })));
  measure();
  const v = dots.findIndex(b => b.dataset.view === view);
  const go = () => { measure(); on ? scrollTo(0, start + v * step + 2) : shots[v].scrollIntoView(); };
  if (v > 0) { go(); document.fonts.ready.then(go); }
  frame();
  tour.querySelector('.pills').hidden = false;
}

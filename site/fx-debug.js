// Debug hooks for fx.js, loaded only with ?fx=debug or ?cursor=X,Y.
let t0 = 0, n0 = 0;
export function report(stat, now) {
  const t = `fx ${stat.avg.toFixed(2)} ms avg, ${stat.max.toFixed(2)} max; stage ${(stat.stage / Math.max(1, stat.stageN)).toFixed(2)} ms; ${Math.round((stat.drawn - n0) * 1000 / Math.max(1, now - t0))} drawn/s; ${stat.w}x${stat.h}; ${stat.mode}; ${stat.gpu}`;
  t0 = now; n0 = stat.drawn;
  let el = document.getElementById('fx-debug');
  if (!el) { el = document.createElement('output'); el.id = 'fx-debug'; document.body.append(el); }
  el.textContent = t; console.log(t);
}
// an arrow for screenshots
export const arrow = (x, y) => document.body.insertAdjacentHTML('beforeend', `<svg aria-hidden="true" width="18" height="26" viewBox="0 0 18 26" style="position:fixed;z-index:50;pointer-events:none;left:${x - 1}px;top:${y - 1}px"><path d="M1.5 1.5v19.5l5-4.8 3.6 7.8 3.4-1.6-3.5-7.6h7Z" fill="#fff" stroke="#000" stroke-width="1.2" stroke-linejoin="round"/></svg>`);

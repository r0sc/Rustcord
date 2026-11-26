// Effects, loaded after 'load'; the page is complete without them. Debug: ?cursor=X,Y, ?t=<ms>, ?ring=0..1, ?fx=debug.

// 'off': no canvas; 'still': one frame; 'live': the loop
export const gate = ({ fx, webgl, motion, fine, software, hook }) =>
  !fx || !webgl ? 'off' : motion && fine && (!software || hook) ? 'live' : 'still';
export const renderScale = (w, h, dpr) => Math.min(Math.min(dpr || 1, 1) * 0.75, Math.sqrt(1.2e6 / Math.max(1, w * h)));
// exponential follow; dt is clamped so a 0 or NaN step can't break it
export const follow = (v, to, dt, tau) => v + (to - v) * (1 - Math.exp(-Math.min(64, Math.max(1, dt || 1)) / tau));
export const ringAt = k => { k = Math.min(1, Math.max(0, k)); return { r: 1 - (1 - k) ** 2, a: (1 - k) ** 1.5 }; };
export const isSoftware = gpu => /SwiftShader|Basic Render|llvmpipe|softpipe|Software/i.test(gpu || '');

const VS = `#version 300 es
void main(){ vec2 p = vec2((gl_VertexID << 1) & 2, gl_VertexID & 2); gl_Position = vec4(p * 2. - 1., 0., 1.); }`;
// Simplex noise: Ashima Arts / Stefan Gustavson (MIT).
const FS = `#version 300 es
precision highp float;
uniform vec2 R; uniform float T, S, G; uniform vec3 M, L, C; uniform vec4 P, B; uniform float K;
out vec4 o;
vec3 pm(vec3 x){ return mod((x * 34. + 1.) * x, 289.); }
float sn(vec2 v){
const vec4 C = vec4(.211324865, .366025404, -.577350269, .024390244);
vec2 i = floor(v + dot(v, C.yy)), x0 = v - i + dot(i, C.xx), i1 = x0.x > x0.y ? vec2(1, 0) : vec2(0, 1);
vec4 x12 = x0.xyxy + C.xxzz; x12.xy -= i1; i = mod(i, 289.);
vec3 p = pm(pm(i.y + vec3(0, i1.y, 1)) + i.x + vec3(0, i1.x, 1));
vec3 m = max(.5 - vec3(dot(x0, x0), dot(x12.xy, x12.xy), dot(x12.zw, x12.zw)), 0.); m *= m; m *= m;
vec3 x = 2. * fract(p * C.www) - 1., h = abs(x) - .5, a = x - floor(x + .5);
m *= 1.79284291 - .85373472 * (a * a + h * h);
return 130. * dot(m, vec3(a.x * x0.x + h.x * x0.y, a.yz * x12.xz + h.yz * x12.yw));
}
void main(){
vec2 p = gl_FragCoord.xy; float k = 1. / R.y, t = T * .04;
vec2 u = vec2(p.x, p.y - S) * k;
float n = .65 * sn(u * 1.1 + vec2(t, -.6 * t)) + .35 * sn(u * 2.4 + vec2(4. - .7 * t, 1.1 * t));
vec2 dm = (p - M.xy) * k; float r2 = dot(dm, dm);
n += M.z * mix(.5, .9, G) * exp(-r2 * mix(16., 42., G)); float rp = (length(p - P.xy) - P.z) * k, wave = P.w * exp(-rp * rp * 300.);
n += wave * .2;
float f = n * 6., a = abs(fract(f + .5) - .5) / max(fwidth(f), 1e-4);
float line = 1. - smoothstep(.35, 1.15, a);
vec2 dl = (p - L.xy) * k; float l = L.z * exp(-dot(dl, dl) * 2.4);
vec2 bq = max(abs(p - B.xy) - B.zw, 0.); float halo = K * exp(-length(bq) * k * mix(9., 18., smoothstep(0., .06, (B.y - B.w - p.y) * k))); l = max(l, halo);
float c = M.z * exp(-r2 * mix(10., 20., G));
vec3 col = vec3(.031, .027, .043) + C * (.11 * L.z * exp(-dot(dl, dl) * 1.1) + .05 * wave + .16 * halo);
col = mix(col, mix(vec3(.745, .404, .941), C, min(l, 1.) * .6), min(line * (.045 + .2 * l + .5 * c * (1. + .4 * G)), .36) + line * .5 * wave); // .36 keeps muted text at 4.5:1
col += (fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453) - .5) / 255.;
o = vec4(col, 1.);
}`;
// room light per screen: purple, live pink for Voice, bright for Lilac
const ROOM = [[.643, .169, .918], [.941, .384, .753], [.70, .22, .80], [.50, .20, .95], [1.3, 1.15, 1.45]];

export function start(q) {
  const d = document.documentElement, fine = matchMedia('(hover: hover) and (pointer: fine)');
  const stat = window.__fx = { frames: 0, drawn: 0, ms: 0, max: 0, avg: 0, stage: 0, stageN: 0, mode: 'off', gpu: '', w: 0, h: 0 };
  const fake = (q.get('cursor') || '').split(',').map(Number), hasFake = fake.length === 2 && fake.every(Number.isFinite);
  const tFix = q.has('t') ? +q.get('t') / 1000 : null, ringFix = q.has('ring') ? +q.get('ring') : null, debug = q.get('fx') === 'debug';
  const hook = hasFake || tFix !== null || ringFix !== null;
  let dbgm; if (debug || hasFake) import('./fx-debug.js').then(m => { dbgm = m; if (hasFake) m.arrow(fake[0], fake[1]); if (debug) m.report(stat, performance.now()); });
  if (hook) d.classList.add('fx-shot');
  const win = document.querySelector('.win'), hero = document.querySelector('.hero'), lit = [...document.querySelectorAll('.lit')];

  const cv = document.createElement('canvas');
  cv.className = 'field'; cv.setAttribute('aria-hidden', 'true');
  const gl = cv.getContext('webgl2', { alpha: false, antialias: false, depth: false, stencil: false, powerPreference: 'low-power', preserveDrawingBuffer: hook });
  const prog = gl && (() => {
    const p = gl.createProgram();
    for (const [type, src] of [[gl.VERTEX_SHADER, VS], [gl.FRAGMENT_SHADER, FS]]) {
      const sh = gl.createShader(type); gl.shaderSource(sh, src); gl.compileShader(sh); gl.attachShader(p, sh);
    }
    gl.linkProgram(p);
    return gl.getProgramParameter(p, gl.LINK_STATUS) ? p : null;
  })();
  const dbg = prog && gl.getExtension('WEBGL_debug_renderer_info');
  stat.gpu = dbg ? gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : '';
  const mode = stat.mode = gate({ fx: true, webgl: !!prog, motion: d.classList.contains('motion'), fine: hasFake || fine.matches, software: isSoftware(stat.gpu), hook });
  if (mode === 'off') { d.classList.add('nolight'); return; }

  gl.useProgram(prog);
  const U = Object.fromEntries('RTSGMLPCBK'.split('').map(n => [n, gl.getUniformLocation(prog, n)]));
  let live = mode === 'live', W = 0, H = 0, s = 1, raf = 0, last = 0, time = tFix ?? 0, odd = false, dirty = true, dead = false, seen = 0;
  let px = -1e4, py = -1e4, cx = -1e4, cy = -1e4, ms = 0, msT = 0, g = 0, bx = 0, by = 0, lx = null, ly = 0, pu = null, lastRing = -1e9;
  let lz = hook || !live ? (hook ? 1 : 0.8) : 0, rampT = null, opening = live && !hook, room = ROOM[0].slice();
  if (hasFake) { px = cx = fake[0]; py = cy = fake[1]; ms = msT = 1; }
  if (ringFix !== null && live) pu = { x: 0, y: 0, t: ringFix, dur: Infinity, reach: 0, fix: true };

  const size = () => {
    s = renderScale(d.clientWidth, innerHeight, devicePixelRatio);
    W = d.clientWidth; H = innerHeight;
    cv.width = Math.round(W * s); cv.height = Math.round(H * s);
    gl.viewport(0, 0, cv.width, cv.height);
    stat.w = cv.width; stat.h = cv.height;
  };
  const ready = () => !d.classList.contains('stage') || ((win.classList.contains('go') || d.classList.contains('go')) && !win.getAnimations().some(a => a.playState === 'running'));
  const put = (el, k, v) => { if (el[k] !== v) { el[k] = v; k[0] === '-' ? el.style.setProperty(k, v) : el.style[k] = v; } };
  const edge = (el, x, y, a) => { put(el._edge, 'transform', `translate(${x.toFixed(1)}px, ${y.toFixed(1)}px)`); put(el._edge, 'opacity', a.toFixed(2)); };

  const step = (now, dt) => {
    const t0 = performance.now();
    // reads first, then writes
    const stage = live && d.classList.contains('stage'), wr = win.getBoundingClientRect(), br = lit[lit.length - 1].getBoundingClientRect(), y = scrollY;
    let near = null, nd = 1e9;
    if (live && ms + msT > 0.01) for (const b of lit) {
      const r = b.getBoundingClientRect(); if (!r.width) continue;
      const e = Math.hypot(Math.max(r.left - cx, 0, cx - r.right), Math.max(r.top - cy, 0, cy - r.bottom));
      if (e < nd) { nd = e; near = [b, r]; }
    }
    if (opening && ready()) {
      opening = false;
      if (y > hero.offsetHeight / 2) lz = 1; // opened scrolled down: no ring
      else { rampT = now; pu = { x: wr.left + wr.width / 2, y: wr.top + wr.height * 0.3, t: 0, dur: 1400, reach: Math.hypot(Math.max(wr.left + wr.width / 2, W - wr.left - wr.width / 2), Math.max(wr.top + wr.height * .3, H - wr.top - wr.height * .3)) }; }
    }
    if (pu && pu.fix) { pu.x = wr.left + wr.width / 2; pu.y = wr.top + wr.height * 0.3; pu.reach = Math.hypot(Math.max(pu.x, W - pu.x), Math.max(pu.y, H - pu.y)); }
    cx = follow(cx, px, dt, 70); cy = follow(cy, py, dt, 70); ms = follow(ms, msT, dt, 140);
    const gT = near ? Math.max(0, 1 - nd / 110) : 0; g = follow(g, gT, dt, 70);
    if (near) { const kb = g < 0.02 ? 1 : follow(0, 1, dt, 70); bx += (near[1].left + near[1].width / 2 - bx) * kb; by += (near[1].top + near[1].height / 2 - by) * kb; }
    const wx = cx + (bx - cx) * g * 0.75, wy = cy + (by - cy) * g * 0.75;
    // the light sits behind the window, else behind the lower Download button
    const onWin = wr.bottom > 0 && wr.top < H;
    const hr = live ? null : lit[0].getBoundingClientRect(); // still: light the hero Download button, the window is a list item
    const tx = hr ? hr.left + hr.width / 2 : onWin ? wr.left + wr.width / 2 : br.left + br.width / 2, ty = hr ? hr.top + hr.height / 2 : onWin ? wr.top + wr.height * 0.45 : br.top + br.height / 2;
    if (lx === null) { lx = tx; ly = ty; } else { lx = follow(lx, tx, dt, 70); ly = follow(ly, ty, dt, 70); }
    const ambient = live && !document.hidden && seen > 0 && tFix === null;
    if (ambient) time += dt / 1000;
    if (rampT !== null) { const k = Math.min(1, (now - rampT) / 1200); lz = 1 - (1 - k) ** 4; if (k >= 1) rampT = null; }
    if (pu && !pu.fix && (pu.t += dt / pu.dur) >= 1) pu = null;
    const want = ROOM[stage && onWin ? win._i || 0 : 0]; let dr = 0;
    for (let j = 0; j < 3; j++) { room[j] = follow(room[j], want[j], dt, 180); dr += Math.abs(want[j] - room[j]); }
    const settling = live && (dr > 0.004 || (pu && !pu.fix) || rampT !== null || opening || Math.abs(tx - lx) + Math.abs(ty - ly) > 0.3
      || Math.abs(px - cx) + Math.abs(py - cy) > 0.3 || Math.abs(msT - ms) > 0.005 || Math.abs(gT - g) > 0.005);
    odd = !odd;
    if (dirty || settling || odd || !live) { // ambient drift alone: 30 fps
      const rg = pu ? ringAt(pu.t) : null;
      gl.uniform2f(U.R, cv.width, cv.height); gl.uniform1f(U.T, time); gl.uniform1f(U.S, -y * 0.3 * s); gl.uniform1f(U.G, g);
      gl.uniform3f(U.M, wx * s, (H - wy) * s, ms); gl.uniform3f(U.L, lx * s, (H - ly) * s, lz); gl.uniform3f(U.C, room[0], room[1], room[2]);
      gl.uniform4f(U.P, rg ? pu.x * s : 0, rg ? (H - pu.y) * s : 0, rg ? rg.r * pu.reach * s : 0, rg ? rg.a : 0);
      gl.uniform4f(U.B, (wr.left + wr.width / 2) * s, (H - wr.top - wr.height / 2) * s, wr.width / 2 * s, wr.height / 2 * s);
      gl.uniform1f(U.K, stage ? lz * Math.min(1, Math.max(0, ((win._p ?? 1) - .8) * 5)) : 0);
      gl.drawArrays(gl.TRIANGLES, 0, 3); stat.drawn++;
      if (!cv.classList.contains('on')) requestAnimationFrame(() => cv.classList.add('on'));
    }
    if (stage) {
      const amp = 1 - (win._p ?? 1), nx = Math.max(-1, Math.min(1, (cx - (wr.left + wr.width / 2)) / (W / 2)));
      const ny = Math.max(-1, Math.min(1, (cy - (wr.top + wr.height / 2)) / (H / 2)));
      put(win, '--ry', (nx * 3 * amp * ms).toFixed(2) + 'deg'); put(win, '--rx', (-ny * 2 * amp * ms).toFixed(2) + 'deg');
      edge(win, cx - wr.left, cy - wr.top, ms);
    }
    for (const b of lit) if (b._edge) edge(b, near && near[0] === b ? cx - near[1].left : -999, near && near[0] === b ? cy - near[1].top : -999, near && near[0] === b ? g : 0);
    dirty = false;
    const c = performance.now() - t0;
    stat.frames++; stat.ms += c; stat.max = Math.max(stat.max, c); stat.avg = stat.ms / stat.frames;
    if (debug && stat.frames % 30 === 0) dbgm?.report(stat, now);
    return settling || ambient;
  };
  const frame = now => {
    raf = 0;
    if (dead || !live || document.hidden) return;
    const dt = last ? now - last : 16.7; last = now;
    if (step(now, dt)) raf = requestAnimationFrame(frame);
  };
  const kick = () => { dirty = true; if (!raf && live && !dead && !document.hidden) raf = requestAnimationFrame(frame); };
  const pulse = (x, y, dur, reach) => { pu = { x, y, t: 0, dur, reach }; lastRing = performance.now(); kick(); };
  const stop = () => { // reduced motion on, or the GPU context is gone
    live = false; cancelAnimationFrame(raf); raf = 0;
    win.style.setProperty('--rx', '0deg'); win.style.setProperty('--ry', '0deg');
    for (const b of [win, ...lit]) if (b._edge) b._edge.style.opacity = 0;
    pu = null; rampT = null; opening = false; lz = hook ? 1 : 0.8; px = cx; py = cy; ms = msT = g = 0;
    if (!dead) step(performance.now(), 16);
  };

  size();
  document.body.prepend(cv);
  if (live) cv.style.transition = 'none'; // the lz ramp is the switch-on
  cv.addEventListener('webglcontextlost', e => { e.preventDefault(); dead = true; stop(); cv.remove(); d.classList.add('nolight'); });
  addEventListener('resize', () => { size(); live ? kick() : step(performance.now(), 16); });
  if (live) {
    for (const el of [win, ...lit]) {
      const e = document.createElement('span'); e.className = 'edge'; e.setAttribute('aria-hidden', 'true');
      el._edge = e.appendChild(document.createElement('i')); el.append(e);
    }
    for (const b of lit) b.addEventListener('pointerdown', e => {
      if (e.pointerType === 'touch' || performance.now() - lastRing < 400) return; // max 2.5 rings/s
      const r = b.getBoundingClientRect(); pulse(r.left + r.width / 2, r.top + r.height / 2, 900, H * 0.7);
    });
    addEventListener('pointermove', e => { if (e.pointerType === 'touch' || hasFake) return; px = e.clientX; py = e.clientY; if (cx < -1e3) { cx = px; cy = py; } msT = 1; kick(); }, { passive: true });
    const away = () => { if (!hasFake) { msT = 0; kick(); } };
    d.addEventListener('pointerleave', away); addEventListener('blur', away);
    addEventListener('scroll', kick, { passive: true });
    win.addEventListener('screen', kick);
    document.addEventListener('visibilitychange', () => { if (document.hidden) { cancelAnimationFrame(raf); raf = 0; last = 0; } else { last = 0; kick(); } });
    const on = new Set(), io = new IntersectionObserver(es => { for (const e of es) on[e.isIntersecting ? 'add' : 'delete'](e.target); seen = on.size; kick(); });
    document.querySelectorAll('.hero, .tour, .dl').forEach(el => io.observe(el));
    // shortcut: reduced motion switched off mid-visit needs a reload
    matchMedia('(prefers-reduced-motion: no-preference)').addEventListener('change', e => { if (!e.matches) stop(); });
    kick();
  } else step(performance.now(), 16);
}

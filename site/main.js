import { SYSTEMS, LATEST, detectOs, fetchLatest, buttonState, mb } from './download.js';

const q = new URLSearchParams(location.search);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];

// ---- download ----
const uad = navigator.userAgentData;
const os = detectOs({ force: q.get('os'), ua: navigator.userAgent, platform: (uad && uad.platform) || navigator.platform,
  mobile: !!(uad && uad.mobile), touch: navigator.maxTouchPoints });
const ALT = { deb: ['tar', 'Other Linux: .tar.gz for any distribution'], tar: ['deb', '.deb for Debian, Ubuntu and Mint'] };
const FIRST = {
  windows: () => 'If SmartScreen warns you, select More info, then Run anyway.',
  mac: () => 'macOS blocks it once: click Open Anyway in System Settings, Privacy & Security.',
  deb: v => `Then run: sudo apt install ./rustcord_${v}_amd64.deb`,
  tar: () => 'Then unpack it and run install.sh (see Install on Linux).',
};
const set = (el, p, v) => { if (el) el[p] = v; };

function paint(rel, failed) {
  const st = buttonState(os, rel, failed), v = rel ? rel.v : '<version>', mobile = os === 'mobile';
  $$('[data-dl]').forEach(sc => {
    const btn = sc.querySelector('[data-dl-button]'), alt = sc.querySelector('[data-dl-alt]'), other = sc.querySelector('[data-dl-other]');
    const meta = sc.querySelector('[data-dl-meta]'), first = sc.querySelector('[data-dl-first]');
    btn.hidden = !!st.hidden; if (meta) meta.hidden = !!st.hidden;
    btn.querySelector('use').setAttribute('href', os === 'unknown' ? '#i-chev' : '#i-dl'); btn.querySelector('.icon').classList.toggle('icon-dl', os !== 'unknown');
    sc.querySelector('[data-dl-mobile]').hidden = !mobile;
    if (!st.hidden) { btn.href = st.href; sc.querySelector('[data-dl-label]').textContent = st.label; set(meta, 'textContent', st.meta); }
    set(sc.querySelector('[data-dl-title]'), 'textContent', mobile ? 'Rustcord is a desktop app' : (st.title || 'Get Rustcord'));
    if (other) { other.hidden = os === 'unknown'; other.textContent = mobile ? 'All systems' : 'Other systems'; }
    if (alt) {
      const a = ALT[os], f = a && rel && rel.files[a[0]];
      alt.hidden = !a; if (a) { alt.textContent = a[1]; alt.href = f ? f.url : (alt.closest('.dl-panel') ? '#systems' : '#download'); }
    }
    if (first) {
      const ok = rel && !failed && rel.files[os] && FIRST[os];
      first.hidden = !ok; if (ok) first.textContent = FIRST[os](v);
    }
  });
  $$('[data-row]').forEach(a => {
    const k = a.dataset.row, s = SYSTEMS[k], f = rel && rel.files[k];
    a.href = f ? f.url : LATEST;
    a.querySelector('[data-file]').textContent = f ? f.name : s.pattern;
    a.querySelector('[data-size]').textContent = f && f.size ? mb(f.size) : '';
    a.querySelector('[data-row-mine]').hidden = k !== os;
  });
  const mine = os in SYSTEMS ? SYSTEMS[os].notes : null;
  $$('[data-notes]').forEach(d => { if (d.dataset.notes === mine && !d.dataset.touched) d.open = true; });
  $$('[data-v]').forEach(c => { c.textContent = c.dataset.v.replace(/\{v\}/g, v); });
}
$$('[data-notes]').forEach(d => d.addEventListener('toggle', () => { d.dataset.touched = '1'; }, { once: true }));
paint(null, false);
fetchLatest(fetch, q.get('api') === 'fail').then(r => paint(r, false), () => paint(null, true));

// ---- copy buttons ----
function copy(btn, text, done) {
  const span = btn.querySelector('span'), label = btn.dataset.label || (btn.dataset.label = span.textContent);
  const show = (msg, ok) => { span.textContent = msg; btn.classList.toggle('is-done', ok); clearTimeout(btn._t);
    btn._t = setTimeout(() => { span.textContent = label; btn.classList.remove('is-done'); }, 1800); };
  (navigator.clipboard ? navigator.clipboard.writeText(text) : Promise.reject()).then(() => show(done, true), () => show('Couldn’t copy. Select the text instead.', false));
}
$$('[data-copy-link]').forEach(b => b.addEventListener('click', () => copy(b, location.origin + location.pathname, 'Link copied')));
$$('[data-copy]').forEach(b => b.addEventListener('click', () => copy(b, b.parentElement.querySelector('code').textContent, 'Copied')));

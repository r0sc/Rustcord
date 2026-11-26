// Builds the website's plugin list from src/plugins/mod.rs (META) and site/plugins.json.
// Usage: node scripts/site-plugins.mjs _site/index.html  (rewrites the page in place)
import { readFileSync, writeFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

export const CATS = ['Chat', 'People', 'Servers and DMs', 'Voice and media', 'Notifications', 'Privacy and tools'];
const KEYS = ['name', 'from', 'url', 'cat', 'top', 'next'];
const BASE = { Vencord: 'https://vencord.dev/plugins/', Equicord: 'https://equicord.org/plugins/', Illegalcord: null };
// shortcut: META's wording lives in src/ (another lane edits it); drop a pair once src says it this way.
const WORDING = [[/\b([Cc])olour/g, '$1olor'], [/\b([Gg])rey/g, '$1ray'], [/ right in the chat/g, ' in the chat'], [/'/g, '’']];

const unescape = s => s.replace(/\\(.)/g, (_, c) => {
  if (c === '"' || c === '\\') return c;
  throw new Error(`src/plugins/mod.rs: unknown escape \\${c} in a plugin text`);
});

export function parseMeta(rs) {
  const a = rs.indexOf('pub const META: &[Meta] = &[');
  if (a < 0) throw new Error('src/plugins/mod.rs has no META table.');
  const b = rs.indexOf('\n];', a);
  const body = rs.slice(a, b < 0 ? undefined : b);
  const re = /meta\(P::\w+,\s*"((?:[^"\\]|\\.)*)",\s*"((?:[^"\\]|\\.)*)"/g;
  return [...body.matchAll(re)].map(m => ({ name: unescape(m[1]), desc: unescape(m[2]) }));
}

export function check(meta, src) {
  if (!meta.length) return ['src/plugins/mod.rs has no plugins in META.'];
  const err = [], names = new Set(meta.map(m => m.name)), seen = new Set();
  for (const m of meta) if (!src.some(s => s.name === m.name))
    err.push(`site/plugins.json has no entry for ${m.name}. Add where it comes from (from, url, cat) before the site can build.`);
  for (const s of src) {
    const n = s.name, p = `site/plugins.json: ${n}`;
    if (!names.has(n)) err.push(`${p} is not in META. Remove it.`);
    if (seen.has(n)) err.push(`${p} is listed twice.`);
    seen.add(n);
    for (const k of Object.keys(s)) if (!KEYS.includes(k)) err.push(`${p} has an unknown key "${k}".`);
    if (!(s.from in BASE)) err.push(`${p}: from must be Vencord, Equicord or Illegalcord.`);
    else if (s.url !== '' && !(BASE[s.from] && s.url === BASE[s.from] + n))
      err.push(`${p}: url must be ""${BASE[s.from] ? ` or ${BASE[s.from]}${n}` : ''}`);
    if (!CATS.includes(s.cat)) err.push(`${p}: cat must be one of ${CATS.join(', ')}.`);
    for (const k of ['top', 'next']) if (k in s && s[k] !== true) err.push(`${p}: top and next must be true or left out.`);
  }
  const top = src.filter(s => s.top).length;
  if (top > 8) err.push(`site/plugins.json: at most 8 plugins can have "top": true (found ${top}).`);
  return err;
}

const esc = s => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
const by = (a, b) => a.name.localeCompare(b.name, 'en', { sensitivity: 'base' });

export function render(meta, src) {
  const row = m => {
    const s = src.find(x => x.name === m.name), n = esc(m.name), f = esc(s.from);
    const label = s.url
      ? `<a class="chip chip-src" href="${esc(s.url)}" aria-label="${f}’s ${n}">${f}</a>`
      : `<span class="chip chip-src">${f}</span>`;
    const next = s.next ? '<span class="chip chip-next">Next release</span>' : '';
    const desc = esc(WORDING.reduce((t, [re, to]) => t.replace(re, to), m.desc));
    return { s, html: `<li class="pl-row"><div class="pl-head"><span class="pl-name">${n}</span>${label}${next}</div><p class="pl-desc">${desc}</p></li>` };
  };
  const rows = meta.map(row).sort((a, b) => by(a.s, b.s));
  const top = rows.filter(r => r.s.top);
  const rest = CATS.map(c => {
    const li = rows.filter(r => !r.s.top && r.s.cat === c).map(r => r.html).join('\n');
    return li && `<div class="pl-group"><h3>${esc(c)}</h3><ul class="pl-list">\n${li}\n</ul></div>`;
  }).filter(Boolean);
  return { count: meta.length, more: meta.length - top.length, top: top.map(r => r.html).join('\n'), rest: rest.join('\n') };
}

export function inject(html, r) {
  const map = { '{{plugin-count}}': String(r.count), '{{plugin-more}}': String(r.more), '<!--plugins-top-->': r.top, '<!--plugins-rest-->': r.rest };
  for (const [m, v] of Object.entries(map)) {
    if (html.split(m).length !== 2) throw new Error(`marker ${m} must appear once in the page`);
    html = html.replace(m, () => v);
  }
  return html;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const page = process.argv[2];
    if (!page) throw new Error('usage: node scripts/site-plugins.mjs <page.html>');
    const meta = parseMeta(readFileSync(new URL('../src/plugins/mod.rs', import.meta.url), 'utf8'));
    const src = JSON.parse(readFileSync(new URL('../site/plugins.json', import.meta.url), 'utf8'));
    const err = check(meta, src);
    if (err.length) throw new Error(err.join('\n'));
    writeFileSync(page, inject(readFileSync(page, 'utf8'), render(meta, src)));
  } catch (e) {
    console.error(e.message);
    process.exit(1);
  }
}

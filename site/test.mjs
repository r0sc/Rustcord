// Run: node --test site/test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import { stageAt } from './stage.js';
import { gate, renderScale, follow, ringAt, isSoftware } from './fx.js';
import { detectOs, pickAssets, fetchLatest, buttonState, mb, LATEST, REPO } from './download.js';
import { readFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { parseMeta, check, render, inject, CATS } from '../scripts/site-plugins.mjs';

const UA = {
  win: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/130.0 Safari/537.36',
  macIntel: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Version/17.0 Safari/605.1.15',
  macArm: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Version/18.0 Safari/605.1.15', // Apple silicon Safari still says Intel
  ubuntu: 'Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0',
  fedora: 'Mozilla/5.0 (X11; Fedora; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0',
  android: 'Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 Chrome/130.0 Mobile Safari/537.36',
  iphone: 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 Mobile/15E148',
  cros: 'Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 Chrome/130.0 Safari/537.36',
};

test('detects desktop systems', () => {
  assert.equal(detectOs({ ua: UA.win, platform: 'Win32' }), 'windows');
  assert.equal(detectOs({ ua: UA.macIntel, platform: 'MacIntel' }), 'mac');
  assert.equal(detectOs({ ua: UA.macArm, platform: 'MacIntel' }), 'mac');
  assert.equal(detectOs({ ua: UA.macArm, platform: 'macOS' }), 'mac');
  assert.equal(detectOs({ ua: UA.ubuntu, platform: 'Linux x86_64' }), 'deb');
  assert.equal(detectOs({ ua: UA.fedora, platform: 'Linux x86_64' }), 'tar');
  assert.equal(detectOs({ ua: UA.win }), 'windows');
});

test('mobile, iPadOS and unknown', () => {
  assert.equal(detectOs({ ua: UA.android, platform: 'Linux armv8l' }), 'mobile');
  assert.equal(detectOs({ ua: UA.iphone, platform: 'iPhone' }), 'mobile');
  assert.equal(detectOs({ ua: UA.macIntel, platform: 'MacIntel', touch: 5 }), 'mobile');
  assert.equal(detectOs({ ua: UA.win, platform: 'Win32', mobile: true }), 'mobile');
  assert.equal(detectOs({ ua: UA.cros, platform: 'Linux x86_64' }), 'unknown');
  assert.equal(detectOs({ ua: UA.ubuntu, platform: 'Linux aarch64' }), 'unknown');
  assert.equal(detectOs({ ua: 'curl/8', platform: '' }), 'unknown');
  assert.equal(detectOs({}), 'unknown');
  assert.equal(detectOs({ force: 'tar', ua: UA.win }), 'tar');
});

const D = REPO + '/releases/download/v0.1.2/';
const rel = {
  tag_name: 'v0.1.2',
  assets: [
    { name: 'Rustcord-Setup-0.1.2.exe', size: 24799288, browser_download_url: D + 'Rustcord-Setup-0.1.2.exe' },
    { name: 'rustcord_0.1.2_amd64.deb', size: 11581552, browser_download_url: D + 'rustcord_0.1.2_amd64.deb' },
    { name: 'rustcord-0.1.2-linux-x86_64.tar.gz', size: 16173740, browser_download_url: D + 'rustcord-0.1.2-linux-x86_64.tar.gz' },
    { name: 'Rustcord-0.1.2-macos-universal.dmg', size: 28429083, browser_download_url: D + 'Rustcord-0.1.2-macos-universal.dmg' },
    { name: 'SHA256SUMS', size: 300, browser_download_url: D + 'SHA256SUMS' },
  ],
};

test('picks every installer from a release', () => {
  const r = pickAssets(rel);
  assert.equal(r.v, '0.1.2');
  assert.deepEqual(Object.keys(r.files).sort(), ['deb', 'mac', 'tar', 'windows']);
  assert.equal(r.files.windows.url, D + 'Rustcord-Setup-0.1.2.exe');
  assert.equal(mb(r.files.windows.size), '23.7 MB');
});

test('rejects bad tags and foreign download hosts', () => {
  assert.throws(() => pickAssets({ tag_name: 'nightly', assets: [] }));
  assert.throws(() => pickAssets({}));
  const evil = { tag_name: 'v1.0.0', assets: [{ name: 'Rustcord-Setup-1.0.0.exe', size: 1, browser_download_url: 'https://evil.example/x.exe' }] };
  assert.deepEqual(pickAssets(evil).files, {});
});

test('button states', () => {
  const r = pickAssets(rel);
  assert.equal(buttonState('windows', r, false).href, D + 'Rustcord-Setup-0.1.2.exe');
  assert.equal(buttonState('windows', r, false).meta, 'Version 0.1.2 for 64-bit Windows, 23.7 MB.');
  assert.equal(buttonState('mac', null, false).href, LATEST);
  assert.equal(buttonState('deb', null, true).href, LATEST);
  assert.match(buttonState('deb', null, true).meta, /rustcord_<version>_amd64\.deb/);
  assert.equal(buttonState('tar', { v: '0.1.2', files: {} }, false).href, REPO + '/releases');
  assert.equal(buttonState('mobile', r, false).hidden, true);
  assert.equal(buttonState('unknown', r, false).href, '#systems');
});

test('API failure and bad status fall back', async () => {
  await assert.rejects(fetchLatest(undefined, true));
  await assert.rejects(fetchLatest(async () => ({ ok: false, status: 403 })));
  await assert.rejects(fetchLatest(async () => { throw new Error('offline'); }));
  const r = await fetchLatest(async () => ({ ok: true, json: async () => rel }));
  assert.equal(r.v, '0.1.2');
});

test('scroll stage', () => {
  // the window sticks after 400 px of scroll; each screen gets 512 px (64vh of 800)
  assert.deepEqual(stageAt(0, 400, 512, 5), { p: 0, i: 0 });
  assert.deepEqual(stageAt(200, 400, 512, 5), { p: 0.5, i: 0 });
  assert.deepEqual(stageAt(400 + 511, 400, 512, 5), { p: 1, i: 0 });
  assert.deepEqual(stageAt(400 + 512, 400, 512, 5), { p: 1, i: 1 });
  assert.deepEqual(stageAt(400 + 4 * 512, 400, 512, 5), { p: 1, i: 4 });
  assert.deepEqual(stageAt(1e6, 400, 512, 5), { p: 1, i: 4 });
  assert.deepEqual(stageAt(-50, 400, 512, 5), { p: 0, i: 0 }); // overscroll at the top
});

test('effect gate', () => {
  const on = { fx: true, webgl: true, motion: true, fine: true, software: false, hook: false };
  assert.equal(gate(on), 'live');
  assert.equal(gate({ ...on, motion: false }), 'still');               // reduced motion or ?static=1
  assert.equal(gate({ ...on, motion: false, hook: true }), 'still');   // hooks never override reduced motion
  assert.equal(gate({ ...on, fine: false }), 'still');                 // touch
  assert.equal(gate({ ...on, software: true }), 'still');              // software GL
  assert.equal(gate({ ...on, software: true, hook: true }), 'live');   // screenshot hooks in headless
  assert.equal(gate({ ...on, fx: false }), 'off');
  assert.equal(gate({ ...on, webgl: false }), 'off');
});

test('effect math', () => {
  assert.equal(renderScale(1280, 800, 1), 0.75);
  assert.equal(renderScale(1280, 800, 2.5), 0.75);
  assert.ok(Math.abs(renderScale(1280, 800, 0.8) - 0.6) < 1e-9);
  assert.ok(Math.abs(renderScale(3840, 2160, 1) - Math.sqrt(1.2e6 / (3840 * 2160))) < 1e-9);
  assert.ok(Math.abs(follow(0, 1, 50, 50) - (1 - Math.exp(-1))) < 1e-9);
  for (const dt of [0, -5, NaN]) { const v = follow(0, 1, dt, 70); assert.ok(v > 0 && v < 0.02); }
  assert.equal(follow(0, 1, 1000, 70), follow(0, 1, 64, 70));
  assert.deepEqual(ringAt(0), { r: 0, a: 1 });
  assert.deepEqual(ringAt(1), { r: 1, a: 0 });
  assert.equal(ringAt(0.5).r, 0.75);
  assert.deepEqual(ringAt(2), ringAt(1));
  assert.ok(isSoftware('ANGLE (Microsoft, Microsoft Basic Render Driver (0x0000008C) Direct3D11 vs_5_0 ps_5_0, D3D11)'));
  assert.ok(isSoftware('ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (Subzero)), SwiftShader driver)'));
  assert.ok(isSoftware('llvmpipe (LLVM 15.0.7, 256 bits)'));
  assert.ok(!isSoftware('ANGLE (AMD, AMD Radeon (TM) Graphics (0x000015E7) Direct3D11 vs_5_0 ps_5_0, D3D11)'));
  assert.ok(!isSoftware(''));
});

// ---- plugin list (scripts/site-plugins.mjs) ----
const root = new URL('..', import.meta.url);
const modRs = readFileSync(new URL('src/plugins/mod.rs', root), 'utf8');
const srcJson = JSON.parse(readFileSync(new URL('site/plugins.json', root), 'utf8'));
const one = (name, extra = {}) => ({ name, from: 'Vencord', url: `https://vencord.dev/plugins/${name}`, cat: 'Chat', ...extra });
const M = (name, desc = 'd') => ({ name, desc });

test('plugins.json covers every plugin in META', () => {
  const meta = parseMeta(modRs);
  assert.ok(meta.length > 0);
  assert.equal(new Set(meta.map(m => m.name)).size, meta.length);
  assert.deepEqual(check(meta, srcJson), []);
});

test('check names what is wrong', () => {
  const ok = [one('A')];
  assert.deepEqual(check([M('A')], ok), []);
  assert.deepEqual(check([], ok), ['src/plugins/mod.rs has no plugins in META.']);
  assert.deepEqual(check([M('A'), M('B')], ok), ['site/plugins.json has no entry for B. Add where it comes from (from, url, cat) before the site can build.']);
  assert.deepEqual(check([M('A')], [...ok, one('Z')]), ['site/plugins.json: Z is not in META. Remove it.']);
  assert.deepEqual(check([M('A')], [...ok, one('A')]), ['site/plugins.json: A is listed twice.']);
  assert.deepEqual(check([M('A')], [one('A', { x: 1 })]), ['site/plugins.json: A has an unknown key "x".']);
  assert.deepEqual(check([M('A')], [one('A', { from: 'Foo' })]), ['site/plugins.json: A: from must be Vencord, Equicord or Illegalcord.']);
  assert.deepEqual(check([M('A')], [one('A', { url: 'https://example.com/A' })]), ['site/plugins.json: A: url must be "" or https://vencord.dev/plugins/A']);
  assert.deepEqual(check([M('A')], [one('A', { from: 'Illegalcord', url: 'https://vencord.dev/plugins/A' })]), ['site/plugins.json: A: url must be ""']);
  assert.deepEqual(check([M('A')], [one('A', { cat: 'Misc' })]), ['site/plugins.json: A: cat must be one of ' + CATS.join(', ') + '.']);
  assert.deepEqual(check([M('A')], [one('A', { next: 'yes' })]), ['site/plugins.json: A: top and next must be true or left out.']);
  const nine = Array.from({ length: 9 }, (_, i) => one('P' + i, { top: true }));
  assert.deepEqual(check(nine.map(n => M(n.name)), nine), ['site/plugins.json: at most 8 plugins can have "top": true (found 9).']);
});

test('parseMeta reads names and descriptions', () => {
  const rs = 'x\npub const META: &[Meta] = &[\n    meta(P::A, "A", "Say \\"hi\\".", &[\n        opt("k", "Label", Bool(true)),\n    ]),\n    meta(P::B, "B", "two", &[]),\n];\nmeta(P::C, "C", "after", &[])';
  assert.deepEqual(parseMeta(rs), [M('A', 'Say "hi".'), M('B', 'two')]);
  assert.throws(() => parseMeta('nothing here'));
  assert.throws(() => parseMeta('pub const META: &[Meta] = &[\n    meta(P::A, "A", "bad \\n escape", &[]),\n];'));
});

test('render escapes, maps wording and groups', () => {
  const meta = [M('A<b>', "it's & <x>"), M('B', 'colour Colourful greyed right in the chat'), M('C'), M('D')];
  const src = [one('A<b>'), one('B', { from: 'Illegalcord', url: '', cat: 'People', next: true }), one('C', { top: true }), one('D', { cat: 'People' })];
  const r = render(meta, src);
  assert.ok(r.rest.includes('<span class="pl-name">A&lt;b&gt;</span>'));
  assert.ok(r.rest.includes('it’s &amp; &lt;x&gt;'));
  assert.ok(r.rest.includes('color Color'));
  assert.ok(r.rest.includes('colorful') === false && r.rest.includes('Colorful gray'));
  assert.ok(r.rest.includes('grayed in the chat'));
  assert.ok(r.rest.includes('<span class="chip chip-src">Illegalcord</span>'));
  assert.ok(r.rest.includes('<span class="chip chip-next">Next release</span>'));
  assert.ok(r.rest.includes('aria-label="Vencord’s A&lt;b&gt;"'));
  assert.ok(!r.rest.includes('Servers and DMs'), 'an empty group is left out');
  assert.ok(r.rest.indexOf('<h3>Chat</h3>') < r.rest.indexOf('<h3>People</h3>'));
  assert.equal(r.count, 4); assert.equal(r.more, 3);
  assert.equal((r.top.match(/pl-row/g) || []).length, 1);
  const real = render(parseMeta(modRs), srcJson);
  assert.equal(((real.top + real.rest).match(/class="pl-row"/g) || []).length, real.count);
  assert.equal((real.top.match(/class="pl-row"/g) || []).length, srcJson.filter(s => s.top).length);
});

test('inject needs every marker exactly once', () => {
  const r = { count: 2, more: 1, top: '$&T', rest: 'R' };
  const page = '{{plugin-count}} {{plugin-more}} <!--plugins-top--> <!--plugins-rest-->';
  assert.equal(inject(page, r), '2 1 $&T R');
  assert.throws(() => inject(page.replace('<!--plugins-rest-->', ''), r), /marker <!--plugins-rest--> must appear once/);
  assert.throws(() => inject(page + ' {{plugin-count}}', r), /marker/);
});

test('Next release flags match the latest tag', t => {
  const git = (...a) => execFileSync('git', a, { cwd: root, stdio: ['ignore', 'pipe', 'ignore'], encoding: 'utf8' });
  let tag, old;
  try { tag = git('describe', '--tags', '--abbrev=0').trim(); old = parseMeta(git('show', `${tag}:src/plugins/mod.rs`)); }
  catch { return t.skip('no release tag in this clone'); }
  const was = new Map(old.map(m => [m.name, m.desc]));
  const err = [];
  for (const m of parseMeta(modRs)) {
    const s = srcJson.find(x => x.name === m.name);
    if (!was.has(m.name) && !s.next) err.push(`${m.name} is not in ${tag}: set "next": true in site/plugins.json.`);
    if (s.next && was.get(m.name) === m.desc)
      err.push(`${m.name} ships in ${tag} as it is now: remove "next" in site/plugins.json, and the Next release chips in site/index.html for what ${tag} ships.`);
  }
  assert.deepEqual(err, []);
});

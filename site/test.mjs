// Run: node --test site/test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import { tabStep } from './viewer.js';
import { detectOs, pickAssets, fetchLatest, buttonState, mb, LATEST, REPO } from './download.js';

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

test('viewer keys', () => {
  assert.equal(tabStep('ArrowDown', 3, 4), 0);
  assert.equal(tabStep('ArrowRight', 1, 4), 2);
  assert.equal(tabStep('ArrowUp', 0, 4), 3);
  assert.equal(tabStep('Home', 2, 4), 0);
  assert.equal(tabStep('End', 0, 4), 3);
  assert.equal(tabStep('a', 1, 4), -1);
});

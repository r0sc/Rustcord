// Pure download logic: no DOM, no fetch, so node can test it (see test.mjs).
export const REPO = 'https://github.com/r0sc/Rustcord';
export const LATEST = REPO + '/releases/latest';
export const API = 'https://api.github.com/repos/r0sc/Rustcord/releases/latest';
const PREFIX = REPO + '/releases/download/';

export const SYSTEMS = {
  windows: { label: 'Download for Windows', title: 'Rustcord for Windows', tail: 'for 64-bit Windows',
    file: /^Rustcord-Setup-\d+\.\d+\.\d+\.exe$/, pattern: 'Rustcord-Setup-<version>.exe', notes: 'windows' },
  mac: { label: 'Download for macOS', title: 'Rustcord for macOS', tail: 'for macOS 11 or newer, Apple silicon and Intel',
    file: /^Rustcord-\d+\.\d+\.\d+-macos-universal\.dmg$/, pattern: 'Rustcord-<version>-macos-universal.dmg', notes: 'mac' },
  deb: { label: 'Download for Linux (.deb)', title: 'Rustcord for Linux', tail: 'for Debian, Ubuntu, Mint and similar distributions, 64-bit x86',
    file: /^rustcord_\d+\.\d+\.\d+_amd64\.deb$/, pattern: 'rustcord_<version>_amd64.deb', notes: 'linux' },
  tar: { label: 'Download for Linux (.tar.gz)', title: 'Rustcord for Linux', tail: 'for any 64-bit x86 Linux with glibc 2.35 or newer',
    file: /^rustcord-\d+\.\d+\.\d+-linux-x86_64\.tar\.gz$/, pattern: 'rustcord-<version>-linux-x86_64.tar.gz', notes: 'linux' },
};

// env: { force, ua, platform, mobile, touch }. Returns windows|mac|deb|tar|mobile|unknown.
export function detectOs({ force, ua = '', platform = '', mobile = false, touch = 0 } = {}) {
  if (force) return force;
  if (/Android|iPhone|iPad|iPod/i.test(ua) || mobile || (/Mac/i.test(platform) && touch > 1)) return 'mobile'; // iPadOS reports Mac
  if (/Win/i.test(platform) || (!platform && /Windows/i.test(ua))) return 'windows';
  if (/Mac/i.test(platform) || (!platform && /Macintosh/i.test(ua))) return 'mac';
  if (/aarch64|arm/i.test(platform)) return 'unknown'; // the Linux builds are x86_64 only
  if (/CrOS|Chrome OS/i.test(ua + platform)) return 'unknown'; // before Linux: CrOS says Linux
  if (/Linux|X11/i.test(platform + ' ' + ua)) return /Fedora|Red Hat|SUSE|Arch|Manjaro/i.test(ua) ? 'tar' : 'deb';
  return 'unknown';
}

// Release JSON from the GitHub API -> { v, files: { key: { url, name, size } } }. Throws on anything odd.
export function pickAssets(j) {
  const v = String((j && j.tag_name) || '').replace(/^v/, '');
  if (!/^\d+\.\d+\.\d+$/.test(v)) throw new Error('unexpected tag');
  const files = {};
  for (const [k, s] of Object.entries(SYSTEMS)) {
    const a = ((j && j.assets) || []).find(a => s.file.test(a.name) &&
      typeof a.browser_download_url === 'string' && a.browser_download_url.startsWith(PREFIX));
    if (a) files[k] = { url: a.browser_download_url, name: a.name, size: Number(a.size) || 0 };
  }
  return { v, files };
}

export async function fetchLatest(fetchFn = fetch, fail = false) {
  if (fail) throw new Error('api=fail hook');
  const res = await fetchFn(API, { headers: { Accept: 'application/vnd.github+json' },
    signal: AbortSignal.timeout ? AbortSignal.timeout(6000) : undefined });
  if (!res.ok) throw new Error('GitHub API ' + res.status); // 403 = rate limit
  return pickAssets(await res.json());
}

export const mb = n => (n / 1048576).toFixed(1) + ' MB';

// What the primary button/meta show. rel: null (loading) | {v, files}; failed: API error.
export function buttonState(os, rel, failed) {
  if (os === 'mobile') return { hidden: true };
  if (os === 'unknown') return { label: 'Choose your system', href: '#systems', meta: 'Your system wasn’t detected. Pick a file below.', title: 'Choose your system' };
  const s = SYSTEMS[os];
  const base = { label: s.label, title: s.title };
  if (failed) return { ...base, href: LATEST, meta: `Opens the latest release on GitHub. Pick ${s.pattern} there.` };
  if (!rel) return { ...base, href: LATEST, meta: 'Opens the latest release on GitHub.' };
  const f = rel.files[os];
  if (!f) return { ...base, href: REPO + '/releases', meta: 'The latest release has no file for this system. The button opens all releases on GitHub.' };
  return { ...base, href: f.url, meta: `Version ${rel.v} ${s.tail}, ${mb(f.size)}.` };
}

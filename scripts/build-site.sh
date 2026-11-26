#!/bin/sh
# Builds the website into _site/ (Netlify runs this, see netlify.toml).
# Local test without libvips: SITE_NO_VIPS=1 sh scripts/build-site.sh (copies the PNGs under the .webp names).
set -eu
cd "$(dirname "$0")/.."

node --test site/test.mjs

rm -rf _site
mkdir -p _site/shots
cp -r site/. _site/
rm _site/test.mjs

if [ -z "${SITE_NO_VIPS:-}" ] && ! command -v vips >/dev/null 2>&1; then
	echo "error: vips (libvips-tools) is missing, cannot make the WebP shots" >&2
	exit 1
fi
for s in main stream profile plugins light; do
	if [ -n "${SITE_NO_VIPS:-}" ]; then
		cp "docs/screenshots/$s.png" "_site/shots/$s.webp"
		cp "docs/screenshots/$s.png" "_site/shots/$s-960.webp"
	else
		vips copy "docs/screenshots/$s.png" "_site/shots/$s.webp[Q=80,effort=6,keep=none]"
		vips thumbnail "docs/screenshots/$s.png" "_site/shots/$s-960.webp[Q=80,effort=6,keep=none]" 960
	fi
done
cp docs/screenshots/main.png _site/shots/og.png
cp assets/logo.svg _site/logo.svg
cp assets/logo-small.svg _site/favicon.svg
cp assets/icons/rustcord-32.png _site/icon-32.png
cp assets/icons/rustcord-256.png _site/icon-256.png

# Size checks only warn, they never fail the build.
wc -c _site/shots/*
for f in _site/shots/*.webp; do
	case "$f" in *-960.webp) continue ;; esac
	[ "$(wc -c < "$f")" -le 250000 ] || echo "warning: $f is over 250 KB"
done
total=$(cat _site/shots/main.webp _site/shots/stream.webp _site/shots/profile.webp _site/shots/plugins.webp _site/shots/light.webp | wc -c)
[ "$total" -le 700000 ] || echo "warning: full-size shots total $total bytes, over 700 KB"

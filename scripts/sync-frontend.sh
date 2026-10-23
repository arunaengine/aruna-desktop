#!/usr/bin/env bash
# Installs the portal this app serves. It is the same artifact an aruna node
# serves at its own root, so the desktop and the web show the same build with
# no fork and no variant.
#
# The portal release is pinned in versions.json: its tarball is downloaded and
# checked against the pinned sha256. When the download fails, the pinned ref is
# exported from the website checkout (ARUNA_WEBSITE_DIR, default ../website)
# and built in a scratch directory; the checkout itself is never touched.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
pins="$root/versions.json"
tag="$(jq -r '.portal.tag' "$pins")"
ref="$(jq -r '.portal.ref' "$pins")"
sha256="$(jq -r '.portal.sha256' "$pins")"
url="https://github.com/arunaengine/website/releases/download/$tag/aruna-portal-dist.tar.gz"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
dist="$work/dist"
mkdir -p "$dist"

digest() {
  if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}

if curl -fsSL --retry 3 -o "$work/portal.tar.gz" "$url"; then
  [ "$(digest "$work/portal.tar.gz")" = "$sha256" ] || {
    echo "the portal tarball $tag does not match the pinned sha256" >&2
    exit 1
  }
  tar -xzf "$work/portal.tar.gz" -C "$dist"
  echo "downloaded the portal $tag"
else
  source_dir="${ARUNA_WEBSITE_DIR:-$root/../website}"
  if [ ! -f "$source_dir/package.json" ]; then
    echo "could not download $url and there is no website checkout at $source_dir" >&2
    exit 1
  fi
  mkdir -p "$work/website"
  git -C "$source_dir" archive "$ref" | tar -x -C "$work/website"
  (cd "$work/website" && npm ci && npm run build)
  cp -R "$work/website/dist/." "$dist/"
  echo "built the portal at $ref"
fi

[ -f "$dist/index.html" ] || { echo "the portal has no index.html" >&2; exit 1; }
rm -rf "$root/ui"
mkdir -p "$root/ui"
cp -R "$dist/." "$root/ui/"
echo "installed the portal into $root/ui"

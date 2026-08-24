#!/usr/bin/env bash
# Installs the portal this app serves. It is the same artifact an aruna node
# serves at its own root, built from the website repository, so the desktop
# and the web show the same build with no fork and no variant.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_dir="${ARUNA_WEBSITE_DIR:-$root/../website}"

if [ ! -f "$source_dir/package.json" ]; then
  echo "no website checkout at $source_dir; set ARUNA_WEBSITE_DIR" >&2
  exit 1
fi

website="$(cd "$source_dir" && pwd)"
if [ ! -d "$website/node_modules" ]; then
  (cd "$website" && npm ci)
fi
(cd "$website" && npm run build)

rm -rf "$root/ui"
mkdir -p "$root/ui"
cp -R "$website/dist/." "$root/ui/"
echo "synced $website/dist -> $root/ui"

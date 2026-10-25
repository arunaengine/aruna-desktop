#!/usr/bin/env bash
# Writes the update manifest (latest.json) of one release to stdout. The app
# reads it from the release of exactly the version its realm runs.
#
# Usage: update-manifest.sh <version> <release-download-url> <signature-dir>
# The directory holds the release's `.sig` assets; each one names the asset it
# signs. Only installers the updater can replace in place get an entry, keyed
# by platform and installer so a deb install is never updated with an AppImage.
set -euo pipefail

version="$1"
base="$2"
dir="$3"

platform() {
  case "$1" in
    *.AppImage) echo linux-x86_64-appimage ;;
    *.deb) echo linux-x86_64-deb ;;
    *.rpm) echo linux-x86_64-rpm ;;
    *-setup.exe) echo windows-x86_64-nsis ;;
    *.app.tar.gz) echo darwin-aarch64-app ;;
  esac
}

platforms='{}'
for sig in "$dir"/*.sig; do
  [ -e "$sig" ] || continue
  asset="$(basename "$sig" .sig)"
  key="$(platform "$asset")"
  [ -n "$key" ] || continue
  platforms="$(jq --arg key "$key" --arg base "$base" --arg asset "$asset" --rawfile sig "$sig" \
    '.[$key] = {url: ($base + "/" + ($asset | @uri)), signature: ($sig | rtrimstr("\n"))}' <<<"$platforms")"
done

[ "$platforms" != '{}' ] || {
  echo "no signed installer found in $dir" >&2
  exit 1
}

jq -n --arg version "$version" --arg date "$(date -u +%Y-%m-%dT%H:%M:%SZ)" --argjson platforms "$platforms" \
  '{version: $version, pub_date: $date, platforms: $platforms}'

#!/usr/bin/env bash
# Installs the node this app supervises as its Tauri sidecar,
# binaries/aruna-<target triple>. It is the aruna workspace's own release
# binary: built from the sibling checkout, or a ready one named by
# ARUNA_NODE_BIN. Dev runs find it next to the app executable and packaged
# builds bundle exactly this file.
#
# Usage: scripts/sync-node.sh [--no-build]
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="${ARUNA_TARGET_TRIPLE:-$(rustc -vV | sed -n 's/^host: //p')}"
[ -n "$target" ] || { echo "could not read the target triple from rustc" >&2; exit 1; }
suffix=""
case "$target" in
  *windows*) suffix=".exe" ;;
esac
dest="$root/binaries/aruna-$target$suffix"

source="${ARUNA_NODE_BIN:-}"
if [ -z "$source" ]; then
  aruna="${ARUNA_DIR:-$root/../aruna}"
  if [ ! -f "$aruna/Cargo.toml" ]; then
    echo "no aruna checkout at $aruna; set ARUNA_DIR or ARUNA_NODE_BIN" >&2
    exit 1
  fi
  if [ "${1:-}" != "--no-build" ]; then
    (cd "$aruna" && cargo build --release -p aruna)
  fi
  source="$aruna/target/release/aruna$suffix"
fi
if [ ! -f "$source" ]; then
  echo "no node binary at $source" >&2
  exit 1
fi

mkdir -p "$root/binaries"
cp -f "$source" "$dest"
chmod +x "$dest"
echo "installed $source -> $dest"

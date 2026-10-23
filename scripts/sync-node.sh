#!/usr/bin/env bash
# Installs the node this app supervises as its Tauri sidecar,
# binaries/aruna-<target triple>. It is the aruna workspace's own release
# binary: built from the sibling checkout, or a ready one named by
# ARUNA_NODE_BIN. Dev runs find it next to the app executable and packaged
# builds bundle exactly this file.
#
# The aruna version is pinned in versions.json. The node is built stripped, with
# the executors this OS supports and never kubernetes.
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
# apptainer uses Linux-only APIs.
case "$target" in
  *linux*) features="docker,apptainer" ;;
  *) features="docker" ;;
esac
pinned="$(jq -r '.aruna.version' "$root/versions.json")"
dest="$root/binaries/aruna-$target$suffix"

source="${ARUNA_NODE_BIN:-}"
if [ -z "$source" ]; then
  aruna="${ARUNA_DIR:-$root/../aruna}"
  if [ ! -f "$aruna/Cargo.toml" ]; then
    echo "no aruna checkout at $aruna; set ARUNA_DIR or ARUNA_NODE_BIN" >&2
    exit 1
  fi
  built="$(sed -n 's/^version = "\(.*\)"/\1/p' "$aruna/Cargo.toml" | head -1)"
  if [ "$built" != "$pinned" ]; then
    echo "the aruna checkout is $built but versions.json pins $pinned" >&2
    exit 1
  fi
  release="$aruna/target/release"
  build=(cargo build --locked --release -p aruna --no-default-features --features "$features")
  if [ -n "${ARUNA_TARGET_TRIPLE:-}" ]; then
    build+=(--target "$target")
    release="$aruna/target/$target/release"
  fi
  if [ "${1:-}" != "--no-build" ]; then
    (cd "$aruna" && CARGO_PROFILE_RELEASE_STRIP=symbols "${build[@]}")
  fi
  source="$release/aruna$suffix"
fi
if [ ! -f "$source" ]; then
  echo "no node binary at $source" >&2
  exit 1
fi

mkdir -p "$root/binaries"
cp -f "$source" "$dest"
chmod +x "$dest"
echo "installed $source -> $dest"

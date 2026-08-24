#!/usr/bin/env bash
# Gives a development run the desktop identity a packaged install brings with
# it. A Wayland compositor takes a window's icon from an installed desktop
# entry whose name matches the window's app id, and a cargo run installs none,
# so the window shows a placeholder instead of the Aruna mark.
#
# Usage: scripts/install-dev-desktop.sh [--remove]
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
config="$root/tauri.conf.json"
manifest="$root/Cargo.toml"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
apps="$data/applications"
icons="$data/icons/hicolor"

# Sizes are the ones the icon set actually holds; a lie here would make the
# icon theme pick a file it then has to rescale.
sources=(
  "32:icons/32x32.png"
  "64:icons/64x64.png"
  "128:icons/128x128.png"
  "256:icons/128x128@2x.png"
  "512:icons/icon.png"
)

field() { # key file
  sed -n "s/.*\"$1\": *\"\([^\"]*\)\".*/\1/p" "$2" | head -1
}

product="$(field productName "$config")"
identifier="$(field identifier "$config")"
binary="$(field mainBinaryName "$config")"
if [ -z "$binary" ]; then
  binary="$(sed -n '/^\[\[bin\]\]/,/^\[/ s/^name *= *"\([^"]*\)".*/\1/p' "$manifest" | head -1)"
fi
if [ -z "$binary" ] || [ -z "$product" ]; then
  echo "could not read the app name from $config and $manifest" >&2
  exit 1
fi

# What a compositor matches is the window's app id, and GTK takes that from the
# program name rather than from the Tauri identifier ($identifier): a window of
# this app reports set_app_id("$binary"), verified with WAYLAND_DEBUG=1. The
# app.enableGtkAppId switch would name the GLib application instead, so it is
# honoured here, but it does not rename the window itself.
app_id="$binary"
if grep -q '"enableGtkAppId": *true' "$config"; then
  app_id="$identifier"
fi

entry="$apps/$app_id.desktop"

remove() {
  rm -f "$entry"
  for source in "${sources[@]}"; do
    rm -f "$icons/${source%%:*}x${source%%:*}/apps/$app_id.png"
  done
  echo "removed $entry and its icons"
}

install() {
  for source in "${sources[@]}"; do
    size="${source%%:*}"
    file="$root/${source#*:}"
    [ -f "$file" ] || continue
    target="$icons/${size}x${size}/apps/$app_id.png"
    mkdir -p "$(dirname "$target")"
    cp -f "$file" "$target"
  done

  # No MimeType: the deep-link plugin writes its own "$binary-handler.desktop"
  # for the aruna scheme at runtime, and two entries claiming it would fight.
  mkdir -p "$apps"
  cat > "$entry" <<ENTRY
[Desktop Entry]
Type=Application
Name=$product
Comment=Runs an Aruna user node on this machine
Exec=${ARUNA_DESKTOP_BIN:-$root/target/release/$binary}
Icon=$app_id
Terminal=false
Categories=Utility;Network;
StartupWMClass=$app_id
ENTRY
  echo "installed $entry for app id $app_id"
}

refresh() {
  if command -v update-desktop-database > /dev/null; then
    update-desktop-database "$apps" || true
  else
    echo "warning: update-desktop-database is missing; log out to pick the entry up" >&2
  fi
  # The cache is only read where the theme has an index; without one the files
  # are found directly, so a missing index is not worth a warning.
  if [ -f "$icons/index.theme" ] && command -v gtk-update-icon-cache > /dev/null; then
    gtk-update-icon-cache -q -t -f "$icons" || true
  fi
}

case "${1:-}" in
  --remove) remove ;;
  "") install ;;
  *)
    echo "usage: $(basename "$0") [--remove]" >&2
    exit 2
    ;;
esac
refresh

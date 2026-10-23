#!/usr/bin/env bash
# Builds dist/Vigie-<version>-x86_64.AppImage on Linux (decision 0009). Run on the oldest
# distribution to support (the release workflow uses Ubuntu 22.04): the AppImage carries the
# binary, not the system libraries it links to.
set -euo pipefail

# appimagetool, pinned.
APPIMAGETOOL_VERSION=1.9.0
APPIMAGETOOL_URL="https://github.com/AppImage/appimagetool/releases/download/${APPIMAGETOOL_VERSION}/appimagetool-x86_64.AppImage"

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
out="$root/dist"
work="$root/target/appimage"
rm -rf "$work"
mkdir -p "$out" "$work"

cargo build --release -p vigie

appdir="$work/Vigie.AppDir"
id=fr.evanpluchart.Vigie
mkdir -p "$appdir/usr/bin" "$appdir/usr/share/applications" "$appdir/usr/share/metainfo" \
  "$appdir/usr/share/icons/hicolor/256x256/apps"
install -m 755 target/release/vigie "$appdir/usr/bin/vigie"
install -m 644 "packaging/linux/$id.desktop" "$appdir/usr/share/applications/$id.desktop"
install -m 644 "packaging/linux/$id.metainfo.xml" "$appdir/usr/share/metainfo/$id.appdata.xml"
install -m 644 assets/brand/icons/vigie-256.png "$appdir/usr/share/icons/hicolor/256x256/apps/$id.png"
# The AppImage root needs the desktop file, the icon and an entry point.
cp "packaging/linux/$id.desktop" "$appdir/$id.desktop"
cp assets/brand/icons/vigie-256.png "$appdir/$id.png"
cat > "$appdir/AppRun" <<'EOF'
#!/bin/sh
here="$(dirname "$(readlink -f "$0")")"
exec "$here/usr/bin/vigie" "$@"
EOF
chmod 755 "$appdir/AppRun"

tool="$work/appimagetool"
curl -fsSL -o "$tool" "$APPIMAGETOOL_URL"
chmod 755 "$tool"
target="$out/Vigie-$version-x86_64.AppImage"
ARCH=x86_64 "$tool" --appimage-extract-and-run "$appdir" "$target"
echo "dist: $target"

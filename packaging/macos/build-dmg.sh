#!/usr/bin/env bash
# Builds dist/Vigie-<version>.dmg on macOS (decision 0009): a universal binary (Apple silicon and
# Intel), a .app with its icon, signed with a Developer ID when the secrets exist, ad hoc
# otherwise, then notarised when possible.
#
# Optional environment, from repository secrets:
#   MACOS_CERT_P12 (base64), MACOS_CERT_PASSWORD, MACOS_SIGN_IDENTITY ("Developer ID Application: …")
#   APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PASSWORD (notarisation)
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
out="$root/dist"
work="$root/target/macos-bundle"
rm -rf "$work"
mkdir -p "$out" "$work"

rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo build --release -p vigie --target aarch64-apple-darwin
cargo build --release -p vigie --target x86_64-apple-darwin

app="$work/Vigie.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
lipo -create -output "$app/Contents/MacOS/vigie" \
  target/aarch64-apple-darwin/release/vigie target/x86_64-apple-darwin/release/vigie
sed "s/@VERSION@/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"

# The icon set from the brand masters.
iconset="$work/vigie.iconset"
mkdir -p "$iconset"
icons=assets/brand/icons
for size in 16 32 128 256; do
  cp "$icons/vigie-$size.png" "$iconset/icon_${size}x${size}.png"
  double=$((size * 2))
  sips -z "$double" "$double" "$icons/vigie-1024.png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
sips -z 512 512 "$icons/vigie-1024.png" --out "$iconset/icon_512x512.png" >/dev/null
cp "$icons/vigie-1024.png" "$iconset/icon_512x512@2x.png"
iconutil -c icns "$iconset" -o "$app/Contents/Resources/vigie.icns"

if [[ -n "${MACOS_CERT_P12:-}" && -n "${MACOS_SIGN_IDENTITY:-}" ]]; then
  keychain="$work/build.keychain"
  echo "$MACOS_CERT_P12" | base64 --decode > "$work/cert.p12"
  security create-keychain -p vigie "$keychain"
  security default-keychain -s "$keychain"
  security unlock-keychain -p vigie "$keychain"
  security import "$work/cert.p12" -k "$keychain" -P "${MACOS_CERT_PASSWORD:-}" -T /usr/bin/codesign
  security set-key-partition-list -S apple-tool:,apple: -s -k vigie "$keychain" >/dev/null
  codesign --force --options runtime --timestamp --sign "$MACOS_SIGN_IDENTITY" "$app"
  signed=developer-id
else
  # Ad hoc: runs on the Mac that built it; elsewhere Gatekeeper asks for right click > Open.
  codesign --force --sign - "$app"
  signed=ad-hoc
fi

dmg="$out/Vigie-$version.dmg"
rm -f "$dmg"
staging="$work/dmg"
mkdir -p "$staging"
cp -R "$app" "$staging/"
ln -s /Applications "$staging/Applications"
hdiutil create -volname "Vigie $version" -srcfolder "$staging" -ov -format UDZO "$dmg"

if [[ "$signed" == developer-id && -n "${APPLE_ID:-}" && -n "${APPLE_TEAM_ID:-}" && -n "${APPLE_APP_PASSWORD:-}" ]]; then
  codesign --force --timestamp --sign "$MACOS_SIGN_IDENTITY" "$dmg"
  xcrun notarytool submit "$dmg" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_PASSWORD" --wait
  xcrun stapler staple "$dmg"
  signed=notarised
fi

echo "dist: $dmg ($signed)"

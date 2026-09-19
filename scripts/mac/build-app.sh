#!/bin/zsh
# Builds AltTabio.app from the Rust executable without Xcode.
#
# Usage: scripts/mac/build-app.sh [--debug]
#
# Output: target/mac/AltTabio.app
#
# macOS keys Accessibility and Screen Recording grants to the app bundle identifier plus its code
# signature. Ad-hoc signatures change with every build, which makes macOS forget the grants. The
# script therefore signs with a persistent self-signed "AltTabio Dev" certificate when one exists
# (scripts/mac/make-dev-cert.sh creates it) and falls back to an ad-hoc signature otherwise.
set -euo pipefail

cd "$(dirname "$0")/../.."
profile=release
cargo_flags=(--release)
if [[ "${1:-}" == "--debug" ]]; then
    profile=debug
    cargo_flags=()
fi

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
app=target/mac/AltTabio.app
contents="$app/Contents"

cargo build --quiet "${cargo_flags[@]}"

rm -rf "$app"
mkdir -p "$contents/MacOS" "$contents/Resources"
cp "target/$profile/AltTabio" "$contents/MacOS/AltTabio"
sed "s/__VERSION__/$version/g" assets/mac/Info.plist > "$contents/Info.plist"
printf 'APPL????' > "$contents/PkgInfo"

# The .icns is generated from the product icon with the tools that ship with macOS.
iconset=target/mac/AltTabio.iconset
rm -rf "$iconset"
mkdir -p "$iconset"
for size in 16 32 128 256; do
    sips -z $size $size docs/alttabio-icon.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z $double $double docs/alttabio-icon.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$contents/Resources/AltTabio.icns"

identity=$(security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(AltTabio Dev\)".*/\1/p' | head -1)
if [[ -n "$identity" ]]; then
    codesign --force --sign "$identity" --identifier com.vibeslop.AltTabio "$app"
    echo "Signed $app with the persistent '$identity' certificate"
else
    codesign --force --sign - --identifier com.vibeslop.AltTabio "$app"
    echo "Signed $app ad hoc; run scripts/mac/make-dev-cert.sh once to keep permissions across builds"
fi
echo "Built $app ($profile, version $version)"

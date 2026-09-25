#!/bin/zsh
# Builds AltTabio.app from the Rust executable without Xcode.
#
# Usage: scripts/mac/build-app.sh [--debug | --universal]
#   --universal   build the release binary for Apple silicon and Intel, as published releases are
#
# Output: target/mac/AltTabio.app
#
# macOS keys Accessibility and Screen Recording grants to the app bundle identifier plus its code
# signature. Ad-hoc signatures change with every build, which makes macOS forget the grants. The
# script therefore signs with the persistent self-signed "AltTabio Code Signing" certificate when
# it exists (scripts/mac/make-signing-cert.sh creates it) and falls back to an ad-hoc signature
# otherwise.
set -euo pipefail

cd "$(dirname "$0")/../.."
usage="Usage: scripts/mac/build-app.sh [--debug | --universal]"
(( $# <= 1 )) || { print -u2 -- "$usage"; exit 2; }
profile=release
cargo_flags=(--release)
targets=()
case "${1:-}" in
    "") ;;
    --debug)
        profile=debug
        cargo_flags=()
        ;;
    --universal)
        profile=universal
        targets=(aarch64-apple-darwin x86_64-apple-darwin)
        ;;
    *)
        print -u2 -- "$usage"
        exit 2
        ;;
esac

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
app=target/mac/AltTabio.app
contents="$app/Contents"

binaries=()
if (( ${#targets} )); then
    installed=$(rustup target list --installed)
    for target in "${targets[@]}"; do
        if [[ "$installed" != *"$target"* ]]; then
            rustup target add "$target"
        fi
        cargo build --quiet --release --target "$target"
        binaries+=("target/$target/release/AltTabio")
    done
else
    cargo build --quiet "${cargo_flags[@]}"
    binaries=("target/$profile/AltTabio")
fi

rm -rf "$app"
mkdir -p "$contents/MacOS" "$contents/Resources"
if (( ${#binaries} > 1 )); then
    lipo -create "${binaries[@]}" -output "$contents/MacOS/AltTabio"
else
    cp "${binaries[@]}" "$contents/MacOS/AltTabio"
fi
sed "s/__VERSION__/$version/g" assets/mac/Info.plist > "$contents/Info.plist"
printf 'APPL????' > "$contents/PkgInfo"
# The notices ship inside the bundle because the bundle is all that an install keeps.
cp LICENSE THIRD_PARTY_LICENSES.md "$contents/Resources/"

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

identity=$(security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(AltTabio Code Signing\)".*/\1/p' | head -1)
if [[ -n "$identity" ]]; then
    codesign --force --sign "$identity" --identifier com.vibeslop.AltTabio "$app"
    echo "Signed $app with the persistent '$identity' certificate"
else
    codesign --force --sign - --identifier com.vibeslop.AltTabio "$app"
    echo "Signed $app ad hoc; run scripts/mac/make-signing-cert.sh once to keep permissions across builds"
fi
echo "Built $app ($profile, version $version)"

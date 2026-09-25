#!/bin/zsh
# Builds AltTabio.app from the Rust executable without Xcode.
#
# Usage: scripts/mac/build-app.sh [--debug | --universal]
#   --universal   build the release binary for Apple silicon and Intel, as published releases are
#
# Output: target/mac/AltTabio.app
#
# macOS keys Accessibility and Screen Recording grants to the app bundle identifier plus the
# certificate it is signed with. Ad-hoc signatures change with every build, which makes macOS
# forget the grants. The script therefore signs with the release certificate pinned in
# scripts/mac/release-certificate.sha1 when the keychain holds it, else with a personal
# "AltTabio Code Signing" certificate (scripts/mac/make-signing-cert.sh creates either), and ad hoc
# only when there is neither. ALTTABIO_KEYCHAIN names a keychain outside the search list to take
# them from, as the release workflow does.
set -euo pipefail

cd "$(dirname "$0")/../.."
usage="Usage: scripts/mac/build-app.sh [--debug | --universal]"
(( $# <= 1 )) || { print -u2 -- "$usage"; exit 2; }
profile=release
cargo_flags=(--release)
output=release
# Naming the target keeps these objects apart from a plain `cargo build`, which would otherwise
# rebuild everything each time the two alternate, since they differ in the deployment target.
targets=("$(rustc -vV | sed -n 's/^host: //p')")
case "${1:-}" in
    "") ;;
    --debug)
        profile=debug
        cargo_flags=()
        output=debug
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

# sed quits at the package version itself: head would close the pipe early, and pipefail counts
# the writer it cuts off as a failure.
version=$(sed -n 's/^version = "\(.*\)"/\1/p;/^version = /q' Cargo.toml)
app=target/mac/AltTabio.app
contents="$app/Contents"

# The binaries name the same oldest macOS as Info.plist; unset, rustc would mark them as running on
# macOS 11.
minimum=$(plutil -extract LSMinimumSystemVersion raw assets/mac/Info.plist)
export MACOSX_DEPLOYMENT_TARGET=$minimum
binaries=()
installed=
if [[ "$profile" == universal ]]; then
    installed=$(rustup target list --installed)
fi
for target in "${targets[@]}"; do
    if [[ "$profile" == universal && "$installed" != *"$target"* ]]; then
        rustup target add "$target"
    fi
    cargo build --quiet "${cargo_flags[@]}" --target "$target"
    binaries+=("target/$target/$output/AltTabio")
done

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

# macOS matches the grants against the certificate's SHA-1 hash, so codesign gets the hash. It
# offers only certificates trusted for code signing, at least on macOS 26, so an untrusted one is
# passed over here rather than failing in codesign; make-signing-cert.sh trusts what it adds.
keychain=()
codesign_keychain=()
if [[ -n "${ALTTABIO_KEYCHAIN:-}" ]]; then
    keychain=("$ALTTABIO_KEYCHAIN")
    codesign_keychain=(--keychain "$ALTTABIO_KEYCHAIN")
fi
identities=$(security find-identity -v -p codesigning "${keychain[@]}" 2>/dev/null) || identities=
pin_file=scripts/mac/release-certificate.sha1
identity=
if [[ -f "$pin_file" && "$identities" == *"$(<"$pin_file")"* ]]; then
    identity=$(<"$pin_file")
    certificate="the release certificate"
else
    for line in "${(@f)identities}"; do
        if [[ "$line" == *'"AltTabio Code Signing"'* ]]; then
            # "  1) <SHA-1> "AltTabio Code Signing" (...)"
            identity=${${line##*\) }%% *}
            certificate="the personal 'AltTabio Code Signing' certificate"
            break
        fi
    done
fi
if [[ -n "$identity" ]]; then
    codesign --force --sign "$identity" "${codesign_keychain[@]}" \
        --identifier com.vibeslop.AltTabio "$app"
    echo "Signed $app with $certificate"
else
    codesign --force --sign - --identifier com.vibeslop.AltTabio "$app"
    echo "Signed $app ad hoc; run scripts/mac/make-signing-cert.sh once to keep permissions across builds"
fi
echo "Built $app ($profile, version $version)"

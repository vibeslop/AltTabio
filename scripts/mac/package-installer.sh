#!/bin/sh
# Package the installer script in an image authenticated before mounting or interpretation.
# Called by package.sh in the existing protected macOS release environment.
set -eu
cd "$(dirname "$0")/../.."

release=$(cat scripts/mac/release-certificate.sha1)
image=target/mac/AltTabio-macos-installer.dmg
work=$(mktemp -d)
mounted=false
trap 'if ! $mounted || hdiutil detach "$work/mount" -quiet; then rm -rf "$work"; fi' EXIT
mkdir "$work/content" "$work/mount"
cp scripts/mac/install.sh "$work/content/install.sh"
mkdir -p target/mac
hdiutil create -quiet -fs HFS+ -format UDZO -volname AltTabioInstaller \
    -srcfolder "$work/content" -ov "$image"
set --
if [ -n "${ALTTABIO_KEYCHAIN:-}" ]; then
    set -- --keychain "$ALTTABIO_KEYCHAIN"
fi
codesign --force --sign "$release" "$@" --timestamp=none \
    --identifier com.vibeslop.AltTabio.installer "$image"
# Verify the exact artifact and read back its sealed script before publication.
codesign --verify --strict \
    -R "=identifier \"com.vibeslop.AltTabio.installer\" and certificate leaf = H\"$release\"" "$image"
hdiutil attach "$image" -readonly -nobrowse -mountpoint "$work/mount" -quiet
mounted=true
cmp scripts/mac/install.sh "$work/mount/install.sh"
shasum -a 256 "$image"

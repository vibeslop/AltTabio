#!/bin/zsh
# Builds the universal AltTabio.app and zips it for GitHub Releases.
#
# Usage: scripts/mac/package.sh
#
# Output: target/mac/AltTabio-<version>-macos.zip, to be attached to the v<version> release, where
# docs/install.sh looks for it.
#
# Users keep their Accessibility and Screen Recording grants across updates only while every
# release is signed with the same certificate, so an ad-hoc signed bundle is refused here.
set -euo pipefail

cd "$(dirname "$0")/../.."
scripts/mac/build-app.sh --universal

app=target/mac/AltTabio.app
if ! codesign -dvv "$app" 2>&1 | grep -qx 'Authority=AltTabio Code Signing'; then
    echo "$app is not signed with the 'AltTabio Code Signing' certificate." >&2
    echo "Run scripts/mac/make-signing-cert.sh once, or import the backup of the certificate" >&2
    echo "that signed the earlier releases." >&2
    exit 1
fi

version=$(plutil -extract CFBundleShortVersionString raw "$app/Contents/Info.plist")
archive=target/mac/AltTabio-$version-macos.zip
rm -f "$archive"
ditto -c -k --keepParent "$app" "$archive"
echo "Packaged $archive for the v$version release"
shasum -a 256 "$archive"

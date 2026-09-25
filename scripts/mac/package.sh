#!/bin/zsh
# Builds the universal AltTabio.app and zips it for GitHub Releases.
#
# Usage: scripts/mac/package.sh
#
# Output: target/mac/AltTabio-<version>-macos.zip, to be attached to the v<version> release, where
# docs/install.sh looks for it.
#
# Users keep their Accessibility and Screen Recording grants across updates only while every
# release is signed with the same certificate. scripts/mac/release-certificate.sha1 names it by the
# SHA-1 hash macOS records for it. The first release writes that file; later releases refuse any
# other certificate, including a new one that carries the same name.
set -euo pipefail

cd "$(dirname "$0")/../.."
pin_file=scripts/mac/release-certificate.sha1
scripts/mac/build-app.sh --universal

app=target/mac/AltTabio.app
codesign --verify --deep --strict "$app"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# Writes the signing certificate to certificate0; an ad-hoc signature has none, and neither does a
# bundle codesign cannot read, which the check below reports the same way.
codesign -d --extract-certificates="$work/certificate" "$app" 2>/dev/null || true
if [[ ! -s "$work/certificate0" ]]; then
    print -u2 -- "$app is signed ad hoc, not with the release certificate."
    if [[ -f "$pin_file" ]]; then
        print -u2 -- "Restore the certificate from its backup:"
        print -u2 -- "  scripts/mac/make-signing-cert.sh --import <backup.p12>"
    else
        print -u2 -- "Create it once with scripts/mac/make-signing-cert.sh."
    fi
    exit 1
fi
fingerprint=$(shasum -a 1 <"$work/certificate0" | awk '{ print toupper($1) }')

if [[ ! -f "$pin_file" ]]; then
    print -- "$fingerprint" >"$pin_file"
    print -- "This is the first release, so $pin_file now pins its certificate:"
    print -- "  $fingerprint"
    print -- "Commit the file; later releases refuse any other certificate."
elif [[ "$(<"$pin_file")" != "$fingerprint" ]]; then
    print -u2 -- "$app is signed with a certificate that releases do not use:"
    print -u2 -- "  this build  $fingerprint"
    print -u2 -- "  releases    $(<"$pin_file")  ($pin_file)"
    print -u2 -- "A release with another certificate makes every user grant Accessibility and Screen"
    print -u2 -- "Recording again. Delete the 'AltTabio Code Signing' certificate in Keychain Access"
    print -u2 -- "and restore the release one from its backup:"
    print -u2 -- "  scripts/mac/make-signing-cert.sh --import <backup.p12>"
    exit 1
fi

version=$(plutil -extract CFBundleShortVersionString raw "$app/Contents/Info.plist")
archive=target/mac/AltTabio-$version-macos.zip
rm -f "$archive"
ditto -c -k --keepParent "$app" "$archive"
print -- "Packaged $archive for the v$version release"
shasum -a 256 "$archive"

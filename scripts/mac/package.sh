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
# SHA-1 hash macOS records for it, and anything signed with another certificate is refused. The
# release workflow runs this script on a version tag; by hand, it needs the release certificate
# imported with scripts/mac/make-signing-cert.sh --import.
set -euo pipefail

cd "$(dirname "$0")/../.."
pin_file=scripts/mac/release-certificate.sha1
if [[ ! -f "$pin_file" ]]; then
    print -u2 -- "$pin_file pins no release certificate yet. Create it once with"
    print -u2 -- "  scripts/mac/make-signing-cert.sh --release"
    exit 1
fi
release=$(<"$pin_file")
scripts/mac/build-app.sh --universal

app=target/mac/AltTabio.app
codesign --verify --deep --strict "$app"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# Writes the signing certificate to certificate0; an ad-hoc signature has none, and neither does a
# bundle codesign cannot read, which the check below reports the same way.
codesign -d --extract-certificates="$work/certificate" "$app" 2>/dev/null || true
fingerprint=
if [[ -s "$work/certificate0" ]]; then
    fingerprint=$(shasum -a 1 <"$work/certificate0" | awk '{ print toupper($1) }')
fi
if [[ "$fingerprint" != "$release" ]]; then
    print -u2 -- "$app is not signed with the release certificate:"
    print -u2 -- "  this build  ${fingerprint:-ad hoc}"
    print -u2 -- "  releases    $release  ($pin_file)"
    print -u2 -- "A release with another certificate makes every user grant Accessibility and Screen"
    print -u2 -- "Recording again. Push the version tag to have the release workflow build it, or"
    print -u2 -- "import the certificate from its backup in the maintainers' vault:"
    print -u2 -- "  scripts/mac/make-signing-cert.sh --import <backup.p12>"
    exit 1
fi

version=$(plutil -extract CFBundleShortVersionString raw "$app/Contents/Info.plist")
archive=target/mac/AltTabio-$version-macos.zip
rm -f "$archive"
ditto -c -k --keepParent "$app" "$archive"
print -- "Packaged $archive for the v$version release"
shasum -a 256 "$archive"

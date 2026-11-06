#!/bin/sh
# Verify the single executable in an AltTabio bundle, including every universal slice.
# This checks signing policy, not runtime behavior or TCC grant continuity.
set -euf
LC_ALL=C
export LC_ALL

if [ "$#" -ne 1 ]; then
    printf '%s\n' 'Usage: sh scripts/mac/verify-hardened-runtime.sh <AltTabio.app>' >&2
    exit 2
fi
app=$1
codesign --verify --deep --strict --all-architectures "$app"
architectures=$(lipo -archs "$app/Contents/MacOS/AltTabio")
if [ -z "$architectures" ]; then
    printf '%s\n' "$app has no executable architectures" >&2
    exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
for arch in $architectures; do
    signature=$(codesign --display --verbose=4 --arch "$arch" "$app" 2>&1)
    if ! printf '%s\n' "$signature" |
        grep -Eq '^CodeDirectory .* flags=0x[[:xdigit:]]+\(([^)]*,)?runtime([,)])'; then
        printf '%s\n' "$app ($arch) is missing the hardened runtime signature flag" >&2
        exit 1
    fi

    codesign --display --arch "$arch" --entitlements - "$app" > "$work/entitlements.plist"
    if [ -s "$work/entitlements.plist" ]; then
        # AltTabio needs no entitlements. Fail closed on malformed plists or any added key,
        # including DYLD, library-validation, executable-memory, and debugger exceptions.
        entitlements=$(plutil -convert json -o - "$work/entitlements.plist")
        if [ "$(printf '%s' "$entitlements" | tr -d '[:space:]')" != '{}' ]; then
            printf '%s\n' "$app ($arch) has unexpected entitlements" >&2
            exit 1
        fi
    fi
done

#!/bin/zsh
# Compiles the Icon Composer document assets/mac/AltTabio.icon into the files the app bundle
# ships: Assets.car, which macOS 26 and later draw in every icon appearance (default, dark, clear,
# tinted), and AltTabio.icns. actool comes with Xcode rather than the Command Line Tools, so the
# compiled files are committed and build-app.sh only copies them. Run this after changing the icon.
#
# Usage: scripts/mac/compile-icon.sh
set -euo pipefail

cd "$(dirname "$0")/../.."
# actool hands the work to a long-lived server that resolves relative paths against its own working
# directory, so every path it sees is absolute.
out=$PWD/assets/mac/icon
minimum=$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' assets/mac/Info.plist)
partial=$(mktemp)
trap 'rm -f "$partial"' EXIT

rm -rf "$out"
mkdir -p "$out"
if ! log=$(xcrun actool "$PWD/assets/mac/AltTabio.icon" --compile "$out" --platform macosx \
    --minimum-deployment-target "$minimum" --app-icon AltTabio \
    --output-partial-info-plist "$partial" --output-format human-readable-text \
    --errors --warnings 2>&1); then
    print -u2 -- "$log"
    exit 1
fi

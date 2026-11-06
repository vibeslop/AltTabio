#!/bin/sh
# Trusted bootstrap, copied verbatim into README.md. Run a local trusted copy only.
# The release certificate is embedded here, never downloaded alongside the installer.
set -eu

work=$(mktemp -d)
mounted=false
trap 'if ! $mounted || hdiutil detach "$work/mount" -quiet; then rm -rf "$work"; fi' EXIT
trap 'exit 1' HUP INT TERM
# Bound actual file writes even when Content-Length is absent or an older curl only
# checks advertised sizes. ulimit -f blocks can be 512 or 1024 bytes; use the larger unit.
fetch() (
    ulimit -c 0
    ulimit -f "$(($3 / 1024))"
    curl --fail --location --silent --show-error --proto '=https' --proto-redir '=https' \
        --connect-timeout 15 --max-time "$4" --max-filesize "$3" --output "$2" -- "$1"
    [ "$(wc -c < "$2")" -le "$3" ]
)
fetch 'https://api.github.com/repos/vibeslop/AltTabio/releases?per_page=30' "$work/releases.json" 4194304 30
url=$(osascript -l JavaScript -e 'function run() {
        ObjC.import("Foundation");
        const data = $.NSFileHandle.fileHandleWithStandardInput.readDataToEndOfFile;
        const releases = JSON.parse($.NSString.alloc.initWithDataEncoding(data, $.NSUTF8StringEncoding).js);
        for (const release of releases) {
            if (release.draft || release.prerelease) continue;
            const asset = release.assets.find(a => a.name === "AltTabio-macos-installer.dmg");
            if (asset) return asset.browser_download_url;
        }
        return "";
    }' < "$work/releases.json")
[ -n "$url" ] || { echo 'No signed macOS installer is published yet.' >&2; exit 1; }
fetch "$url" "$work/installer.dmg" 33554432 180
codesign --verify --strict -R '=identifier "com.vibeslop.AltTabio.installer" and certificate leaf = H"617A82407E9767025EDE4A62A9B990ADEA21F53B"' "$work/installer.dmg"
mkdir "$work/mount"
hdiutil attach "$work/installer.dmg" -readonly -nobrowse -mountpoint "$work/mount" -quiet
mounted=true
/bin/sh "$work/mount/install.sh"

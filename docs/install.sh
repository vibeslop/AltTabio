#!/bin/sh
# Installs or updates AltTabio on macOS from GitHub Releases:
#
#   curl -fsSL https://vibeslop.github.io/AltTabio/install.sh | sh
#
# AltTabio is not notarized, which takes a paid Apple developer account. Browsers mark what they
# download as quarantined, and Gatekeeper refuses to open a quarantined app that is not notarized.
# curl sets no quarantine flag, so the copy installed here opens right away.
#
# Everything runs from main, so a download cut short by the pipe runs nothing.
set -eu

repo=vibeslop/AltTabio

# Reads the GitHub API's release list on stdin and prints the tag of the newest published release
# that carries a macOS archive, and the archive's URL. Windows and macOS share releases, so the
# newest release may have no macOS archive. macOS ships no JSON tool for the shell, so JavaScript
# for Automation parses it.
pick_release='
ObjC.import("Foundation");
function run() {
    const input = $.NSFileHandle.fileHandleWithStandardInput.readDataToEndOfFile;
    const text = $.NSString.alloc.initWithDataEncoding(input, $.NSUTF8StringEncoding).js;
    for (const release of JSON.parse(text)) {
        if (release.draft || release.prerelease) continue;
        const name = `AltTabio-${release.tag_name.replace(/^v/, "")}-macos.zip`;
        const asset = release.assets.find((asset) => asset.name === name);
        if (asset) return `${release.tag_name} ${asset.browser_download_url}`;
    }
    return "";
}'

fail() {
    printf 'AltTabio: %s\n' "$1" >&2
    exit 1
}

# The SHA-1 hash of the certificate that signed the app at $1, which macOS ties the permissions
# to; empty for an ad-hoc signature.
certificate() {
    rm -f "$work/certificate0"
    codesign -d --extract-certificates="$work/certificate" "$1" 2>/dev/null || return 0
    [ -s "$work/certificate0" ] || return 0
    shasum -a 1 <"$work/certificate0" | cut -c1-40
}

main() {
    macos=$(sw_vers -productVersion)
    if [ "${macos%%.*}" -lt 26 ]; then
        fail "AltTabio needs macOS 26 or later; this Mac runs macOS $macos."
    fi

    releases=$(curl -fsSL "https://api.github.com/repos/$repo/releases?per_page=30") ||
        fail "could not get the list of releases from GitHub."
    release=$(printf '%s' "$releases" | osascript -l JavaScript -e "$pick_release") ||
        fail "could not read the list of releases from GitHub."
    [ -n "$release" ] || fail "no release has a macOS download yet."
    tag=${release%% *}
    url=${release#* }
    version=${tag#v}

    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    printf 'Downloading AltTabio %s\n' "$version"
    curl -fL --progress-bar -o "$work/AltTabio.zip" "$url" || fail "could not download $url."
    ditto -x -k "$work/AltTabio.zip" "$work" || fail "the download is not a readable archive."
    [ -d "$work/AltTabio.app" ] || fail "the download holds no AltTabio.app."
    # A damaged download fails here, before anything is replaced.
    codesign --verify --deep --strict "$work/AltTabio.app" ||
        fail "the downloaded AltTabio.app fails its signature check; nothing was changed."
    new_certificate=$(certificate "$work/AltTabio.app")
    [ -n "$new_certificate" ] ||
        fail "the downloaded AltTabio.app is not signed with a certificate; nothing was changed."

    # An update replaces the installed copy. A first install goes to /Applications unless this
    # account cannot write there.
    if [ -d /Applications/AltTabio.app ]; then
        target=/Applications
    elif [ -d "$HOME/Applications/AltTabio.app" ] || [ ! -w /Applications ]; then
        target=$HOME/Applications
        mkdir -p "$target"
    else
        target=/Applications
    fi
    [ -w "$target" ] ||
        fail "this account cannot replace $target/AltTabio.app; run the command as an administrator."

    # A running copy keeps the old code and turns the new one away as a second instance. Only
    # this account's copy is asked to quit; another account's is not ours to stop.
    account=$(id -u)
    if pkill -x -U "$account" AltTabio; then
        tries=0
        while pgrep -x -U "$account" AltTabio >/dev/null; do
            tries=$((tries + 1))
            [ "$tries" -le 50 ] || fail "the running AltTabio did not quit."
            sleep 0.1
        done
    fi

    updating=false
    old_certificate=
    if [ -e "$target/AltTabio.app" ]; then
        updating=true
        old_certificate=$(certificate "$target/AltTabio.app")
        mv "$target/AltTabio.app" "$work/previous.app"
    fi
    mv "$work/AltTabio.app" "$target/AltTabio.app"
    open "$target/AltTabio.app"

    if $updating; then
        printf 'Updated AltTabio in %s to %s.\n' "$target" "$version"
        if [ "$old_certificate" != "$new_certificate" ]; then
            printf '%s\n' \
                "The new copy is signed with another certificate than the one it replaced, so" \
                "macOS asks for Accessibility and Screen Recording again."
        fi
        return
    fi
    printf 'Installed AltTabio %s in %s. It runs from the menu bar.\n\n' "$version" "$target"
    printf '%s\n' \
        "AltTabio now asks for two permissions, allowed in System Settings > Privacy & Security:" \
        "  Accessibility, so AltTabio can take over Cmd+Tab and raise windows." \
        "  Screen Recording, for the live preview. After allowing it, quit AltTabio from" \
        "  its menu bar icon and open it again."
}

main

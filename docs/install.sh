#!/bin/sh
# Installs or updates AltTabio on macOS from the latest GitHub release:
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

fail() {
    printf 'AltTabio: %s\n' "$1" >&2
    exit 1
}

main() {
    macos=$(sw_vers -productVersion)
    if [ "${macos%%.*}" -lt 26 ]; then
        fail "AltTabio needs macOS 26 or later; this Mac runs macOS $macos."
    fi

    # The latest-release page redirects to its tag, and the tag names the archive.
    latest=$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
        "https://github.com/$repo/releases/latest") || fail "could not reach GitHub."
    tag=${latest##*/}
    version=${tag#v}

    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    printf 'Downloading AltTabio %s\n' "$version"
    curl -fL --progress-bar -o "$work/AltTabio.zip" \
        "https://github.com/$repo/releases/download/$tag/AltTabio-$version-macos.zip" ||
        fail "release $tag has no macOS download."
    ditto -x -k "$work/AltTabio.zip" "$work"
    [ -d "$work/AltTabio.app" ] || fail "the download holds no AltTabio.app."

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

    # A running copy keeps the old code and turns the new one away as a second instance.
    if pkill -x AltTabio; then
        tries=0
        while pgrep -x AltTabio >/dev/null; do
            tries=$((tries + 1))
            [ "$tries" -le 50 ] || fail "the running AltTabio did not quit."
            sleep 0.1
        done
    fi

    updating=false
    if [ -e "$target/AltTabio.app" ]; then
        updating=true
        mv "$target/AltTabio.app" "$work/previous.app"
    fi
    mv "$work/AltTabio.app" "$target/AltTabio.app"
    open "$target/AltTabio.app"

    if $updating; then
        printf 'Updated AltTabio in %s to %s.\n' "$target" "$version"
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

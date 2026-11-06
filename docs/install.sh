#!/bin/sh
# Installs or updates AltTabio on macOS from inside the signed release installer image.
# Use the trusted bootstrap in README.md to authenticate that image before executing this script.
# Never pipe the Pages-hosted copy into a shell; its own checks cannot authenticate this script.
#
# AltTabio is not notarized, which takes a paid Apple developer account. Browsers mark what they
# download as quarantined, and Gatekeeper refuses to open a quarantined app that is not notarized.
# curl sets no quarantine flag, so the copy installed here opens right away.
#
# Everything runs from main.
set -eu

repo=vibeslop/AltTabio
# Keep this standalone installer aligned with scripts/mac/release-certificate.sha1.
release_certificate=617A82407E9767025EDE4A62A9B990ADEA21F53B

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
        if (asset) return `${release.tag_name}\t${asset.browser_download_url}`;
    }
    return "";
}'

work=
staged=
previous=

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

cleanup() {
    # Cut short between the two renames, the install has set the old copy aside with no new one
    # in its place; put it back.
    if [ -n "$previous" ] && [ -e "$previous" ] && [ ! -e "$target/AltTabio.app" ]; then
        if ! mv "$previous" "$target/AltTabio.app"; then
            printf 'AltTabio: the previous copy is at %s\n' "$previous" >&2
            previous=
        fi
    fi
    [ -z "$previous" ] || rm -rf "$previous"
    [ -z "$staged" ] || rm -rf "$staged"
    [ -z "$work" ] || rm -rf "$work"
}

# Reject untrusted downloads before changing the installed copy.
verify_bundle() {
    codesign --verify --deep --strict \
        -R "=identifier \"com.vibeslop.AltTabio\" and certificate leaf = H\"$release_certificate\"" "$1" ||
        fail "the downloaded AltTabio.app fails its signature or release identity check; nothing was changed."
}

# These helpers also run from the in-app updater, which embeds this script. No archive is
# passed to ditto: unzip streams only individually selected members into regular files.
# File-size limits constrain actual decoder output, even if ZIP sizes or CRCs are forged.
download_archive() (
    PATH=/usr/bin:/bin:/usr/sbin:/sbin
    export PATH
    ulimit -c 0 || exit 1
    # sh implementations count -f blocks as 512 or 1024 bytes. Use the larger unit so the
    # limit is never above 100 MiB, including when curl receives no Content-Length.
    ulimit -f 102400 || exit 1
    curl --fail --location --silent --show-error --proto '=https' --proto-redir '=https' \
        --max-time 600 --max-filesize 104857600 --output "$2" -- "$1"
)

# Some unzip builds return success after stdout hits a file-size limit. Let dd own the
# bounded file instead, and require both its exit status and unzip's completion marker.
bounded_unzip() (
    output=$1
    blocks=$2
    shift 2
    status=$destination/.archive-status
    rm -f "$status" || exit 1
    (ulimit -t 10 && unzip "$@" < /dev/null 2>/dev/null && printf ok > "$status") |
        (ulimit -f "$blocks" && ulimit -t 10 && dd bs=65536 of="$output" 2>/dev/null) || exit 1
    [ -s "$status" ] || exit 1
    rm "$status" || exit 1
)

extract_archive() (
    PATH=/usr/bin:/bin:/usr/sbin:/sbin
    export PATH
    LC_ALL=C
    export LC_ALL
    # Limit signals must not turn a rejected archive into a large core dump.
    ulimit -c 0 || exit 1
    # Deliberately accept a small release envelope: <=100 MiB compressed, <=256 entries,
    # <=32 KiB of names, <=64 MiB per file, <=256 MiB actually written, <=16 path levels.
    # A future bundle that needs more must change these limits explicitly.
    archive=$1
    destination=$2
    [ "$(wc -c < "$archive")" -le 104857600 ] || exit 1
    mkdir "$destination" || exit 1
    listing=$destination/.archive-members
    # Bound the listing itself before counting entries, and contain parser CPU usage.
    bounded_unzip "$listing" 32 -Z -1 "$archive" || exit 1
    [ -s "$listing" ] && [ "$(wc -c < "$listing")" -le 32768 ] || exit 1
    [ "$(wc -l < "$listing")" -le 256 ] || exit 1
    entries=0
    deadline=$(( $(date +%s) + 120 ))
    remaining=268435456
    while IFS= read -r member; do
        entries=$((entries + 1))
        [ "$entries" -le 256 ] && [ "${#member}" -le 240 ] || exit 1
        [ "$(date +%s)" -le "$deadline" ] || exit 1
        # Exclude wildcard matching, control characters, traversal, absolute paths and
        # ambiguous components. Names are ASCII and rooted in the one app bundle.
        case "$member" in
            ''|*[!a-zA-Z0-9._/-]*|/*|*//*|*/./*|*/../*|./*|../*) exit 1 ;;
        esac
        case "/$member/" in */../*|*/./*) exit 1 ;; esac
        depth=$(printf '%s' "$member" | tr -cd / | wc -c)
        [ "$depth" -le 16 ] || exit 1
        case "$member" in
            # ditto's AppleDouble metadata is not needed by our signed bundle. Never
            # decode it, or any other non-bundle payload.
            __MACOSX/|__MACOSX/*) continue ;;
            AltTabio.app/|AltTabio.app/*) ;;
            *) exit 1 ;;
        esac
        output=$destination/$member
        case "$member" in
            */) mkdir -p "$output" || exit 1; continue ;;
        esac
        # We create no archive-specified symlinks, devices or permissions. Duplicate and
        # case-colliding files fail rather than replacing an earlier member.
        [ ! -e "$output" ] || exit 1
        mkdir -p "${output%/*}" || exit 1
        limit=67108864
        [ "$remaining" -ge "$limit" ] || limit=$remaining
        [ "$limit" -ge 1024 ] || exit 1
        blocks=$((limit / 1024))
        bounded_unzip "$output" "$blocks" -p -P '' "$archive" "$member" || exit 1
        size=$(wc -c < "$output")
        [ "$size" -le "$limit" ] || exit 1
        remaining=$((remaining - size))
    done < "$listing"
    [ "$entries" -gt 0 ] && [ "$(date +%s)" -le "$deadline" ] || exit 1
    rm "$listing" || exit 1
    [ -d "$destination/AltTabio.app" ] || exit 1
    # unzip -p intentionally does not restore executable bits or extended attributes.
    # Only the app's known executable needs execute permission; codesign checks it next.
    chmod 755 "$destination/AltTabio.app/Contents/MacOS/AltTabio" || exit 1
)

# Stable releases use three unsigned 32-bit numbers. Require canonical decimal spelling.
# Reject whitespace and suffixes rather than normalizing signed bundle metadata.
valid_version() {
    printf '%s\n' "$1" | LC_ALL=C awk -F. '
        NR != 1 || NF != 3 { exit 1 }
        {
            for (i = 1; i <= 3; i++) {
                if ($i !~ /^(0|[1-9][0-9]*)$/ || length($i) > 10 || $i + 0 > 4294967295)
                    exit 1
            }
        }'
}

bundle_version() {
    # Preserve trailing newlines in the value so shell interpolation cannot hide a mismatch.
    found=$(plutil -extract CFBundleShortVersionString raw -expect string -n -o - "$1/Contents/Info.plist" && printf '.') ||
        fail "could not read the version of $1; nothing was changed."
    found=${found%.}
    valid_version "$found" || fail "invalid version in $1; nothing was changed."
    printf '%s\n' "$found"
}

# Both arguments have already passed valid_version, so numeric comparison is exact in awk.
version_at_least() {
    LC_ALL=C awk -v candidate="$1" -v installed="$2" 'BEGIN {
        split(candidate, c, ".")
        split(installed, p, ".")
        for (i = 1; i <= 3; i++) {
            if (c[i] + 0 > p[i] + 0) exit 0
            if (c[i] + 0 < p[i] + 0) exit 1
        }
        exit 0
    }'
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
    separator=$(printf '\t')
    tag=${release%%"$separator"*}
    url=${release#*"$separator"}
    version=${tag#v}
    valid_version "$version" || fail "the selected release has an invalid version."

    work=$(mktemp -d)
    trap cleanup EXIT
    # Ctrl-C and the like exit through the cleanup too.
    trap 'exit 130' INT
    trap 'exit 143' TERM
    trap 'exit 129' HUP
    printf 'Downloading AltTabio %s\n' "$version"
    download_archive "$url" "$work/AltTabio.zip" || fail "could not download $url within the archive limits."
    extract_archive "$work/AltTabio.zip" "$work/extracted" ||
        fail "the download is malformed or exceeds the archive limits."
    work_app=$work/extracted/AltTabio.app
    [ -d "$work_app" ] || fail "the download holds no AltTabio.app."
    verify_bundle "$work_app"
    found=$(bundle_version "$work_app") || exit 1
    [ "$found" = "$version" ] ||
        fail "the download of AltTabio $version holds version $found; nothing was changed."
    new_certificate=$(certificate "$work_app")
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

    # Refuse a replay of an older release even when its metadata and signed version agree.
    # A copy set aside by an interrupted install is still the installed version for this check.
    installed=$target/AltTabio.app
    if [ ! -e "$installed" ] && [ -e "$target/.AltTabio.app.old" ]; then
        installed=$target/.AltTabio.app.old
    fi
    if [ -e "$installed" ]; then
        installed_version=$(bundle_version "$installed") || exit 1
        version_at_least "$version" "$installed_version" ||
            fail "refusing to replace AltTabio $installed_version with older version $version; nothing was changed."
    fi

    # The new copy goes next to the old one first, so the swap below is two renames on one volume.
    staged=$target/.AltTabio.app.new
    previous=$target/.AltTabio.app.old
    # A run killed outright between the renames left the old copy aside; it comes back first.
    if [ -e "$previous" ] && [ ! -e "$target/AltTabio.app" ]; then
        mv "$previous" "$target/AltTabio.app"
    fi
    rm -rf "$staged" "$previous"
    ditto "$work_app" "$staged" || fail "could not copy AltTabio.app into $target."

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
        mv "$target/AltTabio.app" "$previous"
    fi
    mv "$staged" "$target/AltTabio.app"
    staged=
    rm -rf "$previous"
    previous=
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
        "AltTabio now asks for Accessibility, so it can take over Cmd+Tab and raise windows." \
        "After your first Cmd+Tab it asks for Screen Recording, which window previews need."
}

# Internal entry points let the updater use the identical resource limits without running
# discovery or installation. Normal curl | sh usage still enters main.
case "${1:-}" in
    --download-update-archive) [ "$#" -eq 3 ] && download_archive "$2" "$3" ;;
    --extract-update-archive) [ "$#" -eq 3 ] && extract_archive "$2" "$3" ;;
    '') main ;;
    *) fail "unexpected installer arguments." ;;
esac

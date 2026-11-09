#!/bin/sh
# Exercise the installer's trust gate without downloads or installation side effects.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
installer=$root/scripts/mac/install.sh
expected_certificate=$(tr -d '[:space:]' < "$root/scripts/mac/release-certificate.sha1")
expected_requirement="=identifier \"com.vibeslop.AltTabio\" and certificate leaf = H\"$expected_certificate\""
fixture_parent=$(CDPATH= cd -- "${TMPDIR:-/tmp}" && pwd -P)
fixture=$(mktemp -d "$fixture_parent/alttabio-installer-test.XXXXXXXX")
case $fixture in
    "$fixture_parent"/alttabio-installer-test.*) ;;
    *) printf '%s\n' 'Unexpected fixture path' >&2; exit 1 ;;
esac
trap 'rm -rf "$fixture"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
mkdir "$fixture/bin" "$fixture/AltTabio.app"
# Load the real helpers, excluding main's definition and invocation. Never run the installer.
sed '/^main() {$/,$d' "$installer" > "$fixture/functions.sh"
cat > "$fixture/bin/codesign" <<'MOCK'
#!/bin/sh
set -eu
# Signature integrity alone also accepts intact ad-hoc and differently signed applications.
if [ "$TEST_SIGNING_CASE" = corrupt ]; then exit 1; fi
if [ "$#" -eq 4 ]; then exit 0; fi
if [ "$#" -ne 6 ] || [ "$1" != --verify ] || [ "$2" != --deep ] ||
   [ "$3" != --strict ] || [ "$4" != -R ] || [ "$5" != "$EXPECTED_REQUIREMENT" ]; then
    exit 1
fi
[ "$TEST_SIGNING_CASE" = trusted ]
MOCK
chmod +x "$fixture/bin/codesign"
original_path=$PATH
for signing_case in wrong_certificate wrong_identifier adhoc corrupt; do
    if (
        PATH="$fixture/bin:$original_path"
        export PATH
        TEST_SIGNING_CASE=$signing_case
        EXPECTED_REQUIREMENT=$expected_requirement
        export TEST_SIGNING_CASE EXPECTED_REQUIREMENT
        . "$fixture/functions.sh"
        verify_bundle "$fixture/AltTabio.app"
    ) > "$fixture/output" 2>&1; then
        printf 'Installer accepted %s\n' "$signing_case" >&2
        exit 1
    fi
done
if ! (
    PATH="$fixture/bin:$original_path"
    export PATH
    TEST_SIGNING_CASE=trusted
    EXPECTED_REQUIREMENT=$expected_requirement
    export TEST_SIGNING_CASE EXPECTED_REQUIREMENT
    . "$fixture/functions.sh"
    verify_bundle "$fixture/AltTabio.app"
) > "$fixture/output" 2>&1; then
    cat "$fixture/output" >&2
    printf '%s\n' 'Installer rejected the pinned signing identity' >&2
    exit 1
fi
# On macOS also exercise real codesign and plist extraction against disposable fixtures.
if [ "$(uname -s)" = Darwin ]; then
    cp /usr/bin/true "$fixture/adhoc"
    /usr/bin/codesign --force --sign - --identifier com.vibeslop.AltTabio "$fixture/adhoc" 2>/dev/null
    /usr/bin/codesign --verify --strict "$fixture/adhoc"
    if (
        PATH=$original_path
        export PATH
        . "$fixture/functions.sh"
        verify_bundle "$fixture/adhoc"
    ) > "$fixture/output" 2>&1; then
        printf '%s\n' 'Installer accepted a real ad-hoc signature' >&2
        exit 1
    fi
    mkdir -p "$fixture/metadata.app/Contents"
    plist=$fixture/metadata.app/Contents/Info.plist
    /usr/bin/plutil -create xml1 "$plist"
    /usr/bin/plutil -insert CFBundleShortVersionString -string 1.3.0 "$plist"
    (
        PATH=$original_path
        export PATH
        . "$fixture/functions.sh"
        found=$(bundle_version "$fixture/metadata.app") || exit 1
        [ "$found" = 1.3.0 ]
    )
    for invalid in '1.3.0 ' '1.3.0
' wrong_type missing; do
        case $invalid in
            wrong_type) /usr/bin/plutil -replace CFBundleShortVersionString -integer 130 "$plist" ;;
            missing) /usr/bin/plutil -remove CFBundleShortVersionString "$plist" ;;
            *) /usr/bin/plutil -replace CFBundleShortVersionString -string "$invalid" "$plist" ;;
        esac
        if (
            PATH=$original_path
            export PATH
            . "$fixture/functions.sh"
            bundle_version "$fixture/metadata.app"
        ) > "$fixture/output" 2>&1; then
            printf 'Installer accepted malformed plist version <%s>\n' "$invalid" >&2
            exit 1
        fi
    done
fi
printf '%s\n' 'Installer signing checks passed.'

# Run main in a disposable filesystem with mocked macOS/network commands. All application paths
# in this copy, including HOME's fallback, stay beneath the fixture. No installed app is touched.
# Override only transport in this disposable copy. A bounded extractor still uses its
# production system PATH and consumes the inert real ZIP prepared by run_install below.
bounded_archive=false
if grep -q '^extract_archive() (' "$installer"; then bounded_archive=true; fi
sed "s|/Applications|$fixture/Applications|g" "$installer" |
    awk '/^main\(\) \{$/ { print "download_archive() { cp \"$TEST_ARCHIVE\" \"$2\"; }" } { print }' > "$fixture/install.sh"
cat > "$fixture/bin/codesign" <<'MOCK'
#!/bin/sh
set -eu
if [ "$1" = -d ]; then
    printf 'fixture certificate' > "${2#--extract-certificates=}0"
else
    [ "$#" -eq 6 ] && [ "$1" = --verify ] && [ "$2" = --deep ] &&
        [ "$3" = --strict ] && [ "$4" = -R ] && [ "$5" = "$EXPECTED_REQUIREMENT" ] || exit 1
    printf '%s\n' verified >> "$TEST_EVENTS"
fi
MOCK
cat > "$fixture/bin/plutil" <<'MOCK'
#!/bin/sh
set -eu
[ "$#" -eq 9 ] && [ "$1" = -extract ] && [ "$2" = CFBundleShortVersionString ] &&
    [ "$3" = raw ] && [ "$4" = -expect ] && [ "$5" = string ] &&
    [ "$6" = -n ] && [ "$7" = -o ] && [ "$8" = - ] || exit 1
printf '%s\n' version >> "$TEST_EVENTS"
# Fixtures hold the raw version value; a missing file models a missing/unreadable plist key.
cat "$9"
MOCK
cat > "$fixture/bin/shasum" <<'MOCK'
#!/bin/sh
# Certificate extraction uses fixture bytes; keep their fake fingerprint stable across updates.
printf '%s\n' 0000000000000000000000000000000000000000
MOCK
cat > "$fixture/bin/sw_vers" <<'MOCK'
#!/bin/sh
printf '%s\n' 26.0
MOCK
cat > "$fixture/bin/curl" <<'MOCK'
#!/bin/sh
set -eu
if [ "$1" = -fsSL ]; then printf '[]'; else : > "$4"; fi
MOCK
cat > "$fixture/bin/osascript" <<'MOCK'
#!/bin/sh
printf 'v%s\thttps://example.invalid/AltTabio-%s-macos.zip\n' "$TEST_RELEASE" "$TEST_RELEASE"
MOCK
cat > "$fixture/bin/ditto" <<'MOCK'
#!/bin/sh
set -eu
if [ "$1" = -x ]; then
    mkdir -p "$4/AltTabio.app/Contents"
    if [ "$TEST_BUNDLE" != missing ]; then
        printf '%s' "$TEST_BUNDLE" > "$4/AltTabio.app/Contents/Info.plist"
    fi
else
    printf '%s\n' staged >> "$TEST_EVENTS"
    cp -R "$1" "$2"
fi
MOCK
cat > "$fixture/bin/pkill" <<'MOCK'
#!/bin/sh
printf '%s\n' stopped >> "$TEST_EVENTS"
exit 1
MOCK
cat > "$fixture/bin/open" <<'MOCK'
#!/bin/sh
printf '%s\n' opened >> "$TEST_EVENTS"
MOCK
cat > "$fixture/bin/mv" <<'MOCK'
#!/bin/sh
set -eu
case $1 in
    */.AltTabio.app.new) [ "$TEST_SWAP_FAILURE" = false ] || exit 1 ;;
esac
command -p mv "$@"
MOCK
chmod +x "$fixture/bin/"*

run_install() {
    if $bounded_archive; then
        TEST_BUNDLE="$2" python3 - "$fixture/download.zip" <<'ARCHIVE' || return 1
import os
import sys
import zipfile

with zipfile.ZipFile(sys.argv[1], "w", zipfile.ZIP_DEFLATED) as archive:
    archive.writestr("AltTabio.app/Contents/MacOS/AltTabio", "inert executable")
    if os.environ["TEST_BUNDLE"] != "missing":
        archive.writestr("AltTabio.app/Contents/Info.plist", os.environ["TEST_BUNDLE"])
ARCHIVE
    fi
    PATH="$fixture/bin:$original_path" HOME="$fixture/home" TMPDIR="$fixture" \
        TEST_ARCHIVE="$fixture/download.zip" \
        EXPECTED_REQUIREMENT="$expected_requirement" TEST_EVENTS="$fixture/events" \
        TEST_RELEASE="$1" TEST_BUNDLE="$2" TEST_SWAP_FAILURE="${3:-false}" \
        sh "$fixture/install.sh" > "$fixture/output" 2>&1
}

reset_install() {
    rm -rf "$fixture/Applications"
    mkdir "$fixture/Applications"
    : > "$fixture/events"
}

put_installed() {
    mkdir -p "$fixture/Applications/${2:-AltTabio.app}/Contents"
    printf '%s' "$1" > "$fixture/Applications/${2:-AltTabio.app}/Contents/Info.plist"
}

assert_rejected() {
    if run_install "$1" "$2"; then
        printf 'Installer accepted release <%s> with bundle <%s>\n' "$1" "$2" >&2
        exit 1
    fi
    if grep -Eq '^(staged|stopped|opened)$' "$fixture/events"; then
        cat "$fixture/output" >&2
        printf '%s\n' 'Rejected download reached staging or application control' >&2
        exit 1
    fi
    grep -q "$3" "$fixture/output" || { cat "$fixture/output" >&2; exit 1; }
}

# Identity-valid bundles must match the selected version exactly, in both directions.
for bundle in 1.2.9 1.3.1; do
    reset_install
    put_installed 1.3.0
    assert_rejected 1.3.0 "$bundle" 'holds version'
    [ "$(cat "$fixture/Applications/AltTabio.app/Contents/Info.plist")" = 1.3.0 ]
    # Signature verification precedes reading bundle metadata.
    [ "$(sed -n '1p' "$fixture/events")" = verified ]
done

# Missing metadata and unsupported spelling fail closed, including whitespace that command
# substitution would otherwise trim. Run each malformed value as tag, download, and installed copy.
for invalid in '' 1.3 1.3.0.0 v1.3.0 01.3.0 +1.3.0 1.3.0-beta 1.3.0+build \
    ' 1.3.0' '1.3.0 ' '1.3.0
' '1.3.0
1.3.0' 4294967296.0.0 999999999999999999999999999999.0.0; do
    reset_install
    assert_rejected "$invalid" 1.3.0 'invalid version'
    reset_install
    assert_rejected 1.3.0 "$invalid" 'invalid version'
    reset_install
    put_installed "$invalid"
    assert_rejected 1.3.0 1.3.0 'invalid version'
    [ -f "$fixture/Applications/AltTabio.app/Contents/Info.plist" ]
done
reset_install
assert_rejected 1.3.0 missing 'could not read the version'
put_installed 1.3.0
rm "$fixture/Applications/AltTabio.app/Contents/Info.plist"
assert_rejected 1.3.0 1.3.0 'could not read the version'

# Numeric component ordering, rather than lexical or whole-version numeric ordering.
for versions in '1.3.9 1.3.10' '1.9.99 1.10.0' '1.99.99 2.0.0' \
    '1.3.0 1.3.1' '0.0.0 0.0.1' '4294967294.0.0 4294967295.0.0'; do
    older=${versions% *}
    newer=${versions#* }
    reset_install
    put_installed "$newer"
    assert_rejected "$older" "$older" 'older version'
    [ "$(cat "$fixture/Applications/AltTabio.app/Contents/Info.plist")" = "$newer" ]
    reset_install
    put_installed "$older"
    run_install "$newer" "$newer" || { cat "$fixture/output" >&2; exit 1; }
    [ "$(cat "$fixture/Applications/AltTabio.app/Contents/Info.plist")" = "$newer" ]
    grep -q '^opened$' "$fixture/events"
done

# A first install and a same-version reinstall are allowed.
reset_install
run_install 1.3.0 1.3.0
run_install 1.3.0 1.3.0
[ "$(cat "$fixture/Applications/AltTabio.app/Contents/Info.plist")" = 1.3.0 ]

# Interrupted-install recovery cannot bypass freshness; a valid update still restores and swaps.
reset_install
put_installed 1.3.0 .AltTabio.app.old
assert_rejected 1.2.9 1.2.9 'older version'
[ "$(cat "$fixture/Applications/.AltTabio.app.old/Contents/Info.plist")" = 1.3.0 ]
run_install 1.3.1 1.3.1
[ "$(cat "$fixture/Applications/AltTabio.app/Contents/Info.plist")" = 1.3.1 ]

# If the final rename fails, cleanup still puts the installed version back.
reset_install
put_installed 1.3.0
if run_install 1.3.1 1.3.1 true; then
    printf '%s\n' 'Installer ignored a failed replacement' >&2
    exit 1
fi
[ "$(cat "$fixture/Applications/AltTabio.app/Contents/Info.plist")" = 1.3.0 ]
[ ! -e "$fixture/Applications/.AltTabio.app.old" ]
[ ! -e "$fixture/Applications/.AltTabio.app.new" ]
printf '%s\n' 'Installer version and rollback checks passed.'

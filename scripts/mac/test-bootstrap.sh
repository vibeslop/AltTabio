#!/bin/sh
# Exercise the complete trusted bootstrap without network, installation or release secrets.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture_parent=$(CDPATH= cd -- "${TMPDIR:-/tmp}" && pwd -P)
fixture=$(mktemp -d "$fixture_parent/alttabio-bootstrap-test.XXXXXXXX")
case $fixture in
    "$fixture_parent"/alttabio-bootstrap-test.*) ;;
    *) echo 'Unexpected fixture path' >&2; exit 1;;
esac
trap 'rm -rf "$fixture"' EXIT
trap 'exit 1' HUP INT TERM
mkdir "$fixture/bin"
export TEST_FIXTURE=$fixture
expected_certificate=$(tr -d '[:space:]' < "$root/scripts/mac/release-certificate.sha1")
export EXPECTED_REQUIREMENT="=identifier \"com.vibeslop.AltTabio.installer\" and certificate leaf = H\"$expected_certificate\""

# The copied command must execute exactly the reviewed local bootstrap, including its pin.
sed -n "/^\/bin\/sh <<'ALTTABIO_BOOTSTRAP'$/,/^ALTTABIO_BOOTSTRAP$/p" "$root/README.md" > "$fixture/copied.sh"
sed '1d;$d' "$fixture/copied.sh" > "$fixture/copied-body.sh"
sed '1,3d' "$root/scripts/mac/bootstrap.sh" > "$fixture/bootstrap-body.sh"
cmp "$fixture/bootstrap-body.sh" "$fixture/copied-body.sh"

cat > "$fixture/bin/curl" <<'MOCK'
#!/bin/sh
set -eu
output=
url=
while [ "$#" -gt 0 ]; do
    case $1 in
        --output|-o) output=$2; shift 2;;
        --proto|--proto-redir|--connect-timeout|--max-time|--max-filesize) shift 2;;
        --fail|--location|--silent|--show-error|--) shift;;
        *) url=$1; shift;;
    esac
done
[ -n "$output" ]
case $url in
    *api.github.com*)
        case $TEST_CASE in
            metadata_failure) printf '{' > "$output";;
            no_asset) printf '[]' > "$output";;
            *) cat > "$output" <<'RELEASES'
[{"draft":true,"assets":[{"name":"AltTabio-macos-installer.dmg","browser_download_url":"draft"}]},
 {"prerelease":true,"assets":[{"name":"AltTabio-macos-installer.dmg","browser_download_url":"prerelease"}]},
 {"assets":[{"name":"AltTabio-1.2.4-windows-x64.zip","browser_download_url":"windows"}]},
 {"assets":[{"name":"AltTabio-macos-installer.dmg","browser_download_url":"https://github.com/vibeslop/AltTabio/releases/download/v1.2.3/AltTabio-macos-installer.dmg"}]},
 {"assets":[{"name":"AltTabio-macos-installer.dmg","browser_download_url":"older"}]}]
RELEASES
            ;;
        esac
        ;;
    'https://github.com/vibeslop/AltTabio/releases/download/v1.2.3/AltTabio-macos-installer.dmg')
        [ "$TEST_CASE" != download_failure ] || exit 1
        printf '%s' "$TEST_CASE" > "$output"
        ;;
    *) exit 1;;
esac
MOCK
cat > "$fixture/bin/osascript" <<'MOCK'
#!/bin/sh
if [ "$(uname -s)" = Darwin ]; then exec /usr/bin/osascript "$@"; fi
cat >/dev/null
case $TEST_CASE in
    metadata_failure) exit 1;;
    no_asset) exit 0;;
esac
printf '%s\n' 'https://github.com/vibeslop/AltTabio/releases/download/v1.2.3/AltTabio-macos-installer.dmg'
MOCK
cat > "$fixture/bin/codesign" <<'MOCK'
#!/bin/sh
set -eu
printf 'verify\n' >> "$TEST_FIXTURE/events"
# Five arguments are required; a signature-only check must fail this regression test.
if [ "$#" -ne 5 ] || [ "$1" != --verify ] || [ "$2" != --strict ] ||
   [ "$3" != -R ] || [ "$4" != "$EXPECTED_REQUIREMENT" ] || [ ! -s "$5" ]; then
    exit 1
fi
case $TEST_CASE in
    trusted|mount_failure|installer_failure) exit 0;;
    *) exit 1;;
esac
MOCK
cat > "$fixture/bin/hdiutil" <<'MOCK'
#!/bin/sh
set -eu
case $1 in
    attach)
        printf 'mount\n' >> "$TEST_FIXTURE/events"
        [ "$TEST_CASE" != mount_failure ] || exit 1
        [ "$3" = -readonly ] && [ "$4" = -nobrowse ] && [ "$5" = -mountpoint ]
        cat > "$6/install.sh" <<'INSTALLER'
printf 'execute\n' >> "$TEST_FIXTURE/events"
[ "$TEST_CASE" != installer_failure ]
INSTALLER
        ;;
    detach) printf 'detach\n' >> "$TEST_FIXTURE/events";;
    *) exit 1;;
esac
MOCK
chmod +x "$fixture/bin/"*
original_path=$PATH
for test_case in missing_signature invalid_signature wrong_certificate wrong_identifier adhoc corrupt \
    metadata_failure no_asset download_failure mount_failure installer_failure trusted; do
    export TEST_CASE=$test_case
    : > "$fixture/events"
    result=0
    TMPDIR="$fixture" PATH="$fixture/bin:$original_path" /bin/sh "$fixture/copied.sh" > "$fixture/output" 2>&1 || result=$?
    case $test_case in
        metadata_failure|no_asset|download_failure) expected=;;
        mount_failure) expected=$(printf 'verify\nmount');;
        installer_failure|trusted) expected=$(printf 'verify\nmount\nexecute\ndetach');;
        *) expected=verify;;
    esac
    if [ "$(cat "$fixture/events")" != "$expected" ] ||
       { [ "$test_case" = trusted ] && [ "$result" -ne 0 ]; } ||
       { [ "$test_case" != trusted ] && [ "$result" -eq 0 ]; }; then
        cat "$fixture/output" "$fixture/events" >&2
        printf 'Bootstrap failed case: %s (exit %s)\n' "$test_case" "$result" >&2
        exit 1
    fi
done

# Native CI also verifies that the real macOS verifier rejects an intact ad-hoc image
# under the pinned requirement. No new release certificate or signing secret is needed.
if [ "$(uname -s)" = Darwin ]; then
    mkdir "$fixture/content"
    printf '%s\n' 'exit 1' > "$fixture/content/install.sh"
    hdiutil create -quiet -fs HFS+ -format UDZO -volname AltTabioTest \
        -srcfolder "$fixture/content" "$fixture/adhoc.dmg"
    codesign --force --sign - --identifier com.vibeslop.AltTabio.installer "$fixture/adhoc.dmg"
    codesign --verify --strict "$fixture/adhoc.dmg"
    if codesign --verify --strict -R "$EXPECTED_REQUIREMENT" "$fixture/adhoc.dmg" 2>/dev/null; then
        echo 'Bootstrap requirement accepted a real ad-hoc image' >&2
        exit 1
    fi
fi
printf '%s\n' 'Bootstrap trust and execution-order checks passed.'

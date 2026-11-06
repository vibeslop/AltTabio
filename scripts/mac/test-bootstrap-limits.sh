#!/bin/sh
# Exercise actual capture ceilings without relying on Content-Length or curl's size check.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
trap 'exit 1' HUP INT TERM
mkdir "$fixture/bin"
export TEST_FIXTURE=$fixture
cat > "$fixture/bin/curl" <<'MOCK'
#!/bin/sh
set -eu
output=
url=
deadline=
connect=
maximum=
https=false
redirect=false
while [ "$#" -gt 0 ]; do
    case $1 in
        --output|-o) output=$2; shift 2;;
        --max-time) deadline=$2; shift 2;;
        --connect-timeout) connect=$2; shift 2;;
        --max-filesize) maximum=$2; shift 2;;
        --proto) [ "$2" != '=https' ] || https=true; shift 2;;
        --proto-redir) [ "$2" != '=https' ] || redirect=true; shift 2;;
        -fsSL|-fL|--fail|--location|--silent|--show-error|--progress-bar|--) shift;;
        *) url=$1; shift;;
    esac
done
kind=image
case $url in *api.github.com*) kind=metadata;; esac
if [ "$TEST_CASE" = settings ]; then
    [ -n "$output" ] && [ -n "$maximum" ] && [ -n "$deadline" ] && [ -n "$connect" ]
    [ "$deadline" -gt 0 ] && [ "$deadline" -le 180 ] && [ "$connect" -le 15 ]
    $https && $redirect
fi
if [ "$TEST_CASE" = "${kind}_oversize" ]; then
    count=8192
    [ "$kind" != image ] || count=65536
    result=0
    if [ -n "$output" ]; then
        dd if=/dev/zero of="$output" bs=1024 count="$count" 2>/dev/null || result=$?
        wc -c < "$output" > "$TEST_FIXTURE/bytes"
    else
        dd if=/dev/zero bs=1024 count="$count" 2>/dev/null || result=$?
    fi
    exit "$result"
fi
[ -z "$output" ] || exec > "$output"
if [ "$kind" = metadata ]; then
    printf '%s\n' '[{"assets":[{"name":"AltTabio-macos-installer.dmg","browser_download_url":"https://example.invalid/installer.dmg"}]}]'
else
    printf signed-image
fi
MOCK
cat > "$fixture/bin/osascript" <<'MOCK'
#!/bin/sh
cat > /dev/null
printf 'parse\n' >> "$TEST_FIXTURE/events"
printf '%s\n' 'https://example.invalid/installer.dmg'
MOCK
cat > "$fixture/bin/codesign" <<'MOCK'
#!/bin/sh
printf 'verify\n' >> "$TEST_FIXTURE/events"
MOCK
cat > "$fixture/bin/hdiutil" <<'MOCK'
#!/bin/sh
if [ "$1" = attach ]; then
    printf 'mount\n' >> "$TEST_FIXTURE/events"
    cat > "$6/install.sh" <<'INSTALLER'
printf 'execute\n' >> "$TEST_FIXTURE/events"
INSTALLER
fi
MOCK
chmod +x "$fixture/bin/"*
for test_case in metadata_oversize image_oversize settings; do
    export TEST_CASE=$test_case
    : > "$fixture/events"
    rm -f "$fixture/bytes"
    status=0
    PATH="$fixture/bin:$PATH" sh "$root/scripts/mac/bootstrap.sh" > "$fixture/output" 2>&1 || status=$?
    case $test_case in
        metadata_oversize) expected=; ceiling=4194304;;
        image_oversize) expected=parse; ceiling=33554432;;
        settings) expected=$(printf 'parse\nverify\nmount\nexecute'); ceiling=0;;
    esac
    if [ "$(cat "$fixture/events")" != "$expected" ] ||
       { [ "$test_case" = settings ] && [ "$status" -ne 0 ]; } ||
       { [ "$test_case" != settings ] && { [ "$status" -eq 0 ] || [ ! -f "$fixture/bytes" ] || [ "$(cat "$fixture/bytes")" -gt "$ceiling" ]; }; }; then
        printf 'Bootstrap limit failed: %s (exit %s)\n' "$test_case" "$status" >&2
        cat "$fixture/events" "$fixture/output" >&2
        exit 1
    fi
done
printf '%s\n' 'Bootstrap byte ceilings, deadlines and pre-authentication ordering passed.'

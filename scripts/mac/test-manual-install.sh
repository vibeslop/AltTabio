#!/bin/sh
# Check the copyable manual-install trust gate without opening or installing an app.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
expected_certificate=$(tr -d '[:space:]' < "$root/scripts/mac/release-certificate.sha1")
expected_requirement="=identifier \"com.vibeslop.AltTabio\" and certificate leaf = H\"$expected_certificate\""
command=$(sed -n 's/.*<code id="manual-verify-cmd">\(.*\)<\/code>.*/\1/p' "$root/docs/mac/index.html" | sed 's/&amp;/\&/g')
[ -n "$command" ] || { printf '%s\n' 'Missing manual verification command' >&2; exit 1; }
fixture_parent=$(CDPATH= cd -- "${TMPDIR:-/tmp}" && pwd -P)
fixture=$(mktemp -d "$fixture_parent/alttabio-manual-test.XXXXXXXX")
case $fixture in
    "$fixture_parent"/alttabio-manual-test.*) ;;
    *) printf '%s\n' 'Unexpected fixture path' >&2; exit 1 ;;
esac
trap 'rm -rf "$fixture"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
mkdir -p "$fixture/bin" "$fixture/home with spaces/Downloads/AltTabio.app"
# Substitute the documented location with a disposable path, preserving its quoting.
case $command in
    *'"$HOME/Downloads/AltTabio.app"'*) ;;
    *) printf '%s\n' 'Missing quoted app path in manual check' >&2; exit 1 ;;
esac
printf '%s\n' "$command" | sed 's|\$HOME/Downloads/AltTabio\.app|$TEST_MANUAL_APP|g' > "$fixture/verify.sh"
sh -n "$fixture/verify.sh"
cat > "$fixture/bin/codesign" <<'MOCK'
#!/bin/sh
set -eu
# Model intact unrelated/ad-hoc signatures passing an integrity-only check.
if [ "$TEST_SIGNING_CASE" = corrupt ]; then exit 1; fi
if [ "$#" -eq 4 ]; then exit 0; fi
if [ "$#" -ne 6 ] || [ "$1" != --verify ] || [ "$2" != --deep ] ||
   [ "$3" != --strict ] || [ "$4" != -R ] || [ "$5" != "$EXPECTED_REQUIREMENT" ] ||
   [ "$6" != "$TEST_MANUAL_APP" ]; then
    exit 1
fi
[ "$TEST_SIGNING_CASE" = trusted ]
MOCK
chmod +x "$fixture/bin/codesign"
original_path=$PATH
TEST_MANUAL_APP="$fixture/home with spaces/Downloads/AltTabio.app"
PATH="$fixture/bin:$original_path"
EXPECTED_REQUIREMENT=$expected_requirement
export TEST_MANUAL_APP PATH EXPECTED_REQUIREMENT
for signing_case in wrong_certificate wrong_identifier adhoc corrupt trusted; do
    TEST_SIGNING_CASE=$signing_case
    export TEST_SIGNING_CASE
    if sh "$fixture/verify.sh" > "$fixture/output" 2>&1; then
        [ "$signing_case" = trusted ] || {
            printf 'Manual check accepted %s\n' "$signing_case" >&2; exit 1;
        }
        grep -qx 'Verified AltTabio release identity\.' "$fixture/output" || {
            printf '%s\n' 'Missing success message for pinned identity' >&2; exit 1;
        }
    else
        [ "$signing_case" != trusted ] || {
            cat "$fixture/output" >&2
            printf '%s\n' 'Manual check rejected the pinned identity' >&2; exit 1;
        }
        if grep -q 'Verified AltTabio release identity\.' "$fixture/output"; then
            printf 'Manual check printed success for %s\n' "$signing_case" >&2; exit 1
        fi
    fi
done

# On macOS also reject a real intact ad-hoc signed bundle using the documented command.
if [ "$(uname -s)" = Darwin ]; then
    app=$TEST_MANUAL_APP
    mkdir -p "$app/Contents/MacOS"
    cp /usr/bin/true "$app/Contents/MacOS/AltTabio"
    cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>AltTabio</string>
<key>CFBundleIdentifier</key><string>com.vibeslop.AltTabio</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>
PLIST
    /usr/bin/codesign --force --sign - --identifier com.vibeslop.AltTabio "$app" 2>/dev/null
    /usr/bin/codesign --verify --deep --strict "$app"
    PATH=$original_path
    export PATH
    if sh "$fixture/verify.sh" > "$fixture/output" 2>&1; then
        printf '%s\n' 'Manual check accepted a real ad-hoc signature' >&2; exit 1
    fi
    if grep -q 'Verified AltTabio release identity\.' "$fixture/output"; then
        printf '%s\n' 'Manual check printed success for a real ad-hoc signature' >&2; exit 1
    fi
else
    printf '%s\n' 'Skipping real codesign check: requires macOS.'
fi
printf '%s\n' 'Manual install signing checks passed.'

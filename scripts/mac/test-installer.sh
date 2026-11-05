#!/bin/sh
# Exercise the installer's trust gate without downloads or installation side effects.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
installer=$root/docs/install.sh
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
# On macOS also exercise real codesign against an intact ad-hoc signed Mach-O copy.
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
fi
printf '%s\n' 'Installer signing checks passed.'

#!/bin/sh
# Exercise the real build/package scripts without compiling, signing keys, or installation.
# Requires zsh; all macOS tools and Cargo run as inert fixture commands.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
test_zsh=${ALTTABIO_TEST_ZSH:-$(command -v zsh)}
fixture_parent=$(CDPATH='' cd -- "${TMPDIR:-/tmp}" && pwd -P)
fixture=$(mktemp -d "$fixture_parent/alttabio-signing-test.XXXXXXXX")
case $fixture in
    "$fixture_parent"/alttabio-signing-test.*) ;;
    *) printf '%s\n' 'Unexpected fixture path' >&2; exit 1 ;;
esac
trap 'rm -rf "$fixture"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
repo=$fixture/repo
mkdir -p "$fixture/bin" "$repo/scripts/mac" "$repo/assets/mac/icon"
# env zsh lets a locally unpacked Linux zsh run the fixture copies too.
for script in build-app.sh package.sh; do
    sed '1c\
#!/usr/bin/env zsh' "$root/scripts/mac/$script" > "$repo/scripts/mac/$script"
    chmod +x "$repo/scripts/mac/$script"
done
cp "$root/scripts/mac/verify-hardened-runtime.sh" "$repo/scripts/mac/"
# Installer image packaging has its own trust tests. Keep this runtime-policy fixture
# isolated when package.sh also delegates to the authenticated installer packager.
cat > "$repo/scripts/mac/package-installer.sh" <<'INSTALLER'
#!/bin/sh
printf 'packaged\n' > "$TEST_FIXTURE/installer-packaged"
INSTALLER
printf 'version = "0.0.0"\n' > "$repo/Cargo.toml"
for file in LICENSE THIRD_PARTY_LICENSES.md assets/mac/Info.plist \
    assets/mac/icon/Assets.car assets/mac/icon/AltTabio.icns; do
    printf 'fixture\n' > "$repo/$file"
done
printf 'fixture certificate' > "$fixture/certificate"
shasum -a 1 < "$fixture/certificate" | awk '{ print toupper($1) }' > "$repo/scripts/mac/release-certificate.sha1"
TEST_PIN=$(cat "$repo/scripts/mac/release-certificate.sha1")
TEST_FIXTURE=$fixture
TEST_ZSH=$test_zsh
TEST_IDENTITY=release
TEST_SIGNING_CASE=valid
export TEST_PIN TEST_FIXTURE TEST_ZSH TEST_IDENTITY TEST_SIGNING_CASE

cat > "$fixture/tool" <<'MOCK'
#!/bin/sh
set -eu
tool=${0##*/}
case $tool in
    zsh) exec "$TEST_ZSH" -f "$@" ;;
    rustc) printf 'host: aarch64-apple-darwin\n' ;;
    rustup) printf 'aarch64-apple-darwin\nx86_64-apple-darwin\n' ;;
    cargo)
        profile=debug
        target=
        while [ "$#" -gt 0 ]; do
            case $1 in
                --release) profile=release ;;
                --target) shift; target=$1 ;;
            esac
            shift
        done
        mkdir -p "target/$target/$profile"
        printf 'inert binary\n' > "target/$target/$profile/AltTabio"
        ;;
    security)
        case $TEST_IDENTITY in
            release) printf '  1) %s "AltTabio Release"\n' "$TEST_PIN" ;;
            personal) printf '  1) AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA "AltTabio Code Signing"\n' ;;
            none) ;;
            *) exit 2 ;;
        esac
        ;;
    lipo)
        if [ "$1" = -archs ]; then
            [ "$TEST_SIGNING_CASE" != no_architectures ] || exit 0
            printf 'arm64 x86_64\n'
        else
            [ "$1" = -create ] || exit 2
            while [ "$1" != -output ]; do shift; done
            printf 'inert universal binary\n' > "$2"
        fi
        ;;
    plutil)
        case $1 in
            -extract)
                case $2 in
                    LSMinimumSystemVersion) printf '13.0\n' ;;
                    CFBundleShortVersionString) printf '0.0.0\n' ;;
                    *) exit 2 ;;
                esac
                ;;
            -convert)
                [ "$2" = json ] && [ "$3" = -o ] && [ "$4" = - ] || exit 2
                [ "$(cat "$5")" != malformed ] || exit 1
                cat "$5"
                ;;
            *) exit 2 ;;
        esac
        ;;
    codesign)
        case $1 in
            --force)
                printf '%s\n' "$@" > "$TEST_FIXTURE/signing-arguments"
                runtime=no
                while [ "$#" -gt 0 ]; do
                    if [ "$1" = --options ]; then
                        shift
                        [ "$1" != runtime ] || runtime=yes
                    fi
                    shift
                done
                printf '%s' "$runtime" > "$TEST_FIXTURE/runtime"
                printf '0' > "$TEST_FIXTURE/verifications"
                ;;
            --verify)
                [ "$2" = --deep ] && [ "$3" = --strict ] && [ "$4" = --all-architectures ] || exit 2
                count=$(cat "$TEST_FIXTURE/verifications")
                printf '%s' "$((count + 1))" > "$TEST_FIXTURE/verifications"
                [ "$TEST_SIGNING_CASE" != corrupt ]
                ;;
            --display)
                arch=
                entitlements=no
                while [ "$#" -gt 0 ]; do
                    case $1 in
                        --arch) shift; arch=$1 ;;
                        --entitlements) shift; [ "$1" = - ] || exit 2; entitlements=yes ;;
                    esac
                    shift
                done
                [ -n "$arch" ] || exit 2
                [ "$TEST_SIGNING_CASE" != unreadable_signature ] || exit 1
                if [ "$entitlements" = yes ]; then
                    [ "$TEST_SIGNING_CASE" != unreadable_entitlements ] || exit 1
                    case $TEST_SIGNING_CASE in
                        empty_entitlements) printf '{}\n' ;;
                        malformed_entitlements) printf 'malformed\n' ;;
                        exception_*)
                            # Only the second slice is weakened, so host-only checks miss it.
                            if [ "$arch" = x86_64 ]; then
                                printf '{"%s":true}\n' "${TEST_SIGNING_CASE#exception_}"
                            fi
                            ;;
                    esac
                else
                    flags='0x10000(runtime)'
                    if [ "$(cat "$TEST_FIXTURE/runtime")" != yes ] ||
                        { [ "$TEST_SIGNING_CASE" = missing_runtime ] && [ "$arch" = x86_64 ]; } ||
                        { [ "$TEST_SIGNING_CASE" = late_missing_runtime ] &&
                          [ "$(cat "$TEST_FIXTURE/verifications")" -gt 1 ] && [ "$arch" = x86_64 ]; }; then
                        flags='0x0(none)'
                    fi
                    [ "$TEST_SIGNING_CASE" != extra_flags ] || flags='0x10002(adhoc,runtime)'
                    printf 'CodeDirectory v=20500 size=100 flags=%s hashes=1+1 location=embedded\n' "$flags" >&2
                fi
                ;;
            -d)
                prefix=${2#--extract-certificates=}
                [ "$prefix" != "$2" ] || exit 2
                [ "$TEST_IDENTITY" != none ] || exit 0
                if [ "$TEST_IDENTITY" = release ]; then
                    cp "$TEST_FIXTURE/certificate" "${prefix}0"
                else
                    printf 'personal certificate' > "${prefix}0"
                fi
                ;;
            *) exit 2 ;;
        esac
        ;;
    ditto)
        [ "$1" = -c ] && [ "$2" = -k ] && [ "$3" = --keepParent ] || exit 2
        printf 'inert archive\n' > "$5"
        ;;
    *) exit 2 ;;
esac
MOCK
for tool in zsh rustc rustup cargo security lipo plutil codesign ditto; do
    cp "$fixture/tool" "$fixture/bin/$tool"
    chmod +x "$fixture/bin/$tool"
done
PATH=$fixture/bin:$PATH
export PATH
archive=$repo/target/mac/AltTabio-0.0.0-macos.zip

for TEST_SIGNING_CASE in valid empty_entitlements extra_flags; do
    export TEST_SIGNING_CASE
    rm -f "$archive" "$fixture/installer-packaged"
    if ! zsh "$repo/scripts/mac/package.sh" > "$fixture/output" 2>&1; then
        cat "$fixture/output" >&2
        exit 1
    fi
    test -s "$archive"
    if grep -q '^sh scripts/mac/package-installer.sh$' "$root/scripts/mac/package.sh"; then
        test -s "$fixture/installer-packaged"
    fi
done

for TEST_SIGNING_CASE in missing_runtime late_missing_runtime corrupt no_architectures unreadable_signature \
    unreadable_entitlements malformed_entitlements \
    exception_com.apple.security.cs.allow-dyld-environment-variables \
    exception_com.apple.security.cs.disable-library-validation \
    exception_com.apple.security.cs.disable-executable-page-protection \
    exception_com.apple.security.cs.allow-jit \
    exception_com.apple.security.cs.allow-unsigned-executable-memory \
    exception_com.apple.security.get-task-allow; do
    export TEST_SIGNING_CASE
    rm -f "$archive" "$fixture/installer-packaged"
    if zsh "$repo/scripts/mac/package.sh" > "$fixture/output" 2>&1; then
        printf 'Packaging accepted %s\n' "$TEST_SIGNING_CASE" >&2
        exit 1
    fi
    test ! -e "$archive"
    test ! -e "$fixture/installer-packaged"
done

TEST_SIGNING_CASE=valid
export TEST_SIGNING_CASE
for TEST_IDENTITY in personal none; do
    export TEST_IDENTITY
    rm -f "$archive" "$fixture/installer-packaged"
    if zsh "$repo/scripts/mac/package.sh" > "$fixture/output" 2>&1; then
        printf 'Packaging accepted %s signing\n' "$TEST_IDENTITY" >&2
        exit 1
    fi
    test ! -e "$archive"
    test ! -e "$fixture/installer-packaged"
done

# Non-universal release builds are hardened too; debug keeps all existing identity fallbacks.
for TEST_IDENTITY in release personal none; do
    export TEST_IDENTITY
    for mode in release debug; do
        if [ "$mode" = debug ]; then set -- --debug; else set --; fi
        if ! zsh "$repo/scripts/mac/build-app.sh" "$@" > "$fixture/output" 2>&1; then
            cat "$fixture/output" >&2
            exit 1
        fi
        expected_runtime=yes
        [ "$mode" != debug ] || expected_runtime=no
        test "$(cat "$fixture/runtime")" = "$expected_runtime"
        case $TEST_IDENTITY in
            release) expected_identity=$TEST_PIN ;;
            personal) expected_identity=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA ;;
            none) expected_identity=- ;;
        esac
        test "$(sed -n '3p' "$fixture/signing-arguments")" = "$expected_identity"
    done
done
printf '%s\n' 'macOS signing policy checks passed.'

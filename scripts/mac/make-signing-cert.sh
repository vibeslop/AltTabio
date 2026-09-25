#!/bin/zsh
# Creates the certificates that sign AltTabio.app.
#
# Usage: scripts/mac/make-signing-cert.sh                a personal certificate for your own builds
#        scripts/mac/make-signing-cert.sh --release      the release certificate, once per project
#        scripts/mac/make-signing-cert.sh --import FILE  the release certificate from its backup
#
# macOS ties Accessibility and Screen Recording grants to the certificate an app is signed with.
# A personal certificate keeps your own grants across rebuilds. Releases all carry one release
# certificate, pinned in scripts/mac/release-certificate.sha1, so users keep their grants across
# updates; the release workflow signs with it from the secrets of the repository's "release"
# environment, and --release puts it there.
#
# Whoever holds the release key can sign an app that macOS treats as AltTabio, grants included,
# so it stays out of keychains: --release writes a backup encrypted with a password you choose,
# which goes to the private vibeslop/release-signing repository, encrypted again to the
# maintainers' SSH keys, and --import is for packaging a release by hand.
set -euo pipefail

cd "$(dirname "$0")/../.."
keychain="$HOME/Library/Keychains/login.keychain-db"
pin_file=scripts/mac/release-certificate.sha1
environment=release
# The system LibreSSL writes a PKCS#12 file the keychain can import. Homebrew's OpenSSL 3, often
# first on PATH, needs -legacy for that, and LibreSSL rejects the flag.
openssl=/usr/bin/openssl

fail() {
    print -u2 -- "$1"
    exit 1
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# zsh skips the exit trap when a signal ends it, which would leave the unencrypted private key
# behind; exiting from the signal runs the trap.
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

# Writes a new key and a self-signed code-signing certificate named $1 to $work.
make_certificate() {
    cat >"$work/cert.cnf" <<CONF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $1
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
CONF
    # Twenty years: codesign stops offering an expired certificate, and replacing the release one
    # would cost every user their grants.
    $openssl req -x509 -newkey rsa:2048 -nodes -days 7300 -config "$work/cert.cnf" \
        -keyout "$work/key.pem" -out "$work/cert.pem" 2>"$work/openssl.log" ||
        fail "openssl could not create the certificate: $(<"$work/openssl.log")"
}

# Packs the key and certificate in $work into the PKCS#12 file $1, named $2, under the password
# in ALTTABIO_P12_PASSWORD.
export_p12() {
    $openssl pkcs12 -export -inkey "$work/key.pem" -in "$work/cert.pem" -name "$2" \
        -passout env:ALTTABIO_P12_PASSWORD -out "$1" 2>"$work/openssl.log" ||
        fail "openssl could not write $1: $(<"$work/openssl.log")"
}

# The SHA-1 hash of the PEM certificate $1, as codesign and security print it.
fingerprint() {
    $openssl x509 -in "$1" -outform DER | shasum -a 1 | awk '{ print toupper($1) }'
}

# Read in full before matching: grep -q in a pipe exits at its match, and pipefail would count
# the writer it cut off as a failure. The second list holds only the identities trusted for code
# signing, the only ones codesign offers.
identities=$(security find-identity -p codesigning "$keychain")
trusted=$(security find-identity -v -p codesigning "$keychain")

# Trusts the certificate in $work/cert.pem for code signing; macOS asks for the login password.
trust() {
    security add-trusted-cert -p codeSign -k "$keychain" "$work/cert.pem"
}

case "${1:-}" in
    "")
        if [[ "$trusted" == *'"AltTabio Code Signing"'* ]]; then
            print -- "Your 'AltTabio Code Signing' certificate is already in the login keychain."
            exit 0
        elif [[ "$identities" == *'"AltTabio Code Signing"'* ]]; then
            fail "Your 'AltTabio Code Signing' certificate is not trusted for code signing, so builds
pass it over. Delete it in Keychain Access and run this again."
        fi
        make_certificate "AltTabio Code Signing"
        # The file only carries the key into the keychain, so a random password does.
        password=$($openssl rand -hex 16)
        export ALTTABIO_P12_PASSWORD=$password
        export_p12 "$work/personal.p12" "AltTabio Code Signing"
        security import "$work/personal.p12" -k "$keychain" -f pkcs12 -P "$password" \
            -T /usr/bin/codesign >/dev/null
        trust
        print -- "Created your 'AltTabio Code Signing' certificate. scripts/mac/build-app.sh signs with"
        print -- "it, so macOS keeps AltTabio's permissions across your builds. If a build asks"
        print -- "whether codesign may use the key, choose Always Allow."
        ;;

    --release)
        if [[ -f "$pin_file" ]]; then
            fail "$pin_file already pins the release certificate, and its backup is in
vibeslop/release-signing. A new one would make every user grant Accessibility and Screen
Recording again."
        fi
        backup=$HOME/AltTabio-Release-Certificate.p12
        [[ ! -e "$backup" ]] || fail "$backup already exists; move it away first."
        # Everything the certificate needs is checked before it exists, so a failure here leaves
        # nothing half made.
        command -v gh >/dev/null || fail "The GitHub CLI stores the certificate: brew install gh"
        repo=$(gh repo view --json nameWithOwner --jq .nameWithOwner) ||
            fail "gh cannot see this repository; sign in with gh auth login."
        gh api "repos/$repo/environments/$environment" >/dev/null 2>&1 ||
            fail "$repo has no '$environment' environment for the certificate to go to."

        print -- "The backup at $backup is encrypted with a password; it protects the private key."
        read -rs "password?Backup password: "
        print
        read -rs "again?Backup password again: "
        print
        [[ -n "$password" ]] || fail "The password is empty."
        [[ "$password" == "$again" ]] || fail "The passwords differ."
        export ALTTABIO_P12_PASSWORD=$password

        make_certificate "AltTabio Release"
        export_p12 "$backup" "AltTabio Release"
        chmod 600 "$backup"
        release=$(fingerprint "$work/cert.pem")
        print -- "$release" >"$pin_file"
        print -- ""
        print -- "Created the release certificate $release."
        print -- "  $pin_file pins it: commit that file."
        if base64 -i "$backup" |
            gh secret set MACOS_RELEASE_CERTIFICATE --env "$environment" --repo "$repo" &&
            print -rn -- "$password" |
            gh secret set MACOS_RELEASE_CERTIFICATE_PASSWORD --env "$environment" --repo "$repo"
        then
            print -- "  The '$environment' environment of $repo holds it for the release workflow."
        else
            print -- "  It did not reach the '$environment' environment. Store it by hand:"
            print -- "    base64 -i $backup | gh secret set MACOS_RELEASE_CERTIFICATE --env $environment"
            print -- "    gh secret set MACOS_RELEASE_CERTIFICATE_PASSWORD --env $environment"
        fi
        print -- "  $backup is its backup: store it and its password in"
        print -- "  vibeslop/release-signing as its README says, then delete it here."
        ;;

    --import)
        backup=${2:-}
        [[ -f "$backup" ]] || fail "Usage: $0 --import <backup.p12>"
        [[ -f "$pin_file" ]] || fail "$pin_file pins no release certificate yet."
        release=$(<"$pin_file")
        if [[ "$trusted" == *"$release"* ]]; then
            print -- "The release certificate is already in the login keychain."
            exit 0
        elif [[ "$identities" == *"$release"* ]]; then
            fail "The release certificate is in the login keychain but not trusted for code signing.
Delete it in Keychain Access and run this again."
        fi
        read -rs "password?Password of $backup: "
        print
        export ALTTABIO_P12_PASSWORD=$password
        $openssl pkcs12 -in "$backup" -clcerts -nokeys -passin env:ALTTABIO_P12_PASSWORD \
            -out "$work/cert.pem" 2>"$work/openssl.log" ||
            fail "Could not open $backup; is the password right? $(<"$work/openssl.log")"
        found=$(fingerprint "$work/cert.pem")
        [[ "$found" == "$release" ]] ||
            fail "$backup holds certificate $found, not the release certificate $release."
        security import "$backup" -k "$keychain" -f pkcs12 -P "$password" -T /usr/bin/codesign \
            >/dev/null
        trust
        print -- "The release certificate is in the login keychain; scripts/mac/build-app.sh and"
        print -- "scripts/mac/package.sh sign with it."
        ;;

    *)
        fail "Usage: $0 [--release | --import <backup.p12>]"
        ;;
esac

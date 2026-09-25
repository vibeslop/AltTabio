#!/bin/zsh
# Creates the self-signed "AltTabio Code Signing" certificate in the login keychain, or restores it
# from the backup that creating it wrote.
#
# Usage: scripts/mac/make-signing-cert.sh              create the certificate and its backup
#        scripts/mac/make-signing-cert.sh --import FILE restore the certificate from a backup
#
# macOS remembers Accessibility and Screen Recording grants per code signature. A stable
# certificate keeps them across rebuilds and across releases, so the permissions only have to be
# granted once. Every published release must be signed with the same certificate: on a new Mac,
# restore the backup with --import. A release signed with a new certificate makes every user grant
# both permissions again.
#
# Whoever holds the private key can sign an app that macOS treats as AltTabio, grants included,
# so the backup is encrypted with a password you choose.
set -euo pipefail

name="AltTabio Code Signing"
keychain="$HOME/Library/Keychains/login.keychain-db"
# The system LibreSSL writes a PKCS#12 file the keychain can import. Homebrew's OpenSSL 3, often
# first on PATH, needs -legacy for that, and LibreSSL rejects the flag.
openssl=/usr/bin/openssl

fail() {
    print -u2 -- "$1"
    exit 1
}

# Read in full before matching: grep -q in a pipe exits at its match, and pipefail would count
# the writer it cut off as a failure.
identities=$(security find-identity -p codesigning "$keychain")
if [[ "$identities" == *"\"$name\""* ]]; then
    print -- "The '$name' certificate is already in the login keychain."
    if [[ "$(security find-identity -v -p codesigning "$keychain")" != *"\"$name\""* ]]; then
        print -- "It is not trusted for code signing, so builds cannot use it. Delete it in"
        print -- "Keychain Access and run this again."
        exit 1
    fi
    print -- "To replace it, delete it in Keychain Access first."
    exit 0
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

case "${1:-}" in
    --import)
        backup=${2:-}
        [[ -f "$backup" ]] || fail "Usage: $0 --import <backup.p12>"
        read -rs "password?Password of $backup: "
        print
        ;;
    "")
        backup=$HOME/AltTabio-Code-Signing.p12
        [[ ! -e "$backup" ]] || fail "$backup already exists; restore it with --import, or move it away."
        print -- "The backup at $backup is encrypted with a password; it protects the private key."
        read -rs "password?Backup password: "
        print
        read -rs "again?Backup password again: "
        print
        [[ -n "$password" ]] || fail "The password is empty."
        [[ "$password" == "$again" ]] || fail "The passwords differ."
        cat > "$work/cert.cnf" <<'CONF'
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = AltTabio Code Signing
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
CONF
        # Twenty years: codesign stops offering an expired certificate, and its replacement would
        # cost every user their grants.
        $openssl req -x509 -newkey rsa:2048 -nodes -days 7300 -config "$work/cert.cnf" \
            -keyout "$work/key.pem" -out "$work/cert.pem" 2>"$work/openssl.log" ||
            fail "openssl could not create the certificate: $(<"$work/openssl.log")"
        ALTTABIO_P12_PASSWORD=$password $openssl pkcs12 -export -inkey "$work/key.pem" \
            -in "$work/cert.pem" -name "$name" -passout env:ALTTABIO_P12_PASSWORD \
            -out "$backup" 2>"$work/openssl.log" ||
            fail "openssl could not write the backup: $(<"$work/openssl.log")"
        chmod 600 "$backup"
        ;;
    *)
        fail "Usage: $0 [--import <backup.p12>]"
        ;;
esac

ALTTABIO_P12_PASSWORD=$password $openssl pkcs12 -in "$backup" -clcerts -nokeys \
    -passin env:ALTTABIO_P12_PASSWORD -out "$work/cert.pem" 2>"$work/openssl.log" ||
    fail "Could not open $backup; is the password right? $(<"$work/openssl.log")"
security import "$backup" -k "$keychain" -f pkcs12 -P "$password" -T /usr/bin/codesign
# codesign only offers the certificate once it is trusted for code signing; macOS asks for the
# login password once for this step.
security add-trusted-cert -p codeSign -k "$keychain" "$work/cert.pem"

print -- "The '$name' certificate is in the login keychain; scripts/mac/build-app.sh uses it."
# `security set-key-partition-list` could grant this up front, but the keychain labels every
# imported key "Imported Private Key", so it cannot single this one out and would open all of
# them to Apple's command-line tools.
print -- "The first build asks whether codesign may use the key: enter the login password and"
print -- "choose Always Allow."
if [[ "${1:-}" != --import ]]; then
    print -- ""
    print -- "Store the backup and its password somewhere safe, such as a password manager, then"
    print -- "delete $backup. Every release must be signed with this certificate."
fi

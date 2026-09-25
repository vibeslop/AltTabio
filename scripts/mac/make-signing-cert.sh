#!/bin/zsh
# Creates the self-signed "AltTabio Code Signing" certificate in the login keychain.
#
# macOS remembers Accessibility and Screen Recording grants per code signature. A stable
# certificate keeps them across rebuilds and across releases, so the permissions only have to be
# granted once. Every published release must be signed with the same certificate: keep a backup
# of it, because a release signed with a new one makes every user grant both permissions again.
set -euo pipefail

# The system LibreSSL writes a PKCS#12 file the keychain can import. Homebrew's OpenSSL 3, often
# first on PATH, needs -legacy for that, and LibreSSL rejects the flag.
openssl=/usr/bin/openssl

fail() {
    print -u2 -- "$1"
    exit 1
}

if security find-identity -v -p codesigning 2>/dev/null | grep -q '"AltTabio Code Signing"'; then
    echo "The 'AltTabio Code Signing' certificate already exists"
    exit 0
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
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
$openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -config "$work/cert.cnf" \
    -keyout "$work/key.pem" -out "$work/cert.pem" 2>"$work/openssl.log" ||
    fail "openssl could not create the certificate: $(<"$work/openssl.log")"
$openssl pkcs12 -export -inkey "$work/key.pem" -in "$work/cert.pem" \
    -name "AltTabio Code Signing" -passout pass:alttabio -out "$work/cert.p12" \
    2>"$work/openssl.log" || fail "openssl could not export the certificate: $(<"$work/openssl.log")"
security import "$work/cert.p12" -k "$HOME/Library/Keychains/login.keychain-db" \
    -P alttabio -T /usr/bin/codesign >/dev/null
# codesign only accepts the certificate once it is trusted for code signing; macOS asks for the
# login password once for this step.
security add-trusted-cert -p codeSign -k "$HOME/Library/Keychains/login.keychain-db" "$work/cert.pem"
echo "Created the 'AltTabio Code Signing' certificate; scripts/mac/build-app.sh uses it from now on"
# `security set-key-partition-list` could grant this up front, but the keychain labels every
# imported key "Imported Private Key", so it cannot single this one out and would open all of
# them to Apple's command-line tools.
echo "The first build asks whether codesign may use the key: enter the login password and"
echo "choose Always Allow."

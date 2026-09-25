#!/bin/zsh
# Creates the self-signed "AltTabio Code Signing" certificate in the login keychain.
#
# macOS remembers Accessibility and Screen Recording grants per code signature. A stable
# certificate keeps them across rebuilds and across releases, so the permissions only have to be
# granted once. Every published release must be signed with the same certificate: keep a backup
# of it, because a release signed with a new one makes every user grant both permissions again.
set -euo pipefail

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
openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -config "$work/cert.cnf" \
    -keyout "$work/key.pem" -out "$work/cert.pem" >/dev/null 2>&1
openssl pkcs12 -export -legacy -inkey "$work/key.pem" -in "$work/cert.pem" \
    -name "AltTabio Code Signing" -passout pass:alttabio -out "$work/cert.p12" >/dev/null 2>&1
security import "$work/cert.p12" -k "$HOME/Library/Keychains/login.keychain-db" \
    -P alttabio -T /usr/bin/codesign >/dev/null
# codesign only accepts the certificate once it is trusted for code signing; macOS asks for the
# login password once for this step.
security add-trusted-cert -p codeSign -k "$HOME/Library/Keychains/login.keychain-db" "$work/cert.pem"
# Without this, codesign stops on a keychain access prompt at every build.
security set-key-partition-list -S apple-tool:,apple: -s -k "" \
    "$HOME/Library/Keychains/login.keychain-db" >/dev/null 2>&1 || true
echo "Created the 'AltTabio Code Signing' certificate; scripts/mac/build-app.sh uses it from now on"

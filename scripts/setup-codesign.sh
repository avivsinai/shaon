#!/bin/bash
# One-time setup: create a self-signed codesigning identity for shaon-cli.
#
# Why: ad-hoc codesigning (`codesign -s -`) makes the designated requirement
# cdhash-based, so every rebuild invalidates keychain ACLs and macOS
# re-prompts for "Always Allow". A stable signing identity makes the DR
# identifier-based instead, so ACL entries persist across rebuilds.
#
# Idempotent — safe to re-run.

set -euo pipefail

IDENTITY="shaon-cli-signer"
CN="shaon-cli-signer"
KEYCHAIN="${HOME}/Library/Keychains/login.keychain-db"

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "[setup-codesign] not macOS, nothing to do." >&2
    exit 0
fi

if security find-certificate -c "$CN" >/dev/null 2>&1; then
    if security find-identity -v -p codesigning 2>/dev/null | grep -q "$CN"; then
        echo "[setup-codesign] codesigning identity '$CN' already present — nothing to do." >&2
        exit 0
    fi
    echo "[setup-codesign] certificate '$CN' exists but is not a valid codesigning identity." >&2
    echo "[setup-codesign] remove it via Keychain Access and re-run, or troubleshoot manually." >&2
    exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

cd "$WORK"

echo "[setup-codesign] generating 2048-bit RSA private key…" >&2
openssl genrsa -out key.pem 2048 2>/dev/null

cat > ext.cnf <<EOF
[req]
distinguished_name = req_dn
prompt = no
[req_dn]
CN = ${CN}
[v3]
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
subjectKeyIdentifier = hash
EOF

echo "[setup-codesign] creating self-signed certificate with codeSigning EKU…" >&2
openssl req -new -x509 \
    -key key.pem \
    -out cert.pem \
    -days 3650 \
    -config ext.cnf \
    -extensions v3 2>/dev/null

echo "[setup-codesign] bundling key + cert into PKCS#12…" >&2
# Use a known passphrase to avoid PKCS12 MAC quirks with empty passwords on
# newer openssl (3.x) vs older macOS-shipped openssl.
P12PASS="shaon-setup"
openssl pkcs12 -export -legacy \
    -inkey key.pem \
    -in cert.pem \
    -name "$IDENTITY" \
    -out bundle.p12 \
    -passout "pass:${P12PASS}" 2>/dev/null || openssl pkcs12 -export \
    -inkey key.pem \
    -in cert.pem \
    -name "$IDENTITY" \
    -out bundle.p12 \
    -passout "pass:${P12PASS}" 2>/dev/null

echo "[setup-codesign] importing into login keychain…" >&2
echo "[setup-codesign] macOS will ask for your login password (once) to import." >&2

# Restrict which apps can use this private key to sign: only /usr/bin/codesign.
# Do NOT use -A (allow any app) — that would let any local process mint
# binaries signed as com.avivsinai.shaon and satisfy the keychain ACL trust
# granted to this identity.
security import bundle.p12 -k "$KEYCHAIN" -P "$P12PASS" \
    -T /usr/bin/codesign

# Explicitly trust the cert for codesigning. Requires admin auth once.
echo "[setup-codesign] trusting cert for codesigning (requires sudo once)…" >&2
cat > trust.plist <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<array>
  <dict>
    <key>kSecTrustSettingsPolicy</key>
    <data>KoZIhvdjZAED</data>
    <key>kSecTrustSettingsPolicyName</key>
    <string>Code Signing</string>
    <key>kSecTrustSettingsResult</key>
    <integer>1</integer>
  </dict>
</array>
</plist>
EOF
security add-trusted-cert -d -r trustRoot -p codeSign -k "$KEYCHAIN" cert.pem 2>&1 || {
    echo "[setup-codesign] trust-settings add failed (non-fatal for local signing)" >&2
}

echo "[setup-codesign] verifying identity…" >&2
if security find-identity -v -p codesigning 2>/dev/null | grep -q "$CN"; then
    echo "[setup-codesign] ✓ '$IDENTITY' is a valid codesigning identity." >&2
    echo "[setup-codesign]   scripts/run.sh will use it automatically on next build." >&2
    echo "[setup-codesign]   Next run you'll be prompted for 'Always Allow' ONCE, then never again." >&2
else
    echo "[setup-codesign] ✗ identity creation failed verification." >&2
    exit 1
fi

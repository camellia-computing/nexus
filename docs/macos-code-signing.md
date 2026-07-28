# macOS code signing

This document defines the supported local and GitHub Actions signing modes for Camellia Nexus. Native platform signing is optional; every published release asset is still protected by SHA-256 checksums and keyless Sigstore/Cosign bundles.

## Trust model

| Mode | Configuration | Result | Suitable use |
| --- | --- | --- | --- |
| Unsigned | No Apple values | Tauri builds with `--no-sign` | Internal testing; users must approve the app manually |
| Ad hoc | `APPLE_SIGNING_IDENTITY=-` only | Structurally signed, no publisher identity or notarization | Local/controlled Apple Silicon testing |
| Certificate | Complete certificate group | Signed with the configured identity | Free Apple development certificate or controlled private-CA distribution |
| Notarized | Complete certificate and App Store Connect API groups | Signed, notarized and stapled | Public distribution outside the App Store |

The workflow rejects every partial group. Candidate builds never inherit signing secrets. `RELEASE-METADATA.json` records `unsigned`, `ad-hoc`, `signed`, or `notarized` for each macOS architecture.

`scripts/resolve-macos-signing.sh` is the shared validator used by Actions and regression tests. Keep new signing modes in that resolver instead of duplicating secret-group decisions in workflow YAML.

An Apple Developer free membership can create development identities for testing, but it cannot notarize a public download. A private CA or ad-hoc identity also cannot establish public Gatekeeper trust. Public commercial distribution should use a paid Apple Developer membership, a `Developer ID Application` certificate, hardened runtime and notarization.

## GitHub Actions configuration

Certificate signing requires all three values:

- variable `APPLE_SIGNING_IDENTITY`: the exact identity shown by `security find-identity -v -p codesigning`;
- secret `APPLE_CERTIFICATE`: one-line base64 of the exported `.p12`;
- secret `APPLE_CERTIFICATE_PASSWORD`: the `.p12` export password.

Notarization additionally requires all three values:

- variable `APPLE_API_ISSUER`: App Store Connect issuer UUID;
- variable `APPLE_API_KEY`: 10-character key ID;
- secret `APPLE_API_PRIVATE_KEY`: the complete downloaded `AuthKey_<key-id>.p8` PEM.

Example configuration without embedding any repository or account name:

```bash
openssl base64 -A -in camellia-nexus-codesign.p12 -out certificate-base64.txt
gh variable set APPLE_SIGNING_IDENTITY --body 'Developer ID Application: Example Organization (TEAMID)'
gh secret set APPLE_CERTIFICATE < certificate-base64.txt
gh secret set APPLE_CERTIFICATE_PASSWORD

gh variable set APPLE_API_ISSUER --body '<issuer-uuid>'
gh variable set APPLE_API_KEY --body '<key-id>'
gh secret set APPLE_API_PRIVATE_KEY < 'AuthKey_<key-id>.p8'
```

For intentional ad-hoc release signing, configure only:

```bash
gh variable set APPLE_SIGNING_IDENTITY --body '-'
```

Remove the variable again to return to unsigned releases. Do not leave certificate, password, identity or notarization values partially configured.

## Free Apple development certificate

1. Enroll the Apple ID in the free Apple Developer program and use Xcode or Keychain Access to create a code-signing request.
2. Create an Apple Development certificate through the developer portal and install it in the login keychain.
3. Confirm that the certificate and its private key appear under **My Certificates**:

   ```bash
   security find-identity -v -p codesigning
   ```

4. Export the identity and private key together as a password-protected `.p12`, then configure the certificate group above.
5. Leave the App Store Connect API group absent. The result is `signed`, not `notarized`, and must not be presented as a publicly trusted macOS release.

Free development certificates can be short-lived and device/account constrained. Treat expiration as an expected rotation event.

## Private development CA

Use this only for controlled test machines. Never distribute the root private key, never commit any generated key/P12 file, and never ask public customers to install a private root merely to run the product.

```bash
umask 077
mkdir macos-development-ca
cd macos-development-ca

openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:4096 -out root.key
openssl req -x509 -new -sha256 -days 3650 \
  -key root.key \
  -subj '/CN=Camellia Nexus Development Root CA' \
  -addext 'basicConstraints=critical,CA:TRUE,pathlen:0' \
  -addext 'keyUsage=critical,keyCertSign,cRLSign' \
  -out root.crt

openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:3072 -out codesign.key
openssl req -new -sha256 \
  -key codesign.key \
  -subj '/CN=Camellia Nexus Development Code Signing' \
  -out codesign.csr

cat > codesign.ext <<'EOF'
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature
extendedKeyUsage=codeSigning
subjectKeyIdentifier=hash
authorityKeyIdentifier=keyid,issuer
EOF

openssl x509 -req -sha256 -days 825 \
  -in codesign.csr \
  -CA root.crt \
  -CAkey root.key \
  -CAcreateserial \
  -extfile codesign.ext \
  -out codesign.crt

openssl pkcs12 -export \
  -name 'Camellia Nexus Development Code Signing' \
  -inkey codesign.key \
  -in codesign.crt \
  -certfile root.crt \
  -out camellia-nexus-development-codesign.p12
```

Import `root.crt` only on managed test machines, then import the `.p12` into the login keychain and confirm the identity:

```bash
security import camellia-nexus-development-codesign.p12 -k ~/Library/Keychains/login.keychain-db
security find-identity -v -p codesigning
```

Export the `.p12` base64 and configure the certificate group. Do not configure notarization: Apple will not notarize a private-CA identity.

## Local build and verification

With an identity already installed in the login keychain:

```bash
export APPLE_SIGNING_IDENTITY='Camellia Nexus Development Code Signing'
export CAMELLIA_NEXUS_MACOS_SIGN=required
bash scripts/ci-local.sh --desktop-package --skip-quality

codesign --verify --deep --strict --verbose=2 target/release/bundle/macos/*.app
codesign --display --deep --verbose=4 target/release/bundle/macos/*.app
```

For a notarized build, also verify the stapled ticket and Gatekeeper assessment:

```bash
xcrun stapler validate target/release/bundle/macos/*.app
spctl --assess --type execute --verbose=2 target/release/bundle/macos/*.app
```

Tauri notarizes and staples the App bundle before placing it in the DMG. With a certificate identity it also signs the DMG; ad-hoc mode intentionally skips DMG self-signing. The release staging check mirrors that contract instead of requiring a separate stapled ticket on the DMG.

Structural `codesign --verify` success for a private or ad-hoc identity does not mean another Mac trusts its issuer.

## Rotation and incident handling

- Keep the root/private key and App Store Connect key out of source control, logs, artifacts and command arguments.
- Rotate an expiring or exposed certificate/key as one atomic configuration group.
- Revoke compromised Apple credentials before replacing repository values.
- A release whose native signing status is unexpected must remain a draft; do not relabel metadata or add a signature after publication.

## References

- [Tauri macOS code-signing and notarization guide](https://v2.tauri.app/distribute/sign/macos/)
- [Apple Developer membership comparison](https://developer.apple.com/support/compare-memberships/)
- [Apple Keychain Access: create self-signed certificates](https://support.apple.com/en-gb/guide/keychain-access/kyca8916/mac)
- [Apple Keychain Access: create a private certificate authority](https://support.apple.com/en-euro/guide/keychain-access/kyca2686/mac)
- [Apple code-signing concepts](https://developer.apple.com/library/archive/documentation/Security/Conceptual/CodeSigningGuide/AboutCS/AboutCS.html)

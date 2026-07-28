# Linux artifact signing

Camellia Nexus can optionally publish ASCII-armored OpenPGP detached signatures for the Linux
AppImage, Debian package and portable tar archive. This is an artifact-integrity and controlled
distribution facility, not Linux executable code signing: installing a package does not make an
untrusted OpenPGP key trusted by the operating system. SHA-256 checksums and keyless
Sigstore/Cosign bundles remain mandatory for every formal release regardless of this option.

## Trust and release contract

The workflow accepts either no Linux OpenPGP values or one complete configuration group. A partial
group fails before packaging. When enabled, the release contains:

- one `.asc` detached signature beside each Linux package;
- one `camellia-nexus-<version>-linux-x64.signing-key.asc` public key;
- the full configured fingerprint in `RELEASE-METADATA.json`.

Publication reimports that exact public key into a temporary keyring and requires every signature's
`VALIDSIG` fingerprint to match the metadata. Candidate workflows never receive signing secrets.
The private key, passphrase and temporary keyring are never published.
`scripts/resolve-linux-signing.sh` centralizes the optional-group and fingerprint validation used by
Actions and regression tests.

## Create a controlled signing key

Generate the key on a protected workstation. An offline primary key with a dedicated signing
subkey is preferred for long-lived production custody; a single signing-capable key is sufficient
for controlled internal distribution.

```bash
umask 077
export GNUPGHOME="$(mktemp -d)"
gpg --quick-generate-key 'Camellia Nexus Linux Release <release@example.invalid>' ed25519 cert 1y
gpg --quick-add-key '<primary-fingerprint>' ed25519 sign 1y
gpg --armor --export-secret-subkeys '<signing-subkey-fingerprint>!' > linux-release-private.asc
gpg --armor --export '<primary-fingerprint>' > linux-release-public.asc
```

Keep the exported private key outside the repository and rotate it before expiry. Publish a
fingerprint through an authenticated channel independent of the release download when users need
publisher identity, rather than treating a public key shipped beside its own signature as a trust
anchor.

## GitHub Actions configuration

Configure all three values together:

- variable `LINUX_GPG_FINGERPRINT`: the complete 40- or 64-hexadecimal fingerprint of the exact
  signing key;
- secret `LINUX_GPG_PRIVATE_KEY`: the complete ASCII-armored secret-key export;
- secret `LINUX_GPG_PASSPHRASE`: its non-empty passphrase.

```bash
gh variable set LINUX_GPG_FINGERPRINT --body '<full-fingerprint>'
gh secret set LINUX_GPG_PRIVATE_KEY < linux-release-private.asc
gh secret set LINUX_GPG_PASSPHRASE
```

Remove all three values to return to the unsigned-Linux-artifact mode. Never configure an abbreviated
key ID, place the passphrase in a variable, or commit either key export.

## Local signing and verification

The release staging workflow calls the same repository scripts shown here:

```bash
export LINUX_GPG_FINGERPRINT='<full-fingerprint>'
export LINUX_GPG_PRIVATE_KEY="$(< linux-release-private.asc)"
read -r -s -p 'Signing key passphrase: ' LINUX_GPG_PASSPHRASE
export LINUX_GPG_PASSPHRASE
export LINUX_GPG_PUBLIC_KEY_OUTPUT='dist/camellia-nexus.signing-key.asc'
bash scripts/linux-openpgp-sign.sh dist/camellia-nexus-*.AppImage \
  dist/camellia-nexus-*.deb dist/camellia-nexus-*.tar.gz

LINUX_GPG_PUBLIC_KEY="$LINUX_GPG_PUBLIC_KEY_OUTPUT" \
  bash scripts/linux-openpgp-verify.sh dist/camellia-nexus-*.AppImage \
    dist/camellia-nexus-*.deb dist/camellia-nexus-*.tar.gz
```

Both scripts use isolated temporary keyrings, reject symlink inputs and match the full fingerprint.
The signing script verifies its own output with a separate public-only keyring before staging it.

## Rotation and incident handling

- Replace the private key, passphrase and fingerprint as one atomic configuration group.
- Keep a release in draft state when its expected signing mode, key or signature set differs from
  metadata; never repair or relabel a published release in place.
- Revoke and remove an exposed key, investigate every release produced during the exposure window,
  and communicate the new fingerprint over the independent trust channel.
- Retaining old public keys is useful for verifying archived artifacts, but old private keys should
  remain offline or be securely destroyed according to the release custody policy.

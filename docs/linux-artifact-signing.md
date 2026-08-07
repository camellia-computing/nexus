# Linux artifact signing

Camellia Nexus can optionally publish ASCII-armored OpenPGP detached signatures for the Linux
AppImage, Debian package and portable tar archive. This is an artifact-integrity and controlled
distribution facility, not Linux executable code signing: installing a package does not make an
untrusted OpenPGP key trusted by the operating system. The public `SHA256SUMS` and retained internal
keyless Sigstore/Cosign bundles remain mandatory for every formal release regardless of this option.

OpenPGP signing is currently disabled. Until the complete configuration is deliberately enabled and
the full fingerprint is committed to the durable security policy, Releases contain no detached
signature or public-key placeholder.

## Trust and release contract

The workflow accepts either no Linux OpenPGP values or one complete configuration group. A partial
group fails before packaging. When enabled, the public GitHub Release contains only:

- `camellia-nexus-<version>-linux-x64.AppImage.asc`;
- `camellia-nexus-<version>-linux-x64.deb.asc`;
- `camellia-nexus-<version>-linux-x64.tar.gz.asc`;
- `RELEASE-SIGNING-KEY.asc`.

The workflow never uploads `*.asc` as a glob. The exact signatures are derived from the known Linux
final-artifact names, and the internal versioned key export is copied to the fixed public key name
only after its signing mode and fingerprint have been validated. The internal Actions Artifact also
retains the full configured fingerprint and `platform-key` trust classification in
`RELEASE-METADATA.json`, the same state in `NATIVE-SIGNING.md`, and all supply-chain evidence and
Cosign bundles; those internal files do not become ordinary GitHub Release Assets.

Publication reimports that exact public key into a temporary keyring and requires every signature's
`VALIDSIG` fingerprint to match the metadata before upload. Public readback repeats signature
verification and requires all three signatures to resolve to one full fingerprint. Candidate
workflows never receive signing secrets.
The private key, passphrase and temporary keyring are never published.
`scripts/resolve-linux-signing.sh` centralizes the optional-group and fingerprint validation used by
Actions and regression tests.

## Create a controlled signing key

Generate the key on a protected workstation with the organization tool. It
creates an offline primary key, a dedicated signing subkey, public identity
metadata, and the exact GitHub Actions configuration bundle:

```bash
bash scripts/new-camellia-linux-openpgp-key.sh \
  "$HOME/Secure/camellia-nexus-linux-signing" \
  'Camellia Computing Release <release@example.invalid>'
```

Run it from a checked-out `camellia-computing/.github` repository. Keep the
exported private subkey outside the repository and rotate it before expiry.
Commit the full fingerprint and activation/rotation state to `SECURITY.md` before enabling the
configuration, and publish it through an authenticated channel independent of the Release download.
`RELEASE-SIGNING-KEY.asc` is a convenience for obtaining the key, never the trust anchor for a key
shipped beside its own signatures.

## GitHub Actions configuration

Configure all three values together:

- variable `LINUX_GPG_FINGERPRINT`: the complete 40- or 64-hexadecimal fingerprint of the exact
  signing key;
- secret `LINUX_GPG_PRIVATE_KEY`: the complete ASCII-armored secret-key export;
- secret `LINUX_GPG_PASSPHRASE`: its non-empty passphrase.

Review the generated `metadata.json` and `variables.env`, then use its
uploader. Use the selected organization scope only after Nexus and Remote
Client are both approved to consume the same identity:

```bash
./github-actions/upload.sh --apply \
  --org camellia-computing --repos nexus,remote-client
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

- Publish the current non-secret fingerprint, validity period and rotation state in both
  `SECURITY.md` and the organization governance repository's signing registry before enabling it.
- Replace the private key, passphrase and fingerprint as one atomic configuration group.
- Keep a release in draft state when its expected signing mode, key or signature set differs from
  metadata; never repair or relabel a published release in place.
- Revoke and remove an exposed key, investigate every release produced during the exposure window,
  and communicate the new fingerprint over the independent trust channel.
- Published Release signatures, keys and artifact bytes are immutable. Keep every required public
  key and rotation record available for verification; retired private keys should remain offline or be
  securely destroyed according to the release custody policy.

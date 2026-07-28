#!/usr/bin/env bash
set -euo pipefail

repository="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
test_root="$(mktemp -d)"
trap 'rm -rf -- "$test_root"' EXIT

macos_env="$test_root/macos-env"
env -i PATH="$PATH" SIGNING_ENV_FILE="$macos_env" SIGNING_TEMP_DIRECTORY="$test_root" \
  bash "$repository/scripts/resolve-macos-signing.sh" >/dev/null
grep -Fqx 'CAMELLIA_NEXUS_MACOS_SIGN=disabled' "$macos_env"
grep -Fqx 'NATIVE_SIGNING=unsigned' "$macos_env"

: > "$macos_env"
env -i PATH="$PATH" APPLE_SIGNING_IDENTITY=- SIGNING_ENV_FILE="$macos_env" \
  SIGNING_TEMP_DIRECTORY="$test_root" bash "$repository/scripts/resolve-macos-signing.sh" >/dev/null
grep -Fqx 'CAMELLIA_NEXUS_MACOS_SIGN=required' "$macos_env"
grep -Fqx 'NATIVE_SIGNING=ad-hoc' "$macos_env"

if env -i PATH="$PATH" APPLE_CERTIFICATE=partial SIGNING_ENV_FILE="$macos_env" \
  SIGNING_TEMP_DIRECTORY="$test_root" bash "$repository/scripts/resolve-macos-signing.sh" >/dev/null 2>&1; then
  echo 'Partial macOS signing configuration was accepted' >&2
  exit 1
fi

linux_env="$test_root/linux-env"
env -i PATH="$PATH" SIGNING_ENV_FILE="$linux_env" \
  bash "$repository/scripts/resolve-linux-signing.sh" >/dev/null
grep -Fqx 'ARTIFACT_SIGNING=none' "$linux_env"
if env -i PATH="$PATH" LINUX_GPG_FINGERPRINT=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA \
  SIGNING_ENV_FILE="$linux_env" bash "$repository/scripts/resolve-linux-signing.sh" >/dev/null 2>&1; then
  echo 'Partial Linux signing configuration was accepted' >&2
  exit 1
fi

command -v gpg >/dev/null 2>&1 || { echo 'gpg is required for signing tests' >&2; exit 127; }
key_home="$test_root/key-home"
mkdir -m 700 "$key_home"
passphrase='fixture-passphrase'
GNUPGHOME="$key_home" gpg --batch --pinentry-mode loopback --passphrase "$passphrase" \
  --quick-generate-key 'Camellia Nexus signing fixture <fixture@example.invalid>' ed25519 cert 1d >/dev/null 2>&1
primary_fingerprint="$(GNUPGHOME="$key_home" gpg --batch --with-colons --with-fingerprint \
  --list-secret-keys | awk -F: '$1 == "fpr" { print toupper($10); exit }')"
GNUPGHOME="$key_home" gpg --batch --pinentry-mode loopback --passphrase "$passphrase" \
  --quick-add-key "$primary_fingerprint" ed25519 sign 1d >/dev/null 2>&1
fingerprint="$(GNUPGHOME="$key_home" gpg --batch --with-colons --with-subkey-fingerprint \
  --list-secret-keys | awk -F: '$1 == "fpr" { count += 1; if (count == 2) { print toupper($10); exit } }')"
private_key="$(GNUPGHOME="$key_home" gpg --batch --pinentry-mode loopback \
  --passphrase "$passphrase" --armor --export-secret-subkeys "$fingerprint!")"
: > "$linux_env"
LINUX_GPG_FINGERPRINT=" $fingerprint " \
LINUX_GPG_PRIVATE_KEY="$private_key" \
LINUX_GPG_PASSPHRASE="$passphrase" \
SIGNING_ENV_FILE="$linux_env" \
  bash "$repository/scripts/resolve-linux-signing.sh" >/dev/null
grep -Fqx 'ARTIFACT_SIGNING=openpgp-detached' "$linux_env"
grep -Fqx "LINUX_GPG_FINGERPRINT=$fingerprint" "$linux_env"

artifact_one="$test_root/camellia-nexus-fixture.AppImage"
artifact_two="$test_root/camellia-nexus-fixture.deb"
printf 'appimage fixture\n' > "$artifact_one"
printf 'deb fixture\n' > "$artifact_two"
public_key="$test_root/camellia-nexus-fixture.signing-key.asc"
LINUX_GPG_FINGERPRINT="$fingerprint" \
LINUX_GPG_PRIVATE_KEY="$private_key" \
LINUX_GPG_PASSPHRASE="$passphrase" \
LINUX_GPG_PUBLIC_KEY_OUTPUT="$public_key" \
  bash "$repository/scripts/linux-openpgp-sign.sh" "$artifact_one" "$artifact_two" >/dev/null

LINUX_GPG_FINGERPRINT="$fingerprint" LINUX_GPG_PUBLIC_KEY="$public_key" \
  bash "$repository/scripts/linux-openpgp-verify.sh" "$artifact_one" "$artifact_two" >/dev/null

stage_root="$test_root/stage"
mkdir -p "$stage_root/target/release/bundle/appimage" "$stage_root/target/release/bundle/deb"
printf '# Test\n' > "$stage_root/README.md"
printf 'Test license\n' > "$stage_root/LICENSE"
printf '#!/usr/bin/env bash\n' > "$stage_root/target/release/camellia-nexus"
printf '#!/usr/bin/env bash\n' > "$stage_root/target/release/camellia-nexus-privilege-broker"
chmod +x "$stage_root/target/release/camellia-nexus" \
  "$stage_root/target/release/camellia-nexus-privilege-broker"
printf 'appimage\n' > "$stage_root/target/release/bundle/appimage/Camellia Nexus.AppImage"
printf 'deb\n' > "$stage_root/target/release/bundle/deb/Camellia Nexus.deb"
(
  cd "$stage_root"
  APP_VERSION=1.2.3 \
  ARCH=x64 \
  ARTIFACT_SIGNING=openpgp-detached \
  BUILD_ID=1.2.3 \
  LINUX_GPG_FINGERPRINT="$fingerprint" \
  LINUX_GPG_PRIVATE_KEY="$private_key" \
  LINUX_GPG_PASSPHRASE="$passphrase" \
  NATIVE_SIGNING=not-applicable \
  PACKAGE_SHA=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
  PLATFORM=linux \
  RUNNER_ARCH=X64 \
    bash "$repository/scripts/stage-unix-release.sh" >/dev/null
)
[[ "$(find "$stage_root/dist-artifacts" -maxdepth 1 -type f | wc -l | tr -d ' ')" == 7 ]]
jq -e --arg fingerprint "$fingerprint" '
  .schemaVersion == 2 and
  .artifactSigning == {scheme:"openpgp-detached", fingerprint:$fingerprint}
' "$stage_root/build-metadata/linux-x64.json" >/dev/null
LINUX_GPG_FINGERPRINT="$fingerprint" \
LINUX_GPG_PUBLIC_KEY="$stage_root/dist-artifacts/camellia-nexus-1.2.3-linux-x64.signing-key.asc" \
  bash "$repository/scripts/linux-openpgp-verify.sh" \
    "$stage_root/dist-artifacts/camellia-nexus-1.2.3-linux-x64.AppImage" \
    "$stage_root/dist-artifacts/camellia-nexus-1.2.3-linux-x64.deb" \
    "$stage_root/dist-artifacts/camellia-nexus-1.2.3-linux-x64.tar.gz" >/dev/null

printf 'tampered\n' >> "$artifact_one"
if LINUX_GPG_FINGERPRINT="$fingerprint" LINUX_GPG_PUBLIC_KEY="$public_key" \
  bash "$repository/scripts/linux-openpgp-verify.sh" "$artifact_one" >/dev/null 2>&1; then
  echo 'Tampered Linux artifact passed OpenPGP verification' >&2
  exit 1
fi

echo 'Cross-platform signing tests passed'

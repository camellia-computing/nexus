#!/usr/bin/env bash
set -euo pipefail

: "${SIGNING_ENV_FILE:?SIGNING_ENV_FILE is required}"
: "${SIGNING_TEMP_DIRECTORY:?SIGNING_TEMP_DIRECTORY is required}"
[[ -d "$SIGNING_TEMP_DIRECTORY" ]] || { echo 'SIGNING_TEMP_DIRECTORY is not a directory' >&2; exit 2; }

signing_values=(APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY)
notary_values=(APPLE_API_ISSUER APPLE_API_KEY APPLE_API_PRIVATE_KEY)
signing_count=0
notary_count=0
for name in "${signing_values[@]}"; do
  [[ -z "${!name:-}" ]] || signing_count=$((signing_count + 1))
done
for name in "${notary_values[@]}"; do
  [[ -z "${!name:-}" ]] || notary_count=$((notary_count + 1))
done

if [[ "${APPLE_SIGNING_IDENTITY:-}" == - ]]; then
  [[ "$signing_count" == 1 && "$notary_count" == 0 ]] || {
    echo 'Ad-hoc signing uses APPLE_SIGNING_IDENTITY=- without certificate or notarization values' >&2
    exit 1
  }
  {
    echo 'CAMELLIA_NEXUS_MACOS_SIGN=required'
    echo 'NATIVE_SIGNING=ad-hoc'
  } >> "$SIGNING_ENV_FILE"
  echo 'macOS ad-hoc signing enabled'
  exit 0
fi

[[ "$signing_count" == 0 || "$signing_count" == 3 ]] || {
  echo 'APPLE_CERTIFICATE, APPLE_CERTIFICATE_PASSWORD and APPLE_SIGNING_IDENTITY must be configured together' >&2
  exit 1
}
[[ "$notary_count" == 0 || "$notary_count" == 3 ]] || {
  echo 'APPLE_API_ISSUER, APPLE_API_KEY and APPLE_API_PRIVATE_KEY must be configured together' >&2
  exit 1
}
[[ "$notary_count" == 0 || "$signing_count" == 3 ]] || {
  echo 'macOS notarization requires a complete signing configuration' >&2
  exit 1
}
if [[ "$notary_count" == 3 ]]; then
  [[ "$APPLE_API_ISSUER" =~ ^[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}$ ]] || {
    echo 'APPLE_API_ISSUER must be a canonical UUID' >&2
    exit 1
  }
  [[ "$APPLE_API_KEY" =~ ^[A-Z0-9]{10}$ ]] || {
    echo 'APPLE_API_KEY must be a 10-character App Store Connect key ID' >&2
    exit 1
  }
  [[ "$APPLE_API_PRIVATE_KEY" == *'BEGIN PRIVATE KEY'* && "$APPLE_API_PRIVATE_KEY" == *'END PRIVATE KEY'* ]] || {
    echo 'APPLE_API_PRIVATE_KEY is not a PEM private key' >&2
    exit 1
  }
fi

if [[ "$signing_count" == 0 ]]; then
  {
    echo 'CAMELLIA_NEXUS_MACOS_SIGN=disabled'
    echo 'NATIVE_SIGNING=unsigned'
  } >> "$SIGNING_ENV_FILE"
  echo 'macOS package will be unsigned'
  exit 0
fi

echo 'CAMELLIA_NEXUS_MACOS_SIGN=required' >> "$SIGNING_ENV_FILE"
if [[ "$notary_count" == 3 ]]; then
  api_key_path="$SIGNING_TEMP_DIRECTORY/AuthKey_$APPLE_API_KEY.p8"
  umask 077
  printf '%s\n' "$APPLE_API_PRIVATE_KEY" > "$api_key_path"
  {
    echo "APPLE_API_ISSUER=$APPLE_API_ISSUER"
    echo "APPLE_API_KEY=$APPLE_API_KEY"
    echo "APPLE_API_KEY_PATH=$api_key_path"
    echo "CAMELLIA_NEXUS_MACOS_API_KEY=$api_key_path"
    echo 'NATIVE_SIGNING=notarized'
  } >> "$SIGNING_ENV_FILE"
  echo 'macOS signing and notarization enabled'
else
  echo 'NATIVE_SIGNING=signed' >> "$SIGNING_ENV_FILE"
  echo 'macOS signing enabled without notarization'
fi

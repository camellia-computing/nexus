#!/usr/bin/env bash
set -euo pipefail

: "${SIGNING_ENV_FILE:?SIGNING_ENV_FILE is required}"
: "${SIGNING_TEMP_DIRECTORY:?SIGNING_TEMP_DIRECTORY is required}"
[[ -d "$SIGNING_TEMP_DIRECTORY" ]] || {
  echo 'SIGNING_TEMP_DIRECTORY is not a directory' >&2
  exit 2
}

all_values=(
  APPLE_CERTIFICATE
  APPLE_CERTIFICATE_PASSWORD
  APPLE_SIGNING_CERTIFICATE_SHA256
  APPLE_SIGNING_IDENTITY
  APPLE_API_ISSUER
  APPLE_API_KEY
  APPLE_API_PRIVATE_KEY
  APPLE_SECONDARY_CERTIFICATE
  APPLE_SECONDARY_CERTIFICATE_PASSWORD
  APPLE_SECONDARY_SIGNING_CERTIFICATE_SHA256
  APPLE_SECONDARY_SIGNING_IDENTITY
  APPLE_SECONDARY_API_ISSUER
  APPLE_SECONDARY_API_KEY
  APPLE_SECONDARY_API_PRIVATE_KEY
)

if [[ "${APPLE_SIGNING_IDENTITY:-}" == - ]]; then
  for name in "${all_values[@]}"; do
    [[ "$name" == APPLE_SIGNING_IDENTITY || -z "${!name:-}" ]] || {
      echo 'Ad-hoc signing cannot be combined with certificate or notarization credentials' >&2
      exit 1
    }
  done
  {
    echo 'APPLE_SIGNING_GROUP=ad-hoc'
    echo 'CAMELLIA_NEXUS_MACOS_SIGN=required'
    echo 'NATIVE_SIGNING=ad-hoc'
    echo 'DISTRIBUTION_TRUST=none'
  } >> "$SIGNING_ENV_FILE"
  echo 'macOS ad-hoc signing enabled'
  exit 0
fi

group_count() {
  local name count=0
  for name in "$@"; do
    [[ -z "${!name:-}" ]] || count=$((count + 1))
  done
  printf '%s\n' "$count"
}

primary_signing=(
  APPLE_CERTIFICATE
  APPLE_CERTIFICATE_PASSWORD
  APPLE_SIGNING_CERTIFICATE_SHA256
  APPLE_SIGNING_IDENTITY
)
secondary_signing=(
  APPLE_SECONDARY_CERTIFICATE
  APPLE_SECONDARY_CERTIFICATE_PASSWORD
  APPLE_SECONDARY_SIGNING_CERTIFICATE_SHA256
  APPLE_SECONDARY_SIGNING_IDENTITY
)
primary_notary=(APPLE_API_ISSUER APPLE_API_KEY APPLE_API_PRIVATE_KEY)
secondary_notary=(
  APPLE_SECONDARY_API_ISSUER
  APPLE_SECONDARY_API_KEY
  APPLE_SECONDARY_API_PRIVATE_KEY
)

primary_signing_count="$(group_count "${primary_signing[@]}")"
secondary_signing_count="$(group_count "${secondary_signing[@]}")"
primary_notary_count="$(group_count "${primary_notary[@]}")"
secondary_notary_count="$(group_count "${secondary_notary[@]}")"
for specification in \
  "primary signing:$primary_signing_count:4" \
  "secondary signing:$secondary_signing_count:4" \
  "primary notarization:$primary_notary_count:3" \
  "secondary notarization:$secondary_notary_count:3"
do
  IFS=: read -r label count complete <<< "$specification"
  [[ "$count" == 0 || "$count" == "$complete" ]] || {
    echo "The $label credential group is partial" >&2
    exit 1
  }
done
[[ "$primary_notary_count" == 0 || "$primary_signing_count" == 4 ]] || {
  echo 'Primary notarization requires the complete primary signing group' >&2
  exit 1
}
[[ "$secondary_notary_count" == 0 || "$secondary_signing_count" == 4 ]] || {
  echo 'Secondary notarization requires the complete secondary signing group' >&2
  exit 1
}

selected_group=none
selected_notary_count=0
if [[ "$primary_signing_count" == 4 ]]; then
  selected_group=primary
  selected_certificate="$APPLE_CERTIFICATE"
  selected_password="$APPLE_CERTIFICATE_PASSWORD"
  selected_sha256="$APPLE_SIGNING_CERTIFICATE_SHA256"
  selected_identity="$APPLE_SIGNING_IDENTITY"
  selected_notary_count="$primary_notary_count"
  selected_api_issuer="${APPLE_API_ISSUER:-}"
  selected_api_key="${APPLE_API_KEY:-}"
  selected_api_private_key="${APPLE_API_PRIVATE_KEY:-}"
elif [[ "$secondary_signing_count" == 4 ]]; then
  selected_group=secondary
  selected_certificate="$APPLE_SECONDARY_CERTIFICATE"
  selected_password="$APPLE_SECONDARY_CERTIFICATE_PASSWORD"
  selected_sha256="$APPLE_SECONDARY_SIGNING_CERTIFICATE_SHA256"
  selected_identity="$APPLE_SECONDARY_SIGNING_IDENTITY"
  selected_notary_count="$secondary_notary_count"
  selected_api_issuer="${APPLE_SECONDARY_API_ISSUER:-}"
  selected_api_key="${APPLE_SECONDARY_API_KEY:-}"
  selected_api_private_key="${APPLE_SECONDARY_API_PRIVATE_KEY:-}"
fi

if [[ "$selected_group" == none ]]; then
  [[ "$primary_notary_count" == 0 && "$secondary_notary_count" == 0 ]] || {
    echo 'Notarization credentials cannot be used without a signing identity' >&2
    exit 1
  }
  {
    echo 'APPLE_SIGNING_GROUP=none'
    echo 'CAMELLIA_NEXUS_MACOS_SIGN=disabled'
    echo 'NATIVE_SIGNING=unsigned'
    echo 'DISTRIBUTION_TRUST=none'
  } >> "$SIGNING_ENV_FILE"
  echo 'macOS package will be unsigned'
  exit 0
fi

[[ "$selected_identity" != *$'\n'* && "$selected_identity" != *$'\r'* ]] || {
  echo 'APPLE signing identity must not contain line breaks' >&2
  exit 1
}
[[ "$selected_sha256" =~ ^[0-9A-F]{64}$ ]] || {
  echo 'APPLE signing certificate SHA-256 must be canonical uppercase hexadecimal' >&2
  exit 1
}
if [[ "$selected_notary_count" == 3 ]]; then
  [[ "$selected_api_issuer" =~ ^[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}$ ]] || {
    echo 'Apple API issuer must be a canonical UUID' >&2
    exit 1
  }
  [[ "$selected_api_key" =~ ^[A-Z0-9]{10}$ ]] || {
    echo 'Apple API key must be a 10-character App Store Connect key ID' >&2
    exit 1
  }
  [[ "$selected_api_private_key" == *'BEGIN PRIVATE KEY'* &&
     "$selected_api_private_key" == *'END PRIVATE KEY'* ]] || {
    echo 'Apple API private key is not a PEM private key' >&2
    exit 1
  }
fi

certificate_path="$SIGNING_TEMP_DIRECTORY/camellia-nexus-macos-validation.p12"
certificate_pem="$SIGNING_TEMP_DIRECTORY/camellia-nexus-macos-certificate.pem"
umask 077
if ! printf '%s' "$selected_certificate" |
  python3 -c '
import base64
import sys

payload = b"".join(sys.stdin.buffer.read().split())
try:
    decoded = base64.b64decode(payload, validate=True)
except Exception as error:
    raise SystemExit(f"Apple certificate is invalid: {error}")
if not decoded:
    raise SystemExit("Apple certificate decoded to an empty file")
sys.stdout.buffer.write(decoded)
' > "$certificate_path"; then
  rm -f -- "$certificate_path"
  exit 1
fi
APPLE_SELECTED_CERTIFICATE_PASSWORD="$selected_password" openssl pkcs12 \
  -in "$certificate_path" \
  -clcerts \
  -nokeys \
  -passin env:APPLE_SELECTED_CERTIFICATE_PASSWORD \
  -out "$certificate_pem" >/dev/null 2>&1 || {
    rm -f -- "$certificate_path" "$certificate_pem"
    echo 'Apple certificate is not a valid password-protected PKCS#12 identity' >&2
    exit 1
  }
certificate_count="$(grep -c '^-----BEGIN CERTIFICATE-----$' "$certificate_pem" || true)"
[[ "$certificate_count" == 1 ]] || {
  rm -f -- "$certificate_path" "$certificate_pem"
  echo "Apple certificate must contain exactly one leaf; found $certificate_count" >&2
  exit 1
}
certificate_sha256="$(
  openssl x509 -in "$certificate_pem" -outform DER |
    shasum -a 256 |
    awk '{ print toupper($1) }'
)"
rm -f -- "$certificate_path" "$certificate_pem"
[[ "$certificate_sha256" == "$selected_sha256" ]] || {
  echo 'The selected macOS P12 does not match its reviewed SHA-256 fingerprint' >&2
  exit 1
}

{
  echo "APPLE_SIGNING_GROUP=$selected_group"
  echo 'CAMELLIA_NEXUS_MACOS_SIGN=required'
  echo 'DISTRIBUTION_TRUST=derive'
  echo "SIGNING_CERTIFICATE_SHA256=$certificate_sha256"
  echo "SIGNING_IDENTITY=$selected_identity"
} >> "$SIGNING_ENV_FILE"
if [[ "$selected_notary_count" == 3 ]]; then
  api_key_path="$SIGNING_TEMP_DIRECTORY/AuthKey_$selected_api_key.p8"
  umask 077
  printf '%s\n' "$selected_api_private_key" > "$api_key_path"
  {
    echo "APPLE_API_ISSUER=$selected_api_issuer"
    echo "APPLE_API_KEY=$selected_api_key"
    echo "APPLE_API_KEY_PATH=$api_key_path"
    echo "CAMELLIA_NEXUS_MACOS_API_KEY=$api_key_path"
    echo 'NATIVE_SIGNING=notarized'
  } >> "$SIGNING_ENV_FILE"
  echo "macOS signing and notarization enabled with the $selected_group group"
else
  echo 'NATIVE_SIGNING=signed' >> "$SIGNING_ENV_FILE"
  echo "macOS signing enabled with the $selected_group group"
fi

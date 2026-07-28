#!/usr/bin/env bash
set -euo pipefail

: "${APP_VERSION:?APP_VERSION is required}"
: "${ARCH:?ARCH is required}"
: "${BUILD_ID:?BUILD_ID is required}"
: "${PACKAGE_SHA:?PACKAGE_SHA is required}"
: "${NATIVE_SIGNING:?NATIVE_SIGNING is required}"
: "${DISTRIBUTION_TRUST:?DISTRIBUTION_TRUST is required}"
: "${ARTIFACT_SIGNING:?ARTIFACT_SIGNING is required}"
: "${PLATFORM:?PLATFORM is required}"
: "${RUNNER_ARCH:?RUNNER_ARCH is required}"

[[ "$APP_VERSION" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || {
  echo "Invalid package version: $APP_VERSION" >&2
  exit 1
}
[[ "$BUILD_ID" =~ ^[A-Za-z0-9][A-Za-z0-9.-]{0,127}$ ]] || {
  echo "Invalid package build ID: $BUILD_ID" >&2
  exit 1
}
[[ "$PACKAGE_SHA" =~ ^[0-9a-f]{40}$ ]] || {
  echo "Invalid package commit: $PACKAGE_SHA" >&2
  exit 1
}

case "$RUNNER_ARCH:$ARCH" in
  X64:x64|ARM64:arm64) ;;
  *) echo "Unsupported release architecture: runner=$RUNNER_ARCH expected=$ARCH" >&2; exit 1 ;;
esac

case "$PLATFORM:$NATIVE_SIGNING" in
  linux:not-applicable|macos:unsigned|macos:ad-hoc|macos:signed|macos:notarized) ;;
  *) echo "Invalid native signing state for $PLATFORM: $NATIVE_SIGNING" >&2; exit 1 ;;
esac
case "$PLATFORM:$ARTIFACT_SIGNING" in
  linux:none|linux:openpgp-detached|macos:none) ;;
  *) echo "Invalid artifact signing state for $PLATFORM: $ARTIFACT_SIGNING" >&2; exit 1 ;;
esac
signing_identity="${SIGNING_IDENTITY:-}"
case "$PLATFORM:$NATIVE_SIGNING:$DISTRIBUTION_TRUST" in
  linux:not-applicable:not-applicable|macos:unsigned:none|macos:ad-hoc:none) ;;
  macos:signed:private-trust|macos:signed:public-trust|macos:notarized:public-trust)
    [[ -n "$signing_identity" && "$signing_identity" != *$'\n'* &&
       "$signing_identity" != *$'\r'* ]] || {
      echo "A signed macOS package requires one printable signing identity" >&2
      exit 1
    }
    ;;
  *) echo "Invalid distribution trust for $PLATFORM/$NATIVE_SIGNING: $DISTRIBUTION_TRUST" >&2; exit 1 ;;
esac
if [[ "$NATIVE_SIGNING" == unsigned || "$NATIVE_SIGNING" == ad-hoc ||
      "$NATIVE_SIGNING" == not-applicable ]]; then
  [[ -z "$signing_identity" ]] || {
    echo "$NATIVE_SIGNING packages may not claim a signing identity" >&2
    exit 1
  }
fi

name="camellia-nexus-$BUILD_ID-$PLATFORM-$ARCH"
bundle=target/release/bundle
mkdir -p build-metadata dist-artifacts
[[ -d "$bundle" ]] || { echo "Bundle directory was not produced: $bundle" >&2; exit 1; }

if [[ "$PLATFORM" == linux ]]; then
  binary_stage="dist-staging/$name"
  mkdir -p "$binary_stage"
  cp target/release/camellia-nexus "$binary_stage/camellia-nexus"
  [[ -x target/release/camellia-nexus-privilege-broker ]] || {
    echo "Linux privilege broker was not produced" >&2
    exit 1
  }
  cp target/release/camellia-nexus-privilege-broker \
    "$binary_stage/camellia-nexus-privilege-broker"
  chmod +x "$binary_stage/camellia-nexus"
  chmod +x "$binary_stage/camellia-nexus-privilege-broker"
  cp README.md LICENSE "$binary_stage/"
  tar -czf "dist-artifacts/$name.tar.gz" -C dist-staging "$name"
fi

if [[ "$PLATFORM" == macos ]]; then
  shopt -s nullglob
  app_bundles=("$bundle"/macos/*.app "$bundle"/*.app)
  shopt -u nullglob
  ((${#app_bundles[@]} == 1)) || {
    echo "Expected exactly one macOS app bundle; found ${#app_bundles[@]}" >&2
    exit 1
  }
  app_bundle="${app_bundles[0]}"
  if [[ "$NATIVE_SIGNING" != unsigned ]]; then
    codesign --verify --deep --strict --verbose=2 "$app_bundle"
  fi
  if [[ "$NATIVE_SIGNING" == notarized ]]; then
    xcrun stapler validate "$app_bundle"
    spctl --assess --type execute --verbose=2 "$app_bundle"
  fi
  tar -czf "dist-artifacts/$name.tar.gz" -C "$(dirname "$app_bundle")" "$(basename "$app_bundle")"
fi

stage_exact_package() {
  local pattern="$1" extension="$2" file
  local -a matches=()
  while IFS= read -r -d '' file; do
    matches+=("$file")
  done < <(find "$bundle" -type f -name "$pattern" -print0)
  ((${#matches[@]} == 1)) || {
    echo "Expected exactly one $pattern package; found ${#matches[@]}" >&2
    return 1
  }
  cp "${matches[0]}" "dist-artifacts/$name$extension"
}

case "$PLATFORM" in
  linux)
    stage_exact_package '*.AppImage' .AppImage
    stage_exact_package '*.deb' .deb
    expected=(
      "$name.tar.gz"
      "$name.AppImage"
      "$name.deb"
    )
    ;;
  macos)
    stage_exact_package '*.dmg' .dmg
    expected=(
      "$name.tar.gz"
      "$name.dmg"
    )
    if [[ "$NATIVE_SIGNING" == signed || "$NATIVE_SIGNING" == notarized ]]; then
      codesign --verify --verbose=2 "dist-artifacts/$name.dmg"
    fi
    ;;
  *) echo "Unsupported package platform: $PLATFORM" >&2; exit 1 ;;
esac

for artifact in "${expected[@]}"; do
  [[ -f "dist-artifacts/$artifact" ]] || {
    echo "Missing $PLATFORM package: $artifact" >&2
    exit 1
  }
done
artifact_signing_json='{"scheme":"none","trust":"none"}'
if [[ "$PLATFORM:$ARTIFACT_SIGNING" == linux:openpgp-detached ]]; then
  signing_key="dist-artifacts/$name.signing-key.asc"
  artifacts_to_sign=()
  for artifact in "${expected[@]}"; do
    artifacts_to_sign+=("dist-artifacts/$artifact")
  done
  LINUX_GPG_PUBLIC_KEY_OUTPUT="$signing_key" \
    bash "$(dirname "$0")/linux-openpgp-sign.sh" "${artifacts_to_sign[@]}"
  fingerprint="${LINUX_GPG_FINGERPRINT//[[:space:]]/}"
  fingerprint="${fingerprint^^}"
  artifact_signing_json="$(jq -cn --arg fingerprint "$fingerprint" \
    '{scheme:"openpgp-detached", trust:"platform-key", fingerprint:$fingerprint}')"
fi
staged_count="$(find dist-artifacts -maxdepth 1 -type f -name "$name.*" | wc -l | tr -d ' ')"
[[ "$staged_count" == "${#expected[@]}" ]] || {
  if [[ "$ARTIFACT_SIGNING" != openpgp-detached || \
        "$staged_count" != "$(( ${#expected[@]} * 2 + 1 ))" ]]; then
    echo "Unexpected release artifacts were staged for $name" >&2
    exit 1
  fi
}

jq -n \
  --arg architecture "$ARCH" \
  --arg buildId "$BUILD_ID" \
  --arg commit "$PACKAGE_SHA" \
  --arg delivery "installable" \
  --arg distributionTrust "$DISTRIBUTION_TRUST" \
  --arg identity "$signing_identity" \
  --arg nativeSigning "$NATIVE_SIGNING" \
  --argjson artifactSigning "$artifact_signing_json" \
  --arg platform "$PLATFORM" \
  --arg version "$APP_VERSION" \
  '{
    schemaVersion: 3,
    product: "Camellia Nexus",
    version: $version,
    buildId: $buildId,
    commit: $commit,
    platform: $platform,
    architecture: $architecture,
    nativeSigning: $nativeSigning,
    distributionTrust: $distributionTrust,
    identity: (if $identity == "" then null else $identity end),
    artifactSigning: $artifactSigning,
    delivery: $delivery
  }' > "build-metadata/$PLATFORM-$ARCH.json"

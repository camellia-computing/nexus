#!/usr/bin/env bash
set -euo pipefail

repository="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
test_root="$(mktemp -d)"
trap 'rm -rf "$test_root"' EXIT

mkdir -p "$test_root/target/release/bundle/appimage" "$test_root/target/release/bundle/deb"
printf '# Test\n' > "$test_root/README.md"
printf 'Test license\n' > "$test_root/LICENSE"
printf '#!/usr/bin/env bash\n' > "$test_root/target/release/camellia-nexus"
chmod +x "$test_root/target/release/camellia-nexus"
printf '#!/usr/bin/env bash\n' > "$test_root/target/release/camellia-nexus-privilege-broker"
chmod +x "$test_root/target/release/camellia-nexus-privilege-broker"
printf 'appimage\n' > "$test_root/target/release/bundle/appimage/Camellia Nexus.AppImage"
printf 'deb\n' > "$test_root/target/release/bundle/deb/Camellia Nexus.deb"

run_stage() (
  local platform="$1" architecture="$2" runner_architecture="$3" native_signing="$4"
  local distribution_trust="$5" signing_identity="${6:-}"
  cd "$test_root"
  APP_VERSION=1.2.3 \
    ARTIFACT_SIGNING=none \
    ARCH="$architecture" \
    BUILD_ID=1.2.3 \
    DISTRIBUTION_TRUST="$distribution_trust" \
    PACKAGE_SHA=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
    NATIVE_SIGNING="$native_signing" \
    PLATFORM="$platform" \
    RUNNER_ARCH="$runner_architecture" \
    SIGNING_IDENTITY="$signing_identity" \
    bash "$repository/scripts/stage-unix-release.sh"
)

run_stage linux x64 X64 not-applicable not-applicable
[[ "$(find "$test_root/dist-artifacts" -maxdepth 1 -type f | wc -l | tr -d ' ')" == 3 ]]
expected_assets=(
  camellia-nexus-1.2.3-linux-x64.AppImage
  camellia-nexus-1.2.3-linux-x64.deb
  camellia-nexus-1.2.3-linux-x64.tar.gz
)
for asset in "${expected_assets[@]}"; do
  [[ -f "$test_root/dist-artifacts/$asset" ]] || {
    echo "Expected staged fixture is missing: $asset" >&2
    exit 1
  }
done
tar -tzf "$test_root/dist-artifacts/camellia-nexus-1.2.3-linux-x64.tar.gz" |
  grep -Fqx 'camellia-nexus-1.2.3-linux-x64/camellia-nexus-privilege-broker'
tar -tzf "$test_root/dist-artifacts/camellia-nexus-1.2.3-linux-x64.tar.gz" |
  grep -Fqx 'camellia-nexus-1.2.3-linux-x64/camellia-nexus'
jq -e '
  .schemaVersion == 3 and
  .version == "1.2.3" and
  .commit == "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" and
  .platform == "linux" and
  .architecture == "x64" and
  .nativeSigning == "not-applicable" and
  .distributionTrust == "not-applicable" and
  .identity == null and
  .artifactSigning == {scheme:"none", trust:"none"} and
  .delivery == "installable"
' "$test_root/build-metadata/linux-x64.json" >/dev/null

mkdir -p "$test_root/target/release/bundle/duplicate"
printf 'duplicate\n' > "$test_root/target/release/bundle/duplicate/duplicate.AppImage"
rm -rf "$test_root/build-metadata" "$test_root/dist-artifacts" "$test_root/dist-staging"
if run_stage linux x64 X64 not-applicable not-applicable >/dev/null 2>&1; then
  echo 'Duplicate Linux packages were accepted' >&2
  exit 1
fi

rm -rf "$test_root/build-metadata" "$test_root/dist-artifacts" "$test_root/dist-staging" "$test_root/target/release/bundle"
mkdir -p "$test_root/target/release/bundle/macos/Camellia Nexus.app/Contents/MacOS" "$test_root/target/release/bundle/dmg"
printf 'application\n' > "$test_root/target/release/bundle/macos/Camellia Nexus.app/Contents/MacOS/Camellia Nexus"
printf 'dmg\n' > "$test_root/target/release/bundle/dmg/Camellia Nexus.dmg"
run_stage macos arm64 ARM64 unsigned none
for asset in camellia-nexus-1.2.3-macos-arm64.dmg camellia-nexus-1.2.3-macos-arm64.tar.gz; do
  [[ -f "$test_root/dist-artifacts/$asset" ]] || {
    echo "Expected staged fixture is missing: $asset" >&2
    exit 1
  }
done
[[ "$(find "$test_root/dist-artifacts" -maxdepth 1 -type f | wc -l | tr -d ' ')" == 2 ]]
jq -e '
  .platform == "macos" and
  .architecture == "arm64" and
  .nativeSigning == "unsigned" and
  .distributionTrust == "none" and
  .identity == null and
  .delivery == "installable"
' "$test_root/build-metadata/macos-arm64.json" >/dev/null

rm -rf "$test_root/build-metadata" "$test_root/dist-artifacts" "$test_root/dist-staging"
if run_stage macos arm64 ARM64 unsigned public-trust >/dev/null 2>&1; then
  echo 'Unsigned macOS package was allowed to claim public trust' >&2
  exit 1
fi

echo 'Unix release staging tests passed'

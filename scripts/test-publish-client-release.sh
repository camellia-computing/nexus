#!/usr/bin/env bash
set -euo pipefail

root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT
remote="$root/remote"
mkdir "$remote" "$root/runner"

raw_assets=(
  camellia-nexus-1.2.3-linux-x64.AppImage
  camellia-nexus-1.2.3-linux-x64.deb
  camellia-nexus-1.2.3-linux-x64.tar.gz
  camellia-nexus-1.2.3-macos-arm64.dmg
  camellia-nexus-1.2.3-macos-arm64.tar.gz
  camellia-nexus-1.2.3-macos-x64.dmg
  camellia-nexus-1.2.3-macos-x64.tar.gz
  camellia-nexus-1.2.3-windows-x64.msi
  camellia-nexus-1.2.3-windows-x64-portable.zip
)

package_upload_paths() {
  awk '
    /- name: Upload package/ { upload = 1 }
    upload && /path: \|/ { capture = 1; next }
    capture && /if-no-files-found:/ { exit }
    capture { sub(/^[[:space:]]+/, ""); print }
  ' .github/workflows/client-packages.yml
}

# shellcheck disable=SC2016 # GitHub expressions must remain literal in this workflow assertion.
grep -Fxq \
  'dist-artifacts/camellia-nexus-${{ inputs.build-id }}-${{ matrix.platform }}-${{ matrix.arch }}-portable.zip' \
  < <(package_upload_paths) || {
  echo 'Package upload does not include the Windows portable archive' >&2
  exit 1
}

workflow_asset_templates() {
  awk '
    /cat > "\$RUNNER_TEMP\/expected-build-assets" <<EOF/ { capture = 1; next }
    capture && /^[[:space:]]*EOF$/ { exit }
    capture { sub(/^[[:space:]]+/, ""); print }
  ' .github/workflows/publish-release.yml
}

publisher_asset_templates() {
  awk '
    /cat > "\$expected_raw_names" <<EOF/ { capture = 1; next }
    capture && /^RELEASE-METADATA\.json$/ { exit }
    capture { print }
  ' scripts/publish-client-release.sh
}

fixture_asset_templates() {
  printf '%s\n' "${raw_assets[@]}" | sed 's/-1\.2\.3-/-$VERSION-/'
}

diff -u <(workflow_asset_templates) <(publisher_asset_templates)
diff -u <(workflow_asset_templates) <(fixture_asset_templates)

for name in "${raw_assets[@]}"; do
  printf 'fixture:%s\n' "$name" > "$remote/$name"
done
linux_signing_fingerprint=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA
linux_signing_assets=(
  camellia-nexus-1.2.3-linux-x64.AppImage.asc
  camellia-nexus-1.2.3-linux-x64.deb.asc
  camellia-nexus-1.2.3-linux-x64.signing-key.asc
  camellia-nexus-1.2.3-linux-x64.tar.gz.asc
)
for name in "${linux_signing_assets[@]}"; do
  printf 'OpenPGP fixture:%s\n' "$name" > "$remote/$name"
done
jq -n --arg fingerprint "$linux_signing_fingerprint" '{
  schemaVersion: 2,
  product: "Camellia Nexus",
  version: "1.2.3",
  commit: "release-sha",
  builds: [
    {schemaVersion: 2, product: "Camellia Nexus", version: "1.2.3", buildId: "1.2.3", commit: "release-sha", platform: "linux", architecture: "x64", nativeSigning: "not-applicable", artifactSigning: {scheme: "openpgp-detached", fingerprint: $fingerprint}},
    {schemaVersion: 2, product: "Camellia Nexus", version: "1.2.3", buildId: "1.2.3", commit: "release-sha", platform: "macos", architecture: "arm64", nativeSigning: "unsigned", artifactSigning: {scheme: "none"}},
    {schemaVersion: 2, product: "Camellia Nexus", version: "1.2.3", buildId: "1.2.3", commit: "release-sha", platform: "macos", architecture: "x64", nativeSigning: "unsigned", artifactSigning: {scheme: "none"}},
    {schemaVersion: 2, product: "Camellia Nexus", version: "1.2.3", buildId: "1.2.3", commit: "release-sha", platform: "windows", architecture: "x64", nativeSigning: "unsigned", artifactSigning: {scheme: "none"}}
  ]
}' > "$remote/RELEASE-METADATA.json"
(cd "$remote" && sha256sum camellia-nexus-* RELEASE-METADATA.json > SHA256SUMS)
for subject in "$remote"/*; do
  printf 'mock bundle for %s\n' "$(basename "$subject")" > "$subject.sigstore.json"
done

# Invoked by the publisher subprocess through the exported function.
# shellcheck disable=SC2329
gh() {
  if [[ "$1" == api && "$2" == --paginate && "$3" == --slurp &&
        "$4" == 'repos/test/repository/releases?per_page=100' ]]; then
    [[ "$GH_TOKEN" == policy-token ]] || return 1
    assets="$(find "$MOCK_REMOTE" -maxdepth 1 -type f -printf '%f\n' | LC_ALL=C sort |
      jq -Rsc 'split("\n")[:-1] | to_entries | map({id: (.key + 1), name: .value})')"
    jq -nc --argjson assets "$assets" '[[{
      id: 42,
      draft: false,
      immutable: true,
      target_commitish: "release-sha",
      tag_name: "v1.2.3",
      name: "Camellia Nexus 1.2.3",
      author: {login: "release-bot"},
      body: "<!-- release-pr:17 -->\n<!-- release-commit:release-sha -->",
      assets: $assets
    }]]'
  elif [[ "$1" == api && "$2" == -H && "$4" =~ ^repos/test/repository/releases/assets/([1-9][0-9]*)$ ]]; then
    local name
    name="$(find "$MOCK_REMOTE" -maxdepth 1 -type f -printf '%f\n' | LC_ALL=C sort | sed -n "${BASH_REMATCH[1]}p")"
    [[ -n "$name" ]]
    command cat "$MOCK_REMOTE/$name"
  else
    echo "Unexpected mock gh call: $*" >&2
    return 1
  fi
}

# Invoked by the publisher subprocess through the exported function.
# shellcheck disable=SC2329
cosign() {
  [[ "$1" == verify-blob && -f "$2" ]] || {
    echo "Verification-only recovery attempted an unexpected Cosign operation: $*" >&2
    return 1
  }
}
gpg() {
  if [[ " $* " == *' --with-colons '* && " $* " == *' --list-keys '* ]]; then
    printf 'pub:-:255:22:0000000000000000:0:0::::::scESC:::::ed25519:::0:\n'
    printf 'fpr:::::::::%s:\n' "$MOCK_GPG_FINGERPRINT"
  elif [[ " $* " == *' --verify '* ]]; then
    printf '[GNUPG:] VALIDSIG %s 2026-01-01 0 0 0 0 0 0 0\n' "$MOCK_GPG_FINGERPRINT"
  fi
}
export -f gh cosign gpg
export MOCK_REMOTE="$remote"
export MOCK_GPG_FINGERPRINT="$linux_signing_fingerprint"

run_verification() {
  ASSETS_DIRECTORY="$root/nonexistent" \
  GH_TOKEN=test-token \
  GITHUB_REPOSITORY=test/repository \
  RELEASE_APP_LOGIN=release-bot \
  RELEASE_ID=42 \
  RELEASE_POLICY_TOKEN=policy-token \
  RELEASE_PR_NUMBER=17 \
  RELEASE_SHA=release-sha \
  RELEASE_SIGNING_IDENTITY=https://github.com/test/repository/.github/workflows/publish-release.yml@refs/tags/v1.2.3 \
  RELEASE_TAG=v1.2.3 \
  RUNNER_TEMP="$root/runner" \
  VERIFY_PUBLISHED_ONLY=true \
  VERSION=1.2.3 \
    bash scripts/publish-client-release.sh
}

run_verification >/dev/null
rm "$remote/SHA256SUMS.sigstore.json"
if run_verification >/dev/null 2>&1; then
  echo "Published release verification accepted an incomplete asset set" >&2
  exit 1
fi

draft_local="$root/draft-local"
draft_remote="$root/draft-remote"
mkdir "$draft_local" "$draft_remote"
while IFS= read -r -d '' subject; do
  cp "$subject" "$draft_local/"
  cp "$subject" "$draft_remote/"
done < <(find "$remote" -maxdepth 1 -type f ! -name '*.sigstore.json' -print0)

write_bundle() {
  local subject="$1"
  sha256sum "$subject" | awk '{print $1}' > "$subject.sigstore.json"
}
for subject in "$draft_local"/* "$draft_remote"/*; do
  [[ "$subject" == *.sigstore.json ]] || write_bundle "$subject"
done

conflicting_asset=camellia-nexus-1.2.3-windows-x64-portable.zip
printf 'conflicting draft bytes\n' > "$draft_remote/$conflicting_asset"
printf 'invalid bundle\n' > "$draft_remote/SHA256SUMS.sigstore.json"
replacement_log="$root/draft-replacements"

gh() {
  if [[ "$1" == api && "$2" == --paginate && "$3" == --slurp &&
        "$4" == 'repos/test/repository/releases?per_page=100' ]]; then
    [[ "$GH_TOKEN" == policy-token ]] || return 1
    assets="$(find "$MOCK_REMOTE" -maxdepth 1 -type f -printf '%f\n' | LC_ALL=C sort |
      jq -Rsc 'split("\n")[:-1] | to_entries | map({id: (.key + 1), name: .value})')"
    jq -nc --argjson assets "$assets" '[[{
      id: 42,
      draft: true,
      immutable: false,
      target_commitish: "release-sha",
      tag_name: "v1.2.3",
      name: "Camellia Nexus 1.2.3",
      author: {login: "release-bot"},
      body: "<!-- release-pr:17 -->\n<!-- release-commit:release-sha -->",
      assets: $assets
    }]]'
  elif [[ "$1" == api && "$2" == -H && "$4" =~ ^repos/test/repository/releases/assets/([1-9][0-9]*)$ ]]; then
    local name
    name="$(find "$MOCK_REMOTE" -maxdepth 1 -type f -printf '%f\n' | LC_ALL=C sort | sed -n "${BASH_REMATCH[1]}p")"
    [[ -n "$name" ]]
    command cat "$MOCK_REMOTE/$name"
  elif [[ "$1" == api && "$2" == -X && "$3" == DELETE && "$4" =~ ^repos/test/repository/releases/assets/([1-9][0-9]*)$ ]]; then
    local name
    name="$(find "$MOCK_REMOTE" -maxdepth 1 -type f -printf '%f\n' | LC_ALL=C sort | sed -n "${BASH_REMATCH[1]}p")"
    [[ -n "$name" ]]
    printf '%s\n' "$name" >> "$MOCK_REPLACEMENTS"
    rm "$MOCK_REMOTE/$name"
  elif [[ "$1" == release && "$2" == upload && "$3" == v1.2.3 && -f "$4" ]]; then
    cp "$4" "$MOCK_REMOTE/$(basename "$4")"
  else
    echo "Intentional stop after draft reconciliation: $*" >&2
    return 1
  fi
}

cosign() {
  case "$1" in
    sign-blob)
      [[ "$2" == --yes && "$3" == --bundle && -f "$5" ]] || return 1
      sha256sum "$5" | awk '{print $1}' > "$4"
      ;;
    verify-blob)
      [[ -f "$2" && "$3" == --bundle && -f "$4" ]] || return 1
      [[ "$(cat "$4")" == "$(sha256sum "$2" | awk '{print $1}')" ]]
      ;;
    *) return 1 ;;
  esac
}
export -f gh cosign gpg
export MOCK_REMOTE="$draft_remote" MOCK_REPLACEMENTS="$replacement_log"

if ASSETS_DIRECTORY="$draft_local" \
  GH_TOKEN=test-token \
  GITHUB_REPOSITORY=test/repository \
  RELEASE_APP_LOGIN=release-bot \
  RELEASE_ID=42 \
  RELEASE_POLICY_TOKEN=policy-token \
  RELEASE_PR_NUMBER=17 \
  RELEASE_SHA=release-sha \
  RELEASE_SIGNING_IDENTITY=https://github.com/test/repository/.github/workflows/publish-release.yml@refs/heads/main \
  RELEASE_TAG=v1.2.3 \
  RUNNER_TEMP="$root/runner" \
  VERSION=1.2.3 \
    bash scripts/publish-client-release.sh >/dev/null 2>&1; then
  echo "Draft reconciliation unexpectedly crossed the mocked authorization boundary" >&2
  exit 1
fi
[[ "$(sort -u "$replacement_log" | wc -l | tr -d ' ')" == 2 ]] || {
  echo "Draft reconciliation did not replace both conflicting assets" >&2
  exit 1
}
cmp "$draft_local/$conflicting_asset" "$draft_remote/$conflicting_asset"
cmp "$draft_local/SHA256SUMS.sigstore.json" "$draft_remote/SHA256SUMS.sigstore.json"

echo "Published client release recovery tests passed"

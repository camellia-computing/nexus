#!/usr/bin/env bash
set -euo pipefail

: "${GH_TOKEN:?GH_TOKEN is required}"
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
: "${RELEASE_POLICY_TOKEN:?RELEASE_POLICY_TOKEN is required}"
: "${RELEASE_SHA:?RELEASE_SHA is required}"
: "${RELEASE_SIGNING_IDENTITY:?RELEASE_SIGNING_IDENTITY is required}"
: "${RELEASE_TAG:?RELEASE_TAG is required}"
: "${VERSION:?VERSION is required}"
: "${RELEASE_ID:?RELEASE_ID is required}"
: "${RELEASE_APP_LOGIN:?RELEASE_APP_LOGIN is required}"
: "${RELEASE_PR_NUMBER:?RELEASE_PR_NUMBER is required}"

assets_directory="${ASSETS_DIRECTORY:-release-assets}"
verify_published_only="${VERIFY_PUBLISHED_ONLY:-false}"
script_directory="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
tag="v$VERSION"
[[ "$RELEASE_TAG" == "$tag" ]] || { echo "Release tag does not match v$VERSION" >&2; exit 1; }
tag_identity="https://github.com/$GITHUB_REPOSITORY/.github/workflows/publish-release.yml@refs/tags/$tag"
main_identity="https://github.com/$GITHUB_REPOSITORY/.github/workflows/publish-release.yml@refs/heads/main"
[[ "$RELEASE_SIGNING_IDENTITY" == "$tag_identity" || "$RELEASE_SIGNING_IDENTITY" == "$main_identity" ]] || {
  echo 'Release signing identity is not an authorized publication workflow' >&2
  exit 1
}
identity="$RELEASE_SIGNING_IDENTITY"
issuer="https://token.actions.githubusercontent.com"
work_directory="$(mktemp -d "${RUNNER_TEMP:-/tmp}/client-release.XXXXXX")"
trap 'rm -rf "$work_directory"' EXIT

verify_bundle() {
  local subject="$1" bundle="$2" allowed_identity
  local -a allowed_identities=("$tag_identity")
  if [[ "$identity" != "$tag_identity" ]]; then
    allowed_identities+=("$identity")
  fi
  for allowed_identity in "${allowed_identities[@]}"; do
    if cosign verify-blob "$subject" \
      --bundle "$bundle" \
      --certificate-identity "$allowed_identity" \
      --certificate-oidc-issuer "$issuer" >/dev/null 2>&1; then
      return 0
    fi
  done
  echo "Signature bundle is not bound to the managed tag or recovery workflow: $(basename "$bundle")" >&2
  return 1
}

verify_asset_directory() {
  local directory="$1" subject bundle artifact_signing fingerprint
  (cd "$directory" && sha256sum --check SHA256SUMS)
  jq -e --arg commit "$RELEASE_SHA" --arg version "$VERSION" '
    .schemaVersion == 2 and
    .product == "Camellia Nexus" and
    .version == $version and
    .commit == $commit and
    (.builds | length) == 4 and
    (.builds | map(.platform + "-" + .architecture) | unique | length) == 4 and
    all(.builds[];
      .schemaVersion == 2 and
      .product == "Camellia Nexus" and
      .version == $version and
      .buildId == $version and
      .commit == $commit and
      (
        (.platform == "linux" and .architecture == "x64" and .nativeSigning == "not-applicable" and
          ((.artifactSigning == {scheme:"none"}) or
           ((.artifactSigning | keys | sort) == ["fingerprint", "scheme"] and
            .artifactSigning.scheme == "openpgp-detached" and
            (.artifactSigning.fingerprint | test("^[0-9A-F]{40}$|^[0-9A-F]{64}$"))))) or
        (.platform == "windows" and .architecture == "x64" and (.nativeSigning == "unsigned" or .nativeSigning == "signed") and .artifactSigning == {scheme:"none"}) or
        (.platform == "macos" and (.architecture == "x64" or .architecture == "arm64") and
          (.nativeSigning == "unsigned" or .nativeSigning == "ad-hoc" or .nativeSigning == "signed" or .nativeSigning == "notarized") and
          .artifactSigning == {scheme:"none"})
      )
    )
  ' "$directory/RELEASE-METADATA.json" >/dev/null || {
    echo "Release build metadata is invalid" >&2
    return 1
  }
  artifact_signing="$(jq -r '.builds[] | select(.platform == "linux") | .artifactSigning.scheme' "$directory/RELEASE-METADATA.json")"
  if [[ "$artifact_signing" == openpgp-detached ]]; then
    fingerprint="$(jq -r '.builds[] | select(.platform == "linux") | .artifactSigning.fingerprint' "$directory/RELEASE-METADATA.json")"
    LINUX_GPG_FINGERPRINT="$fingerprint" \
    LINUX_GPG_PUBLIC_KEY="$directory/camellia-nexus-$VERSION-linux-x64.signing-key.asc" \
      bash "$script_directory/linux-openpgp-verify.sh" \
        "$directory/camellia-nexus-$VERSION-linux-x64.AppImage" \
        "$directory/camellia-nexus-$VERSION-linux-x64.deb" \
        "$directory/camellia-nexus-$VERSION-linux-x64.tar.gz" >/dev/null
  fi
  while IFS= read -r -d '' bundle; do
    subject="${bundle%.sigstore.json}"
    [[ -f "$subject" ]] || { echo "Signature bundle has no subject: $(basename "$bundle")" >&2; return 1; }
    verify_bundle "$subject" "$bundle"
  done < <(find "$directory" -maxdepth 1 -type f -name '*.sigstore.json' -print0 | sort -z)
}

release_json="$work_directory/release.json"
assets_json="$work_directory/assets.json"
expected_names="$work_directory/expected-assets"
expected_raw_names="$work_directory/expected-raw-assets"
remote_names="$work_directory/remote-assets"
configure_expected_assets() {
  local metadata="$1" artifact_signing
  cat > "$expected_raw_names" <<EOF
camellia-nexus-$VERSION-linux-x64.AppImage
camellia-nexus-$VERSION-linux-x64.deb
camellia-nexus-$VERSION-linux-x64.tar.gz
camellia-nexus-$VERSION-macos-arm64.dmg
camellia-nexus-$VERSION-macos-arm64.tar.gz
camellia-nexus-$VERSION-macos-x64.dmg
camellia-nexus-$VERSION-macos-x64.tar.gz
camellia-nexus-$VERSION-windows-x64.msi
camellia-nexus-$VERSION-windows-x64-portable.zip
RELEASE-METADATA.json
SHA256SUMS
EOF
  artifact_signing="$(jq -r '.builds[] | select(.platform == "linux") | .artifactSigning.scheme // empty' "$metadata")"
  if [[ "$artifact_signing" == openpgp-detached ]]; then
    cat >> "$expected_raw_names" <<EOF
camellia-nexus-$VERSION-linux-x64.AppImage.asc
camellia-nexus-$VERSION-linux-x64.deb.asc
camellia-nexus-$VERSION-linux-x64.signing-key.asc
camellia-nexus-$VERSION-linux-x64.tar.gz.asc
EOF
  elif [[ "$artifact_signing" != none ]]; then
    echo "Unsupported Linux artifact signing mode: $artifact_signing" >&2
    return 1
  fi
  LC_ALL=C sort -o "$expected_raw_names" "$expected_raw_names"
  {
    cat "$expected_raw_names"
    sed 's/$/.sigstore.json/' "$expected_raw_names"
  } | LC_ALL=C sort > "$expected_names"
}

refresh_release() {
  local body draft immutable
  GH_TOKEN="$RELEASE_POLICY_TOKEN" gh api --paginate --slurp \
    "repos/$GITHUB_REPOSITORY/releases?per_page=100" |
    jq -ce --arg tag "$tag" --argjson id "$RELEASE_ID" '
      [.[][] | select(.tag_name == $tag)] as $matches |
      if ($matches | length) == 1 and $matches[0].id == $id then $matches[0]
      elif ($matches | length) == 0 then error("managed release not found")
      elif ($matches | length) > 1 then error("multiple releases use the same tag")
      else error("managed release identity changed") end
    ' > "$release_json"
  [[ "$(jq -r '.id // empty' "$release_json")" == "$RELEASE_ID" ]] || { echo "Release identity changed for $tag" >&2; return 1; }
  [[ "$(jq -r '.tag_name // empty' "$release_json")" == "$tag" ]] || { echo "Release tag metadata changed" >&2; return 1; }
  [[ "$(jq -r '.target_commitish // empty' "$release_json")" == "$RELEASE_SHA" ]] || { echo "Release target changed" >&2; return 1; }
  [[ "$(jq -r '.name // empty' "$release_json")" == "Camellia Nexus $VERSION" ]] || { echo "Release title changed" >&2; return 1; }
  [[ "$(jq -r '.author.login // empty' "$release_json")" == "$RELEASE_APP_LOGIN" ]] || { echo "Release author changed" >&2; return 1; }
  draft="$(jq -r '.draft | if . == true then "true" elif . == false then "false" else empty end' "$release_json")"
  immutable="$(jq -r '.immutable | if . == true then "true" elif . == false then "false" else empty end' "$release_json")"
  [[ -n "$draft" && -n "$immutable" ]] || { echo "Release state metadata is invalid" >&2; return 1; }
  [[ "$draft" == true || "$immutable" == true ]] || { echo "Published Release $tag is not immutable" >&2; return 1; }
  body="$(jq -r '.body // ""' "$release_json")"
  [[ "$(grep -Fxc "<!-- release-commit:$RELEASE_SHA -->" <<< "$body" || true)" == 1 ]] || { echo "Managed commit marker changed" >&2; return 1; }
  [[ "$(grep -Fxc "<!-- release-pr:$RELEASE_PR_NUMBER -->" <<< "$body" || true)" == 1 ]] || { echo "Managed PR marker changed" >&2; return 1; }
  jq -ce '.assets | if type == "array" then . else error("release assets are unavailable") end' \
    "$release_json" > "$assets_json"
  jq -r '.[].name' "$assets_json" | LC_ALL=C sort > "$remote_names"
  [[ "$(jq -r '.[].name' "$assets_json" | LC_ALL=C sort | uniq -d)" == "" ]] || {
    echo "Release contains duplicate asset names" >&2
    return 1
  }
}

download_asset() {
  local name="$1" destination="$2" asset_id
  asset_id="$(jq -r --arg name "$name" '.[] | select(.name == $name) | .id' "$assets_json")"
  [[ "$asset_id" =~ ^[1-9][0-9]*$ ]] || { echo "Unable to resolve remote asset $name" >&2; return 1; }
  GH_TOKEN="$RELEASE_POLICY_TOKEN" gh api -H 'Accept: application/octet-stream' \
    "repos/$GITHUB_REPOSITORY/releases/assets/$asset_id" > "$destination"
}

delete_asset() {
  local name="$1" asset_id
  asset_id="$(jq -r --arg name "$name" '.[] | select(.name == $name) | .id' "$assets_json")"
  [[ "$asset_id" =~ ^[1-9][0-9]*$ ]] || { echo "Unable to resolve draft asset $name" >&2; return 1; }
  GH_TOKEN="$RELEASE_POLICY_TOKEN" gh api -X DELETE \
    "repos/$GITHUB_REPOSITORY/releases/assets/$asset_id" >/dev/null
}

refresh_release
if [[ "$verify_published_only" == true || "$(jq -r '.draft' "$release_json")" == false ]]; then
  metadata_bootstrap="$work_directory/metadata-bootstrap.json"
  metadata_bundle="$metadata_bootstrap.sigstore.json"
  download_asset RELEASE-METADATA.json "$metadata_bootstrap"
  download_asset RELEASE-METADATA.json.sigstore.json "$metadata_bundle"
  verify_bundle "$metadata_bootstrap" "$metadata_bundle"
  configure_expected_assets "$metadata_bootstrap"
else
  configure_expected_assets "$assets_directory/RELEASE-METADATA.json"
fi
verify_remote_release() {
  local destination="$1" name
  diff -u "$expected_names" "$remote_names"
  mkdir "$destination"
  while IFS= read -r name; do
    download_asset "$name" "$destination/$name"
  done < "$expected_names"
  verify_asset_directory "$destination"
}

if [[ "$verify_published_only" == true || "$(jq -r '.draft' "$release_json")" == false ]]; then
  [[ "$(jq -r '.draft' "$release_json")" == false ]] || {
    echo "Verification-only mode requires an already published release" >&2
    exit 1
  }
  verify_remote_release "$work_directory/published-existing"
  echo "Verified existing published $tag by API, checksum and Sigstore readback"
  exit 0
fi
[[ "$verify_published_only" == false ]] || { echo "VERIFY_PUBLISHED_ONLY must be true or false" >&2; exit 1; }

find "$assets_directory" -maxdepth 1 -type f ! -name '*.sigstore.json' -printf '%f\n' | LC_ALL=C sort > "$work_directory/local-raw-assets"
diff -u "$expected_raw_names" "$work_directory/local-raw-assets"
mapfile -d '' subjects < <(find "$assets_directory" -maxdepth 1 -type f ! -name '*.sigstore.json' -print0 | sort -z)
for subject in "${subjects[@]}"; do
  cosign sign-blob --yes --bundle "$subject.sigstore.json" "$subject" >/dev/null
  verify_bundle "$subject" "$subject.sigstore.json"
done
find "$assets_directory" -maxdepth 1 -type f -printf '%f\n' | LC_ALL=C sort > "$work_directory/local-assets"
diff -u "$expected_names" "$work_directory/local-assets"

refresh_release
if [[ "$(jq -r '.draft' "$release_json")" == false ]]; then
  verify_remote_release "$work_directory/published-during-signing"
  echo "Release $tag was published concurrently and passed complete readback"
  exit 0
fi
LC_ALL=C comm -13 "$expected_names" "$remote_names" > "$work_directory/unexpected-assets"
if [[ -s "$work_directory/unexpected-assets" ]]; then
  echo "Release contains unexpected assets:" >&2
  sed 's/^/  /' "$work_directory/unexpected-assets" >&2
  exit 1
fi

draft="$(jq -r '.draft' "$release_json")"
while IFS= read -r name; do
  local_path="$assets_directory/$name"
  if grep -Fxq "$name" "$remote_names"; then
    remote_path="$work_directory/existing-$name"
    download_asset "$name" "$remote_path"
    if [[ "$name" == *.sigstore.json ]]; then
      if ! verify_bundle "$assets_directory/${name%.sigstore.json}" "$remote_path"; then
        echo "Replacing invalid draft signature bundle: $name"
        delete_asset "$name"
        GH_TOKEN="$RELEASE_POLICY_TOKEN" gh release upload "$tag" "$local_path" --repo "$GITHUB_REPOSITORY"
      fi
    elif ! cmp -s "$local_path" "$remote_path"; then
      echo "Replacing conflicting draft asset: $name"
      delete_asset "$name"
      GH_TOKEN="$RELEASE_POLICY_TOKEN" gh release upload "$tag" "$local_path" --repo "$GITHUB_REPOSITORY"
    fi
  elif [[ "$draft" == true ]]; then
    GH_TOKEN="$RELEASE_POLICY_TOKEN" gh release upload "$tag" "$local_path" --repo "$GITHUB_REPOSITORY"
  else
    echo "Published release is missing asset: $name" >&2
    exit 1
  fi
done < "$expected_names"

refresh_release
diff -u "$expected_names" "$remote_names"
verified_draft="$work_directory/verified-draft"
verify_remote_release "$verified_draft"

# Asset signing and upload can take long enough for managed state to change.
# Repeat the complete PR/tag/Release authorization at the publication boundary.
EXPECTED_VERSION="$VERSION" bash "$script_directory/manage-release.sh" validate-publish >/dev/null
refresh_release
if [[ "$(jq -r '.draft' "$release_json")" == true ]]; then
  GH_TOKEN="$RELEASE_POLICY_TOKEN" gh release edit "$tag" --repo "$GITHUB_REPOSITORY" --draft=false --latest
fi

# Re-read the public state and bytes after the publication mutation.
refresh_release
[[ "$(jq -r '.draft' "$release_json")" == false ]] || { echo "Release $tag remained a draft" >&2; exit 1; }
diff -u "$expected_names" "$remote_names"
published="$work_directory/published"
verify_remote_release "$published"
echo "Published and independently re-read $tag with verified checksums and Sigstore bundles"

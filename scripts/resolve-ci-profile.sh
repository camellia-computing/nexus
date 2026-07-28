#!/usr/bin/env bash
set -euo pipefail

event_name=
release_path=
diff_resolved=

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --event)
      [[ "$#" -ge 2 ]] || { echo 'Missing value for --event' >&2; exit 2; }
      event_name="$2"
      shift 2
      ;;
    --release-path)
      [[ "$#" -ge 2 ]] || { echo 'Missing value for --release-path' >&2; exit 2; }
      release_path="$2"
      shift 2
      ;;
    --diff-resolved)
      [[ "$#" -ge 2 ]] || { echo 'Missing value for --diff-resolved' >&2; exit 2; }
      diff_resolved="$2"
      shift 2
      ;;
    *) echo "Unknown argument: $1" >&2; exit 2 ;;
  esac
done

[[ -n "$event_name" ]] || { echo '--event is required' >&2; exit 2; }
case "$diff_resolved" in
  true|false) ;;
  *) echo 'Diff resolution must be true or false' >&2; exit 2 ;;
esac
case "$release_path" in
  open|merged)
    printf '%s\n' release
    exit 0
    ;;
  none) ;;
  *) echo 'Release path must be none, open, or merged' >&2; exit 2 ;;
esac

if [[ "$event_name" == workflow_dispatch ]]; then
  printf '%s\n' candidate
  exit 0
fi
if [[ "$event_name" != pull_request && "$event_name" != push ]]; then
  printf '%s\n' candidate
  exit 0
fi
if [[ "$diff_resolved" != true ]]; then
  printf '%s\n' candidate
  exit 0
fi

profile=standard
seen=0

while IFS= read -r -d '' path; do
  [[ -n "$path" ]] || continue
  seen=1
  case "$path" in
    .github/dependabot.yml|.github/workflows/*|.github/actions/*)
      profile=candidate
      ;;
    .node-version|rust-toolchain.toml|Cargo.toml|Cargo.lock|*/Cargo.toml)
      profile=candidate
      ;;
    ui/package.json|ui/pnpm-lock.yaml)
      profile=candidate
      ;;
    src-tauri/build.rs|src-tauri/tauri*.conf.json|src-tauri/windows-app-manifest.xml|src-tauri/icons/*)
      profile=candidate
      ;;
    scripts/audit-release-security.mjs|scripts/check-version-policy.sh|scripts/ci-local.sh|scripts/ci-local.ps1|scripts/prepare-privilege-broker.mjs|scripts/windows-authenticode.ps1)
      profile=candidate
      ;;
    scripts/manage-release.sh|scripts/publish-client-release.sh|scripts/report-toolchains.sh)
      profile=candidate
      ;;
    scripts/resolve-ci-profile.sh|scripts/test-ci-local.sh|scripts/test-resolve-ci-profile.sh|scripts/test-update-toolchains.sh|scripts/test-windows-authenticode.ps1|scripts/update-toolchains.sh)
      profile=candidate
      ;;
    scripts/stage-unix-release.sh|scripts/stage-windows-release.ps1)
      profile=candidate
      ;;
  esac
done

if [[ "$seen" == 0 ]]; then
  profile=candidate
fi

printf '%s\n' "$profile"

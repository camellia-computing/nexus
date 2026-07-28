#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
resolver="$repository_root/scripts/resolve-ci-profile.sh"

fail() {
  echo "CI profile test: $*" >&2
  exit 1
}

assert_profile() {
  local expected="$1"
  local label="$2"
  local event_name="$3"
  local release_path="$4"
  local diff_resolved="$5"
  shift 5

  local actual
  if [[ "$#" == 0 ]]; then
    actual="$(bash "$resolver" \
      --event "$event_name" \
      --release-path "$release_path" \
      --diff-resolved "$diff_resolved" </dev/null)"
  else
    actual="$(printf '%s\0' "$@" | bash "$resolver" \
      --event "$event_name" \
      --release-path "$release_path" \
      --diff-resolved "$diff_resolved")"
  fi
  [[ "$actual" == "$expected" ]] ||
    fail "$label resolved to $actual instead of $expected"
}

assert_profile standard 'ordinary source pull request' pull_request none true src-tauri/src/lib.rs
assert_profile standard 'ordinary source main push' push none true src-tauri/src/lib.rs
assert_profile standard 'documentation change' pull_request none true docs/dependency-management.md
assert_profile candidate 'Cargo lock change' pull_request none true Cargo.lock
assert_profile candidate 'workspace manifest change' pull_request none true crates/camellia-nexus-core/Cargo.toml
assert_profile candidate 'frontend lock change' pull_request none true ui/pnpm-lock.yaml
assert_profile candidate 'toolchain change' push none true .node-version
assert_profile candidate 'workflow change' pull_request none true .github/workflows/ci.yml
assert_profile candidate 'Dependabot policy change' pull_request none true .github/dependabot.yml
assert_profile candidate 'desktop package asset change' pull_request none true src-tauri/icons/icon.ico
assert_profile candidate 'privilege broker sidecar config change' pull_request none true src-tauri/tauri.privilege-broker.conf.json
assert_profile candidate 'package script change' pull_request none true scripts/ci-local.ps1
assert_profile candidate 'privilege broker preparation change' pull_request none true scripts/prepare-privilege-broker.mjs
assert_profile candidate 'Authenticode implementation change' pull_request none true scripts/windows-authenticode.ps1
assert_profile candidate 'Authenticode regression change' pull_request none true scripts/test-windows-authenticode.ps1
assert_profile candidate 'package retry test change' pull_request none true scripts/test-ci-local.sh
assert_profile candidate 'profile resolver change' pull_request none true scripts/resolve-ci-profile.sh
assert_profile candidate 'mixed change set' push none true docs/dependency-management.md Cargo.lock
assert_profile candidate 'unresolved empty change set' pull_request none true
assert_profile candidate 'failed diff resolution' push none false docs/dependency-management.md
assert_profile candidate 'manual candidate request' workflow_dispatch none false docs/dependency-management.md
assert_profile candidate 'unknown caller event' workflow_call none false docs/dependency-management.md
assert_profile release 'open release proposal' pull_request open false Cargo.lock
assert_profile release 'merged release proposal' push merged false Cargo.lock

if bash "$resolver" --event pull_request --release-path invalid --diff-resolved true </dev/null 2>/dev/null; then
  fail 'invalid release path was accepted'
fi
if bash "$resolver" --event pull_request --release-path none --diff-resolved invalid </dev/null 2>/dev/null; then
  fail 'invalid diff resolution was accepted'
fi

printf '%s\n' 'CI profile tests passed.'

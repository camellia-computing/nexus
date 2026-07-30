#!/usr/bin/env bash
set -euo pipefail

root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT
counter="$root/counter"
arguments_log="$root/arguments"
export CARGO_TARGET_DIR="$root/target"
export MOCK_ARGUMENTS_LOG="$arguments_log" MOCK_COUNTER="$counter"

fail() {
  echo "Local CI retry test: $*" >&2
  exit 1
}

uname() {
  printf '%s\n' Darwin
}

pnpm() {
  printf 'pnpm:%s\n' "$*" >> "$MOCK_ARGUMENTS_LOG"
  return 0
}

sleep() {
  [[ "$1" == 5 ]] || fail "unexpected retry delay: $1"
}

node() {
  if [[ "$1" == "scripts/prepare-privilege-broker.mjs" ]]; then
    [[ -n "${TAURI_ENV_TARGET_TRIPLE:-}" ]] ||
      fail 'privilege broker target was not provided'
    [[ "${TAURI_ENV_DEBUG:-}" == "false" ]] ||
      fail 'desktop package did not prepare a release-mode privilege broker'
    return 0
  fi
  local count
  count="$(cat "$MOCK_COUNTER")"
  printf '%s\n' "$((count + 1))" > "$MOCK_COUNTER"
  printf '%s\n' "$*" >> "$MOCK_ARGUMENTS_LOG"
  case "$MOCK_MODE" in
    dmg-retry)
      [[ " $* " == *" --verbose "* ]] || return 9
      if [[ "$count" == 0 ]]; then
        mkdir -p "$CARGO_TARGET_DIR/release/bundle/macos"
        touch "$CARGO_TARGET_DIR/release/bundle/macos/rw.incomplete.dmg"
        echo 'Error failed to bundle project: error running bundle_dmg.sh'
        return 1
      fi
      [[ ! -e "$CARGO_TARGET_DIR/release/bundle/macos/rw.incomplete.dmg" ]] ||
        fail 'incomplete DMG output survived the retry cleanup'
      ;;
    deterministic)
      echo 'deterministic compiler failure' >&2
      return 7
      ;;
    no-dmg)
      [[ " $* " != *" --verbose "* ]] || return 9
      ;;
    *) return 10 ;;
  esac
}

export -f fail node pnpm sleep uname

printf '0\n' > "$counter"
: > "$arguments_log"
export MOCK_MODE=dmg-retry
output="$(CAMELLIA_NEXUS_TAURI_BUNDLES=app,dmg \
  bash scripts/ci-local.sh --desktop-package --skip-quality 2>&1)"
[[ "$(cat "$counter")" == 2 ]] || fail 'DMG failure did not retry exactly once'
grep -Fq 'Tauri DMG creation failed; retrying once' <<< "$output" ||
  fail 'DMG retry was not reported'

printf '0\n' > "$counter"
: > "$arguments_log"
export MOCK_MODE=deterministic
set +e
CAMELLIA_NEXUS_TAURI_BUNDLES=app,dmg \
  bash scripts/ci-local.sh --desktop-package --skip-quality >/dev/null 2>&1
status=$?
set -e
[[ "$status" == 7 ]] || fail "deterministic failure returned $status instead of 7"
[[ "$(cat "$counter")" == 1 ]] || fail 'non-DMG failure was retried'

printf '0\n' > "$counter"
: > "$arguments_log"
export MOCK_MODE=no-dmg
(
  unset CAMELLIA_NEXUS_TAURI_BUNDLES
  bash scripts/ci-local.sh --desktop-package --skip-quality >/dev/null
)
[[ "$(cat "$counter")" == 1 ]] || fail 'package without a DMG used the retry path'
grep -Fxq 'pnpm:--dir ui peers check' "$arguments_log" ||
  fail 'package validation did not reject frontend peer dependency drift'

echo 'Local CI retry tests passed.'

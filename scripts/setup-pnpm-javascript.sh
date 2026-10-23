#!/usr/bin/env bash
set -euo pipefail

: "${RUNNER_TEMP:?RUNNER_TEMP is required}"
: "${GITHUB_PATH:?GITHUB_PATH is required}"

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
package_manager="$(node -p 'require(process.argv[1]).packageManager' "$repository_root/ui/package.json")"
[[ "$package_manager" =~ ^pnpm@([0-9]+\.[0-9]+\.[0-9]+)$ ]] || {
  echo 'packageManager must declare an exact stable pnpm release' >&2
  exit 1
}
pnpm_version="${BASH_REMATCH[1]}"
setup_dir="$(mktemp -d "$RUNNER_TEMP/camellia-pnpm-js.XXXXXX")"

(
  cd "$setup_dir"
  npm install --ignore-scripts --no-fund --no-audit \
    --registry=https://registry.npmjs.org "$package_manager"
  npm audit signatures --registry=https://registry.npmjs.org
  [[ "$(node -p 'require("./node_modules/pnpm/package.json").version')" == "$pnpm_version" ]]
  [[ "$(node node_modules/pnpm/bin/pnpm.mjs --version)" == "$pnpm_version" ]]
)

printf '%s\n' "$setup_dir/node_modules/.bin" >> "$GITHUB_PATH"

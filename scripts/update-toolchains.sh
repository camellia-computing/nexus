#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/update-toolchains.sh [options]

Update one or more coupled toolchain versions as a validated transaction:
  --node <x.y.z>   Update .node-version and engines.node
  --node-types <x.y.z> Update @types/node with a reviewed Node.js major change
  --pnpm <x.y.z>    Update ui/package.json packageManager
  --rust <x.y.z>    Update rust-toolchain.toml and Cargo.toml rust-version
  -h, --help        Show this help
EOF
}

fail() {
  echo "toolchain update: $*" >&2
  exit 1
}

node_version=
node_types=
pnpm_version=
rust_version=
while (($# > 0)); do
  case "$1" in
    --node|--node-types|--pnpm|--rust)
      (($# >= 2)) || fail "$1 requires a value"
      case "$1" in
        --node) node_version="$2" ;;
        --node-types) node_types="$2" ;;
        --pnpm) pnpm_version="$2" ;;
        --rust) rust_version="$2" ;;
      esac
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *) fail "unknown option: $1" ;;
  esac
done

[[ -n "$node_version$pnpm_version$rust_version" ]] || fail 'at least one version option is required'
semver='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
if [[ -n "$node_version" ]]; then
  [[ "$node_version" =~ $semver ]] || fail 'Node.js must be exact stable SemVer'
fi
[[ -z "$node_types" || -n "$node_version" ]] || fail '--node-types requires --node'
[[ -z "$node_types" || "$node_types" =~ $semver ]] || fail 'Node.js types must be exact stable SemVer'
[[ -z "$pnpm_version" || "$pnpm_version" =~ $semver ]] || fail 'pnpm must be exact stable SemVer'
[[ -z "$rust_version" || "$rust_version" =~ $semver ]] || fail 'Rust must be exact stable SemVer'

for command in cp git grep jq mktemp sed; do
  command -v "$command" >/dev/null || fail "required command is unavailable: $command"
done

repository_root="$(git rev-parse --show-toplevel 2>/dev/null)" || fail 'run this command from a Git worktree'
cd "$(dirname "${BASH_SOURCE[0]}")/.."
[[ "$PWD" == "$repository_root" ]] || fail 'script location and Git worktree root do not match'

if [[ -n "$node_version" ]]; then
  node_types="${node_types:-$(jq -r '.devDependencies["@types/node"]' ui/package.json)}"
  [[ "${node_version%%.*}" == "${node_types%%.*}" ]] || fail 'supply matching --node-types for a Node.js major change'
fi

targets=()
[[ -z "$node_version" ]] || targets+=(.node-version ui/package.json)
[[ -z "$pnpm_version" || -n "$node_version" ]] || targets+=(ui/package.json)
[[ -z "$rust_version" ]] || targets+=(rust-toolchain.toml Cargo.toml)
for file in "${targets[@]}"; do
  [[ -f "$file" ]] || fail "target file is missing: $file"
  [[ -z "$(git status --porcelain=v1 -- "$file")" ]] || fail "target file has uncommitted changes: $file"
done

backup_dir="$(mktemp -d "${TMPDIR:-/tmp}/camellia-toolchains.XXXXXX")"
completed=false
cleanup() {
  local status=$?
  trap - EXIT
  if [[ "$completed" != true ]]; then
    for file in "${targets[@]}"; do
      cp -p "$backup_dir/original/$file" "$file"
    done
    echo 'toolchain update: validation failed; original files restored' >&2
  fi
  rm -rf "$backup_dir"
  exit "$status"
}
trap cleanup EXIT

for file in "${targets[@]}"; do
  mkdir -p "$backup_dir/original/$(dirname "$file")"
  cp -p "$file" "$backup_dir/original/$file"
done

replace_line() {
  local file="$1" pattern="$2" replacement="$3" rendered
  [[ "$(grep -Ec "$pattern" "$file")" == 1 ]] || fail "expected exactly one matching line in $file"
  rendered="$(mktemp "$backup_dir/rendered.XXXXXX")"
  sed -E "s|$pattern|$replacement|" "$file" > "$rendered"
  cp "$rendered" "$file"
}

if [[ -n "$node_version" ]]; then
  printf '%s\n' "$node_version" > .node-version
fi
if [[ -n "$pnpm_version$node_version" ]]; then
  rendered="$(mktemp "$backup_dir/package.XXXXXX")"
  jq --arg pnpm "$pnpm_version" --arg node "$node_version" --arg types "$node_types" '
    if $pnpm != "" then .packageManager = "pnpm@\($pnpm)" else . end
    | if $node != "" then
        ($node | split(".")[0] | tonumber) as $major
        | .engines.node = ">=\($major) <\($major + 1)"
        | .devDependencies["@types/node"] = $types
      else . end
  ' ui/package.json > "$rendered"
  cp "$rendered" ui/package.json
fi
if [[ -n "$rust_version" ]]; then
  replace_line rust-toolchain.toml '^channel = "[^"]+"$' "channel = \"$rust_version\""
  replace_line Cargo.toml '^rust-version = "[^"]+"$' "rust-version = \"${rust_version%.*}\""
fi

bash scripts/check-version-policy.sh
completed=true
echo 'Toolchain sources updated. Review the diff and run the complete quality gates before committing.'

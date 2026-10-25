#!/usr/bin/env bash
set -euo pipefail

fail() {
  echo "toolchain updater test: $*" >&2
  exit 1
}

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fixture="$(mktemp -d "${TMPDIR:-/tmp}/camellia-toolchains-test.XXXXXX")"
trap 'rm -rf "$fixture"' EXIT

mkdir -p "$fixture/ui/patches" "$fixture/scripts" "$fixture/docs" "$fixture/.github"
cp "$repository_root/.node-version" "$repository_root/Cargo.toml" "$repository_root/rust-toolchain.toml" "$fixture/"
cp \
  "$repository_root/ui/package.json" \
  "$repository_root/ui/pnpm-lock.yaml" \
  "$repository_root/ui/pnpm-workspace.yaml" \
  "$fixture/ui/"
cp "$repository_root/ui/patches/brace-expansion@5.0.12.patch" "$fixture/ui/patches/"
cp \
  "$repository_root/scripts/check-version-policy.sh" \
  "$repository_root/scripts/ci-local.sh" \
  "$repository_root/scripts/update-toolchains.sh" \
  "$fixture/scripts/"
cp "$repository_root/docs/dependency-management.md" "$fixture/docs/"
cp -R "$repository_root/.github/workflows" "$fixture/.github/"
cp "$repository_root/.github/dependabot.yml" "$fixture/.github/"
chmod +x "$fixture/scripts/update-toolchains.sh"

git -C "$fixture" init -q
git -C "$fixture" add .
git -C "$fixture" -c user.name=test -c user.email=test@example.invalid -c commit.gpgsign=false commit -q --no-gpg-sign -m fixture

(
  cd "$fixture"
  bash scripts/update-toolchains.sh --node 24.99.1 --pnpm 11.99.1 --rust 1.99.1 >/dev/null
)
[[ "$(tr -d '[:space:]' < "$fixture/.node-version")" == 24.99.1 ]] || fail 'Node.js source was not updated'
[[ "$(jq -r .packageManager "$fixture/ui/package.json")" == pnpm@11.99.1 ]] || fail 'pnpm source was not updated'
grep -Fxq 'channel = "1.99.1"' "$fixture/rust-toolchain.toml" || fail 'Rust toolchain source was not updated'
grep -Fxq 'rust-version = "1.99"' "$fixture/Cargo.toml" || fail 'Cargo rust-version was not updated'

git -C "$fixture" reset -q --hard HEAD
before="$(git -C "$fixture" status --porcelain=v1)"
if (cd "$fixture" && bash scripts/update-toolchains.sh --node 26.0.0 >/dev/null 2>&1); then
  fail 'a Node.js major change without matching types was accepted'
fi
[[ "$(git -C "$fixture" status --porcelain=v1)" == "$before" ]] || fail 'invalid input changed the worktree'

(
  cd "$fixture"
  bash scripts/update-toolchains.sh --node 26.0.0 --node-types 26.6.5 >/dev/null
)
[[ "$(jq -r '.engines.node' "$fixture/ui/package.json")" == '>=26 <27' ]] || fail 'Node.js engines did not follow the configured major'
[[ "$(jq -r '.devDependencies["@types/node"]' "$fixture/ui/package.json")" == 26.6.5 ]] || fail 'Node.js types were not updated'

git -C "$fixture" reset -q --hard HEAD
sed -i 's/"@types\/node": "24\.[^"]*"/"@types\/node": "26.6.5"/' "$fixture/ui/package.json"
if (cd "$fixture" && bash scripts/check-version-policy.sh >/dev/null 2>&1); then
  fail 'Node.js types newer than the runtime were accepted'
fi

git -C "$fixture" reset -q --hard HEAD
# Match the updater's literal jq variable.
# shellcheck disable=SC2016
sed -i '/| .devDependencies\["@types\/node"\] = \$types/a\        | .engines.node = ">=0 <1"' "$fixture/scripts/update-toolchains.sh"
if (cd "$fixture" && bash scripts/update-toolchains.sh --node 26.0.0 --node-types 26.6.5 >/dev/null 2>&1); then
  fail 'a toolchain update with failed validation was accepted'
fi
[[ "$(tr -d '[:space:]' < "$fixture/.node-version")" == "$(cat "$repository_root/.node-version")" ]] || fail 'failed update did not restore Node.js'
cmp "$repository_root/ui/package.json" "$fixture/ui/package.json" || fail 'failed update did not restore package metadata'

git -C "$fixture" reset -q --hard HEAD
sed -i '/package-ecosystem: github-actions/,/open-pull-requests-limit:/ {
  /open-pull-requests-limit:/a\    ignore:\
      - dependency-name: pnpm/action-setup\
        versions: ["~> 6.0"]
}' "$fixture/.github/dependabot.yml"
if (cd "$fixture" && bash scripts/check-version-policy.sh >/dev/null 2>&1); then
  fail 'the removed pnpm/action-setup ignore was accepted'
fi

git -C "$fixture" reset -q --hard HEAD
sed -i '/package-ecosystem: github-actions/,/open-pull-requests-limit:/ {
  /default-days: 7/a\      semver-major-days: 30
}' "$fixture/.github/dependabot.yml"
if (cd "$fixture" && bash scripts/check-version-policy.sh >/dev/null 2>&1); then
  fail 'unsupported GitHub Actions tiered cooldown was accepted'
fi

git -C "$fixture" reset -q --hard HEAD
sed -i '/serialize-javascript: 7.1.3/d' "$fixture/ui/pnpm-workspace.yaml"
if (cd "$fixture" && bash scripts/check-version-policy.sh >/dev/null 2>&1); then
  fail 'the serialize-javascript security override could be removed silently'
fi

git -C "$fixture" reset -q --hard HEAD
sed -i "/'@napi-rs\\/wasm-runtime': 1.1.6/d" "$fixture/ui/pnpm-workspace.yaml"
if (cd "$fixture" && bash scripts/check-version-policy.sh >/dev/null 2>&1); then
  fail 'the Rolldown WASI compatibility override could be removed silently'
fi

git -C "$fixture" reset -q --hard HEAD
sed -i '/uvx --from zizmor==1.31.1/d' "$fixture/.github/workflows/ci.yml"
if (cd "$fixture" && bash scripts/check-version-policy.sh >/dev/null 2>&1); then
  fail 'the blocking workflow security scan could be removed silently'
fi

git -C "$fixture" reset -q --hard HEAD
printf '\n---\npackageManager: pnpm@99.0.0\n' >> "$fixture/ui/pnpm-lock.yaml"
if (cd "$fixture" && bash scripts/check-version-policy.sh >/dev/null 2>&1); then
  fail 'a multi-document lockfile was accepted despite the dependency graph limitation'
fi

git -C "$fixture" reset -q --hard HEAD
sed -i 's/package-ecosystem: npm/package-ecosystem: unavailable/' "$fixture/.github/dependabot.yml"
if (cd "$fixture" && bash scripts/check-version-policy.sh >/dev/null 2>&1); then
  fail 'the frontend dependency updater could be removed silently'
fi

printf '%s\n' 'Toolchain updater tests passed.'

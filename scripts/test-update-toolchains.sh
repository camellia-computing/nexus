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
cp "$repository_root/ui/patches/brace-expansion@5.0.8.patch" "$fixture/ui/patches/"
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
if (cd "$fixture" && bash scripts/update-toolchains.sh --node 25.0.0 >/dev/null 2>&1); then
  fail 'unsupported Node.js major was accepted'
fi
[[ "$(git -C "$fixture" status --porcelain=v1)" == "$before" ]] || fail 'invalid input changed the worktree'

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
sed -i '/serialize-javascript: 7.0.7/d' "$fixture/ui/pnpm-workspace.yaml"
if (cd "$fixture" && bash scripts/check-version-policy.sh >/dev/null 2>&1); then
  fail 'the serialize-javascript security override could be removed silently'
fi

git -C "$fixture" reset -q --hard HEAD
sed -i "/'@napi-rs\\/wasm-runtime': 1.1.6/d" "$fixture/ui/pnpm-workspace.yaml"
if (cd "$fixture" && bash scripts/check-version-policy.sh >/dev/null 2>&1); then
  fail 'the Rolldown WASI compatibility override could be removed silently'
fi

git -C "$fixture" reset -q --hard HEAD
sed -i '/uvx --from zizmor==1.28.0/d' "$fixture/.github/workflows/ci.yml"
if (cd "$fixture" && bash scripts/check-version-policy.sh >/dev/null 2>&1); then
  fail 'the blocking workflow security scan could be removed silently'
fi

printf '%s\n' 'Toolchain updater tests passed.'

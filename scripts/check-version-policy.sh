#!/usr/bin/env bash
# Workflow expressions and shell fragments are intentionally matched literally.
# shellcheck disable=SC2016
set -euo pipefail

fail() {
  echo "version policy: $*" >&2
  exit 1
}

for command in awk grep jq sed tr; do
  command -v "$command" >/dev/null || fail "required command is unavailable: $command"
done

node_version="$(tr -d '[:space:]' < .node-version)"
[[ "$node_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail '.node-version must contain exact stable SemVer'
[[ "${node_version%%.*}" == 24 ]] || fail 'Node.js must remain on the supported 24.x major'
[[ "$(jq -r '.engines.node // empty' ui/package.json)" == '>=24 <25' ]] || fail 'ui engines.node must express the supported major window'
jq -e '.engines.pnpm == null' ui/package.json >/dev/null || fail 'pnpm must not be repeated in engines'
package_manager="$(jq -r '.packageManager // empty' ui/package.json)"
[[ "$package_manager" =~ ^pnpm@[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail 'packageManager must pin an exact pnpm version'
jq -e '
  [(.dependencies // {}), (.devDependencies // {})]
  | add
  | to_entries
  | all(.value | test("^[0-9]+\\.[0-9]+\\.[0-9]+(?:-[0-9A-Za-z.-]+)?$"))
' ui/package.json >/dev/null || fail 'direct JavaScript dependencies must use exact versions'
jq -e '
  .devDependencies["@wdio/native-utils"] == "2.5.0" and
  .devDependencies["serialize-javascript"] == "7.0.7"
' ui/package.json >/dev/null || fail 'reviewed test compatibility and security overrides must be direct dependencies'
grep -Fxq "  '@wdio/native-utils': 2.5.0" ui/pnpm-workspace.yaml ||
  fail 'the Tauri WebdriverIO graph must override native-utils from the pnpm 11 workspace settings'
grep -Fxq "  '@napi-rs/wasm-runtime': 1.1.6" ui/pnpm-workspace.yaml ||
  fail 'the Rolldown WASI fallback must use the stable emnapi-compatible runtime'
grep -Fxq '  serialize-javascript: 7.0.7' ui/pnpm-workspace.yaml ||
  fail 'the WebdriverIO Mocha graph must override serialize-javascript to its reviewed patched release'
grep -Fxq '  brace-expansion: 5.0.8' ui/pnpm-workspace.yaml ||
  fail 'the WebdriverIO graph must use the reviewed current brace-expansion release'
grep -Fxq '  brace-expansion@5.0.8: patches/brace-expansion@5.0.8.patch' ui/pnpm-workspace.yaml ||
  fail 'the reviewed brace-expansion compatibility patch is missing from the workspace policy'
[[ -f ui/patches/brace-expansion@5.0.8.patch ]] ||
  fail 'the reviewed brace-expansion compatibility patch file is missing'
grep -Eq '^  brace-expansion@5\.0\.8: [0-9a-f]{64}$' ui/pnpm-lock.yaml ||
  fail 'the frozen lockfile does not bind the reviewed brace-expansion patch'

rust_toolchain="$(sed -nE 's/^channel = "([^"]+)"/\1/p' rust-toolchain.toml)"
rust_version="$(sed -nE 's/^rust-version = "([^"]+)"/\1/p' Cargo.toml | head -n 1)"
[[ "$rust_toolchain" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail 'Rust toolchain must be exact stable SemVer'
[[ "${rust_toolchain%.*}" == "$rust_version" ]] || fail 'Cargo rust-version must match the toolchain major.minor'

shopt -s nullglob
workflow_files=(.github/workflows/*.yml .github/workflows/*.yaml)
((${#workflow_files[@]} > 0)) || fail 'no workflows found'

if grep -nE 'RUSTUP_TOOLCHAIN|^[[:space:]]+toolchain:|^[[:space:]]+node-version:' "${workflow_files[@]}"; then
  fail 'workflows must read Rust and Node.js versions from repository source files'
fi
if grep -h -A4 'uses: pnpm/action-setup@' "${workflow_files[@]}" | grep -qE '^[[:space:]]+version:'; then
  fail 'workflows must read pnpm from packageManager'
fi
setup_node_count="$({ grep -hEc 'uses: actions/setup-node@' "${workflow_files[@]}" || true; } | awk '{ total += $1 } END { print total + 0 }')"
node_file_count="$({ grep -hEc 'node-version-file:[[:space:]]+\.node-version' "${workflow_files[@]}" || true; } | awk '{ total += $1 } END { print total + 0 }')"
[[ "$setup_node_count" -gt 0 && "$setup_node_count" == "$node_file_count" ]] || fail 'every setup-node step must use .node-version'
pnpm_setup_count="$({ grep -hEc 'uses: pnpm/action-setup@' "${workflow_files[@]}" || true; } | awk '{ total += $1 } END { print total + 0 }')"
pnpm_file_count="$({ grep -hEc 'package_json_file:[[:space:]]+ui/package.json' "${workflow_files[@]}" || true; } | awk '{ total += $1 } END { print total + 0 }')"
[[ "$pnpm_setup_count" -gt 0 && "$pnpm_setup_count" == "$pnpm_file_count" ]] || fail 'every pnpm setup step must use ui/package.json'
checkout_count="$({ grep -hEc 'uses: actions/checkout@' "${workflow_files[@]}" || true; } | awk '{ total += $1 } END { print total + 0 }')"
nonpersistent_checkout_count="$({ grep -hEc 'persist-credentials:[[:space:]]+false' "${workflow_files[@]}" || true; } | awk '{ total += $1 } END { print total + 0 }')"
[[ "$checkout_count" -gt 0 && "$checkout_count" == "$nonpersistent_checkout_count" ]] || fail 'every checkout must disable persisted credentials'
if grep -q 'secrets:[[:space:]]+inherit' "${workflow_files[@]}"; then
  fail 'reusable workflows must receive only explicitly declared secrets'
fi

require_workflow_text() {
  local file="$1"
  local text="$2"
  local message="$3"
  grep -Fq -- "$text" "$file" || fail "$message"
}

cross_repo_action='uses: actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1'
cross_repo_workflows=(
  .github/workflows/native-e2e.yml
  .github/workflows/contract-monitor.yml
)
[[ "$({ grep -hFc "$cross_repo_action" "${cross_repo_workflows[@]}" || true; } | awk '{ total += $1 } END { print total + 0 }')" == 2 ]] ||
  fail 'each client cross-repository workflow must mint one optional read token'
for workflow in "${cross_repo_workflows[@]}"; do
  require_workflow_text "$workflow" 'APP_CLIENT_ID: ${{ vars.CROSS_REPO_READ_APP_CLIENT_ID }}' \
    "$workflow does not read the cross-repository App client ID"
  require_workflow_text "$workflow" 'APP_PRIVATE_KEY: ${{ secrets.CROSS_REPO_READ_APP_PRIVATE_KEY }}' \
    "$workflow does not read the cross-repository App private key"
  require_workflow_text "$workflow" "TRUSTED_CONTEXT: \${{ (github.event_name != 'pull_request' || github.event.pull_request.head.repo.full_name == github.repository) && github.actor != 'dependabot[bot]' }}" \
    "$workflow does not isolate fork and Dependabot pull requests from App authentication"
  require_workflow_text "$workflow" 'if: ${{ steps.sibling-auth.outputs.mode == '\''public'\'' }}' \
    "$workflow does not verify the public fallback"
  require_workflow_text "$workflow" '.private == false' \
    "$workflow does not reject a private repository in public mode"
  require_workflow_text "$workflow" "$cross_repo_action" \
    "$workflow does not use the pinned cross-repository token action"
  require_workflow_text "$workflow" 'owner: ${{ github.repository_owner }}' \
    "$workflow hard-codes the cross-repository App owner"
  require_workflow_text "$workflow" 'CURRENT_LOGICAL_ID: nexus-client' \
    "$workflow does not bind the current repository to its stable logical identity"
  require_workflow_text "$workflow" 'REPOSITORY_MAP: ${{ vars.NEXUS_REPOSITORY_MAP }}' \
    "$workflow does not read the centrally audited repository map"
  require_workflow_text "$workflow" 'SIBLING_LOGICAL_ID: nexus-management' \
    "$workflow does not select the management service by stable logical identity"
  require_workflow_text "$workflow" 'keys == ["nexus-client", "nexus-management"]' \
    "$workflow does not reject incomplete repository maps"
  require_workflow_text "$workflow" 'echo "repository=$GITHUB_REPOSITORY_OWNER/$sibling_name"' \
    "$workflow does not resolve the mapped sibling under the runtime owner"
  require_workflow_text "$workflow" 'echo "repository_name=$sibling_name"' \
    "$workflow does not expose one validated sibling name for token scoping"
  require_workflow_text "$workflow" 'repositories: ${{ steps.sibling-auth.outputs.repository_name }}' \
    "$workflow does not scope the App token to the mapped management repository"
  require_workflow_text "$workflow" 'permission-contents: read' \
    "$workflow does not request read-only sibling contents"
  require_workflow_text "$workflow" 'permission-metadata: read' \
    "$workflow does not request read-only sibling metadata"
  require_workflow_text "$workflow" 'repository: ${{ steps.sibling-auth.outputs.repository }}' \
    "$workflow does not check out the validated mapped sibling"
  require_workflow_text "$workflow" 'token: ${{ steps.sibling-token.outputs.token || github.token }}' \
    "$workflow does not select App or public checkout credentials"
  if grep -qE 'permission-[^:]+:[[:space:]]+(write|admin)' "$workflow"; then
    fail "$workflow grants write or administration permission to cross-repository access"
  fi
done
if grep -qE 'server-repository|camellia-nexus/nexus-management-server|github\.repository_owner }}/nexus-management-server|repositories:[[:space:]]+nexus-management-server' \
  .github/workflows/ci.yml .github/workflows/native-e2e.yml .github/workflows/contract-monitor.yml; then
  fail 'client cross-repository workflows must resolve siblings from the centrally audited map'
fi
require_workflow_text .github/workflows/native-e2e.yml 'CROSS_REPO_READ_APP_PRIVATE_KEY:' \
  'the native reusable workflow does not declare its optional App secret'
require_workflow_text .github/workflows/ci.yml 'CROSS_REPO_READ_APP_PRIVATE_KEY:' \
  'CI does not declare or pass the optional App secret'
require_workflow_text .github/workflows/main.yml \
  'CROSS_REPO_READ_APP_PRIVATE_KEY: ${{ secrets.CROSS_REPO_READ_APP_PRIVATE_KEY }}' \
  'Main does not explicitly pass the optional App secret to CI'
if grep -q 'RELEASE_APP_' "${cross_repo_workflows[@]}"; then
  fail 'cross-repository validation must not reuse the Release App'
fi
require_workflow_text .github/workflows/ci.yml 'uses: astral-sh/setup-uv@' \
  'CI does not install the pinned workflow security runtime'
require_workflow_text .github/workflows/ci.yml 'version: 0.12.0' \
  'CI does not select the reviewed exact uv release'
require_workflow_text .github/workflows/ci.yml \
  'run: uvx --from zizmor==1.28.0 zizmor --strict-collection --persona=pedantic --format=github .' \
  'CI does not block on the reviewed pedantic workflow security scan'
require_workflow_text .github/workflows/native-e2e.yml \
  'E2E_SUITE: ${{ github.event_name == '\''schedule'\'' && '\''full'\'' || inputs.suite || '\''smoke'\'' }}' \
  'native E2E does not isolate its selected suite from PowerShell source'
require_workflow_text .github/workflows/native-e2e.yml '-Suite "$env:E2E_SUITE"' \
  'native E2E does not pass its selected suite as PowerShell data'
if grep -Fq -- '-Suite "${{' .github/workflows/native-e2e.yml; then
  fail 'native E2E interpolates a workflow input into PowerShell source'
fi
require_workflow_text .github/workflows/publish-release.yml \
  'RELEASE_SHA: ${{ needs.metadata.outputs.sha }}' \
  'release asset aggregation does not isolate the authorized SHA from shell source'
require_workflow_text .github/workflows/publish-release.yml '--commit "$RELEASE_SHA"' \
  'release metadata does not consume the authorized SHA as shell data'
if grep -Fq -- '--commit "${{' .github/workflows/publish-release.yml; then
  fail 'release metadata interpolates a workflow output into shell source'
fi
require_workflow_text .github/workflows/client-packages.yml '- name: Stage Linux' \
  'Linux release staging is not isolated from macOS'
require_workflow_text .github/workflows/client-packages.yml '- name: Stage macOS' \
  'macOS release staging is not isolated from Linux secrets'

grep -q 'rustup toolchain install --no-self-update' "${workflow_files[@]}" || fail 'workflows must install the repository Rust toolchain'
if grep -qE '(runs-on:|-[[:space:]]+os:)[[:space:]]+(ubuntu|windows|macos)-latest' "${workflow_files[@]}"; then
  fail 'hosted runner families must be explicit'
fi
grep -q 'pnpm --dir ui install --frozen-lockfile' "${workflow_files[@]}" || fail 'CI must install the frontend from the frozen lockfile'
grep -Fq 'run_step "Check frontend peer dependency graph" pnpm --dir ui peers check' scripts/ci-local.sh ||
  fail 'the local and hosted core gate must reject frontend peer dependency drift'
grep -qE 'cargo .*(--locked.*(test|clippy)|(test|clippy).*--locked)' "${workflow_files[@]}" || fail 'CI must use locked Cargo gates'

while IFS= read -r reference; do
  case "$reference" in
    ./*) ;;
    docker://*)
      [[ "$reference" =~ ^docker://[^@[:space:]]+@sha256:[0-9a-f]{64}$ ]] || fail "container action is not digest-pinned: $reference"
      ;;
    *)
      [[ "$reference" =~ ^[^@[:space:]]+@[0-9a-f]{40}$ ]] || fail "action is not commit-pinned: $reference"
      ;;
  esac
done < <(sed -nE 's/^[[:space:]]*-?[[:space:]]*uses:[[:space:]]+([^[:space:]#]+).*/\1/p' "${workflow_files[@]}")

full_revision='rev[[:space:]]*=[[:space:]]*"[0-9a-f]{40}"'
while IFS= read -r dependency; do
  [[ "$dependency" =~ $full_revision ]] || fail "Cargo git dependency lacks a full revision: $dependency"
done < <(grep -hE 'git[[:space:]]*=' -- */Cargo.toml Cargo.toml 2>/dev/null || true)

policy=docs/dependency-management.md
[[ -f "$policy" ]] || fail 'dependency policy documentation is missing'
grep -q 'RUSTSEC-2024-0429' "$policy" || fail 'Rust advisory exception is undocumented'
grep -Fq 'cargo audit --deny yanked --ignore RUSTSEC-2024-0429' .github/workflows/ci.yml ||
  fail 'Cargo audit must deny yanked packages and ignore only the registered glib advisory'
audit_ignore_count="$(
  grep -RhoE -- '--ignore[[:space:]]+RUSTSEC-[0-9]{4}-[0-9]{4}' .github/workflows |
    wc -l |
    tr -d '[:space:]'
)"
[[ "$audit_ignore_count" == 1 ]] || fail 'exactly one registered Rust advisory ignore is permitted'
grep -q 'pnpm 11' "$policy" || fail 'pnpm 11 update exception is undocumented'
grep -q '@wdio/native-utils' "$policy" || fail 'WebdriverIO native-utils override is undocumented'
grep -q '@napi-rs/wasm-runtime' "$policy" || fail 'Rolldown WASI runtime override is undocumented'
grep -q 'GHSA-5c6j-r48x-rmvq' "$policy" || fail 'serialize-javascript security override is undocumented'
grep -q 'GHSA-mh99-v99m-4gvg' "$policy" || fail 'brace-expansion compatibility patch is undocumented'
grep -q 'TypeScript 7' "$policy" || fail 'the unsupported TypeScript major is undocumented'
grep -q 'workflow_run' "$policy" || fail 'the reviewed release merge trigger is undocumented'

dependabot=.github/dependabot.yml
[[ -f "$dependabot" ]] || fail 'Dependabot configuration is missing'
ecosystem_count="$(grep -c 'package-ecosystem:' "$dependabot")"
default_cooldown_count="$(grep -c 'default-days: 7' "$dependabot")"
[[ "$ecosystem_count" == "$default_cooldown_count" ]] || fail 'every Dependabot ecosystem must define the default cooldown'
if grep -qE 'package-ecosystem:[[:space:]]+(npm|rust-toolchain)' "$dependabot"; then
  fail 'pnpm 11 and coupled Rust toolchain updates must use scripts/update-toolchains.sh'
fi
unsupported_cooldowns="$(awk '
  /package-ecosystem:/ {
    ecosystem = $NF
    gsub(/"/, "", ecosystem)
    next
  }
  /semver-(major|minor|patch)-days:/ && ecosystem != "cargo" {
    print ecosystem ":" $1
  }
' "$dependabot")"
[[ -z "$unsupported_cooldowns" ]] ||
  fail "only Cargo supports tiered cooldowns in this policy: $unsupported_cooldowns"
awk '
  /package-ecosystem:[[:space:]]+cargo$/ { cargo = 1; next }
  /package-ecosystem:/ { cargo = 0 }
  cargo && /semver-major-days:[[:space:]]+30$/ { major++ }
  cargo && /semver-minor-days:[[:space:]]+7$/ { minor++ }
  cargo && /semver-patch-days:[[:space:]]+3$/ { patch++ }
  END { exit !(major == 1 && minor == 1 && patch == 1) }
' "$dependabot" || fail 'Cargo cooldowns must be major=30, minor=7, and patch=3 days'
if grep -qE 'dependency-name:[[:space:]]+pnpm/action-setup' "$dependabot"; then
  fail 'pnpm/action-setup must not remain ignored after the supported v6 migration'
fi
[[ -x scripts/update-toolchains.sh ]] || fail 'atomic toolchain updater is missing or not executable'

echo 'Version policy is consistent.'

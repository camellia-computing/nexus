# Dependency and Toolchain Management

This policy keeps builds reproducible without turning every compatible library update into manual version bookkeeping. Product versions remain independent from the management server and continue to use stable SemVer from the workspace root `Cargo.toml`.

## Canonical version sources

| Concern | Canonical source | Policy |
| --- | --- | --- |
| Rust toolchain | `rust-toolchain.toml` | Exact stable patch, including required components |
| Rust minimum version | Root `Cargo.toml` `rust-version` | Must match the toolchain major and minor |
| Node.js | `.node-version` | Exact 24.x LTS patch |
| Supported Node.js range | `ui/package.json` `engines.node` | Current supported major only |
| pnpm | `ui/package.json` `packageManager` | Exact patch; CI reads this field |
| JavaScript direct dependencies | `ui/package.json` | Exact versions, with the lockfile fixing the full graph |
| Rust dependencies | Cargo manifests plus `Cargo.lock` | Normal Cargo-compatible ranges; the lockfile fixes builds |
| GitHub Actions | Workflow `uses` entries | Full commit SHA with a readable release comment |
| Hosted runners | Workflow `runs-on` entries | Explicit image family; hosted image patching remains provider-managed |

Workflow files must not repeat Node.js, pnpm, or Rust version literals. `scripts/check-version-policy.sh` enforces the relationship between their canonical files, dependency declarations, immutable action references, and the exception register below. Use `scripts/update-toolchains.sh` for Node.js, pnpm, or Rust changes so every coupled source is updated and validated as one worktree transaction.

## Update policy

Dependabot groups compatible minor and patch Cargo and GitHub Actions updates and opens major upgrades separately. Cargo updates use a cooldown of 3 days for patches, 7 days for minors, and 30 days for majors. GitHub Actions use the supported 7-day default cooldown. Security updates are not delayed by this routine cooldown.

Automated commit titles use `build(deps)` for application dependencies and `ci(deps)` for GitHub Actions. Reviewed toolchain changes use `build(toolchain)`. An update that fixes a concrete product or security defect may instead use `fix(deps)` or `fix(security)`.

Dependency, toolchain, packaging, and workflow pull requests must pass the complete non-publishing candidate matrix before merge. The same matrix validates the exact `main` commit after merge. The client quality gate validates the canonical Start at login argument contract once, followed by unsigned Windows x64, Linux x64, macOS Intel, and macOS ARM package builds. The Windows package build supplies the stronger compile, link, normal-token PE, and installer proof, so a separate Windows check is redundant. Ordinary source pull requests retain quality and focused Windows validation. Trusted release proposals reuse the recorded validation of their exact base commit, validate only the generated release delta, and leave signing and publication to the protected tag workflow.

Updates are never auto-merged. A maintainer reviews release notes, lockfile scope, licensing, security impact, platform behavior, and rollback risk before merge. Node.js, pnpm, Rust, runner-family, and major framework changes require an explicit compatibility review. CI classifies updates from changed files rather than actor, account, remote, or branch naming; an unresolved change set receives the complete candidate matrix instead of a reduced check.

GitHub's current Dependabot support covers pnpm lockfiles through pnpm 10, so the npm version updater is deliberately disabled while this project uses pnpm 11. Vulnerability alerts and automated security updates remain enabled at repository level, while maintainers review `pnpm outdated` and create narrow dependency-update pull requests with a regenerated frozen lockfile. Restore the updater only after GitHub documents pnpm 11 support and a test pull request preserves the lockfile.

The Rust toolchain is also excluded from a standalone Dependabot update because one Rust change must update both `rust-toolchain.toml` and the root `rust-version`. Apply coupled updates with, for example:

```bash
bash scripts/update-toolchains.sh --node 24.18.0 --pnpm 11.15.0 --rust 1.97.0
```

The command rejects dirty target files, prepares every related edit, runs the version policy, and restores the original files if validation fails. Review upstream release notes and run the complete quality matrix before merging. CI denies yanked Cargo packages in addition to audited vulnerabilities.

## Exceptions

| Exception | Reason | Owner | Review by | Removal condition |
| --- | --- | --- | --- | --- |
| pnpm 11 Dependabot updater disabled | GitHub does not yet document pnpm 11 lockfile support; unreliable generated pull requests would fail the frozen-lockfile gate. | Release maintainers | 2026-10-15 | Re-enable the npm ecosystem after documented pnpm 11 support and a representative update pull request pass. |
| `@wdio/native-utils` 2.5.0 root override | `@wdio/tauri-service` 1.2.0 imports `installMockSyncOverride` but declares 2.4.0, whose public module does not export that function. The exact root override aligns the Tauri plugin, service and native core on the first compatible release, and `pnpm test` imports the complete driver contract. | Desktop maintainers | 2026-10-15 | Remove the override when a supported `@wdio/tauri-service` release declares a compatible native-utils version, then run the complete native Windows matrix. |
| `serialize-javascript` 7.0.7 root override | `@wdio/mocha-framework` 9.29.1 resolves Mocha 10.8.2 and its vulnerable 6.x serializer range; no patched 6.x exists for `GHSA-5c6j-r48x-rmvq`. The exact Node 24-compatible override selects the current patched release for test-only report serialization, and dependency audit plus browser/native suites validate it. | Desktop maintainers | 2026-10-15 | Remove the override when the supported WebdriverIO/Mocha graph natively resolves a patched release, then rerun dependency audit and both browser and native Windows suites. |
| `brace-expansion` 5.0.8 root override and compatibility patch | `GHSA-mh99-v99m-4gvg` requires the current 5.0.8 security release. WebdriverIO 9.29.1 still reaches `minimatch` 3.1.5, 5.1.9 and 9.0.9, whose v1/v2 dependency and callable/default import contracts are incompatible with the current named v5 API. Downgrading violates the current-version and security policies. The exact pnpm patch restores only the legacy module surface while retaining the official 5.0.8 implementation and limits. This graph is test-only and is not part of the Tauri `ui/dist` release payload; frozen installation, dependency audit, the driver contract and native Windows E2E remain mandatory. | Desktop maintainers | 2026-08-15 | Remove the patch and override as one transaction when the supported current WebdriverIO graph natively consumes the current `brace-expansion` API, or the current `brace-expansion` release officially supplies the required compatibility surface. Regenerate the lockfile, confirm no older incompatible `minimatch` remains, and run audit plus browser and native Windows suites. |
| `RUSTSEC-2024-0429` (`glib` 0.18.5) | The affected crate is a transitive GTK3 dependency in the current Tauri Linux stack. Cargo audit ignores only this advisory; normal lint, test, and packaging gates remain required. | Desktop maintainers | 2026-10-15 | Remove the audit ignore when the supported Tauri/GTK dependency graph no longer requires the affected `glib` series, then run the complete desktop and package matrix. |
| Transitive maintenance warnings | The supported Tauri/GTK dependency graph still contains unmaintained GTK3, `unic` and procedural-macro support crates. They are warnings rather than known vulnerabilities; yanked packages are denied. | Desktop maintainers | 2026-10-15 | Remove the entry when the supported desktop stack eliminates the warnings; escalate immediately if an affected crate receives a vulnerability advisory. |

No other ignored advisory, accepted maintenance warning, floating action, mutable container action, unbounded JavaScript range, or unpinned Git revision is permitted without a dated entry here.

### WebdriverIO brace-expansion bridge lifecycle

The root override intentionally selects the newest reviewed `brace-expansion` release even though
some transitive WebdriverIO consumers declare older majors. The patch is not a fork of the expansion
algorithm: it adds an ESM default alias, restores the callable CommonJS export while retaining its
named properties, and aligns the generated declarations. It must not change expansion behavior,
security limits, dependency metadata, or the package's supported Node.js range.

Treat any new `brace-expansion`, `minimatch`, WebdriverIO or Tauri WebdriverIO release as a removal
review, not as a reason to carry the patch forward automatically. A maintainer must compare
`pnpm why brace-expansion` and `pnpm why minimatch` before and after the update. If every reachable
consumer accepts the current official module surface, remove the root override, `patchedDependencies`
entry and patch file together. If compatibility is still required, update the exact latest version,
rebase the smallest module-surface-only patch, refresh this exception and its review date, and rerun
the frozen-install, audit, frontend, native-driver and Windows-native gates.

## Hosted repository checklist

For every repository location used to run CI or publish a release, verify after the first push that:

- the dependency graph, vulnerability alerts, and automated security updates are enabled where the hosting plan provides them;
- branch protection requires the current gate job and prevents bypass of reviewed release changes;
- the release application variables and secrets are complete for that repository installation;
- workflow permissions remain least-privilege and publication environments retain their configured protections;
- a dry-run or candidate workflow succeeds before relying on that location for a production release.

These are repository capabilities, not account identities. Workflows must behave the same on any correctly configured repository and must not contain account- or remote-specific branches.

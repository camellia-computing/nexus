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
| Workflow security tools | Quality workflow | Exact stable uv and zizmor releases; online pedantic findings fail CI |
| Hosted runners | Workflow `runs-on` entries | Explicit image family; hosted image patching remains provider-managed |

Workflow files must not repeat Node.js, pnpm, or Rust version literals. `scripts/check-version-policy.sh` enforces the relationship between their canonical files, dependency declarations, immutable action references, blocking actionlint and zizmor gates, and the exception register below. Use `scripts/update-toolchains.sh` for Node.js, pnpm, or Rust changes so every coupled source is updated and validated as one worktree transaction.

## Update policy

Dependabot groups compatible minor and patch Cargo and GitHub Actions updates and opens major upgrades separately. Cargo updates use a cooldown of 3 days for patches, 7 days for minors, and 30 days for majors. GitHub Actions use the supported 7-day default cooldown. Security updates are not delayed by this routine cooldown.

Automated commit titles use `build(deps)` for application dependencies and `ci(deps)` for GitHub Actions. Reviewed toolchain changes use `build(toolchain)`. An update that fixes a concrete product or security defect may instead use `fix(deps)` or `fix(security)`.

Dependency, toolchain, packaging, and workflow pull requests must pass the complete non-publishing candidate matrix before merge. Frozen installation is followed by `pnpm peers check`, so a resolvable lockfile with an internally unsupported peer graph still fails. The same matrix validates the exact `main` commit after merge. The client quality gate validates the canonical Start at login argument contract once, followed by unsigned Windows x64, Linux x64, macOS Intel, and macOS ARM package builds. The Windows package build supplies the stronger compile, link, normal-token PE, and installer proof, so a separate Windows check is redundant. Ordinary source pull requests retain quality and focused Windows validation. Trusted release proposals reuse the recorded validation of their exact base commit, validate only the generated release delta, and leave signing and publication to the protected tag workflow.

Updates are never auto-merged. A maintainer reviews release notes, lockfile scope, licensing, security impact, platform behavior, and rollback risk before merge. Node.js, pnpm, Rust, runner-family, and major framework changes require an explicit compatibility review. CI classifies updates from changed files rather than actor, account, remote, or branch naming; an unresolved change set receives the complete candidate matrix instead of a reduced check.

Dependabot updates the frontend's npm ecosystem, including its pnpm lockfile. Keep a single-document application lockfile: GitHub does not yet index the two-document format for dependency alerts and security updates. The version-policy gate rejects that format. The exact pnpm release remains owned by `packageManager`; coupled toolchain updates use `scripts/update-toolchains.sh`.

Intel macOS installs that exact JavaScript pnpm package in an isolated runner directory,
with lifecycle scripts disabled and npm registry signatures verified before execution.
The current action's self-update path selects a pnpm 11 standalone package that is not
published for this architecture. Other package targets use the pinned setup action;
no target disables package-manager identity or integrity checks.

The Rust toolchain is also excluded from a standalone Dependabot update because one Rust change must update both `rust-toolchain.toml` and the root `rust-version`. Apply coupled updates with, for example:

```bash
bash scripts/update-toolchains.sh --node 24.22.1 --pnpm 11.28.7 --rust 1.99.1
```

The command rejects dirty target files, prepares every related edit, runs the version policy, and restores the original files if validation fails. Review upstream release notes and run the complete quality matrix before merging. CI denies yanked Cargo packages in addition to audited vulnerabilities.

## Exceptions

| Exception | Reason | Owner | Review by | Removal condition |
| --- | --- | --- | --- | --- |
| jsdiff 9 override | Mocha 11 requests jsdiff 7, affected by GHSA-73rr-hh4g-fpgx. The current release preserves the patch and line-diff operations used by the test reporters; the driver contract verifies them before running the adapter. | Desktop maintainers | 2026-11-23 | Remove once the supported Mocha graph requests a patched current release directly. |
| Checkout-relative workflow references | The released actionlint 1.7.12 rejects GitHub's self-repository syntax. Line-scoped zizmor annotations retain same-commit references without disabling other workflow checks. | Release maintainers | 2026-11-23 | Adopt self-repository references when a released actionlint validates them. |
| WebdriverIO 9 and Mocha 11 | The current Tauri service requires WebdriverIO 9. Its Mocha adapter imports `mocha/lib/cli/run-helpers.js`, which Mocha 12 no longer exports. Mocha 11.8.0 passes the actual adapter execution test and avoids the affected braces dependency in Mocha 10. | Desktop maintainers | 2026-11-23 | Upgrade together when the Tauri service supports WebdriverIO 10 and its current Mocha adapter; require the native Windows matrix. |
| TypeScript 6.0.3 while TypeScript 7 is current | `svelte-check` 4.7.7 declares support for TypeScript 5 and 6 only. Moving the authoritative Svelte/TypeScript gate to an unsupported compiler major would make a green check non-contractual. | Desktop maintainers | 2026-11-23 | Upgrade when a supported current `svelte-check` release declares TypeScript 7 compatibility, then run Svelte check, unit tests, browser E2E, and all native package gates. |
| `@napi-rs/wasm-runtime` 1.1.6 override | `@rolldown/binding-wasm32-wasi` 1.1.5 pins `@emnapi/core` and `@emnapi/runtime` 1.11.1 but its compatible range otherwise resolves `wasm-runtime` 1.2.0, which requires the incompatible pre-release emnapi 2.0 pair. The exact stable runtime removes peer drift from Vite's optional WASI fallback without introducing alpha dependencies. | Desktop maintainers | 2026-11-23 | Remove the override when the current Rolldown WASI binding and runtime resolve one stable compatible emnapi generation, then require `pnpm peers check`, frontend build, and the four native package gates. |
| `serialize-javascript` 7.1.3 override | The supported WebdriverIO 9 / Mocha 11 test graph requests an affected older serializer. Use the current patched Node 24-compatible release and run the executable adapter contract, audit, browser and native tests. GHSA-5c6j-r48x-rmvq has no patched 6.x release. | Desktop maintainers | 2026-11-23 | Remove when the supported adapter graph requests a patched release itself. |
| `brace-expansion` 5.0.12 override and module-export patch | GHSA-mh99-v99m-4gvg and GHSA-rgw5-rvv9-x895 require patched expansion behavior. Test-only minimatch consumers still require callable CommonJS and default ESM exports. The patch changes only exports, not expansion or limits. | Desktop maintainers | 2026-11-23 | Remove with the override once every reachable consumer accepts the official API; require frozen install, audit, adapter and native tests. |
| Release merge `workflow_run` trigger | Approval and CI can complete in either order, so the merge controller must re-evaluate readiness after either event. The privileged path checks out only trusted `main` control, accepts only the same-repository `release/next` branch and exact successful CI SHA, revalidates approval and repository policy, and mints the scoped write token only after readiness succeeds. | Release maintainers | 2026-11-23 | Remove the trigger when GitHub provides an order-independent protected merge primitive that preserves the exact reviewed-SHA and trusted-control guarantees; otherwise re-audit every change to the merge controller or release readiness checks. |
| `RUSTSEC-2024-0429` (`glib` 0.18.5) | The affected crate is a transitive GTK3 dependency in the current Tauri Linux stack and cannot be upgraded independently. The advisory is limited to `VariantStrIter`; neither product source nor the resolved dependency sources call `VariantStrIter` or `array_iter_str`. Cargo audit ignores exactly this advisory, while yanked-package denial and the complete lint, test, native E2E, and packaging gates remain mandatory. | Desktop maintainers | 2026-11-23 | Remove the audit ignore when the supported Tauri/GTK dependency graph no longer requires the affected `glib` series, or immediately if a reachable call is introduced; then run the complete desktop and package matrix. |
| Transitive maintenance warnings | The supported Tauri/GTK dependency graph still contains unmaintained GTK3, `unic` and procedural-macro support crates. They are warnings rather than known vulnerabilities; yanked packages are denied. | Desktop maintainers | 2026-11-23 | Remove the entry when the supported desktop stack eliminates the warnings; escalate immediately if an affected crate receives a vulnerability advisory. |

The 2026-10-23 review checked the published WASI peer manifests and the frozen peer graph,
scanned the resolved non-GLib Rust sources for the [affected iterator calls](https://rustsec.org/advisories/RUSTSEC-2024-0429.html), and reran the release
authorization audit and exact-head merge-controller tests. The advisory and warning scope did not
expand; the dates above schedule the next review rather than expire an upstream support guarantee.

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

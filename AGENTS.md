# Camellia Nexus engineering guide

This file is the durable starting point for code review and implementation work in the desktop
repository. Keep it aligned with the current product and architecture. Do not add transient Actions
run IDs, release digests, deployment addresses, credentials, or machine-local instructions here.

## Product and authority model

- Camellia Nexus is a Windows-first Tauri 2 desktop lifecycle manager with supported Linux and macOS
  builds. It manages local generic commands, sing-box, Xray, and Mihomo profiles.
- A profile owns executable selection, arguments, working directory, environment, lifecycle state,
  logs, configuration, and type-specific dashboard integration.
- `managed` mode imports an isolated program directory. `external` mode runs the existing executable
  in place; one external executable may belong to only one profile.
- Processes are launched directly with argv and never through a shell. Windows uses Job Objects and
  Unix uses process groups so stop operations cover the complete managed process tree.
- The sibling license-server repository is authoritative for accounts, licenses, plans, commercial
  state, devices, Team membership, billing-derived access, cloud resources, and audit history. The
  desktop verifies signed leases and enforces local boundaries but is not the commercial authority.
- The frontend is a presentation and interaction layer. Rust remains authoritative for program
  lifecycle, licensing, limits, secure state, filesystem commits, and operating-system integration.

## Repository map

- `crates/camellia-nexus-core`: platform-neutral program models, plans, ports, configuration service,
  manager, controller state machine, and generic/sing-box/Xray/Mihomo adapters.
- `crates/camellia-nexus-licensing`: OAuth/PKCE, device identity and proof, secure storage, entitlement
  verification, trusted time, authorization guard, license-service contracts, version policy,
  release-integrity verification, and safe audit models.
- `src-tauri`: composition root, typed commands, storage, settings, configuration download/update,
  credential access, logging, tray/window state, program integrations, and Windows/Unix process
  implementations.
- `ui`: Svelte 5 frontend, typed IPC adapter, shared dialogs/editors/tokens, program-specific
  dashboards, licensing UI, Team workspace UI, localization, themes, unit tests, and Playwright.
- `scripts`: local CI, release-policy validation, packaging, native signing, security audit, native
  Windows E2E orchestration, and regression fixtures. Prefer these scripts over hand-built
  approximations of CI.
- `docs`: dependency/toolchain governance, licensing architecture, production verification, native
  signing, and managed release policy. `README.md` is the bilingual product/developer overview;
  `SECURITY.md` defines the supported security boundary.

## Architecture boundaries

- `camellia-nexus-core` must not depend on Tauri, WebView, UI, platform credential stores, or concrete
  filesystem/process implementations. Adapters produce plans and do not perform I/O.
- `camellia-nexus-licensing` must not depend on desktop commands, UI state, or program controllers.
  Keep cryptographic canonicalization and service contracts centralized in this crate.
- `src-tauri` is the composition and operating-system integration layer. Keep command handlers thin;
  reusable state machines and business invariants belong in the owning core or licensing crate.
- `ui` consumes typed IPC contracts from `ui/src/api.ts` and related model modules. Never duplicate a
  Rust authorization decision, limit, lifecycle transition, or security rule as frontend authority.
- A new program type requires aligned core, desktop, and UI modules plus registry/type extensions,
  executable identity probing, argument/configuration semantics, and tests.
- Cross-repository protocol changes must update the desktop contract, license-server handler and
  contract, tests on both sides, the relevant architecture document, and user-facing error text in
  one coordinated change. Do not keep an abandoned pre-release protocol branch.
- With both sibling repositories checked out, run `node scripts/check-cross-repo-contract.mjs` for
  every public protocol change. Keep the checker, `scripts/public-api-semantics.json`, and semantic
  mutation test byte-identical in both repositories; together they verify wire types, routes, proof
  scopes, body/page limits, mutation/concurrency identities, and mapped business error codes.

## Program lifecycle and configuration invariants

- The controller owns lifecycle transitions including start, stop, restart, startup, backoff,
  `stopFailed`, and process exit. Do not let UI state or an adapter bypass that state machine.
- Stop and local recovery are safety operations and remain available in every authorization state.
  Start, restart, creation, protected edits, scheduled refresh, and other premium mutations must pass
  the centralized guard.
- Slow preparation and network reads occur outside the authorization read gate. Re-authorize at the
  final commit boundary and verify the expected `ProgramSpec` before applying prepared content.
- Configuration writes are atomic and crash recoverable. A failed native validation or replacement
  must leave the previous usable configuration intact.
- Managed configuration sources are merged in visible UI order. Later values win; Mihomo mappings
  merge recursively, same-name object lists replace in place, and ordered lists such as `rules`
  concatenate in UI order.
- Remote configuration sources are HTTPS-only. Optional Basic credentials belong in the OS secure
  store, never Program JSON, logs, command arguments, or frontend storage. The current limits are
  4 MiB per source and 16 MiB total input.
- sing-box and Xray use native JSON; Mihomo uses native YAML. The target binary's native validator is
  the final semantic gate before an atomic apply.
- Automatic refresh scheduling, retry state, and shared configuration behavior belong in shared
  services/components rather than program-specific copies.

## Licensing and secure-state invariants

- Browser activation is first-party OAuth Authorization Code + PKCE with the exact fixed scope
  `camellia.nexus.license`; it is not OpenID Connect and does not issue ID tokens.
- Each installation uses an opaque UUID and P-256 key. Do not collect hardware fingerprints. Device
  proofs use the canonical, domain-separated payload implemented by the licensing crate.
- Refresh sessions, device credentials and metadata, signed entitlement cache, trusted-time records,
  and durable denial/revalidation markers are stored only in the operating-system secure store.
  There is no plaintext JSON, SQLite, registry, environment, command-line, or localStorage fallback.
- If the platform credential service is unavailable, use session-only repair state and disable
  commercial activation. A volatile device identity must not consume a new server slot each restart.
- Entitlements are ES256-only and use release-pinned issuer, audience, and key IDs. Validate signed
  fields, canonical identifiers, capability uniqueness, numeric limits, version policy, commercial
  expiry, device/key binding, and trusted time before use. Never download a trust key selected by an
  untrusted token.
- The runtime guard classifies operations as safety, protected, or restricted. Every protected
  mutation re-authorizes at the commit boundary under the shared runtime gate.
- A terminal denial, logout, enforced client upgrade, or expired safety window disables automatic
  lifecycle actions and stops active managed programs. Transient network failure does not invalidate
  an otherwise valid signed offline lease.
- The post-lease safety window is at most 24 hours and never exceeds signed commercial expiry.
  Existing processes may continue during it, but protected writes and automatic actions are denied.
- A failed enforcement stop retains its handle/PID and remains eligible for bounded repeated stop
  attempts; surface actionable failure details rather than pretending the process stopped.
- `max_programs` uses an atomic reservation across concurrent creates. Source limits are enforced on
  create and update. Frontend visibility is never a substitute for these Rust checks.
- The current repository verifies release manifests but does not implement an automatic updater.
  Minimum/recommended client-version policy is signed by the server and evaluated with trusted time.

## Team workspace contract

- `TeamWorkspacePanel.svelte` presents the server-backed Team surface: profile and membership,
  invitations, additional-device enrollment, member changes, leave and ownership transfer.
- Cloud functionality includes encrypted shared-configuration revisions and lifecycle actions,
  ordered sync feed/checkpoints, alert rules and incident transitions, bounded audit view/export, and
  Webhook endpoint/secret/delivery management.
- Team roles, seat/device limits, permissions, row versions, operation IDs, quotas, encryption, and
  audit records are server-authoritative. The desktop sends typed requests and handles conflicts; it
  must not infer authority from a visible control or cached profile.
- A new member first activates a device with an activation code for the same Team license, then
  accepts the single-use invitation token. Additional devices use a distinct 15-minute one-use
  enrollment token created by an already bound device; an invitation must never be reused for this.
- Invalid, expired, consumed, or wrong-kind Team tokens are recoverable input failures and must not
  clear an otherwise valid device session. Repeating an accepted token from the device it just
  linked is idempotent even after later expiry, so a lost response cannot strand or rebind a device.
- Keep destructive confirmations explicit. A stale row/version conflict reloads authoritative state
  and asks the user to review it; never silently last-write-wins.
- Local program lifecycle and program-native loopback dashboards are not remote-control trust
  boundaries. The product must not imply that server revocation can revoke direct same-user access to
  another program's own local API.

## UI and product behavior

- Major surfaces are the home dashboard, program sidebar and context menu, create flow, detail header
  and tabs, type-specific dashboards, settings/license/Team panels, logs, confirmations, and About.
- Shared program behavior, spacing, resizing, loading/empty/error/conflict states, editor behavior,
  destructive confirmation, and log-follow behavior belong in shared components or design tokens.
- Validate shared changes in Cupertino, Material, and Aurora themes; system/light/dark modes; all
  supported UI scales; and compact laptop widths. Test long Chinese and English strings.
- Preserve keyboard traversal, logical focus restoration, visible focus, dialog focus traps, screen
  reader labels, reduced-motion behavior, sufficient contrast, and non-color-only status cues.
- Primary controls must remain reachable at minimum supported dimensions. Dialogs and notices must
  not obscure their own confirmation, recovery, or close actions.
- Use contextual, dismissible notifications for recoverable events instead of permanent banners.
  Errors must be readable, actionable, localized, and safe; never expose raw backend internals or
  secrets.
- Transient read and synchronization notices dismiss automatically. When a write result is
  ambiguous, retain the original operation identity and its explicit retry action until the user
  retries or dismisses it; a generic timeout must not silently turn a safe retry into a new write.
- Loading and disabled states must distinguish work in progress from missing authority. Prevent
  duplicate writes while retaining the same idempotent operation identity for a legitimate retry.

## Windows and cross-platform constraints

- The Windows desktop process and Start at login always use the caller's normal token. Do not add
  whole-application UAC elevation, `runas`, elevated scheduled tasks, or startup bridges.
- A privilege-boundary change must update both README languages, `SECURITY.md`, the application
  manifest, Windows tests, and the release security audit in the same change.
- Keep Linux and Windows platform branches buildable. For Windows-only work use
  `bash scripts/ci-local.sh --windows-check --skip-quality` when the target toolchain is installed.
- Native desktop E2E is a debug-only test surface with a distinct application identity, data root,
  credential namespace, Tauri capability, and embedded WebDriver. Production configuration must
  never enable that capability, and release assertions must reject the `desktop-e2e` feature.
- Formal Windows signing verification must inspect the embedded file signature with WinTrust
  `WTD_CHOICE_FILE`. `Get-AuthenticodeSignature` may prefer a catalog signature and is not proof of
  the embedded signer. Preserve the catalog-vs-embedded regression fixture.
- Public Windows distribution should use trusted Authenticode plus an RFC 3161 timestamp. Public
  macOS distribution should use Developer ID, hardened runtime, notarization, and stapling. Candidate
  workflows intentionally do not receive native-signing secrets.

## Toolchain and quality gates

- Use the exact Rust toolchain in `rust-toolchain.toml`, Node.js 24.x from `.node-version`, and the
  exact pnpm version in `ui/package.json`.
- Install frontend dependencies with `pnpm --dir ui install --frozen-lockfile`.
- Before committing Rust changes, run `cargo fmt --all -- --check` and the narrowest relevant tests.
  Desktop changes must also pass
  `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings` and
  `cargo test --locked -p camellia-nexus --features desktop`.
- Frontend changes must pass `pnpm --dir ui check`, `pnpm --dir ui test`, and
  `pnpm --dir ui build`. Interaction changes also run the relevant
  `pnpm --dir ui test:e2e` Playwright cases. Key dashboard, settings/Team, and dialog states must
  retain automated WCAG A/AA Axe coverage across the supported themes and color modes.
- Windows-native client/server changes run `scripts/e2e-native.ps1` against a disposable service.
  GitHub-hosted validation uses the verified WSL bundle; local providers must remain dynamically
  configured and must isolate and clean application data, credentials, keys, database state, images,
  volumes, processes, and tunnels.
- `bash scripts/ci-local.sh` is the Unix CI-equivalent entry point; `scripts/ci-local.ps1` supplies
  native Windows modes. Use the narrowest mode during iteration, then the required complete gate.
- Release/signing changes also run their shell, Node, and PowerShell regression scripts, including
  version policy, release-manager, staging, release-security, and Authenticode fixtures.
- Do not weaken, skip, or permanently ignore a required test to make CI green. Platform-specific
  coverage belongs on its native runner when emulation cannot prove the behavior.

## CI/CD and release contract

- The root `Cargo.toml` workspace version is the only committed client version source. Client and
  server versions advance independently.
- Normal PR and `main` validation run the full product gates. A managed Release PR may contain only
  the generated version/lock/changelog delta tied to its exact validated parent and exact Actions
  provenance.
- Release PRs require a current write/admin human approval of the exact head and a successful focused
  gate, then the Release App performs the SHA-guarded squash merge. Do not manually create/move tags
  or edit the generated branch.
- Formal tags build four native packages. Native signing is optional according to the documented
  configuration; SHA-256 and keyless Cosign bundles plus public-byte readback are mandatory.
- Candidate packages are short-lived, commit-addressed, and intentionally unsigned. Never interpret
  candidate `unsigned` metadata as evidence about a formal release.
- A public release is accepted only when tag, commit, version, managed Release metadata, assets,
  checksums, Cosign identities, native signing metadata, and post-publication readback agree.

## Documentation and repository hygiene

- Update `README.md` when user-visible features, supported platforms, runtime behavior, setup, or
  operator expectations change. Keep Chinese and English sections semantically equivalent.
- Update `docs/licensing-architecture.md` for trust, authority, protocol, lease, secure-store, Team,
  or release-integrity changes; update native-signing docs and `docs/releasing.md` with their flows.
- Documentation must describe the current contract, not an abandoned pre-release transition. Remove
  stale plans and superseded wording instead of preserving compatibility prose.
- Preserve unrelated user changes and generated or local-only files. Stage only files belonging to
  the current functional change.
- Keep commits focused and independently revertible. Do not amend, rebase, merge historical commits,
  sign commits, or push unless the user explicitly requests it.
- Never commit secrets, signing material, local review notes, build output, temporary fixtures, or
  machine-specific deployment state. Keep error/log examples redacted.

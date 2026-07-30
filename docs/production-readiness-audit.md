# Camellia Nexus production-readiness audit

Audit date: 2026-07-28
Scope: logical repositories `nexus-client` and `nexus-management` as one commercial product
Baseline policy: fresh repositories, current schema only, no pre-release data or source-history compatibility

## Decision

The source baseline is suitable for protected pre-release integration. No reviewed P0 source defect remains open. A public production release is still conditional on the repository and environment gates in the final section: exact-commit hosted CI, platform-native package acceptance, production key custody, signed artifact readback, restore evidence, and an approved release record.

This distinction is intentional. Passing source tests proves the implementation baseline; it does not manufacture production credentials, operating-system trust, capacity evidence, or an operator-approved rollout.

## System boundaries reviewed

| Boundary | Authority and invariant | Result |
| --- | --- | --- |
| Desktop lifecycle | Rust controller owns state transitions; adapters produce plans; process launch never passes user arguments through a shell | Pass |
| Privilege | Normal desktop stays unelevated; a bounded, content-pinned broker owns only approved child trees and fails closed on identity/protocol loss | Pass |
| Local persistence | Only schema version 3 is accepted; pre-release schemas are rejected instead of migrated; secrets stay in OS credential storage | Pass |
| Licensing | Server is authoritative; client verifies short ES256 leases, issuer/audience/device binding, capability limits, trusted time, and revocation state | Pass |
| Product scope | Client, server, tests, contracts, and workflow fixtures use exactly `camellia.nexus.license` | Pass |
| Team/workspace | Tenant, role, device proof, idempotency, encryption AAD, retention, audit, and webhook SSRF boundaries are enforced on the server | Pass |
| Administration | Public and admin listeners are separated; CSRF, reauthentication, role, idempotency, evidence, job lease, and audit controls are explicit | Pass |
| Database | PostgreSQL is authoritative; one clean `0001_prelaunch_baseline.sql` creates the complete schema and trusted-time singleton | Pass |
| Release | Stable SemVer, exact successful-CI commit, approval, immutable digest, SBOM/provenance, signatures, scans, and readback are encoded as gates | Pass with environment prerequisites |
| Legal | Product code uses a proprietary `LicenseRef`; possession does not grant use/copy/hosting rights; third-party terms and SBOM remain separate | Pass |

## Findings resolved in this baseline

### NX-P0-01 — Development data compatibility could preserve obsolete trust state

Resolved by deleting legacy local migration logic and making stale schema rejection a tested startup contract. Server development migrations were flattened into one empty-database baseline. There is no supported upgrade path from an earlier development build.

### NX-P0-02 — Product authorization identity was coupled to an old scope/repository name

Resolved by adopting one non-configurable scope, `camellia.nexus.license`, a stable `nexus-management` logical repository identity, and the `camellia-nexus-management-server` binary/deployment identity throughout code, contracts, scripts, E2E provisioning, and workflows. Mutable physical repository names now come only from the centrally audited logical map.

### NX-P0-03 — License terms did not express the intended authorization requirement

Resolved with the Camellia Proprietary Software License 1.0, SPDX-style `LicenseRef-Camellia-Proprietary-1.0` metadata, in-product proprietary labeling, notices, and source-provenance records. Third-party dependencies retain their own terms.

### NX-P1-01 — Signing could be mistaken for a functional privilege prerequisite

Resolved by keeping broker authorization bound to normalized executable content while treating Authenticode, Apple signing/notarization, Linux OpenPGP, checksums, and Sigstore as distribution trust. Unsigned or privately signed controlled builds remain functional; partial signing configuration fails closed.

### NX-P1-02 — Windows automation could fall back to obsolete Windows PowerShell

Resolved by using `pwsh` in workflows and requiring PowerShell 7.6+ in every repository PowerShell entry point. No `powershell.exe` fallback is allowed.

### NX-P1-03 — Database concurrency and response-loss paths needed real PostgreSQL proof

Resolved by running the ignored PostgreSQL suite against PostgreSQL 18: administrator recovery, password/session revocation, jobs, billing, Team token replay, pagination, workspace retention, and full migration/readiness checks all pass on a fresh database.

## Verification evidence

- Rust 1.97: workspace check, format, strict Clippy, and 203 ordinary client tests passed; the one ignored test is the intentionally isolated live broker fixture.
- Client UI: locked pnpm install, Svelte/TypeScript check, utility tests, native-driver contract, production Vite build, 85 browser journeys, and six repeated stability journeys passed.
- Client release automation: cross-platform signing, schema-3 signing identity/trust metadata,
  deterministic signing-report, release-manager state, publication recovery, Unix staging, and
  Windows PowerShell 7.6 staging suites passed. Production packages fail closed unless
  `CAMELLIA_NEXUS_ENTITLEMENT_KEYS_JSON` supplies the approved public entitlement trust set.
- Server Rust: 178 ordinary library tests, 4 binary tests, and 17 public API flow tests passed; 10 PostgreSQL-only behavioral tests and the fresh migration/repository test also passed.
- Server Admin UI: locked install, formatting, Svelte/TypeScript check, production build, 36 browser journeys, and two repeated stability journeys passed.
- The final non-root OCI image passed a fresh PostgreSQL 18 migration plus real public, administration, worker, entitlement, Team, activation-code, billing-evidence, ClamAV, readiness, read-only-filesystem, and live-browser control-plane journeys.
- The cross-repository semantic and mutation gate passed across 83 wire types, 52 routes, 35 proof scopes, and 43 business errors; it remains required locally and in hosted CI.
- All 14 Nexus workflow files passed Actionlint. Release workflows pin third-party actions, constrain source/tag identity, require policy-App approval, produce SBOM/provenance, scan the image, sign immutable digests, and read back GitHub/GHCR state.

## Operational and commercial review

- Secrets are file/secret-store inputs with size, type, and permission checks. Entitlement, Admin, Workspace, and Webhook key domains are independent and rotation-aware. The committed entitlement-key document is development-only; production validation rejects it and accepts only the release secret's public keys.
- PostgreSQL backup/restore, key rotation, incident response, and release rollback have explicit runbooks. RPO/RTO are accepted only after a timed restore drill using the production topology.
- Public licensing, private administration, metrics, database, object storage, ClamAV, and ingress are separate trust zones. Admin and metrics listeners must not be publicly routed.
- Logs and metrics use bounded labels and exclude tokens, secret material, tenant identifiers, and Webhook credentials. Payment evidence has content/type/size and scanning state boundaries.
- The initial company name is a product marker, not a claim that support, privacy, tax, export, or consumer-law processes already exist. Those business controls must be completed before selling into a jurisdiction that requires them.

## Mandatory release gates

The release owner must record all of the following for the exact release commit:

1. Required branch rules and hosted CI are green in both repositories; the cross-repository contract references the exact sibling commit.
2. Windows 11 native E2E and package acceptance pass with PowerShell 7.6+; macOS/Linux package tests pass on their owning runners.
3. Production issuer, audience, redirect allow-list, minimum-version policy, TLS ingress, database, object storage, and all independent keyrings are reviewed without exposing secrets.
4. A fresh migration and a timed backup restore pass against PostgreSQL 18; the measured result meets RPO ≤ 1 hour and RTO ≤ 4 hours.
5. The organization signing registry, schema-3 release metadata, deterministic signing report,
   artifact checksums, SBOM, provenance, optional native/private signatures, mandatory Sigstore
   identity, image scan, immutable digest, and registry/release readback all agree.
6. A human approves the protected `release` environment and the rollback owner confirms the previous digest and database recovery point.

Failure of any gate is a no-go. An exception must name an owner, expiry, compensating control, and evidence; security, authorization, migration integrity, or signature/readback gates are not waivable.

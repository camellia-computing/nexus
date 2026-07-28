# Membership and release-security architecture

## Scope and threat model

Camellia Nexus treats the desktop process, WebView, local files, registry, and local clock as potentially modifiable. Frontend state is never an authorization authority. A local attacker with administrative control can patch client checks; therefore subscription state, device limits, revocation, cloud resources, team membership, and billing-derived access remain server-enforced.

The desktop foundation provides defense in depth for local premium operations and protects cached credentials. It does not claim to make an executable unpatchable.

## Data flow

```text
system-browser device activation + Authorization Code/PKCE
  -> rotating refresh session in OS secure storage
  -> per-install P-256 public-key registration
  -> one-time server challenge
  -> device proof over a domain-separated canonical payload
  -> server-issued 24-hour ES256 entitlement lease
  -> pinned-key verification and trusted-time evaluation
  -> centralized EntitlementGuard in the Rust process
```

The browser accepts a commercial activation code; it is not a reusable desktop credential and does not define the subscription lifetime. The server exchanges it for a short-lived, PKCE-bound authorization code. The first-party flow has one exact product scope, `camellia.nexus.license`; it does not advertise OpenID Connect scopes because this service does not issue ID tokens. Authorization requests use a bounded URL-safe state and an exact 43-character unpadded S256 challenge, and the callback must match the complete saved redirect URI including its loopback port. A database activation code is hashed at rest, expires independently, has an explicit redemption limit, and increments its redemption count only after device proof and activation confirmation succeed transactionally.

The desktop repository contains the cross-platform client, strong API schemas, local entitlement verification, and guarded IPC boundaries. The sibling management-server project is the account, license, device, activation-code, refresh-session, audit, and entitlement issuance authority. Billing and subscription ingestion can be attached to that server without adding local trust shortcuts. The desktop package contains only client-side trust metadata; entitlement issuance material is managed by the deployment or signing custody system.

## Module boundaries

The `camellia-nexus-licensing` crate is independent from service lifecycle management:

- `auth_client`: external-browser Authorization Code + PKCE and callback validation
- `secure_store`: fixed secret keys and platform-independent storage interface
- `device_identity`: random install ID, P-256 key, canonical challenge proof, replay guard
- `entitlement`: ES256-only JWS validation and pinned signing-key rotation
- `trusted_time`: signed server-time observation and local rollback detection
- `entitlement_guard`: capabilities, safety operations, offline policy, atomic limit reservations
- `license_api`: HTTPS-only service contracts with bounded responses and redacted secrets
- `release_integrity`: separately keyed update manifest, URL, digest, version, and rollback checks
- `version_policy`: canonical SemVer build identity and signed minimum/recommended-version policy
- `audit`: fixed, non-extensible safe event schema without tokens or configuration content
- `service`: secure-cache bootstrap, lease installation, refresh rotation, revocation, and epoch state

The desktop `AppState` owns one `AuthorizationService`. The UI may read its serialized state but cannot grant capabilities. Every protected IPC handler calls this instance immediately before the business mutation; feature modules must not implement plan checks.

## Device lifecycle

An installation generates an opaque UUID and P-256 device credential. Only the verification material, its SHA-256 thumbprint, platform, application version, and optional display name are registered. Hardware identifiers are not collected. The local credential remains in OS secure storage and can later be replaced by a TPM, Secure Enclave, or TPM2 provider behind the same interface.

Challenges contain a unique ID, high-entropy nonce, requested scope, issue time, and expiry. Proofs sign a length-prefixed, domain-separated binary payload. The service consumes a challenge once before signature verification, so a captured challenge cannot be retried as a signature oracle. Pending activation devices can request only `activation:verify`; active devices use `entitlement:refresh`, `session:recover`, or a target-bound `device:remove:<device-id>` scope. This prevents an activation code from being consumed merely because the desktop cannot verify an entitlement or because the user abandons the browser flow.

Activation proof and entitlement lease are intentionally separate. The short-lived activation proof confirms that the server authorized the activation confirmation step. The real entitlement lease is issued only after device activation is confirmed and is then installed in secure storage.

Device states are `pending_activation`, `active`, `removed`, `revoked`, and `suspicious`. A removed device may enter the normal activation flow again with a new code; revoked and suspicious identities cannot. Direct Admin restoration to `active` is forbidden. Activation-slot checks, code redemption, confirmation, and state changes are transactional so concurrent requests cannot exceed a plan limit.

If a locally registered device loses only its refresh session, it can request a fixed-scope recovery challenge and prove possession of the existing private key without another activation code. Recovery is available to active devices and to an unexpired `pending_activation` device solely so an ambiguously delivered registration can finish activation. It remains unavailable to inactive accounts/licenses and to removed, revoked, suspicious, or expired-pending devices. A successful recovery creates a new proven session family and atomically makes every older session for that device inactive across service replicas.

Switching to another commercial license is an explicit destructive action. The desktop first denies local access and stops protected runtime activity, best-effort revokes the previous server session, then deletes the local refresh material, device key, and registration metadata before opening a fresh browser activation. The old server-side device remains visible on its former license until an authorized user removes it, so switching cannot silently rewrite another license's device inventory.

## Secure storage

| Platform | Backend |
| --- | --- |
| Windows | Windows Credential Manager, protected by Windows user credential facilities |
| macOS | Keychain Services |
| Linux desktop | Secret Service through the user D-Bus session, including compatible GNOME Keyring or KWallet providers |
| Tests | In-memory/session-only implementation |

Stored values are limited to refresh-session material, device credentials, device registration metadata, the signed lease cache, trusted time, refresh metadata, and durable denial/revalidation markers. No plaintext file, SQLite, environment-variable, Program JSON, registry-value, or localStorage fallback exists. A hard server denial is applied in memory before persistence is attempted, so a secure-store error cannot preserve premium access.

When the operating-system credential service is unavailable, the desktop enters a session-only repair state. Commercial device activation is disabled in that state: accepting activation with a volatile device key would create a new server device on every restart and eventually exhaust device slots. The interface directs the user to repair the credential service and restart. Corrupt device identity can be reset independently without deleting service configuration or unrelated membership values.

## Entitlement validation

The verifier accepts ES256 only and selects release-pinned trust metadata by `kid`. It validates signature, issuer, audience, device ID, device-key thumbprint, signing key agreement, minimum license epoch, the signed client-version policy, issued/refresh/expiry ordering, commercial standing, capability uniqueness, numeric limit shape, and trusted time. `past_due` and `canceled` leases are usable only when the signed commercial expiry explicitly defines an unelapsed grace term; without that term they fail with their exact business status. Unknown keys, token-selected algorithms, HMAC, unsigned tokens, and dynamically downloaded trust keys are rejected.

The persisted trusted-time record is loaded once into a runtime anchor. Subsequent hot-path authorization checks derive progress from a cross-process continuous system-uptime clock (`CLOCK_BOOTTIME`, `GetTickCount64`, or `mach_continuous_time`) and remember forward wall-clock movement; they do not synchronously query Keychain, Credential Manager, or Secret Service for every log read or dashboard poll. Successful authenticated online observations update a separate secure high-water mark. The first accepted observation after a reboot checkpoints the new boot transition so repeated rapid restarts cannot freeze lease progress, while the online high-water mark still permits an authenticated correction of a fast local clock. Missing or corrupt persisted time alongside a cached lease fails closed at startup.

Entitlement issuance material and update manifest signing material use different Rust types and trust sets. Release credentials do not belong in this repository or desktop package.

## Authorization and expiry policy

Operations are explicitly typed as safety or protected operations. Safety operations remain available in every state:

- view local data
- export
- stop a process
- remove a managed entry
- recover or delete local configuration

During a valid lease, granted capabilities and limits are available. The Rust IPC layer checks capability and numeric limits before mutation. A dedicated runtime read/write gate linearizes every protected mutation's final authorization and commit against logout, denial, and expiry. Fail-close takes the writer, re-reads the latest entitlement, disables automatic lifecycle activity, and stops managed processes. Slow source downloads and package preparation stay outside the gate, then re-authorize under the read side immediately before their atomic commit; configuration apply also verifies the expected `ProgramSpec` inside the controller so a concurrent edit cannot receive stale content. `max_programs` is enforced with an atomic reservation around create operations so concurrent IPC calls cannot exceed the local entitlement limit; `max_config_sources_per_program` is checked on create and update.

For up to 24 hours after the signed lease expires, existing processes may continue and safety operations remain available, while premium writes, creation, activation, automatic restart, automatic configuration refresh, synchronization, remote control through Camellia Nexus, organization administration, cloud access, and premium updates are denied. This safety window is capped by the signed commercial license expiry and can never extend the purchased term. After the window the Rust runtime repeatedly attempts to stop starting/running/stopping/backoff programs. A failed stop retains its process handle and PID in `stopFailed`, is bounded by a safety deadline, and remains eligible for every later enforcement retry; failed IDs are also reported so the user can intervene. Safety and local recovery operations remain available in every state.

Program-native loopback APIs are a local-host trust boundary, not a server-backed DRM boundary. Once a sing-box/Clash, Xray, or Mihomo dashboard URL has been opened, another process running as the same OS user may address that program's own loopback port directly. Camellia Nexus gates its IPC and refuses to launch or invoke those controls while restricted, but it does not claim to revoke bookmarks or third-party local clients without a program-native authenticated proxy.

Clock rollback, corrupt secure storage, obsolete epochs, and invalid signed proofs cause fail-closed online revalidation. Account suspension/denylisting, license expiry/cancellation/payment denial, device revocation, and refresh-session reuse are represented separately and persisted. The background Rust task checks local time every 30 seconds and performs online status/refresh maintenance approximately every five minutes with jitter and bounded backoff; security does not depend on the WebView being open. Explicit logout always clears local access even if the network call fails.

## Client-version policy

The desktop layer injects one canonical SemVer build identity from the final desktop package into registration metadata and every device proof. Every signed entitlement carries `minimumVersion`, `recommendedVersion`, and `enforceAfter`. Below-recommended builds remain usable with a non-blocking advisory. A build below the minimum remains usable only before enforcement, and its refresh target and lease expiry are capped at the exact enforcement instant. Builds already meeting the minimum keep their normal lease lifetime.

At `enforceAfter` exactly, trusted-time evaluation enters the dedicated hard-inactive `clientUpgradeRequired` state before ordinary lease-expiry handling. The same transition is applied when registration, activation, refresh, or session recovery returns HTTP 426. Registration performs the check before consuming the authorization code; rejected refresh/recovery requests do not rotate the session. The desktop persists the authenticated denial, immediately disables automatic restart and stops protected programs. The same blocked build stays denied across restart; a build satisfying the minimum clears the marker and can refresh normally. Safety operations and sign-out remain available. The blocked build does not continuously retry an impossible background refresh, while an explicit refresh remains available for an administrator's emergency policy rollback.

The version-policy UI never uses the WebView wall clock as an authorization authority. Rust evaluates enforcement using trusted time and emits the authoritative state; the UI only presents the signed policy, current package version, and resulting advisory. Installing a new package is intentionally explicit because this repository currently provides release-manifest verification but not a complete updater.

A device proof authenticates that the holder of the installation key signed an `appVersion` string. It does not attest that the running executable is an unmodified official binary. A patched client can lie about that string. Hardware/platform attestation and code-signing measurements would be a separate trust system; server-backed resources, commercial state, and signing keys remain the enforceable boundary here.

Pricing boundaries must be declared explicitly before wiring an existing IPC handler to `ProtectedOperation`. Once declared, both the handler and every lower business entry point enforce the same centralized guard; a UI-only restriction is never sufficient.

## Server authority and API requirements

The client contracts cover device registration, activation proof verification, challenge issue, proof submission through entitlement refresh, proof-based session recovery, proof-based entitlement status, device listing/removal, and logout. Entitlement status is a `POST` operation that consumes a fresh `entitlement:status` challenge and verifies a current-build device signature; possession of a bearer refresh session alone cannot reveal current device or license state. The production service provides:

- OAuth-style authorization-code completion with PKCE and desktop callback support; the code is consumed once, while an identical device/key/PKCE/redirect request may retrieve the already committed result for ten non-sliding minutes after an ambiguous response
- rotating refresh sessions bound to a registered device
- single-use challenge storage with a 60-second TTL
- transactional device-slot enforcement
- refresh-token family reuse detection and family revocation
- account, license, device, activation-code, and license-epoch authority
- PostgreSQL-backed fixed-window rate limits shared by all replicas for anonymous and device-bound activation, recovery, status, challenge, refresh, listing, removal, and logout traffic
- keyset-paginated device inventory (`license_id`, canonical device UUID) with a default page of 50 and a hard maximum of 100, so neither the database nor the desktop performs unbounded account-wide loads
- production-required canonical minimum/recommended client versions and a human-readable enforcement time, enforced before code consumption, session rotation, or entitlement issuance
- structured audit retention without raw tokens, signing material, machine fingerprints, command arguments, or configuration
- Team membership, invitation, device-enrollment, leave, and ownership-transfer transactions with role, seat, device, row-version, and idempotency enforcement. Every public Team write carries a canonical UUIDv4 `operationId`; the server commits the mutation, audit record, and an encrypted exact response in one PostgreSQL transaction, then replays that response for an identical device/command/request retry. Reusing the identity with changed content returns a conflict. The UI keeps the original request and identity behind an explicit retry after an ambiguous result, including one-time invitation and enrollment secrets. Voluntary leave first checks `GET /v1/team/operations/{operation_id}` with the exact original bearer session, member ID, and row version; only a stored `leave_workspace` request with all bindings equal counts as committed, and local Team authorization is cleared only after that confirmation. A new member activates against the same Team license before accepting the single-use invitation, while additional devices use distinct 15-minute enrollment tokens. Invalid, expired, consumed, or wrong-kind Team tokens return dedicated recoverable business errors and never invalidate an otherwise valid device refresh session. A successfully linked device may replay its consumed token idempotently even after later expiry, so an ambiguous response cannot create a duplicate binding or strand the client. Device removal atomically revokes sessions and deletes its member binding; reactivation remains unassigned, while Owner/Admin can issue a bounded recovery enrollment for an Active member with zero devices
- tenant-scoped shared configurations, sync checkpoints, alert incidents, audit export, and Webhook delivery contracts used by the desktop Team workspace

Current lifetimes are five minutes for PKCE authorization codes, 60 seconds for device challenges, 24 hours for signed entitlement leases, a six-hour refresh target, seven days of refresh-session idle time, and a 30-day absolute session-family lifetime. Rotated session-family records remain for seven additional days after absolute expiry so replay detection is not erased by cleanup.

## Cross-repository protocol validation

The desktop and license-service repositories carry byte-identical copies of the contract checker,
`scripts/public-api-semantics.json`, and its mutation test. With the sibling checkout at its default
adjacent path, run `node scripts/check-cross-repo-contract.mjs` and then
`node scripts/test-cross-repo-contract.mjs` from either repository. The gate derives public wire
shapes, method/path pairs and error mappings from Rust, and additionally pins proof scopes, body/page
limits and mutation/concurrency identities. Scheduled and post-`main` runs detect either source or
semantic-manifest drift; manual runs can select a coordinated sibling ref.

## Release integrity

Update manifests are independently signed with ES256. Verification pins key IDs and validates issuer, audience, HTTPS origin/path policy, artifact URL credentials/query/fragment restrictions, semantic version, minimum supported version, SHA-256 digest, expiry, and highest accepted version. Artifact bytes are checked after download. The repository currently provides the verifier boundary; no updater is installed by this project.

Release credentials remain separated:

- OAuth/session keys
- entitlement issuer credentials
- update manifest release credentials
- Windows Authenticode certificate
- macOS Developer ID certificate
- payment webhook secrets

Windows release scripts support embedded Authenticode signer and RFC 3161 timestamp verification. The macOS workflow supports controlled unsigned, ad-hoc, certificate-signed, and notarized modes; public commercial distribution should use Developer ID, hardened runtime, notarization, and stapling. Linux packages can additionally carry optional full-fingerprint OpenPGP detached signatures and their public key, but this does not create operating-system trust. Every public package remains covered by published SHA-256 checksums and keyless Sigstore/Cosign bundles. Critical upgrade policy may deny server-backed premium calls but cannot disable local safety operations.

## Operational limitations

- This desktop repository does not operate the entitlement service, OAuth issuer, billing control plane, or signing-key custody system; a production deployment must provide and monitor those sibling service boundaries.
- Public entitlement and update verification keys must be rotated through signed releases before changing commercial trust roots.
- OS secure storage protects secrets for the signed-in OS user; malware running as that user may still invoke platform APIs.
- Local checks can be patched on a fully compromised host. Server-backed capabilities remain enforceable because the desktop never contains server signing or payment secrets.

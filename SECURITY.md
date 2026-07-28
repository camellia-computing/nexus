# Security Policy

## Supported versions

Security updates are provided for the latest published release and the current `main` branch.

## Reporting a vulnerability

Report vulnerabilities through the repository's private security advisory channel. Do not disclose exploitable details in public issues.

A report should include the affected version, operating system, reproduction procedure, observed behavior, and security impact. Remove credentials, configuration secrets, and unrelated personal data from all submitted material.

## Security boundary

Camellia Nexus normally executes selected binaries with the privileges of the application. On Windows, the desktop application and Start at login always use the caller's normal token; the full application never elevates itself. A Profile that explicitly or automatically requires TUN, transparent-proxy, or another administrator-only facility launches only its child through the separately packaged privilege broker. The first explicit privileged start opens one operating-system-authorized broker for the current application session; later privileged starts, stops, restarts, and configuration-driven restarts reuse it. No persistent elevated service is installed. Non-interactive work never creates an authorization prompt and proceeds only when that session already exists.

The broker accepts bounded, versioned launch requests, correlates them by operation and Program identity, validates paths and argument/environment limits again at the elevated boundary, launches directly without a shell, and owns every complete Job Object or process group. It pins each Program's first launch definition for the broker session; a changed definition requires a new application session. A malformed frame, protocol mismatch, lost connection, or failed validation terminates the affected or complete brokered tree as appropriate. App-owned configuration is assessed only through regular non-link paths inside its workspace. External or Generic configuration that cannot be assessed remains standard by default and requires an explicit Profile override; Camellia Nexus never guesses from arbitrary stderr text.

Privilege-capable packages bind the privilege broker's normalized executable content digest into the desktop at build time. Runtime rejects symlinks, non-regular files, Unix group/world-writable broker executables, and content mismatches. Authenticode and Mach-O code-signature containers are excluded from this content identity, so code signing is never a functional authorization prerequisite. Windows MSI and portable ZIP distributions include the exact application/broker pair side by side. Trusted native signing remains strongly recommended to authenticate the publisher and protect the distribution channel, especially for portable installations. Approving an administrator launch still grants the selected executable and configuration the corresponding operating-system authority, so only trusted inputs should be imported.

Optional Linux OpenPGP detached signatures authenticate artifacts only to users who independently trust the recorded full fingerprint; they do not replace package ownership checks or create operating-system executable trust. A formal release always retains SHA-256 and keyless Sigstore/Cosign verification, whether optional native or detached signing is enabled or not.

Program isolation separates lifecycle state, managed files, logs, and process trees. It is not a security sandbox and does not restrict access to resources available to the application account.

Configuration files, environment variables, and logs may contain sensitive data. Protect the application data directory and sanitize diagnostic material before distribution.

Remote configuration sources require HTTPS with system certificate validation. Redirects remain subject to HTTPS and embedded URL credentials are rejected. Source URLs, including query tokens, and optional HTTP Basic usernames are stored in restricted Program metadata. Basic passwords are removed before Program serialization and stored only in the operating-system-protected credential store; frontend draft persistence also excludes them. Use scoped credentials and protect both the application data directory and the current operating-system account accordingly. Download limits reduce resource abuse but do not make untrusted configurations safe to execute.

Membership state is server-authoritative. The client accepts only short-lived ES256 entitlement leases issued for the current device key and verified by release-pinned trust metadata. Refresh sessions, device credentials, cached leases, and trusted-time records use the operating-system credential store; they are never persisted to Program metadata or frontend storage. Linux sessions without Secret Service operate without persistent membership credentials or offline continuity.

Local authorization checks are defense in depth because a fully compromised host can patch its own executable. Cloud capabilities, device slots, revocation, subscription state, organization membership, and license epochs must also be enforced by the service. See [docs/licensing-architecture.md](docs/licensing-architecture.md) for the complete trust model.

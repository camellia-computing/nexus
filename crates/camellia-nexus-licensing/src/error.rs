use thiserror::Error;

pub type Result<T> = std::result::Result<T, LicensingError>;

#[derive(Debug, Error)]
pub enum LicensingError {
    #[error("secure storage is unavailable")]
    SecureStoreUnavailable,
    #[error("secure storage contains invalid data")]
    SecureStoreCorrupt,
    #[error("secure storage operation failed")]
    SecureStoreBackend,
    #[error("secure credential operation timed out")]
    SecureStoreTimeout,
    #[error("device identity is unavailable")]
    DeviceIdentityUnavailable,
    #[error("operating-system cryptographic randomness is unavailable")]
    EntropyUnavailable,
    #[error("client build metadata is invalid")]
    InvalidClientBuild,
    #[error("device challenge is invalid or expired")]
    InvalidChallenge,
    #[error("device challenge has already been used")]
    ChallengeReplay,
    #[error("entitlement is malformed")]
    MalformedEntitlement,
    #[error("entitlement signature is invalid")]
    InvalidSignature,
    #[error("entitlement signing algorithm is not allowed")]
    UnsupportedAlgorithm,
    #[error("entitlement signing key is not trusted")]
    UnknownSigningKey,
    #[error("entitlement issuer is not trusted")]
    WrongIssuer,
    #[error("entitlement audience is not trusted")]
    WrongAudience,
    #[error("entitlement belongs to a different device")]
    DeviceMismatch,
    #[error("entitlement device key does not match this installation")]
    DeviceKeyMismatch,
    #[error("entitlement has expired")]
    EntitlementExpired,
    #[error("entitlement epoch is obsolete")]
    ObsoleteLicenseEpoch,
    #[error("entitlement claim values are invalid")]
    InvalidClaims,
    #[error("the local clock must be revalidated online")]
    ClockRollback,
    #[error("this operation requires an active entitlement")]
    AuthorizationRequired,
    #[error("the entitlement does not grant the required capability")]
    CapabilityDenied,
    #[error("the current workspace role does not allow this action")]
    PermissionDenied,
    #[error("the entitlement limit would be exceeded")]
    LimitExceeded,
    #[error("the device is not authorized")]
    DeviceDenied,
    #[error("the device still needs to complete activation")]
    DeviceActivationPending,
    #[error("the pending device activation expired")]
    ActivationPendingExpired,
    #[error("the device was removed from the license")]
    DeviceRemoved,
    #[error("the device was revoked")]
    DeviceRevoked,
    #[error("the device was marked suspicious")]
    DeviceSuspicious,
    #[error("the license account is suspended")]
    AccountSuspended,
    #[error("the license account is denylisted")]
    AccountDenylisted,
    #[error("the license payment is past due")]
    LicensePastDue,
    #[error("the license was canceled")]
    LicenseCanceled,
    #[error("the license has expired")]
    LicenseExpired,
    #[error("this client version is no longer supported")]
    ClientUpgradeRequired { policy: crate::ClientVersionPolicy },
    #[error("no usable license is available for this account")]
    LicenseUnavailable,
    #[error("the license service is not configured")]
    ServiceUnconfigured,
    #[error("license service request failed")]
    Network,
    #[error("license operation timed out")]
    Timeout,
    #[error("license service response is invalid")]
    InvalidServerResponse,
    #[error("license service rejected the request")]
    InvalidRequest,
    #[error("the Team invitation token is invalid, expired, or no longer available")]
    TeamInvitationInvalid,
    #[error("the Team device enrollment token is invalid, expired, or no longer available")]
    TeamDeviceEnrollmentInvalid,
    #[error("the workspace changed; reload it before retrying")]
    WorkspaceVersionConflict,
    #[error("the workspace storage quota was exceeded")]
    WorkspaceQuotaExceeded,
    #[error("the workspace shared configuration limit was reached")]
    WorkspaceDocumentLimitReached,
    #[error("the workspace alert rule limit was reached")]
    WorkspaceAlertRuleLimitReached,
    #[error("the workspace resource is still within its recovery retention period")]
    WorkspaceRetentionActive,
    #[error("the workspace resource does not exist")]
    WorkspaceNotFound,
    #[error("the workspace content failed its integrity check")]
    WorkspaceIntegrity,
    #[error("the workspace encryption key is unavailable")]
    WorkspaceKeyUnavailable,
    #[error("the operation ID was already used for a different request")]
    IdempotencyConflict,
    #[error("the webhook endpoint URL is invalid")]
    WebhookInvalidUrl,
    #[error("the webhook endpoint limit was reached")]
    WebhookEndpointLimitReached,
    #[error("the webhook endpoint does not exist")]
    WebhookNotFound,
    #[error("the webhook encryption key is unavailable")]
    WebhookKeyUnavailable,
    #[error("the request body is too large")]
    RequestTooLarge,
    #[error("refresh session reuse was detected by the license service")]
    RefreshSessionReused,
    #[error("the activation code is invalid")]
    ActivationCodeInvalid,
    #[error("the activation code has expired")]
    ActivationCodeExpired,
    #[error("the activation code has already been redeemed")]
    ActivationCodeConsumed,
    #[error("the activation code has been revoked")]
    ActivationCodeRevoked,
    #[error("the plan device activation limit was reached")]
    ActivationLimitReached,
    #[error("too many license service requests")]
    TooManyRequests { retry_after_seconds: Option<u64> },
    #[error("OAuth callback is invalid")]
    InvalidOAuthCallback,
    #[error("update manifest is invalid")]
    InvalidUpdateManifest,
    #[error("update artifact URL is not allowed")]
    UpdateUrlDenied,
    #[error("update would violate rollback protection")]
    UpdateRollback,
    #[error("update artifact digest does not match the signed manifest")]
    ArtifactDigestMismatch,
}

use serde::{Deserialize, Serialize, Serializer, ser::SerializeStruct};
use thiserror::Error;

pub type Result<T> = std::result::Result<T, CamelliaNexusError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidSpec,
    InvalidPath,
    NotFound,
    AlreadyExists,
    ProgramBusy,
    InvalidState,
    SpawnFailed,
    StopFailed,
    ConfigConflict,
    ConfigInvalid,
    ConfigurationSchemaInvalid,
    UnsupportedBinary,
    OutputLimitExceeded,
    Timeout,
    RateLimited,
    Network,
    Storage,
    SystemIntegration,
    PrivilegeRequired,
    PrivilegeAuthorizationCanceled,
    PrivilegeBrokerUnavailable,
    PrivilegeConfigUnsafe,
    PrivilegeBrokerFailed,
    PrivilegeBrokerConnectionLost,
    LicenseRequired,
    LicenseIdentityAlreadyRegistered,
    LicenseActivationPending,
    LicenseActivationPendingExpired,
    LicensePlanRequired,
    LicensePermissionDenied,
    LicenseTeamInvitationInvalid,
    LicenseTeamDeviceEnrollmentInvalid,
    LicenseWorkspaceConflict,
    LicenseOperationConflict,
    LicenseWorkspaceQuotaExceeded,
    LicenseWorkspaceDocumentLimitReached,
    LicenseWorkspaceAlertRuleLimitReached,
    LicenseWorkspaceRetentionActive,
    LicenseWorkspaceNotFound,
    LicenseWorkspaceIntegrityFailed,
    LicenseWorkspaceKeyUnavailable,
    LicenseWebhookInvalidUrl,
    LicenseWebhookEndpointLimitReached,
    LicenseWebhookNotFound,
    LicenseWebhookKeyUnavailable,
    RequestTooLarge,
    LicenseExpired,
    LicenseAccountSuspended,
    LicenseAccountDenylisted,
    LicensePaymentPastDue,
    LicenseCanceled,
    LicenseClientUpgradeRequired,
    LicenseDeviceDenied,
    LicenseDeviceRemovalIncomplete,
    LicenseRemoteSignoutIncomplete,
    LicenseRevalidationRequired,
    LicenseLimitExceeded,
    LicenseActivationCodeInvalid,
    LicenseActivationCodeExpired,
    LicenseActivationCodeConsumed,
    LicenseActivationCodeRevoked,
    Internal,
}

#[derive(Debug, Clone, Error, Deserialize)]
#[error("{message}")]
#[serde(rename_all = "camelCase")]
pub struct CamelliaNexusError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl Serialize for CamelliaNexusError {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // This is a local IPC boundary: filesystem details are required for actionable
        // diagnostics, while internal runtime errors may still contain implementation data.
        let expose_details = self.code != ErrorCode::Internal;
        let mut state = serializer.serialize_struct(
            "CamelliaNexusError",
            if expose_details && self.details.is_some() {
                3
            } else {
                2
            },
        )?;
        state.serialize_field("code", &self.code)?;
        state.serialize_field("message", &self.message)?;
        if expose_details && let Some(details) = &self.details {
            state.serialize_field("details", details)?;
        }
        state.end()
    }
}

impl CamelliaNexusError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }

    pub fn invalid_spec(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidSpec, message)
    }

    pub fn storage(error: impl std::fmt::Display) -> Self {
        Self::new(ErrorCode::Storage, "Storage operation failed").with_details(error.to_string())
    }

    pub fn internal(error: impl std::fmt::Display) -> Self {
        Self::new(ErrorCode::Internal, "Internal operation failed").with_details(error.to_string())
    }

    pub fn system_integration(message: impl Into<String>, error: impl std::fmt::Display) -> Self {
        Self::new(ErrorCode::SystemIntegration, message).with_details(error.to_string())
    }
}

impl From<std::io::Error> for CamelliaNexusError {
    fn from(value: std::io::Error) -> Self {
        Self::storage(value)
    }
}

impl From<serde_json::Error> for CamelliaNexusError {
    fn from(value: serde_json::Error) -> Self {
        Self::new(ErrorCode::InvalidSpec, "Invalid JSON").with_details(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialization_exposes_actionable_storage_details_only() {
        let storage = CamelliaNexusError::storage("permission denied for /user/path");
        let serialized = serde_json::to_string(&storage).expect("serialize storage");
        assert!(serialized.contains("permission denied"));

        let internal = CamelliaNexusError::internal("private implementation detail");
        let serialized = serde_json::to_string(&internal).expect("serialize internal");
        assert!(!serialized.contains("private implementation detail"));
    }

    #[test]
    fn activation_code_errors_have_stable_ipc_codes() {
        let error = CamelliaNexusError::new(
            ErrorCode::LicenseActivationCodeConsumed,
            "License service operation failed",
        );
        let serialized = serde_json::to_string(&error).expect("serialize activation code error");
        assert!(serialized.contains("LICENSE_ACTIVATION_CODE_CONSUMED"));
    }
}

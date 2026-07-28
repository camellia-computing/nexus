use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Plan {
    Free,
    Pro,
    Team,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LicenseStanding {
    Active,
    PastDue,
    Canceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    ManagedConfigSources,
    AdvancedDiagnostics,
    CloudSync,
    RemoteDashboard,
    Alerts,
    SharedConfigurations,
    TeamAdministration,
    AuditLog,
    Webhooks,
    ManagedProgramPackages,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NumericLimit {
    MaxPrograms,
    MaxConfigSourcesPerProgram,
    MaxTeamMembers,
    MaxRemoteMonitors,
    MaxSharedPrograms,
    MaxWebhookEndpoints,
    MaxWorkspaceStorageBytes,
    MaxAlertRules,
    MaxAuditExportEvents,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientVersionPolicy {
    pub minimum_version: String,
    pub recommended_version: String,
    pub enforce_after: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceState {
    PendingActivation,
    Active,
    Removed,
    Revoked,
    Suspicious,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LicenseInactiveReason {
    AccountSuspended,
    AccountDenylisted,
    LicensePastDue,
    LicenseCanceled,
    LicenseExpired,
    LicenseUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafetyOperation {
    View,
    Export,
    Stop,
    Remove,
    Recover,
    DeleteLocalConfiguration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectedOperation {
    CreatePremiumProgram,
    EditPremiumConfiguration,
    Activate,
    Synchronize,
    RemoteControl,
    OrganizationAdmin,
    UseManagedConfigSources,
    RunAdvancedDiagnostics,
    UseManagedProgramPackages,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "class", content = "operation", rename_all = "snake_case")]
pub enum RestrictedOperation {
    Safety(SafetyOperation),
    Protected(ProtectedOperation),
}

impl ProtectedOperation {
    pub const fn capability(self) -> Option<Capability> {
        match self {
            Self::UseManagedConfigSources => Some(Capability::ManagedConfigSources),
            Self::RunAdvancedDiagnostics => Some(Capability::AdvancedDiagnostics),
            Self::Synchronize => Some(Capability::CloudSync),
            Self::RemoteControl => Some(Capability::RemoteDashboard),
            Self::OrganizationAdmin => Some(Capability::TeamAdministration),
            Self::UseManagedProgramPackages => Some(Capability::ManagedProgramPackages),
            Self::CreatePremiumProgram | Self::EditPremiumConfiguration | Self::Activate => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementClaims {
    pub schema_version: u32,
    pub iss: String,
    pub aud: String,
    pub sub: String,
    pub license_id: String,
    pub device_id: String,
    pub device_key_thumbprint: String,
    pub plan: Plan,
    pub plan_revision: u32,
    pub policy_hash: String,
    pub license_status: LicenseStanding,
    pub capabilities: Vec<Capability>,
    pub workspace_permissions: Vec<String>,
    pub limits: BTreeMap<NumericLimit, u64>,
    pub client_version_policy: ClientVersionPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license_expires_at: Option<i64>,
    pub license_epoch: u64,
    pub device_limit: u32,
    pub member_limit: u32,
    pub offline_access_ends_at: i64,
    #[serde(rename = "iat")]
    pub issued_at: i64,
    pub refresh_after: i64,
    #[serde(rename = "exp")]
    pub expires_at: i64,
    pub token_id: String,
    pub key_id: String,
}

impl EntitlementClaims {
    pub fn capability_set(&self) -> BTreeSet<Capability> {
        self.capabilities.iter().copied().collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedEntitlement {
    pub claims: EntitlementClaims,
    pub key_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationProofClaims {
    pub iss: String,
    pub aud: String,
    pub sub: String,
    pub license_id: String,
    pub device_id: String,
    pub device_key_thumbprint: String,
    pub license_epoch: u64,
    pub purpose: String,
    #[serde(rename = "iat")]
    pub issued_at: i64,
    #[serde(rename = "exp")]
    pub expires_at: i64,
    pub token_id: String,
    pub key_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedActivationProof {
    pub claims: ActivationProofClaims,
    pub key_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum EntitlementState {
    Unauthenticated,
    SessionOnly,
    /// The device registration and refresh session are persisted, but the activation proof,
    /// confirmation, or first signed entitlement still needs to complete online.
    ActivationPending,
    Active {
        entitlement: VerifiedEntitlement,
    },
    RestrictedOffline {
        entitlement: VerifiedEntitlement,
        safety_window_ends_at: i64,
    },
    Expired {
        entitlement: VerifiedEntitlement,
    },
    RevalidationRequired {
        reason: RevalidationReason,
    },
    ClientUpgradeRequired {
        policy: ClientVersionPolicy,
        entitlement: Option<VerifiedEntitlement>,
    },
    DeviceDenied {
        state: DeviceState,
    },
    LicenseInactive {
        reason: LicenseInactiveReason,
        entitlement: Option<VerifiedEntitlement>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevalidationReason {
    ClockRollback,
    ObsoleteEpoch,
    CorruptSecureStore,
    InvalidServerProof,
}

impl EntitlementState {
    pub fn entitlement(&self) -> Option<&VerifiedEntitlement> {
        match self {
            Self::Active { entitlement }
            | Self::RestrictedOffline { entitlement, .. }
            | Self::Expired { entitlement } => Some(entitlement),
            Self::ClientUpgradeRequired {
                entitlement: Some(entitlement),
                ..
            }
            | Self::LicenseInactive {
                entitlement: Some(entitlement),
                ..
            } => Some(entitlement),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRegistrationMetadata {
    pub device_id: String,
    pub public_key_pem: String,
    pub public_key_thumbprint: String,
    pub platform: String,
    pub app_version: String,
    pub display_name: Option<String>,
}

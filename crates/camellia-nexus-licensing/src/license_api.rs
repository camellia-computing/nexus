use std::collections::BTreeMap;

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use url::Url;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{
    DeviceChallenge, DeviceProof, DeviceRegistrationMetadata, DeviceState, LicensingError, Result,
};

const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;
const MAX_CONFIGURATION_RESPONSE_BYTES: u64 = 5 * 1024 * 1024;
const MAX_AUDIT_EXPORT_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;
const DEFAULT_TEAM_MEMBER_PAGE_SIZE: u32 = 100;
const MAX_TEAM_MEMBER_PAGE_SIZE: u32 = 200;
const DEVICE_PROOF_HEADER: &str = "x-camellia-device-proof";

#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(transparent)]
pub struct RefreshSession(pub String);

impl std::fmt::Debug for RefreshSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RefreshSession([REDACTED])")
    }
}

impl RefreshSession {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(transparent)]
pub struct SecretValue(pub String);

impl SecretValue {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretValue([REDACTED])")
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRegistrationRequest {
    pub device: DeviceRegistrationMetadata,
    pub authorization_code: SecretValue,
    pub pkce_verifier: SecretValue,
    pub redirect_uri: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRegistrationResponse {
    pub device_state: DeviceState,
    pub refresh_session: RefreshSession,
    pub server_unix: i64,
}

impl std::fmt::Debug for DeviceRegistrationResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeviceRegistrationResponse")
            .field("device_state", &self.device_state)
            .field("refresh_session", &"[REDACTED]")
            .field("server_unix", &self.server_unix)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceActivationConfirmationResponse {
    pub device_state: DeviceState,
    pub server_unix: i64,
}

impl std::fmt::Debug for DeviceActivationConfirmationResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeviceActivationConfirmationResponse")
            .field("device_state", &self.device_state)
            .field("server_unix", &self.server_unix)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChallengeRequest {
    pub device_id: String,
    pub requested_scope: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecoveryChallengeRequest {
    pub device_id: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecoveryRequest {
    pub proof: DeviceProof,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecoveryResponse {
    pub refresh_session: RefreshSession,
    pub server_unix: i64,
    pub device_state: DeviceState,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRemovalRequest {
    pub proof: DeviceProof,
    pub operation_id: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRemovalStatusResponse {
    pub committed: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementRefreshRequest {
    pub proof: DeviceProof,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementStatusRequest {
    pub proof: DeviceProof,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationVerificationRequest {
    pub proof: DeviceProof,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationVerificationResponse {
    pub activation: SecretValue,
    pub rotated_refresh_session: RefreshSession,
    pub server_unix: i64,
    pub device_state: DeviceState,
}

impl std::fmt::Debug for ActivationVerificationResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActivationVerificationResponse")
            .field("activation", &"[REDACTED]")
            .field("rotated_refresh_session", &"[REDACTED]")
            .field("server_unix", &self.server_unix)
            .field("device_state", &self.device_state)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementRefreshResponse {
    pub entitlement: SecretValue,
    pub rotated_refresh_session: RefreshSession,
    pub server_unix: i64,
    pub device_state: DeviceState,
}

impl std::fmt::Debug for EntitlementRefreshResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EntitlementRefreshResponse")
            .field("entitlement", &"[REDACTED]")
            .field("rotated_refresh_session", &"[REDACTED]")
            .field("server_unix", &self.server_unix)
            .field("device_state", &self.device_state)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementStatusResponse {
    pub device_state: DeviceState,
    pub license_epoch: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredDevice {
    pub device_id: String,
    pub display_name: Option<String>,
    pub platform: String,
    pub state: DeviceState,
    pub last_seen_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredDevicePage {
    pub devices: Vec<RegisteredDevice>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvoiceStatus {
    Open,
    Paid,
    Overdue,
    Void,
    Refunded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentClaimStatus {
    Submitted,
    UnderReview,
    NeedsInformation,
    Verified,
    Rejected,
    Withdrawn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingInvoice {
    pub id: String,
    pub row_version: u64,
    pub account_id: String,
    pub license_id: String,
    pub offer_id: String,
    pub plan: crate::Plan,
    pub plan_revision: u32,
    pub seats: u32,
    pub duration_days: u32,
    pub currency: String,
    pub amount_due: String,
    pub payment_reference: String,
    pub status: InvoiceStatus,
    pub due_at: i64,
    pub paid_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualPaymentMethod {
    pub id: String,
    pub row_version: u64,
    pub name_en: String,
    pub name_zh: String,
    pub instructions_en: String,
    pub instructions_zh: String,
    pub settlement_asset: String,
    pub destination_hint: String,
    pub active: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualPaymentClaim {
    pub id: String,
    pub row_version: u64,
    pub invoice_id: String,
    pub account_id: String,
    pub payment_method_id: String,
    pub external_transaction_id: String,
    pub paid_amount: String,
    pub paid_asset: String,
    pub paid_at: i64,
    pub payer_name: Option<String>,
    pub note: Option<String>,
    pub status: PaymentClaimStatus,
    pub submitted_by: String,
    pub reviewed_by: Option<String>,
    pub review_reason: Option<String>,
    pub submitted_at: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingSummary {
    pub invoices: Vec<BillingInvoice>,
    pub payment_claims: Vec<ManualPaymentClaim>,
    pub payment_methods: Vec<ManualPaymentMethod>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomerPaymentSubmission {
    pub operation_id: String,
    pub invoice_id: String,
    pub payment_method_id: String,
    pub external_transaction_id: String,
    pub paid_amount: String,
    pub paid_asset: String,
    pub paid_at: i64,
    pub payer_name: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRole {
    Owner,
    Admin,
    Billing,
    Operator,
    Viewer,
    Auditor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMemberStatus {
    Invited,
    Active,
    Suspended,
    Removed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMember {
    pub id: String,
    pub email: String,
    pub display_name: String,
    pub role: WorkspaceRole,
    pub status: WorkspaceMemberStatus,
    pub bound_device_count: u32,
    pub row_version: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeamMemberPageRequest {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamMemberPage {
    pub members: Vec<WorkspaceMember>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamProfile {
    pub enabled: bool,
    pub member: Option<WorkspaceMember>,
    pub permissions: Vec<String>,
    pub member_limit: u32,
    pub member_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateTeamInvitation {
    pub operation_id: String,
    pub email: String,
    pub display_name: String,
    pub role: WorkspaceRole,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamInvitation {
    pub id: String,
    pub member: WorkspaceMember,
    pub invitation_token: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcceptTeamInvitation {
    pub operation_id: String,
    pub invitation_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateWorkspaceMember {
    pub operation_id: String,
    pub role: WorkspaceRole,
    pub status: WorkspaceMemberStatus,
    pub row_version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberDeviceEnrollment {
    pub id: String,
    pub member_id: String,
    pub enrollment_token: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcceptMemberDeviceEnrollment {
    pub operation_id: String,
    pub enrollment_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeamOperationRequest {
    pub operation_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamOperationStatusResponse {
    pub committed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LeaveWorkspace {
    pub operation_id: String,
    pub member_id: String,
    pub row_version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferWorkspaceOwnership {
    pub operation_id: String,
    pub new_owner_member_id: String,
    pub owner_row_version: u64,
    pub new_owner_row_version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnershipTransferResult {
    pub previous_owner: WorkspaceMember,
    pub new_owner: WorkspaceMember,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SharedProgramKind {
    Generic,
    SingBox,
    Xray,
    Mihomo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateSharedConfiguration {
    pub name: String,
    pub program_kind: SharedProgramKind,
    pub input: String,
    pub content: String,
    pub operation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviseSharedConfiguration {
    pub base_row_version: u64,
    pub name: String,
    pub program_kind: SharedProgramKind,
    pub input: String,
    pub content: String,
    pub operation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishSharedConfiguration {
    pub base_row_version: u64,
    pub revision: Option<u64>,
    pub operation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VersionedWorkspaceMutation {
    pub base_row_version: u64,
    pub operation_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedConfigurationPageRequest {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
    pub include_deleted: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedConfigurationContentRequest {
    pub revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceStorageUsage {
    pub active_document_count: u32,
    pub max_active_documents: u32,
    pub revision_plaintext_bytes: u64,
    pub max_revision_plaintext_bytes: u64,
    pub row_version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedConfigurationSummary {
    pub id: String,
    pub name: String,
    pub program_kind: SharedProgramKind,
    pub row_version: u64,
    pub draft_revision: u64,
    pub published_revision: Option<u64>,
    pub deleted_at: Option<i64>,
    pub content_sha256: String,
    pub plaintext_bytes: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedConfigurationContent {
    #[serde(flatten)]
    pub configuration: SharedConfigurationSummary,
    pub revision: u64,
    pub input: String,
    pub content: String,
    pub revision_created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedConfigurationPage {
    pub configurations: Vec<SharedConfigurationSummary>,
    pub usage: WorkspaceStorageUsage,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMutationReceipt {
    pub resource_type: String,
    pub resource_id: String,
    pub row_version: u64,
    pub cursor: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceAlertEventKind {
    SyncConflict,
    QuotaWarning,
    ConfigurationCreated,
    ConfigurationRevised,
    ConfigurationPublished,
    ConfigurationDeleted,
    ConfigurationRestored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceAlertSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceIncidentStatus {
    Open,
    Acknowledged,
    Resolved,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateWorkspaceAlertRule {
    pub name: String,
    pub event_kind: WorkspaceAlertEventKind,
    pub severity: WorkspaceAlertSeverity,
    pub enabled: bool,
    pub operation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateWorkspaceAlertRule {
    pub base_row_version: u64,
    pub name: String,
    pub event_kind: WorkspaceAlertEventKind,
    pub severity: WorkspaceAlertSeverity,
    pub enabled: bool,
    pub operation_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceAlertRulePageRequest {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceAlertRule {
    pub id: String,
    pub name: String,
    pub event_kind: WorkspaceAlertEventKind,
    pub severity: WorkspaceAlertSeverity,
    pub enabled: bool,
    pub row_version: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceAlertRulePage {
    pub rules: Vec<WorkspaceAlertRule>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceIncidentPageRequest {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
    pub status: Option<WorkspaceIncidentStatus>,
    pub event_kind: Option<WorkspaceAlertEventKind>,
    pub severity: Option<WorkspaceAlertSeverity>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceAlertIncident {
    pub id: String,
    pub rule_id: String,
    pub event_kind: WorkspaceAlertEventKind,
    pub severity: WorkspaceAlertSeverity,
    pub status: WorkspaceIncidentStatus,
    pub summary: String,
    pub metadata: BTreeMap<String, String>,
    pub row_version: u64,
    pub occurred_at: i64,
    pub acknowledged_at: Option<i64>,
    pub resolved_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceIncidentPage {
    pub incidents: Vec<WorkspaceAlertIncident>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceSyncFeedRequest {
    pub cursor: Option<u64>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSyncChange {
    pub cursor: u64,
    pub operation_id: String,
    pub change_kind: String,
    pub resource_type: String,
    pub resource_id: String,
    pub row_version: u64,
    pub occurred_at: i64,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSyncFeed {
    pub changes: Vec<WorkspaceSyncChange>,
    pub next_cursor: u64,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdvanceWorkspaceCheckpoint {
    pub cursor: u64,
    pub base_row_version: u64,
    pub operation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDeviceCheckpoint {
    pub cursor: u64,
    pub row_version: u64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceAuditPageRequest {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
    pub event_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceAuditEvent {
    pub id: String,
    pub event_type: String,
    pub outcome: String,
    pub occurred_at: i64,
    pub device_id: Option<String>,
    pub reason_code: Option<String>,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceAuditPage {
    pub events: Vec<WorkspaceAuditEvent>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceAuditEventTypes {
    pub event_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceAuditExport {
    pub events: Vec<WorkspaceAuditEvent>,
    pub next_cursor: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceWebhookEndpoint {
    pub id: String,
    pub name: String,
    pub url: String,
    pub event_types: Vec<String>,
    pub active: bool,
    pub secret_version: u32,
    pub row_version: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateWorkspaceWebhookEndpoint {
    pub operation_id: String,
    pub name: String,
    pub url: String,
    pub event_types: Vec<String>,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateWorkspaceWebhookEndpoint {
    pub operation_id: String,
    pub row_version: u64,
    pub name: String,
    pub url: String,
    pub event_types: Vec<String>,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RotateWorkspaceWebhookSecret {
    pub operation_id: String,
    pub row_version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteWorkspaceWebhookEndpoint {
    pub operation_id: String,
    pub row_version: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceWebhookSecretResult {
    pub endpoint: WorkspaceWebhookEndpoint,
    pub secret: Option<SecretValue>,
}

impl std::fmt::Debug for WorkspaceWebhookSecretResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspaceWebhookSecretResult")
            .field("endpoint", &self.endpoint)
            .field("secret", &self.secret.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceWebhookDeletion {
    pub endpoint_id: String,
    pub deleted_at: i64,
    pub row_version: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceWebhookDeliveryStatus {
    Pending,
    InFlight,
    Delivered,
    Retry,
    Dead,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceWebhookDelivery {
    pub id: String,
    pub event_id: String,
    pub endpoint_id: String,
    pub event_type: String,
    pub status: WorkspaceWebhookDeliveryStatus,
    pub attempt_count: u32,
    pub next_attempt_at: i64,
    pub last_http_status: Option<u16>,
    pub last_error_category: Option<String>,
    pub delivered_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[async_trait]
pub trait LicenseApi: Send + Sync {
    async fn register_device(
        &self,
        request: DeviceRegistrationRequest,
    ) -> Result<DeviceRegistrationResponse>;
    async fn confirm_activation(
        &self,
        session: &RefreshSession,
    ) -> Result<DeviceActivationConfirmationResponse>;
    async fn verify_activation(
        &self,
        session: &RefreshSession,
        request: ActivationVerificationRequest,
    ) -> Result<ActivationVerificationResponse>;
    async fn issue_challenge(
        &self,
        session: &RefreshSession,
        request: ChallengeRequest,
    ) -> Result<DeviceChallenge>;
    async fn issue_session_recovery_challenge(
        &self,
        request: SessionRecoveryChallengeRequest,
    ) -> Result<DeviceChallenge>;
    async fn recover_session(
        &self,
        request: SessionRecoveryRequest,
    ) -> Result<SessionRecoveryResponse>;
    async fn refresh_entitlement(
        &self,
        session: &RefreshSession,
        request: EntitlementRefreshRequest,
    ) -> Result<EntitlementRefreshResponse>;
    async fn entitlement_status(
        &self,
        session: &RefreshSession,
        request: EntitlementStatusRequest,
    ) -> Result<EntitlementStatusResponse>;
    async fn list_devices(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        cursor: Option<&str>,
        page_size: u32,
    ) -> Result<RegisteredDevicePage>;
    async fn remove_device(
        &self,
        session: &RefreshSession,
        device_id: &str,
        request: DeviceRemovalRequest,
    ) -> Result<()>;
    async fn device_removal_status(
        &self,
        _session: &RefreshSession,
        _device_id: &str,
        _operation_id: &str,
    ) -> Result<DeviceRemovalStatusResponse> {
        Ok(DeviceRemovalStatusResponse { committed: false })
    }
    async fn billing_summary(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
    ) -> Result<BillingSummary>;
    async fn submit_customer_payment(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        submission: CustomerPaymentSubmission,
    ) -> Result<ManualPaymentClaim>;
    async fn team_profile(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
    ) -> Result<TeamProfile>;
    async fn team_leave_operation_status(
        &self,
        session: &RefreshSession,
        request: &LeaveWorkspace,
    ) -> Result<TeamOperationStatusResponse>;
    async fn team_members(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: TeamMemberPageRequest,
    ) -> Result<TeamMemberPage>;
    async fn create_team_invitation(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: CreateTeamInvitation,
    ) -> Result<TeamInvitation>;
    async fn accept_team_invitation(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: AcceptTeamInvitation,
    ) -> Result<TeamProfile>;
    async fn update_team_member(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        member_id: &str,
        request: UpdateWorkspaceMember,
    ) -> Result<WorkspaceMember>;
    async fn create_team_device_enrollment(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: TeamOperationRequest,
    ) -> Result<MemberDeviceEnrollment>;
    async fn create_team_member_device_enrollment(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _member_id: &str,
        _request: TeamOperationRequest,
    ) -> Result<MemberDeviceEnrollment> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn accept_team_device_enrollment(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: AcceptMemberDeviceEnrollment,
    ) -> Result<TeamProfile>;
    async fn leave_team_workspace(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: LeaveWorkspace,
    ) -> Result<()>;
    async fn transfer_team_ownership(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: TransferWorkspaceOwnership,
    ) -> Result<OwnershipTransferResult>;
    async fn shared_configurations(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _request: SharedConfigurationPageRequest,
    ) -> Result<SharedConfigurationPage> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn shared_configuration_content(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _document_id: &str,
        _request: SharedConfigurationContentRequest,
    ) -> Result<SharedConfigurationContent> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn create_shared_configuration(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _request: CreateSharedConfiguration,
    ) -> Result<WorkspaceMutationReceipt> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn revise_shared_configuration(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _document_id: &str,
        _request: ReviseSharedConfiguration,
    ) -> Result<WorkspaceMutationReceipt> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn publish_shared_configuration(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _document_id: &str,
        _request: PublishSharedConfiguration,
    ) -> Result<WorkspaceMutationReceipt> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn delete_shared_configuration(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _document_id: &str,
        _request: VersionedWorkspaceMutation,
    ) -> Result<WorkspaceMutationReceipt> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn restore_shared_configuration(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _document_id: &str,
        _request: VersionedWorkspaceMutation,
    ) -> Result<WorkspaceMutationReceipt> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn purge_shared_configuration(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _document_id: &str,
        _request: VersionedWorkspaceMutation,
    ) -> Result<WorkspaceMutationReceipt> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn workspace_sync_feed(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _request: WorkspaceSyncFeedRequest,
    ) -> Result<WorkspaceSyncFeed> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn workspace_checkpoint(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
    ) -> Result<Option<WorkspaceDeviceCheckpoint>> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn advance_workspace_checkpoint(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _request: AdvanceWorkspaceCheckpoint,
    ) -> Result<WorkspaceMutationReceipt> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn workspace_alert_rules(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _request: WorkspaceAlertRulePageRequest,
    ) -> Result<WorkspaceAlertRulePage> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn create_workspace_alert_rule(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _request: CreateWorkspaceAlertRule,
    ) -> Result<WorkspaceMutationReceipt> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn update_workspace_alert_rule(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _rule_id: &str,
        _request: UpdateWorkspaceAlertRule,
    ) -> Result<WorkspaceMutationReceipt> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn delete_workspace_alert_rule(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _rule_id: &str,
        _request: VersionedWorkspaceMutation,
    ) -> Result<WorkspaceMutationReceipt> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn workspace_alert_incidents(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _request: WorkspaceIncidentPageRequest,
    ) -> Result<WorkspaceIncidentPage> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn acknowledge_workspace_alert_incident(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _incident_id: &str,
        _request: VersionedWorkspaceMutation,
    ) -> Result<WorkspaceMutationReceipt> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn resolve_workspace_alert_incident(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _incident_id: &str,
        _request: VersionedWorkspaceMutation,
    ) -> Result<WorkspaceMutationReceipt> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn workspace_audit_events(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _request: WorkspaceAuditPageRequest,
    ) -> Result<WorkspaceAuditPage> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn workspace_audit_event_types(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
    ) -> Result<WorkspaceAuditEventTypes> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn export_workspace_audit_events(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _request: WorkspaceAuditPageRequest,
    ) -> Result<WorkspaceAuditExport> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn workspace_webhook_endpoints(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
    ) -> Result<Vec<WorkspaceWebhookEndpoint>> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn create_workspace_webhook_endpoint(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _request: CreateWorkspaceWebhookEndpoint,
    ) -> Result<WorkspaceWebhookSecretResult> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn update_workspace_webhook_endpoint(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _endpoint_id: &str,
        _request: UpdateWorkspaceWebhookEndpoint,
    ) -> Result<WorkspaceWebhookEndpoint> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn rotate_workspace_webhook_secret(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _endpoint_id: &str,
        _request: RotateWorkspaceWebhookSecret,
    ) -> Result<WorkspaceWebhookSecretResult> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn delete_workspace_webhook_endpoint(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _endpoint_id: &str,
        _request: DeleteWorkspaceWebhookEndpoint,
    ) -> Result<WorkspaceWebhookDeletion> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn workspace_webhook_deliveries(
        &self,
        _session: &RefreshSession,
        _proof: &DeviceProof,
        _endpoint_id: Option<&str>,
        _limit: u16,
    ) -> Result<Vec<WorkspaceWebhookDelivery>> {
        Err(LicensingError::ServiceUnconfigured)
    }
    async fn logout(&self, session: RefreshSession) -> Result<()>;
}

#[derive(Clone)]
pub struct HttpLicenseApi {
    base_url: Url,
    client: reqwest::Client,
}

impl HttpLicenseApi {
    pub fn new(mut base_url: Url) -> Result<Self> {
        if !(base_url.scheme() == "https" || is_loopback_http(&base_url))
            || base_url.host_str().is_none()
            || base_url.username() != ""
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            return Err(LicensingError::ServiceUnconfigured);
        }
        if !base_url.path().ends_with('/') {
            let path = format!("{}/", base_url.path());
            base_url.set_path(&path);
        }
        let started = std::time::Instant::now();
        let use_system_proxy = std::env::var_os("CAMELLIA_NEXUS_LICENSE_USE_SYSTEM_PROXY")
            .is_some_and(|value| !value.is_empty());
        let mut builder = reqwest::Client::builder()
            .https_only(!is_loopback_http(&base_url))
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(15))
            .connect_timeout(std::time::Duration::from_secs(10));
        if !use_system_proxy {
            builder = builder.no_proxy();
        }
        let client = builder.build().map_err(|_| LicensingError::Network)?;
        tracing::debug!(
            elapsed_ms = started.elapsed().as_millis(),
            system_proxy = use_system_proxy,
            "license HTTP client initialized"
        );
        Ok(Self { base_url, client })
    }

    fn endpoint(&self, path: &str) -> Result<Url> {
        self.base_url
            .join(path)
            .map_err(|_| LicensingError::ServiceUnconfigured)
    }

    async fn response<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
        Self::response_with_limit(response, MAX_RESPONSE_BYTES).await
    }

    async fn response_with_limit<T: DeserializeOwned>(
        response: reqwest::Response,
        maximum_bytes: u64,
    ) -> Result<T> {
        let status = response.status();
        tracing::debug!(%status, "license service response received");
        if !status.is_success() {
            return Err(Self::response_error(status, response).await?);
        }
        if response
            .content_length()
            .is_some_and(|size| size > maximum_bytes)
        {
            return Err(LicensingError::InvalidServerResponse);
        }
        let bytes = bounded_body(response, maximum_bytes).await?;
        serde_json::from_slice(&bytes).map_err(|_| LicensingError::InvalidServerResponse)
    }

    async fn response_empty(response: reqwest::Response) -> Result<()> {
        let status = response.status();
        tracing::debug!(%status, "license service response received");
        if status.is_success() {
            return Ok(());
        }
        Err(Self::response_error(status, response).await?)
    }

    async fn response_error(
        status: reqwest::StatusCode,
        response: reqwest::Response,
    ) -> Result<LicensingError> {
        let retry_after_seconds = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map(|seconds| seconds.min(60 * 60));
        let bytes = bounded_body(response, MAX_RESPONSE_BYTES).await?;
        if let Ok(error) = serde_json::from_slice::<LicenseServiceError>(&bytes) {
            return Ok(service_error_with_retry_after(
                status,
                &error.code,
                error.client_version_policy,
                retry_after_seconds,
            ));
        }
        Ok(if status.is_server_error() {
            LicensingError::Network
        } else {
            LicensingError::InvalidServerResponse
        })
    }

    fn authenticated(
        &self,
        session: &RefreshSession,
        method: reqwest::Method,
        url: Url,
    ) -> reqwest::RequestBuilder {
        self.client
            .request(method, url)
            .bearer_auth(&session.0)
            .header(reqwest::header::ACCEPT, "application/json")
    }

    fn device_authenticated(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        method: reqwest::Method,
        url: Url,
    ) -> Result<reqwest::RequestBuilder> {
        let encoded = URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(proof).map_err(|_| LicensingError::InvalidServerResponse)?);
        Ok(self
            .authenticated(session, method, url)
            .header(DEVICE_PROOF_HEADER, encoded))
    }
}

fn validated_page_limit(limit: Option<u32>, maximum: u32) -> Result<Option<u32>> {
    if limit.is_some_and(|limit| limit == 0 || limit > maximum) {
        return Err(LicensingError::InvalidRequest);
    }
    Ok(limit)
}

fn append_keyset_page_query(
    url: &mut Url,
    cursor: Option<&str>,
    limit: Option<u32>,
    maximum: u32,
) -> Result<()> {
    if cursor.is_some_and(|cursor| cursor.is_empty() || cursor.len() > 1_024) {
        return Err(LicensingError::InvalidRequest);
    }
    let mut query = url.query_pairs_mut();
    if let Some(cursor) = cursor {
        query.append_pair("cursor", cursor);
    }
    if let Some(limit) = validated_page_limit(limit, maximum)? {
        query.append_pair("limit", &limit.to_string());
    }
    Ok(())
}

fn append_audit_query(
    url: &mut Url,
    request: &WorkspaceAuditPageRequest,
    maximum: u32,
) -> Result<()> {
    append_keyset_page_query(url, request.cursor.as_deref(), request.limit, maximum)?;
    if let Some(event_type) = request.event_type.as_deref() {
        if event_type.is_empty()
            || event_type.len() > 128
            || !event_type.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'_' | b'-' | b'.')
            })
        {
            return Err(LicensingError::InvalidRequest);
        }
        url.query_pairs_mut().append_pair("eventType", event_type);
    }
    Ok(())
}

fn validate_shared_configuration_input(name: &str, input: &str, content: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty()
        || name.len() > 160
        || name.chars().any(char::is_control)
        || input.len() > 4 * 1_024
        || content.len() > 4 * 1_024 * 1_024
    {
        return Err(LicensingError::InvalidRequest);
    }
    Ok(())
}

const fn workspace_incident_status_name(status: WorkspaceIncidentStatus) -> &'static str {
    match status {
        WorkspaceIncidentStatus::Open => "open",
        WorkspaceIncidentStatus::Acknowledged => "acknowledged",
        WorkspaceIncidentStatus::Resolved => "resolved",
    }
}

const fn workspace_alert_event_kind_name(kind: WorkspaceAlertEventKind) -> &'static str {
    match kind {
        WorkspaceAlertEventKind::SyncConflict => "sync_conflict",
        WorkspaceAlertEventKind::QuotaWarning => "quota_warning",
        WorkspaceAlertEventKind::ConfigurationCreated => "configuration_created",
        WorkspaceAlertEventKind::ConfigurationRevised => "configuration_revised",
        WorkspaceAlertEventKind::ConfigurationPublished => "configuration_published",
        WorkspaceAlertEventKind::ConfigurationDeleted => "configuration_deleted",
        WorkspaceAlertEventKind::ConfigurationRestored => "configuration_restored",
    }
}

const fn workspace_alert_severity_name(severity: WorkspaceAlertSeverity) -> &'static str {
    match severity {
        WorkspaceAlertSeverity::Info => "info",
        WorkspaceAlertSeverity::Warning => "warning",
        WorkspaceAlertSeverity::Critical => "critical",
    }
}

fn request_error(error: reqwest::Error) -> LicensingError {
    if error.is_timeout() {
        LicensingError::Timeout
    } else {
        LicensingError::Network
    }
}

fn service_error_with_retry_after(
    status: reqwest::StatusCode,
    code: &str,
    client_version_policy: Option<crate::ClientVersionPolicy>,
    retry_after_seconds: Option<u64>,
) -> LicensingError {
    let mapped = service_error(status, code, client_version_policy);
    if matches!(mapped, LicensingError::TooManyRequests { .. }) {
        LicensingError::TooManyRequests {
            retry_after_seconds,
        }
    } else {
        mapped
    }
}

fn service_error(
    status: reqwest::StatusCode,
    code: &str,
    client_version_policy: Option<crate::ClientVersionPolicy>,
) -> LicensingError {
    use reqwest::StatusCode;

    match (status, code) {
        (StatusCode::UNAUTHORIZED, "authorization_required") => {
            LicensingError::AuthorizationRequired
        }
        (StatusCode::UNAUTHORIZED, "refresh_token_reuse") => LicensingError::RefreshSessionReused,
        (StatusCode::UNAUTHORIZED, "activation_code_invalid") => {
            LicensingError::ActivationCodeInvalid
        }
        (StatusCode::UNAUTHORIZED, "activation_code_expired") => {
            LicensingError::ActivationCodeExpired
        }
        (StatusCode::UNAUTHORIZED, "activation_code_consumed") => {
            LicensingError::ActivationCodeConsumed
        }
        (StatusCode::UNAUTHORIZED, "activation_code_revoked") => {
            LicensingError::ActivationCodeRevoked
        }
        (StatusCode::CONFLICT, "activation_pending_expired") => {
            LicensingError::ActivationPendingExpired
        }
        (StatusCode::CONFLICT, "activation_limit") => LicensingError::ActivationLimitReached,
        (StatusCode::CONFLICT, "workspace_version_conflict") => {
            LicensingError::WorkspaceVersionConflict
        }
        (StatusCode::CONFLICT, "workspace_quota_exceeded") => {
            LicensingError::WorkspaceQuotaExceeded
        }
        (StatusCode::CONFLICT, "workspace_document_limit_reached") => {
            LicensingError::WorkspaceDocumentLimitReached
        }
        (StatusCode::CONFLICT, "workspace_alert_rule_limit_reached") => {
            LicensingError::WorkspaceAlertRuleLimitReached
        }
        (StatusCode::CONFLICT, "workspace_retention_active") => {
            LicensingError::WorkspaceRetentionActive
        }
        (StatusCode::CONFLICT, "webhook_endpoint_limit") => {
            LicensingError::WebhookEndpointLimitReached
        }
        (StatusCode::CONFLICT, "idempotency_conflict") => LicensingError::IdempotencyConflict,
        (StatusCode::NOT_FOUND, "workspace_not_found") => LicensingError::WorkspaceNotFound,
        (StatusCode::NOT_FOUND, "webhook_not_found") => LicensingError::WebhookNotFound,
        (StatusCode::TOO_MANY_REQUESTS, "too_many_requests") => LicensingError::TooManyRequests {
            retry_after_seconds: None,
        },
        (StatusCode::FORBIDDEN, "device_denied") => LicensingError::DeviceDenied,
        (StatusCode::FORBIDDEN, "device_removed") => LicensingError::DeviceRemoved,
        (StatusCode::FORBIDDEN, "device_activation_pending") => {
            LicensingError::DeviceActivationPending
        }
        (StatusCode::FORBIDDEN, "device_state_conflict") => LicensingError::AuthorizationRequired,
        (StatusCode::FORBIDDEN, "device_revoked") => LicensingError::DeviceRevoked,
        (StatusCode::FORBIDDEN, "device_suspicious") => LicensingError::DeviceSuspicious,
        (StatusCode::FORBIDDEN, "account_suspended") => LicensingError::AccountSuspended,
        (StatusCode::FORBIDDEN, "account_denylisted") => LicensingError::AccountDenylisted,
        (StatusCode::FORBIDDEN, "permission_denied") => LicensingError::PermissionDenied,
        (StatusCode::FORBIDDEN, "capability_denied") => LicensingError::CapabilityDenied,
        (StatusCode::PAYMENT_REQUIRED, "license_past_due") => LicensingError::LicensePastDue,
        (StatusCode::PAYMENT_REQUIRED, "license_canceled") => LicensingError::LicenseCanceled,
        (StatusCode::PAYMENT_REQUIRED, "license_expired") => LicensingError::LicenseExpired,
        (StatusCode::PAYMENT_REQUIRED, "license_unavailable") => LicensingError::LicenseUnavailable,
        (StatusCode::UPGRADE_REQUIRED, "client_upgrade_required") => match client_version_policy {
            Some(policy) if crate::validate_client_version_policy(&policy).is_ok() => {
                LicensingError::ClientUpgradeRequired { policy }
            }
            _ => LicensingError::InvalidServerResponse,
        },
        (StatusCode::BAD_REQUEST, "invalid_request") => LicensingError::InvalidRequest,
        (StatusCode::BAD_REQUEST, "team_invitation_invalid") => {
            LicensingError::TeamInvitationInvalid
        }
        (StatusCode::BAD_REQUEST, "team_device_enrollment_invalid") => {
            LicensingError::TeamDeviceEnrollmentInvalid
        }
        (StatusCode::BAD_REQUEST, "webhook_invalid_url") => LicensingError::WebhookInvalidUrl,
        (StatusCode::BAD_REQUEST, "invalid_challenge") => LicensingError::InvalidChallenge,
        (StatusCode::BAD_REQUEST, "challenge_replay") => LicensingError::ChallengeReplay,
        (StatusCode::PAYLOAD_TOO_LARGE, "request_too_large") => LicensingError::RequestTooLarge,
        (StatusCode::SERVICE_UNAVAILABLE, "workspace_key_unavailable") => {
            LicensingError::WorkspaceKeyUnavailable
        }
        (StatusCode::SERVICE_UNAVAILABLE, "webhook_keyring_unavailable") => {
            LicensingError::WebhookKeyUnavailable
        }
        (StatusCode::INTERNAL_SERVER_ERROR, "workspace_integrity_failed") => {
            LicensingError::WorkspaceIntegrity
        }
        _ if status.is_server_error() => LicensingError::Network,
        _ => LicensingError::InvalidServerResponse,
    }
}

fn is_loopback_http(url: &Url) -> bool {
    url.scheme() == "http"
        && matches!(
            url.host_str(),
            Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
        )
}

#[async_trait]
impl LicenseApi for HttpLicenseApi {
    async fn register_device(
        &self,
        request: DeviceRegistrationRequest,
    ) -> Result<DeviceRegistrationResponse> {
        let url = self.endpoint("v1/devices/register")?;
        tracing::info!("registering device with license service");
        let response = self
            .client
            .post(url)
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn team_leave_operation_status(
        &self,
        session: &RefreshSession,
        request: &LeaveWorkspace,
    ) -> Result<TeamOperationStatusResponse> {
        let operation_id = request.operation_id.as_str();
        validate_operation_id(operation_id)?;
        let member_id = canonical_resource_id(&request.member_id, "member_")?;
        if request.row_version == 0 {
            return Err(LicensingError::InvalidRequest);
        }
        let mut url = self.endpoint(&format!("v1/team/operations/{operation_id}"))?;
        url.query_pairs_mut()
            .append_pair("memberId", &member_id)
            .append_pair("rowVersion", &request.row_version.to_string());
        let response = self
            .authenticated(session, reqwest::Method::GET, url)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn shared_configurations(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: SharedConfigurationPageRequest,
    ) -> Result<SharedConfigurationPage> {
        let mut url = self.endpoint("v1/workspace/configurations")?;
        append_keyset_page_query(&mut url, request.cursor.as_deref(), request.limit, 200)?;
        url.query_pairs_mut()
            .append_pair("includeDeleted", &request.include_deleted.to_string());
        let response = self
            .device_authenticated(session, proof, reqwest::Method::GET, url)?
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn shared_configuration_content(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        document_id: &str,
        request: SharedConfigurationContentRequest,
    ) -> Result<SharedConfigurationContent> {
        let document_id = canonical_resource_id(document_id, "shared_config_")?;
        let mut url = self.endpoint(&format!("v1/workspace/configurations/{document_id}"))?;
        if let Some(revision) = request.revision {
            if revision == 0 {
                return Err(LicensingError::InvalidRequest);
            }
            url.query_pairs_mut()
                .append_pair("revision", &revision.to_string());
        }
        let response = self
            .device_authenticated(session, proof, reqwest::Method::GET, url)?
            .send()
            .await
            .map_err(request_error)?;
        Self::response_with_limit(response, MAX_CONFIGURATION_RESPONSE_BYTES).await
    }

    async fn create_shared_configuration(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: CreateSharedConfiguration,
    ) -> Result<WorkspaceMutationReceipt> {
        validate_shared_configuration_input(&request.name, &request.input, &request.content)?;
        let url = self.endpoint("v1/workspace/configurations")?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::POST, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn revise_shared_configuration(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        document_id: &str,
        request: ReviseSharedConfiguration,
    ) -> Result<WorkspaceMutationReceipt> {
        validate_shared_configuration_input(&request.name, &request.input, &request.content)?;
        let document_id = canonical_resource_id(document_id, "shared_config_")?;
        let url = self.endpoint(&format!(
            "v1/workspace/configurations/{document_id}/revisions"
        ))?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::POST, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn publish_shared_configuration(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        document_id: &str,
        request: PublishSharedConfiguration,
    ) -> Result<WorkspaceMutationReceipt> {
        let document_id = canonical_resource_id(document_id, "shared_config_")?;
        let url = self.endpoint(&format!(
            "v1/workspace/configurations/{document_id}/publish"
        ))?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::POST, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn delete_shared_configuration(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        document_id: &str,
        request: VersionedWorkspaceMutation,
    ) -> Result<WorkspaceMutationReceipt> {
        let document_id = canonical_resource_id(document_id, "shared_config_")?;
        let url = self.endpoint(&format!("v1/workspace/configurations/{document_id}"))?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::DELETE, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn restore_shared_configuration(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        document_id: &str,
        request: VersionedWorkspaceMutation,
    ) -> Result<WorkspaceMutationReceipt> {
        let document_id = canonical_resource_id(document_id, "shared_config_")?;
        let url = self.endpoint(&format!(
            "v1/workspace/configurations/{document_id}/restore"
        ))?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::POST, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn purge_shared_configuration(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        document_id: &str,
        request: VersionedWorkspaceMutation,
    ) -> Result<WorkspaceMutationReceipt> {
        let document_id = canonical_resource_id(document_id, "shared_config_")?;
        let url = self.endpoint(&format!("v1/workspace/configurations/{document_id}/purge"))?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::POST, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn workspace_sync_feed(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: WorkspaceSyncFeedRequest,
    ) -> Result<WorkspaceSyncFeed> {
        let mut url = self.endpoint("v1/workspace/sync/changes")?;
        {
            let mut query = url.query_pairs_mut();
            if let Some(cursor) = request.cursor {
                query.append_pair("cursor", &cursor.to_string());
            }
            if let Some(limit) = validated_page_limit(request.limit, 200)? {
                query.append_pair("limit", &limit.to_string());
            }
        }
        let response = self
            .device_authenticated(session, proof, reqwest::Method::GET, url)?
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn workspace_checkpoint(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
    ) -> Result<Option<WorkspaceDeviceCheckpoint>> {
        let url = self.endpoint("v1/workspace/sync/checkpoint")?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::GET, url)?
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn advance_workspace_checkpoint(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: AdvanceWorkspaceCheckpoint,
    ) -> Result<WorkspaceMutationReceipt> {
        let url = self.endpoint("v1/workspace/sync/checkpoint")?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::POST, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn confirm_activation(
        &self,
        session: &RefreshSession,
    ) -> Result<DeviceActivationConfirmationResponse> {
        let url = self.endpoint("v1/activations/confirm")?;
        tracing::info!("confirming license activation with license service");
        let response = self
            .authenticated(session, reqwest::Method::POST, url)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn verify_activation(
        &self,
        session: &RefreshSession,
        request: ActivationVerificationRequest,
    ) -> Result<ActivationVerificationResponse> {
        let url = self.endpoint("v1/activations/verify")?;
        tracing::info!("verifying activation with license service");
        let response = self
            .authenticated(session, reqwest::Method::POST, url)
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn issue_challenge(
        &self,
        session: &RefreshSession,
        request: ChallengeRequest,
    ) -> Result<DeviceChallenge> {
        let url = self.endpoint("v1/entitlements/challenge")?;
        tracing::debug!("requesting license entitlement challenge");
        let response = self
            .authenticated(session, reqwest::Method::POST, url)
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn issue_session_recovery_challenge(
        &self,
        request: SessionRecoveryChallengeRequest,
    ) -> Result<DeviceChallenge> {
        let url = self.endpoint("v1/session/recovery/challenge")?;
        tracing::debug!("requesting license session recovery challenge");
        let response = self
            .client
            .post(url)
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn recover_session(
        &self,
        request: SessionRecoveryRequest,
    ) -> Result<SessionRecoveryResponse> {
        let url = self.endpoint("v1/session/recovery")?;
        tracing::info!("recovering license service session with device proof");
        let response = self
            .client
            .post(url)
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn refresh_entitlement(
        &self,
        session: &RefreshSession,
        request: EntitlementRefreshRequest,
    ) -> Result<EntitlementRefreshResponse> {
        let url = self.endpoint("v1/entitlements/refresh")?;
        tracing::info!("refreshing entitlement with license service");
        let response = self
            .authenticated(session, reqwest::Method::POST, url)
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn entitlement_status(
        &self,
        session: &RefreshSession,
        request: EntitlementStatusRequest,
    ) -> Result<EntitlementStatusResponse> {
        let url = self.endpoint("v1/entitlements/status")?;
        tracing::debug!("checking license entitlement status");
        let response = self
            .authenticated(session, reqwest::Method::POST, url)
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn list_devices(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        cursor: Option<&str>,
        page_size: u32,
    ) -> Result<RegisteredDevicePage> {
        if !(1..=100).contains(&page_size) {
            return Err(LicensingError::InvalidServerResponse);
        }
        let cursor = cursor.map(canonical_device_id).transpose()?;
        let mut url = self.endpoint("v1/devices")?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("pageSize", &page_size.to_string());
            if let Some(cursor) = cursor.as_deref() {
                query.append_pair("cursor", cursor);
            }
        }
        tracing::debug!("listing registered license devices");
        let response = self
            .device_authenticated(session, proof, reqwest::Method::GET, url)?
            .send()
            .await
            .map_err(request_error)?;
        let page: RegisteredDevicePage = Self::response(response).await?;
        validate_registered_device_page(page, cursor.as_deref(), page_size)
    }

    async fn remove_device(
        &self,
        session: &RefreshSession,
        device_id: &str,
        request: DeviceRemovalRequest,
    ) -> Result<()> {
        let device_id = canonical_device_id(device_id)?;
        let path = format!("v1/devices/{device_id}");
        let url = self.endpoint(&path)?;
        tracing::info!(%device_id, "removing license device with license service");
        let response = self
            .authenticated(session, reqwest::Method::DELETE, url)
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response_empty(response).await
    }

    async fn device_removal_status(
        &self,
        session: &RefreshSession,
        device_id: &str,
        operation_id: &str,
    ) -> Result<DeviceRemovalStatusResponse> {
        let device_id = canonical_device_id(device_id)?;
        validate_operation_id(operation_id)?;
        let url = self.endpoint(&format!("v1/devices/{device_id}/removals/{operation_id}"))?;
        let response = self
            .authenticated(session, reqwest::Method::GET, url)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn billing_summary(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
    ) -> Result<BillingSummary> {
        let url = self.endpoint("v1/billing/summary")?;
        tracing::debug!("loading customer billing summary");
        let response = self
            .device_authenticated(session, proof, reqwest::Method::GET, url)?
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn submit_customer_payment(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        submission: CustomerPaymentSubmission,
    ) -> Result<ManualPaymentClaim> {
        let url = self.endpoint("v1/billing/payment-claims")?;
        tracing::info!("submitting manual payment claim to license service");
        let response = self
            .device_authenticated(session, proof, reqwest::Method::POST, url)?
            .json(&submission)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn team_profile(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
    ) -> Result<TeamProfile> {
        let url = self.endpoint("v1/team/profile")?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::GET, url)?
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn team_members(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: TeamMemberPageRequest,
    ) -> Result<TeamMemberPage> {
        let mut url = self.endpoint("v1/team/members")?;
        append_keyset_page_query(
            &mut url,
            request.cursor.as_deref(),
            request.limit,
            MAX_TEAM_MEMBER_PAGE_SIZE,
        )?;
        let requested_limit = request.limit.unwrap_or(DEFAULT_TEAM_MEMBER_PAGE_SIZE);
        let response = self
            .device_authenticated(session, proof, reqwest::Method::GET, url)?
            .send()
            .await
            .map_err(request_error)?;
        let page = Self::response(response).await?;
        validate_team_member_page(page, request.cursor.as_deref(), requested_limit)
    }

    async fn create_team_invitation(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: CreateTeamInvitation,
    ) -> Result<TeamInvitation> {
        validate_operation_id(&request.operation_id)?;
        let url = self.endpoint("v1/team/invitations")?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::POST, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn accept_team_invitation(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: AcceptTeamInvitation,
    ) -> Result<TeamProfile> {
        validate_operation_id(&request.operation_id)?;
        let url = self.endpoint("v1/team/invitations/accept")?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::POST, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn update_team_member(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        member_id: &str,
        request: UpdateWorkspaceMember,
    ) -> Result<WorkspaceMember> {
        validate_operation_id(&request.operation_id)?;
        let member_id = canonical_resource_id(member_id, "member_")?;
        let url = self.endpoint(&format!("v1/team/members/{member_id}"))?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::PATCH, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn create_team_device_enrollment(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: TeamOperationRequest,
    ) -> Result<MemberDeviceEnrollment> {
        validate_operation_id(&request.operation_id)?;
        let url = self.endpoint("v1/team/device-enrollments")?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::POST, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn create_team_member_device_enrollment(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        member_id: &str,
        request: TeamOperationRequest,
    ) -> Result<MemberDeviceEnrollment> {
        validate_operation_id(&request.operation_id)?;
        let member_id = canonical_resource_id(member_id, "member_")?;
        let url = self.endpoint(&format!("v1/team/members/{member_id}/device-enrollments"))?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::POST, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn accept_team_device_enrollment(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: AcceptMemberDeviceEnrollment,
    ) -> Result<TeamProfile> {
        validate_operation_id(&request.operation_id)?;
        let url = self.endpoint("v1/team/device-enrollments/accept")?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::POST, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn leave_team_workspace(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: LeaveWorkspace,
    ) -> Result<()> {
        validate_operation_id(&request.operation_id)?;
        let url = self.endpoint("v1/team/leave")?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::POST, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response_empty(response).await
    }

    async fn transfer_team_ownership(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: TransferWorkspaceOwnership,
    ) -> Result<OwnershipTransferResult> {
        validate_operation_id(&request.operation_id)?;
        let url = self.endpoint("v1/team/ownership-transfer")?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::POST, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn workspace_alert_rules(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: WorkspaceAlertRulePageRequest,
    ) -> Result<WorkspaceAlertRulePage> {
        let mut url = self.endpoint("v1/workspace/alerts/rules")?;
        append_keyset_page_query(&mut url, request.cursor.as_deref(), request.limit, 200)?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::GET, url)?
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn create_workspace_alert_rule(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: CreateWorkspaceAlertRule,
    ) -> Result<WorkspaceMutationReceipt> {
        let url = self.endpoint("v1/workspace/alerts/rules")?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::POST, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn update_workspace_alert_rule(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        rule_id: &str,
        request: UpdateWorkspaceAlertRule,
    ) -> Result<WorkspaceMutationReceipt> {
        let rule_id = canonical_resource_id(rule_id, "alert_rule_")?;
        let url = self.endpoint(&format!("v1/workspace/alerts/rules/{rule_id}"))?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::PATCH, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn delete_workspace_alert_rule(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        rule_id: &str,
        request: VersionedWorkspaceMutation,
    ) -> Result<WorkspaceMutationReceipt> {
        let rule_id = canonical_resource_id(rule_id, "alert_rule_")?;
        let url = self.endpoint(&format!("v1/workspace/alerts/rules/{rule_id}"))?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::DELETE, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn workspace_alert_incidents(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: WorkspaceIncidentPageRequest,
    ) -> Result<WorkspaceIncidentPage> {
        let mut url = self.endpoint("v1/workspace/alerts/incidents")?;
        append_keyset_page_query(&mut url, request.cursor.as_deref(), request.limit, 200)?;
        {
            let mut query = url.query_pairs_mut();
            if let Some(status) = request.status {
                query.append_pair("status", workspace_incident_status_name(status));
            }
            if let Some(event_kind) = request.event_kind {
                query.append_pair("eventKind", workspace_alert_event_kind_name(event_kind));
            }
            if let Some(severity) = request.severity {
                query.append_pair("severity", workspace_alert_severity_name(severity));
            }
        }
        let response = self
            .device_authenticated(session, proof, reqwest::Method::GET, url)?
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn acknowledge_workspace_alert_incident(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        incident_id: &str,
        request: VersionedWorkspaceMutation,
    ) -> Result<WorkspaceMutationReceipt> {
        let incident_id = canonical_resource_id(incident_id, "alert_incident_")?;
        let url = self.endpoint(&format!(
            "v1/workspace/alerts/incidents/{incident_id}/acknowledge"
        ))?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::POST, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn resolve_workspace_alert_incident(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        incident_id: &str,
        request: VersionedWorkspaceMutation,
    ) -> Result<WorkspaceMutationReceipt> {
        let incident_id = canonical_resource_id(incident_id, "alert_incident_")?;
        let url = self.endpoint(&format!(
            "v1/workspace/alerts/incidents/{incident_id}/resolve"
        ))?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::POST, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn workspace_audit_events(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: WorkspaceAuditPageRequest,
    ) -> Result<WorkspaceAuditPage> {
        let mut url = self.endpoint("v1/workspace/audit/events")?;
        append_audit_query(&mut url, &request, 200)?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::GET, url)?
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn workspace_audit_event_types(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
    ) -> Result<WorkspaceAuditEventTypes> {
        let url = self.endpoint("v1/workspace/audit/event-types")?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::GET, url)?
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn export_workspace_audit_events(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: WorkspaceAuditPageRequest,
    ) -> Result<WorkspaceAuditExport> {
        let mut url = self.endpoint("v1/workspace/audit/export")?;
        append_audit_query(&mut url, &request, 5_000)?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::GET, url)?
            .send()
            .await
            .map_err(request_error)?;
        Self::response_with_limit(response, MAX_AUDIT_EXPORT_RESPONSE_BYTES).await
    }

    async fn workspace_webhook_endpoints(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
    ) -> Result<Vec<WorkspaceWebhookEndpoint>> {
        let url = self.endpoint("v1/workspace/webhooks")?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::GET, url)?
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn create_workspace_webhook_endpoint(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        request: CreateWorkspaceWebhookEndpoint,
    ) -> Result<WorkspaceWebhookSecretResult> {
        let url = self.endpoint("v1/workspace/webhooks")?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::POST, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn update_workspace_webhook_endpoint(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        endpoint_id: &str,
        request: UpdateWorkspaceWebhookEndpoint,
    ) -> Result<WorkspaceWebhookEndpoint> {
        let endpoint_id = canonical_resource_id(endpoint_id, "webhook_endpoint_")?;
        let url = self.endpoint(&format!("v1/workspace/webhooks/{endpoint_id}"))?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::PATCH, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn rotate_workspace_webhook_secret(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        endpoint_id: &str,
        request: RotateWorkspaceWebhookSecret,
    ) -> Result<WorkspaceWebhookSecretResult> {
        let endpoint_id = canonical_resource_id(endpoint_id, "webhook_endpoint_")?;
        let url = self.endpoint(&format!(
            "v1/workspace/webhooks/{endpoint_id}/rotate-secret"
        ))?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::POST, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn delete_workspace_webhook_endpoint(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        endpoint_id: &str,
        request: DeleteWorkspaceWebhookEndpoint,
    ) -> Result<WorkspaceWebhookDeletion> {
        let endpoint_id = canonical_resource_id(endpoint_id, "webhook_endpoint_")?;
        let url = self.endpoint(&format!("v1/workspace/webhooks/{endpoint_id}"))?;
        let response = self
            .device_authenticated(session, proof, reqwest::Method::DELETE, url)?
            .json(&request)
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn workspace_webhook_deliveries(
        &self,
        session: &RefreshSession,
        proof: &DeviceProof,
        endpoint_id: Option<&str>,
        limit: u16,
    ) -> Result<Vec<WorkspaceWebhookDelivery>> {
        if limit == 0 || limit > 100 {
            return Err(LicensingError::InvalidRequest);
        }
        let endpoint_id = endpoint_id
            .map(|value| canonical_resource_id(value, "webhook_endpoint_"))
            .transpose()?;
        let mut url = self.endpoint("v1/workspace/webhooks/deliveries")?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("limit", &limit.to_string());
            if let Some(endpoint_id) = endpoint_id {
                query.append_pair("endpointId", &endpoint_id);
            }
        }
        let response = self
            .device_authenticated(session, proof, reqwest::Method::GET, url)?
            .send()
            .await
            .map_err(request_error)?;
        Self::response(response).await
    }

    async fn logout(&self, session: RefreshSession) -> Result<()> {
        let url = self.endpoint("v1/session/logout")?;
        tracing::info!("ending license service session");
        let response = self
            .authenticated(&session, reqwest::Method::POST, url)
            .send()
            .await
            .map_err(request_error)?;
        Self::response_empty(response).await
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LicenseServiceError {
    code: String,
    #[serde(rename = "message")]
    _message: String,
    #[serde(default)]
    client_version_policy: Option<crate::ClientVersionPolicy>,
}

fn canonical_device_id(value: &str) -> Result<String> {
    let parsed = uuid::Uuid::parse_str(value).map_err(|_| LicensingError::InvalidServerResponse)?;
    let canonical = parsed.hyphenated().to_string();
    if value != canonical {
        return Err(LicensingError::InvalidServerResponse);
    }
    Ok(canonical)
}

pub(crate) fn canonical_resource_id(value: &str, prefix: &str) -> Result<String> {
    if !value.starts_with(prefix)
        || value.len() > 96
        || value
            .bytes()
            .any(|byte| !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'))
    {
        return Err(LicensingError::InvalidRequest);
    }
    Ok(value.to_owned())
}

fn validate_operation_id(value: &str) -> Result<()> {
    let parsed = uuid::Uuid::parse_str(value).map_err(|_| LicensingError::InvalidRequest)?;
    if parsed.get_version() != Some(uuid::Version::Random)
        || parsed.get_variant() != uuid::Variant::RFC4122
        || parsed.hyphenated().to_string() != value
    {
        Err(LicensingError::InvalidRequest)
    } else {
        Ok(())
    }
}

fn validate_registered_device_page(
    page: RegisteredDevicePage,
    cursor: Option<&str>,
    page_size: u32,
) -> Result<RegisteredDevicePage> {
    if page.devices.len() > page_size as usize {
        return Err(LicensingError::InvalidServerResponse);
    }
    let mut previous = cursor.map(str::to_owned);
    for device in &page.devices {
        let canonical = canonical_device_id(&device.device_id)?;
        if previous.as_ref().is_some_and(|value| canonical <= *value) {
            return Err(LicensingError::InvalidServerResponse);
        }
        previous = Some(canonical);
    }
    if let Some(next_cursor) = &page.next_cursor {
        let canonical = canonical_device_id(next_cursor)?;
        if page.devices.len() != page_size as usize
            || page.devices.last().map(|device| device.device_id.as_str())
                != Some(canonical.as_str())
        {
            return Err(LicensingError::InvalidServerResponse);
        }
    }
    Ok(page)
}

fn validate_team_member_page(
    page: TeamMemberPage,
    requested_cursor: Option<&str>,
    requested_limit: u32,
) -> Result<TeamMemberPage> {
    if page.members.len() > requested_limit as usize
        || page.has_more != page.next_cursor.is_some()
        || (page.has_more && page.members.is_empty())
        || page.next_cursor.as_ref().is_some_and(|cursor| {
            cursor.is_empty()
                || cursor.len() > 1_024
                || requested_cursor.is_some_and(|requested| requested == cursor)
        })
    {
        return Err(LicensingError::InvalidServerResponse);
    }
    let mut previous: Option<(i64, &str)> = None;
    for member in &page.members {
        let valid_id = member.id.starts_with("member_")
            && member.id.len() <= 96
            && member
                .id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
        let valid_identity = !member.display_name.trim().is_empty()
            && member.display_name.len() <= 160
            && !member.display_name.chars().any(char::is_control)
            && !member.email.trim().is_empty()
            && member.email.len() <= 254
            && !member.email.chars().any(char::is_control);
        let position = (member.created_at, member.id.as_str());
        if !valid_id
            || !valid_identity
            || member.row_version == 0
            || member.created_at < 0
            || member.updated_at < member.created_at
            || previous.is_some_and(|value| position <= value)
        {
            return Err(LicensingError::InvalidServerResponse);
        }
        previous = Some(position);
    }
    Ok(page)
}

async fn bounded_body(mut response: reqwest::Response, maximum_bytes: u64) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(request_error)? {
        if body.len().saturating_add(chunk.len()) > maximum_bytes as usize {
            return Err(LicensingError::InvalidServerResponse);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_path_identifiers_are_canonical_request_input() {
        assert_eq!(
            canonical_resource_id("shared_config_abc123", "shared_config_").unwrap(),
            "shared_config_abc123"
        );
        for value in ["shared_config_ABC", "shared_config_a/b", "wrong_abc", ""] {
            assert!(matches!(
                canonical_resource_id(value, "shared_config_"),
                Err(LicensingError::InvalidRequest)
            ));
        }
    }

    fn team_member(id: &str, created_at: i64) -> WorkspaceMember {
        WorkspaceMember {
            id: id.to_owned(),
            email: format!("{id}@example.test"),
            display_name: id.to_owned(),
            role: WorkspaceRole::Viewer,
            status: WorkspaceMemberStatus::Active,
            bound_device_count: 1,
            row_version: 1,
            created_at,
            updated_at: created_at,
        }
    }

    #[test]
    fn team_member_pages_reject_truncation_and_cursor_loops() {
        let page = TeamMemberPage {
            members: vec![
                team_member("member_aaaaaaaa", 1_000),
                team_member("member_bbbbbbbb", 1_001),
            ],
            next_cursor: Some("opaque_next_cursor".to_owned()),
            has_more: true,
        };
        assert!(validate_team_member_page(page, None, 2).is_ok());
        assert!(matches!(
            validate_team_member_page(
                TeamMemberPage {
                    members: vec![team_member("member_aaaaaaaa", 1_000)],
                    next_cursor: Some("same_cursor".to_owned()),
                    has_more: true,
                },
                Some("same_cursor"),
                2,
            ),
            Err(LicensingError::InvalidServerResponse)
        ));
        assert!(matches!(
            validate_team_member_page(
                TeamMemberPage {
                    members: vec![
                        team_member("member_bbbbbbbb", 1_001),
                        team_member("member_aaaaaaaa", 1_000),
                    ],
                    next_cursor: None,
                    has_more: false,
                },
                None,
                2,
            ),
            Err(LicensingError::InvalidServerResponse)
        ));
    }

    #[test]
    fn requires_https_license_service_without_credentials() {
        assert!(matches!(
            HttpLicenseApi::new(Url::parse("http://license.example").unwrap()),
            Err(LicensingError::ServiceUnconfigured)
        ));
        assert!(matches!(
            HttpLicenseApi::new(Url::parse("https://user:pass@license.example").unwrap()),
            Err(LicensingError::ServiceUnconfigured)
        ));
    }

    #[test]
    fn preserves_actionable_activation_code_failures() {
        let status = reqwest::StatusCode::UNAUTHORIZED;
        assert!(matches!(
            service_error(status, "activation_code_invalid", None),
            LicensingError::ActivationCodeInvalid
        ));
        assert!(matches!(
            service_error(status, "activation_code_expired", None),
            LicensingError::ActivationCodeExpired
        ));
        assert!(matches!(
            service_error(status, "activation_code_consumed", None),
            LicensingError::ActivationCodeConsumed
        ));
        assert!(matches!(
            service_error(status, "activation_code_revoked", None),
            LicensingError::ActivationCodeRevoked
        ));
        assert!(matches!(
            service_error(
                reqwest::StatusCode::CONFLICT,
                "activation_pending_expired",
                None,
            ),
            LicensingError::ActivationPendingExpired
        ));
    }

    #[test]
    fn business_errors_require_their_documented_status() {
        use reqwest::StatusCode;

        let policy = crate::ClientVersionPolicy {
            minimum_version: "2.0.0".into(),
            recommended_version: "2.1.0".into(),
            enforce_after: 2_000,
        };
        let cases = vec![
            (
                StatusCode::UNAUTHORIZED,
                "authorization_required",
                None,
                LicensingError::AuthorizationRequired,
            ),
            (
                StatusCode::UNAUTHORIZED,
                "refresh_token_reuse",
                None,
                LicensingError::RefreshSessionReused,
            ),
            (
                StatusCode::UNAUTHORIZED,
                "activation_code_invalid",
                None,
                LicensingError::ActivationCodeInvalid,
            ),
            (
                StatusCode::UNAUTHORIZED,
                "activation_code_expired",
                None,
                LicensingError::ActivationCodeExpired,
            ),
            (
                StatusCode::UNAUTHORIZED,
                "activation_code_consumed",
                None,
                LicensingError::ActivationCodeConsumed,
            ),
            (
                StatusCode::UNAUTHORIZED,
                "activation_code_revoked",
                None,
                LicensingError::ActivationCodeRevoked,
            ),
            (
                StatusCode::CONFLICT,
                "activation_pending_expired",
                None,
                LicensingError::ActivationPendingExpired,
            ),
            (
                StatusCode::CONFLICT,
                "activation_limit",
                None,
                LicensingError::ActivationLimitReached,
            ),
            (
                StatusCode::CONFLICT,
                "workspace_version_conflict",
                None,
                LicensingError::WorkspaceVersionConflict,
            ),
            (
                StatusCode::CONFLICT,
                "workspace_quota_exceeded",
                None,
                LicensingError::WorkspaceQuotaExceeded,
            ),
            (
                StatusCode::CONFLICT,
                "workspace_document_limit_reached",
                None,
                LicensingError::WorkspaceDocumentLimitReached,
            ),
            (
                StatusCode::CONFLICT,
                "workspace_alert_rule_limit_reached",
                None,
                LicensingError::WorkspaceAlertRuleLimitReached,
            ),
            (
                StatusCode::CONFLICT,
                "workspace_retention_active",
                None,
                LicensingError::WorkspaceRetentionActive,
            ),
            (
                StatusCode::CONFLICT,
                "webhook_endpoint_limit",
                None,
                LicensingError::WebhookEndpointLimitReached,
            ),
            (
                StatusCode::CONFLICT,
                "idempotency_conflict",
                None,
                LicensingError::IdempotencyConflict,
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                "too_many_requests",
                None,
                LicensingError::TooManyRequests {
                    retry_after_seconds: None,
                },
            ),
            (
                StatusCode::FORBIDDEN,
                "device_denied",
                None,
                LicensingError::DeviceDenied,
            ),
            (
                StatusCode::FORBIDDEN,
                "device_removed",
                None,
                LicensingError::DeviceRemoved,
            ),
            (
                StatusCode::FORBIDDEN,
                "device_activation_pending",
                None,
                LicensingError::DeviceActivationPending,
            ),
            (
                StatusCode::FORBIDDEN,
                "device_state_conflict",
                None,
                LicensingError::AuthorizationRequired,
            ),
            (
                StatusCode::FORBIDDEN,
                "device_revoked",
                None,
                LicensingError::DeviceRevoked,
            ),
            (
                StatusCode::FORBIDDEN,
                "device_suspicious",
                None,
                LicensingError::DeviceSuspicious,
            ),
            (
                StatusCode::FORBIDDEN,
                "account_suspended",
                None,
                LicensingError::AccountSuspended,
            ),
            (
                StatusCode::FORBIDDEN,
                "account_denylisted",
                None,
                LicensingError::AccountDenylisted,
            ),
            (
                StatusCode::FORBIDDEN,
                "permission_denied",
                None,
                LicensingError::PermissionDenied,
            ),
            (
                StatusCode::FORBIDDEN,
                "capability_denied",
                None,
                LicensingError::CapabilityDenied,
            ),
            (
                StatusCode::PAYMENT_REQUIRED,
                "license_past_due",
                None,
                LicensingError::LicensePastDue,
            ),
            (
                StatusCode::PAYMENT_REQUIRED,
                "license_canceled",
                None,
                LicensingError::LicenseCanceled,
            ),
            (
                StatusCode::PAYMENT_REQUIRED,
                "license_expired",
                None,
                LicensingError::LicenseExpired,
            ),
            (
                StatusCode::PAYMENT_REQUIRED,
                "license_unavailable",
                None,
                LicensingError::LicenseUnavailable,
            ),
            (
                StatusCode::UPGRADE_REQUIRED,
                "client_upgrade_required",
                Some(policy.clone()),
                LicensingError::ClientUpgradeRequired { policy },
            ),
            (
                StatusCode::BAD_REQUEST,
                "invalid_request",
                None,
                LicensingError::InvalidRequest,
            ),
            (
                StatusCode::BAD_REQUEST,
                "team_invitation_invalid",
                None,
                LicensingError::TeamInvitationInvalid,
            ),
            (
                StatusCode::BAD_REQUEST,
                "team_device_enrollment_invalid",
                None,
                LicensingError::TeamDeviceEnrollmentInvalid,
            ),
            (
                StatusCode::BAD_REQUEST,
                "webhook_invalid_url",
                None,
                LicensingError::WebhookInvalidUrl,
            ),
            (
                StatusCode::NOT_FOUND,
                "workspace_not_found",
                None,
                LicensingError::WorkspaceNotFound,
            ),
            (
                StatusCode::NOT_FOUND,
                "webhook_not_found",
                None,
                LicensingError::WebhookNotFound,
            ),
            (
                StatusCode::PAYLOAD_TOO_LARGE,
                "request_too_large",
                None,
                LicensingError::RequestTooLarge,
            ),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "workspace_integrity_failed",
                None,
                LicensingError::WorkspaceIntegrity,
            ),
            (
                StatusCode::BAD_REQUEST,
                "invalid_challenge",
                None,
                LicensingError::InvalidChallenge,
            ),
            (
                StatusCode::BAD_REQUEST,
                "challenge_replay",
                None,
                LicensingError::ChallengeReplay,
            ),
        ];

        for (status, code, policy, expected) in cases {
            let actual = service_error(status, code, policy.clone());
            assert_eq!(
                std::mem::discriminant(&actual),
                std::mem::discriminant(&expected),
                "incorrect mapping for {code}"
            );
            assert!(matches!(
                service_error(StatusCode::TEMPORARY_REDIRECT, code, policy.clone()),
                LicensingError::InvalidServerResponse
            ));
            assert!(matches!(
                service_error(StatusCode::SERVICE_UNAVAILABLE, code, policy),
                LicensingError::Network
            ));
        }
    }

    #[test]
    fn unavailable_workspace_keyrings_keep_actionable_codes() {
        assert!(matches!(
            service_error(
                reqwest::StatusCode::SERVICE_UNAVAILABLE,
                "workspace_key_unavailable",
                None,
            ),
            LicensingError::WorkspaceKeyUnavailable
        ));
        assert!(matches!(
            service_error(
                reqwest::StatusCode::SERVICE_UNAVAILABLE,
                "webhook_keyring_unavailable",
                None,
            ),
            LicensingError::WebhookKeyUnavailable
        ));
    }

    #[test]
    fn json_business_code_cannot_disguise_a_server_failure() {
        let response: LicenseServiceError = serde_json::from_value(serde_json::json!({
            "code": "device_revoked",
            "message": "Revoked."
        }))
        .expect("server error contract");
        assert!(matches!(
            service_error(
                reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                &response.code,
                response.client_version_policy,
            ),
            LicensingError::Network
        ));
    }

    #[test]
    fn error_contract_accepts_additive_extension_fields() {
        let response: LicenseServiceError = serde_json::from_value(serde_json::json!({
            "code": "invalid_request",
            "message": "Invalid request.",
            "requestId": "request_future_extension"
        }))
        .expect("additive fields remain backward compatible");
        assert!(matches!(
            service_error(
                reqwest::StatusCode::BAD_REQUEST,
                &response.code,
                response.client_version_policy,
            ),
            LicensingError::InvalidRequest
        ));
    }

    #[test]
    fn retry_after_is_preserved_only_for_rate_limits() {
        assert!(matches!(
            service_error_with_retry_after(
                reqwest::StatusCode::TOO_MANY_REQUESTS,
                "too_many_requests",
                None,
                Some(120),
            ),
            LicensingError::TooManyRequests {
                retry_after_seconds: Some(120)
            }
        ));
        assert!(matches!(
            service_error_with_retry_after(
                reqwest::StatusCode::FORBIDDEN,
                "device_revoked",
                None,
                Some(120),
            ),
            LicensingError::DeviceRevoked
        ));
    }

    #[test]
    fn only_explicit_session_errors_trigger_authorization_recovery() {
        let unauthorized = reqwest::StatusCode::UNAUTHORIZED;
        assert!(matches!(
            service_error(unauthorized, "authorization_required", None),
            LicensingError::AuthorizationRequired
        ));
        assert!(matches!(
            service_error(unauthorized, "future_client_error", None),
            LicensingError::InvalidServerResponse
        ));
        assert!(matches!(
            service_error(reqwest::StatusCode::FORBIDDEN, "device_denied", None,),
            LicensingError::DeviceDenied
        ));
    }

    #[test]
    fn upgrade_required_needs_a_valid_signed_policy_shape() {
        let policy = crate::ClientVersionPolicy {
            minimum_version: "2.0.0".into(),
            recommended_version: "2.1.0".into(),
            enforce_after: 2_000,
        };
        assert!(matches!(
            service_error(
                reqwest::StatusCode::UPGRADE_REQUIRED,
                "client_upgrade_required",
                Some(policy),
            ),
            LicensingError::ClientUpgradeRequired { .. }
        ));
        assert!(matches!(
            service_error(
                reqwest::StatusCode::UPGRADE_REQUIRED,
                "client_upgrade_required",
                None,
            ),
            LicensingError::InvalidServerResponse
        ));

        let response: LicenseServiceError = serde_json::from_value(serde_json::json!({
            "code": "client_upgrade_required",
            "message": "Upgrade to continue.",
            "clientVersionPolicy": {
                "minimumVersion": "2.0.0",
                "recommendedVersion": "2.1.0",
                "enforceAfter": 2_000
            }
        }))
        .expect("server error contract");
        assert!(matches!(
            service_error(
                reqwest::StatusCode::UPGRADE_REQUIRED,
                &response.code,
                response.client_version_policy,
            ),
            LicensingError::ClientUpgradeRequired { .. }
        ));
    }

    #[test]
    fn device_pages_require_canonical_strictly_ordered_cursors() {
        let first = "00000000-0000-0000-0000-000000000001";
        let second = "00000000-0000-0000-0000-000000000002";
        let page = RegisteredDevicePage {
            devices: [first, second]
                .into_iter()
                .map(|device_id| RegisteredDevice {
                    device_id: device_id.to_owned(),
                    display_name: None,
                    platform: "test".to_owned(),
                    state: DeviceState::Active,
                    last_seen_at: None,
                })
                .collect(),
            next_cursor: Some(second.to_owned()),
        };
        assert!(validate_registered_device_page(page, None, 2).is_ok());

        let invalid = RegisteredDevicePage {
            devices: vec![RegisteredDevice {
                device_id: first.to_owned(),
                display_name: None,
                platform: "test".to_owned(),
                state: DeviceState::Active,
                last_seen_at: None,
            }],
            next_cursor: Some("AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA".to_owned()),
        };
        assert!(matches!(
            validate_registered_device_page(invalid, None, 1),
            Err(LicensingError::InvalidServerResponse)
        ));
    }

    #[test]
    fn shared_program_kind_serializes_mihomo_as_a_peer_contract() {
        let value = serde_json::to_value(SharedProgramKind::Mihomo).expect("serialize Mihomo");
        assert_eq!(value, serde_json::json!("mihomo"));
        assert_eq!(
            serde_json::from_value::<SharedProgramKind>(value).expect("deserialize Mihomo"),
            SharedProgramKind::Mihomo
        );
    }
}

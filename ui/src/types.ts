export type ProgramKind = 'generic' | 'singBox' | 'xray' | 'mihomo';
export type RestartPolicy = 'never' | 'onFailure' | 'always';
export type PrivilegePolicy =
  | { mode: 'standard' }
  | { mode: 'automatic' }
  | { mode: 'elevated' };
export type PrivilegeRequirement = 'standard' | 'elevated' | 'unknown';
export type PrivilegeReason =
  | { code: 'tunInterface' }
  | { code: 'transparentProxy' }
  | { code: 'privilegedPort'; port: number }
  | { code: 'executableManifest' }
  | { code: 'explicitPolicy' }
  | { code: 'configurationUnavailable' };
export interface PrivilegeAssessment {
  detected: PrivilegeRequirement;
  effective: PrivilegeRequirement;
  reasons: PrivilegeReason[];
  authoritative: boolean;
}
export interface InvalidProgram { path: string; error: string }
export type LogRetention = 'preserve' | 'clearOnStart';
export type AppLogLevel = 'error' | 'warn' | 'info' | 'debug' | 'trace';

export interface AppSettings {
  version: number;
  logRetention: LogRetention;
  logLevel: AppLogLevel;
  programStartupDelayMs: 0 | 750 | 2000;
  language?: 'en' | 'zh-CN';
}

export interface ApplicationInfo {
  name: string;
  version: string;
  author: string;
  copyright: string;
  license: string;
  description: string;
  signatureStatus: 'verified' | 'notVerified' | 'notChecked';
}

export type ProgramState =
  | { status: 'stopped' }
  | { status: 'starting' }
  | { status: 'running'; pid: number; startedUnixMs: number }
  | { status: 'stopping' }
  | { status: 'exited'; code: number | null; success: boolean }
  | { status: 'backoff'; attempt: number; delaySeconds: number }
  | { status: 'stopFailed'; pid: number; message: string }
  | { status: 'error'; code: string; message: string };

export interface ExecutableMetadata {
  size: number;
  modifiedUnixMs: number;
  detectedVersion?: string;
}

export type ExecutableSpec =
  | { mode: 'managed'; path: string; metadata?: ExecutableMetadata }
  | { mode: 'external'; path: string; metadata?: ExecutableMetadata };

export type ProgramType =
  | { kind: 'generic'; args: string[] }
  | { kind: 'singBox'; mainConfig?: string; extraArgs: string[] }
  | { kind: 'xray'; mainConfig?: string; extraArgs: string[] }
  | { kind: 'mihomo'; mainConfig?: string; extraArgs: string[] };

export type ConfigSource =
  | { mode: 'local'; id: string; name: string; enabled: boolean; path: string }
  | {
      mode: 'remote';
      id: string;
      name: string;
      enabled: boolean;
      url: string;
      authentication?: {
        scheme: 'basic';
        username: string;
        credentialId?: string;
        password?: string;
      };
    };

export interface RemoteUpdate {
  enabled: boolean;
  intervalMinutes: number;
}

export interface SingBoxDashboard {
  listenPort: number;
  updateInterval: string;
}

export interface SingBoxClashDashboard {
  listenPort: number;
  downloadUrl?: string;
}

export interface XrayDashboard {
  apiPort: number;
  metricsPort: number;
}

export interface MihomoDashboard {
  listenPort: number;
  downloadUrl?: string;
}

export interface ManagedConfig {
  sources: ConfigSource[];
  remoteUpdate?: RemoteUpdate;
  singBoxDashboard?: SingBoxDashboard;
  singBoxClashDashboard?: SingBoxClashDashboard;
  xrayDashboard?: XrayDashboard;
  mihomoDashboard?: MihomoDashboard;
}

export interface ProgramSpec {
  schemaVersion: number;
  id: string;
  name: string;
  executable: ExecutableSpec;
  type: ProgramType;
  managedConfig?: ManagedConfig;
  workingDirectory: string;
  environment: Record<string, string>;
  autoStart: boolean;
  restartPolicy: RestartPolicy;
  privilegePolicy: PrivilegePolicy;
}

export interface ProgramSummary {
  id: string;
  name: string;
  kind: ProgramKind;
  autoStart: boolean;
  state: ProgramState;
}

export interface ProgramDetail {
  spec: ProgramSpec;
  state: ProgramState;
  workingDirectory: string;
}

export interface ConfigDocument {
  content: string;
  baseHash: string;
  language: 'jsonc' | 'yaml' | 'toml' | 'text';
  documentationUrl: string;
  configurationSchema?: ConfigurationSchemaDescriptor;
}

export type ConfigurationSchemaSource = 'programBinary';
export type JsonSchemaDialect = 'draft2020-12';

export interface ConfigurationSchemaDescriptor {
  source: ConfigurationSchemaSource;
  dialect: JsonSchemaDialect;
}

export interface ConfigurationSchemaDocument extends ConfigurationSchemaDescriptor {
  content: string;
  contentHash: string;
}

export interface ConfigUpdateResult {
  sourceCount: number;
  document: ConfigDocument;
}

export interface AutomaticConfigUpdateEvent {
  programId: string;
  succeeded: boolean;
}

export interface ValidationResult {
  valid: boolean;
  stdout: string;
  stderr: string;
}

export interface ActionDescriptor {
  id: string;
  label: string;
  allowedStates: Array<ProgramState['status']>;
  confirmation: boolean;
}

export interface ActionResult {
  stdout: string;
  stderr: string;
  previewContent?: string;
}

export interface LogChunk {
  content: string;
  truncated: boolean;
}

export interface XrayDashboardSnapshot {
  apiUrl: string;
  metricsUrl: string;
  metrics?: unknown;
  metricsError?: string;
  systemStats?: XraySystemStats;
  systemStatsError?: string;
  topology?: XrayRuntimeTopology;
  topologyError?: string;
  onlineUsers?: XrayOnlineUsersSummary;
  onlineUsersError?: string;
  balancers?: XrayBalancerInfo[];
  routingError?: string;
  fetchedUnixMs: number;
}

export interface XraySystemStats {
  uptimeSeconds: number;
  allocatedBytes: number;
  systemBytes: number;
  goroutines: number;
  liveObjects: number;
  garbageCollections: number;
}

export interface XrayRuntimeTopology {
  inboundTags: string[];
  outboundTags: string[];
}

export interface XrayOnlineUsersSummary {
  policyEnabled: boolean;
  statusAvailable: boolean;
  loopbackOnly: boolean;
  userCount: number;
  addressCount: number;
  users: XrayOnlineUser[];
}

export interface XrayOnlineUser {
  email: string;
  online?: boolean;
  addresses: XrayOnlineAddress[];
  uplink: number;
  downlink: number;
}

export interface XrayOnlineAddress {
  ip: string;
  lastSeenUnix: number;
}

export interface XrayBalancerInfo {
  tag: string;
  selectors: string[];
  candidates: string[];
  availableCandidates: string[];
  currentTarget?: string;
  principleTargets: string[];
  strategy?: string;
  fallbackTarget?: string;
  error?: string;
}

export type ManagerEvent =
  | { type: 'programStateChanged'; id: string; state: ProgramState }
  | { type: 'programListChanged' }
  | { type: 'programAutoStartPrivilegeRequired'; ids: string[] };

export interface CamelliaNexusError {
  code: string;
  message: string;
  details?: string;
}

export type Plan = 'free' | 'pro' | 'team';
export type Capability =
  | 'managed_config_sources'
  | 'advanced_diagnostics'
  | 'cloud_sync'
  | 'remote_dashboard'
  | 'alerts'
  | 'shared_configurations'
  | 'team_administration'
  | 'audit_log'
  | 'webhooks'
  | 'managed_program_packages';

export interface LicenseLimits {
  max_programs: number;
  max_config_sources_per_program: number;
  max_team_members: number;
  max_remote_monitors: number;
  max_shared_programs: number;
  max_webhook_endpoints: number;
  max_workspace_storage_bytes: number;
  max_alert_rules: number;
  max_audit_export_events: number;
}

export interface ClientVersionPolicy {
  minimumVersion: string;
  recommendedVersion: string;
  enforceAfter: number;
}

export interface VerifiedEntitlement {
  keyId: string;
  claims: {
    schemaVersion: 3;
    iss: string;
    aud: string;
    sub: string;
    licenseId: string;
    deviceId: string;
    deviceKeyThumbprint: string;
    plan: Plan;
    planRevision: 2;
    policyHash: string;
    licenseStatus: 'active' | 'past_due' | 'canceled';
    capabilities: Capability[];
    workspacePermissions: string[];
    limits: LicenseLimits;
    licenseExpiresAt?: number;
    licenseEpoch: number;
    deviceLimit: number;
    memberLimit: number;
    offlineAccessEndsAt: number;
    iat: number;
    refreshAfter: number;
    exp: number;
    tokenId: string;
    keyId: string;
    clientVersionPolicy: ClientVersionPolicy;
  };
}

export type EntitlementState =
  | { status: 'unauthenticated' }
  | { status: 'sessionOnly' }
  | { status: 'activationPending' }
  | { status: 'active'; entitlement: VerifiedEntitlement }
  | {
      status: 'restrictedOffline';
      entitlement: VerifiedEntitlement;
      safetyWindowEndsAt: number;
    }
  | { status: 'expired'; entitlement: VerifiedEntitlement }
  | {
      status: 'revalidationRequired';
      reason:
        | 'clock_rollback'
        | 'obsolete_epoch'
        | 'corrupt_secure_store'
        | 'invalid_server_proof';
    }
  | {
      status: 'clientUpgradeRequired';
      policy: ClientVersionPolicy;
      entitlement: VerifiedEntitlement | null;
    }
  | {
      status: 'deviceDenied';
      state: 'pending_activation' | 'active' | 'removed' | 'revoked' | 'suspicious';
    }
  | {
      status: 'licenseInactive';
      reason:
        | 'account_suspended'
        | 'account_denylisted'
        | 'license_past_due'
        | 'license_canceled'
        | 'license_expired'
        | 'license_unavailable';
      entitlement?: VerifiedEntitlement;
    };

export interface LicenseServiceSettings {
  configured: boolean;
  baseUrl?: string;
  loopbackDevelopment: boolean;
  authorizationConfigured: boolean;
  authorizationEndpoint?: string;
  redirectUri?: string;
}

export interface LicenseAuthorizationRequest {
  authorizationUrl: string;
  state: string;
  callbackMode: 'loopback' | 'manual';
  suggestedDeviceName: string;
}

export interface LicenseAuthorizationCallbackEvent {
  state: string;
}

export interface LicenseAuthorizationFailedEvent {
  state: string;
  message: string;
}

export interface EntitlementSnapshot {
  generation: number;
  entitlementState: EntitlementState;
}

export interface LicenseStateChangedEvent extends EntitlementSnapshot {
  reason: string;
  runtimeImpact: 'active' | 'restrictedOffline' | 'hardInactive';
  stoppedPrograms: number;
  failedPrograms: number;
  failedProgramIds: string[];
}

export interface LocalLicenseDevice {
  deviceId: string;
  displayName?: string | null;
  platform: string;
}

export interface RegisteredLicenseDevice {
  deviceId: string;
  displayName?: string | null;
  platform: string;
  state: 'pending_activation' | 'active' | 'removed' | 'revoked' | 'suspicious';
  lastSeenAt?: number | null;
}

export interface RegisteredLicenseDevicePage {
  devices: RegisteredLicenseDevice[];
  nextCursor?: string | null;
}

export type PaymentClaimStatus =
  | 'submitted'
  | 'under_review'
  | 'needs_information'
  | 'verified'
  | 'rejected'
  | 'withdrawn';

export interface BillingInvoice {
  id: string;
  rowVersion: number;
  accountId: string;
  licenseId: string;
  offerId: string;
  plan: 'free' | 'pro' | 'team';
  planRevision: number;
  seats: number;
  durationDays: number;
  currency: string;
  amountDue: string;
  paymentReference: string;
  status: 'open' | 'paid' | 'overdue' | 'void' | 'refunded';
  dueAt: number;
  paidAt?: number | null;
  createdAt: number;
  updatedAt: number;
}

export interface ManualPaymentMethod {
  id: string;
  rowVersion: number;
  nameEn: string;
  nameZh: string;
  instructionsEn: string;
  instructionsZh: string;
  settlementAsset: string;
  destinationHint: string;
  active: boolean;
  createdAt: number;
  updatedAt: number;
}

export interface ManualPaymentClaim {
  id: string;
  rowVersion: number;
  invoiceId: string;
  accountId: string;
  paymentMethodId: string;
  externalTransactionId: string;
  paidAmount: string;
  paidAsset: string;
  paidAt: number;
  payerName?: string | null;
  note?: string | null;
  status: PaymentClaimStatus;
  submittedBy: string;
  reviewedBy?: string | null;
  reviewReason?: string | null;
  submittedAt: number;
  createdAt: number;
  updatedAt: number;
}

export interface LicenseBillingSummary {
  invoices: BillingInvoice[];
  paymentClaims: ManualPaymentClaim[];
  paymentMethods: ManualPaymentMethod[];
}

export interface CustomerPaymentSubmission {
  operationId: string;
  invoiceId: string;
  paymentMethodId: string;
  externalTransactionId: string;
  paidAmount: string;
  paidAsset: string;
  paidAt: number;
  payerName?: string | null;
  note?: string | null;
}

export type WorkspaceRole = 'owner' | 'admin' | 'billing' | 'operator' | 'auditor' | 'viewer';
export type WorkspaceMemberStatus = 'invited' | 'active' | 'suspended' | 'removed';

export type WorkspacePermission =
  | 'team.read'
  | 'team.manage'
  | 'team.transfer_ownership'
  | 'billing.read'
  | 'billing.manage'
  | 'shared.read'
  | 'shared.write'
  | 'shared.publish'
  | 'shared.purge'
  | 'sync.read'
  | 'sync.write'
  | 'remote.read'
  | 'alerts.read'
  | 'alerts.history.read'
  | 'alerts.manage'
  | 'alerts.ack'
  | 'audit.read'
  | 'audit.export'
  | 'webhooks.read'
  | 'webhooks.manage'
  | 'webhooks.delivery.read';

export interface WorkspaceMember {
  id: string;
  email: string;
  displayName: string;
  role: WorkspaceRole;
  status: WorkspaceMemberStatus;
  boundDeviceCount: number;
  rowVersion: number;
  createdAt: number;
  updatedAt: number;
}

export interface TeamMemberPage {
  members: WorkspaceMember[];
  nextCursor?: string | null;
  hasMore: boolean;
}

export interface TeamProfile {
  enabled: boolean;
  member?: WorkspaceMember | null;
  permissions: WorkspacePermission[];
  memberLimit: number;
  memberCount: number;
}

export interface CreateTeamInvitation {
  operationId: string;
  email: string;
  displayName: string;
  role: Exclude<WorkspaceRole, 'owner'>;
}

export interface TeamInvitation {
  id: string;
  member: WorkspaceMember;
  invitationToken: string;
  expiresAt: number;
}

export interface MemberDeviceEnrollment {
  id: string;
  memberId: string;
  enrollmentToken: string;
  expiresAt: number;
}

export interface UpdateWorkspaceMember {
  operationId: string;
  role: Exclude<WorkspaceRole, 'owner'>;
  status: 'active' | 'suspended' | 'removed';
  rowVersion: number;
}

export interface LeaveWorkspace {
  operationId: string;
  memberId: string;
  rowVersion: number;
}

export interface TransferWorkspaceOwnership {
  operationId: string;
  newOwnerMemberId: string;
  ownerRowVersion: number;
  newOwnerRowVersion: number;
}

export interface OwnershipTransferResult {
  previousOwner: WorkspaceMember;
  newOwner: WorkspaceMember;
}

export type SharedProgramKind = 'generic' | 'singBox' | 'xray' | 'mihomo';

export interface CreateSharedConfiguration {
  name: string;
  programKind: SharedProgramKind;
  input: string;
  content: string;
  operationId: string;
}

export interface ReviseSharedConfiguration extends CreateSharedConfiguration {
  baseRowVersion: number;
}

export interface PublishSharedConfiguration {
  baseRowVersion: number;
  revision?: number | null;
  operationId: string;
}

export interface VersionedWorkspaceMutation {
  baseRowVersion: number;
  operationId: string;
}

export interface SharedConfigurationPageRequest {
  cursor?: string | null;
  limit?: number | null;
  includeDeleted?: boolean;
}

export interface SharedConfigurationContentRequest {
  revision?: number | null;
}

export interface WorkspaceStorageUsage {
  activeDocumentCount: number;
  maxActiveDocuments: number;
  revisionPlaintextBytes: number;
  maxRevisionPlaintextBytes: number;
  rowVersion: number;
}

export interface SharedConfigurationSummary {
  id: string;
  name: string;
  programKind: SharedProgramKind;
  rowVersion: number;
  draftRevision: number;
  publishedRevision?: number | null;
  deletedAt?: number | null;
  contentSha256: string;
  plaintextBytes: number;
  createdAt: number;
  updatedAt: number;
}

export interface SharedConfigurationContent extends SharedConfigurationSummary {
  revision: number;
  input: string;
  content: string;
  revisionCreatedAt: number;
}

export interface SharedConfigurationPage {
  configurations: SharedConfigurationSummary[];
  usage: WorkspaceStorageUsage;
  nextCursor?: string | null;
  hasMore: boolean;
}

export interface WorkspaceMutationReceipt {
  resourceType: string;
  resourceId: string;
  rowVersion: number;
  cursor?: number | null;
}

export type WorkspaceAlertEventKind =
  | 'sync_conflict'
  | 'quota_warning'
  | 'configuration_created'
  | 'configuration_revised'
  | 'configuration_published'
  | 'configuration_deleted'
  | 'configuration_restored';

export type WorkspaceAlertSeverity = 'info' | 'warning' | 'critical';
export type WorkspaceIncidentStatus = 'open' | 'acknowledged' | 'resolved';

export interface CreateWorkspaceAlertRule {
  name: string;
  eventKind: WorkspaceAlertEventKind;
  severity: WorkspaceAlertSeverity;
  enabled: boolean;
  operationId: string;
}

export interface UpdateWorkspaceAlertRule extends CreateWorkspaceAlertRule {
  baseRowVersion: number;
}

export interface WorkspaceAlertRulePageRequest {
  cursor?: string | null;
  limit?: number | null;
}

export interface WorkspaceAlertRule {
  id: string;
  name: string;
  eventKind: WorkspaceAlertEventKind;
  severity: WorkspaceAlertSeverity;
  enabled: boolean;
  rowVersion: number;
  createdAt: number;
  updatedAt: number;
}

export interface WorkspaceAlertRulePage {
  rules: WorkspaceAlertRule[];
  nextCursor?: string | null;
  hasMore: boolean;
}

export interface WorkspaceIncidentPageRequest {
  cursor?: string | null;
  limit?: number | null;
  status?: WorkspaceIncidentStatus | null;
  eventKind?: WorkspaceAlertEventKind | null;
  severity?: WorkspaceAlertSeverity | null;
}

export interface WorkspaceAlertIncident {
  id: string;
  ruleId: string;
  eventKind: WorkspaceAlertEventKind;
  severity: WorkspaceAlertSeverity;
  status: WorkspaceIncidentStatus;
  summary: string;
  metadata: Record<string, string>;
  rowVersion: number;
  occurredAt: number;
  acknowledgedAt?: number | null;
  resolvedAt?: number | null;
}

export interface WorkspaceIncidentPage {
  incidents: WorkspaceAlertIncident[];
  nextCursor?: string | null;
  hasMore: boolean;
}

export interface WorkspaceSyncFeedRequest {
  cursor?: number | null;
  limit?: number | null;
}

export interface WorkspaceSyncChange {
  cursor: number;
  operationId: string;
  changeKind: string;
  resourceType: string;
  resourceId: string;
  rowVersion: number;
  occurredAt: number;
  metadata: Record<string, string>;
}

export interface WorkspaceSyncFeed {
  changes: WorkspaceSyncChange[];
  nextCursor: number;
  hasMore: boolean;
}

export interface AdvanceWorkspaceCheckpoint {
  cursor: number;
  baseRowVersion: number;
  operationId: string;
}

export interface WorkspaceDeviceCheckpoint {
  cursor: number;
  rowVersion: number;
  updatedAt: number;
}

export interface WorkspaceAuditPageRequest {
  cursor?: string | null;
  limit?: number | null;
  eventType?: string | null;
}

export interface WorkspaceAuditEvent {
  id: string;
  eventType: string;
  outcome: string;
  occurredAt: number;
  deviceId?: string | null;
  reasonCode?: string | null;
  metadata: Record<string, string>;
}

export interface WorkspaceAuditPage {
  events: WorkspaceAuditEvent[];
  nextCursor?: string | null;
  hasMore: boolean;
}

export interface WorkspaceAuditEventTypes {
  eventTypes: string[];
}

export interface WorkspaceAuditExport {
  events: WorkspaceAuditEvent[];
  nextCursor?: string | null;
  truncated: boolean;
}

export interface WebhookEndpoint {
  id: string;
  name: string;
  url: string;
  eventTypes: string[];
  active: boolean;
  secretVersion: number;
  rowVersion: number;
  createdAt: number;
  updatedAt: number;
}

export interface CreateWebhookEndpoint {
  operationId: string;
  name: string;
  url: string;
  eventTypes: string[];
  active: boolean;
}

export interface UpdateWebhookEndpoint extends CreateWebhookEndpoint {
  rowVersion: number;
}

export interface RotateWebhookSecret {
  operationId: string;
  rowVersion: number;
}

export interface DeleteWebhookEndpoint extends RotateWebhookSecret {}

export interface WebhookSecretResult {
  endpoint: WebhookEndpoint;
  secret?: string | null;
}

export interface WebhookDeletion {
  endpointId: string;
  deletedAt: number;
  rowVersion: number;
}

export type WebhookDeliveryStatus = 'pending' | 'inFlight' | 'delivered' | 'retry' | 'dead';

export interface WebhookDeliverySummary {
  id: string;
  eventId: string;
  endpointId: string;
  eventType: string;
  status: WebhookDeliveryStatus;
  attemptCount: number;
  nextAttemptAt: number;
  lastHttpStatus?: number | null;
  lastErrorCategory?: string | null;
  deliveredAt?: number | null;
  createdAt: number;
  updatedAt: number;
}

export type UiIntent =
  | { type: 'createProgram' }
  | { type: 'selectProgram'; programId: string }
  | { type: 'about' };

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  ActionDescriptor,
  ActionResult,
  AdvanceWorkspaceCheckpoint,
  AutomaticConfigUpdateEvent,
  ApplicationInfo,
  AppSettings,
  ConfigDocument,
  ConfigurationSchemaDocument,
  ConfigUpdateResult,
  EntitlementSnapshot,
  CustomerPaymentSubmission,
  DeleteWebhookEndpoint,
  CreateTeamInvitation,
  CreateSharedConfiguration,
  CreateWebhookEndpoint,
  CreateWorkspaceAlertRule,
  LicenseAuthorizationCallbackEvent,
  LicenseAuthorizationFailedEvent,
  LicenseAuthorizationRequest,
  LicenseStateChangedEvent,
  InvalidProgram,
  LeaveWorkspace,
  LocalLicenseDevice,
  LicenseServiceSettings,
  LicenseBillingSummary,
  ManualPaymentClaim,
  MemberDeviceEnrollment,
  OwnershipTransferResult,
  PublishSharedConfiguration,
  ReviseSharedConfiguration,
  RotateWebhookSecret,
  SharedConfigurationContent,
  SharedConfigurationContentRequest,
  SharedConfigurationPage,
  SharedConfigurationPageRequest,
  TeamInvitation,
  TeamMemberPage,
  TeamProfile,
  TransferWorkspaceOwnership,
  UpdateWebhookEndpoint,
  UpdateWorkspaceAlertRule,
  VersionedWorkspaceMutation,
  WebhookDeletion,
  WebhookDeliverySummary,
  WebhookEndpoint,
  WebhookSecretResult,
  WorkspaceAlertRulePage,
  WorkspaceAlertRulePageRequest,
  WorkspaceAuditExport,
  WorkspaceAuditEventTypes,
  WorkspaceAuditPage,
  WorkspaceAuditPageRequest,
  WorkspaceDeviceCheckpoint,
  WorkspaceIncidentPage,
  WorkspaceIncidentPageRequest,
  WorkspaceMutationReceipt,
  WorkspaceSyncFeed,
  WorkspaceSyncFeedRequest,
  WorkspaceMember,
  UpdateWorkspaceMember,
  RegisteredLicenseDevicePage,
  LogChunk,
  ManagerEvent,
  ProgramDetail,
  PrivilegeAssessment,
  ProgramSpec,
  ProgramSummary,
  ValidationResult,
  UiIntent,
  XrayBalancerInfo,
  XrayDashboardSnapshot,
} from './types';

export { errorInfoOf, type ErrorInfo } from './errors';

export const api = {
  logFrontendEvent: (
    level: 'error' | 'warn' | 'info' | 'debug' | 'trace',
    message: string,
    fields?: Record<string, unknown>,
  ) => invoke<void>('log_frontend_event', { level, message, fields: fields ?? null }),
  getApplicationInfo: () => invoke<ApplicationInfo>('get_application_info'),
  getEntitlementState: () => invoke<EntitlementSnapshot>('get_entitlement_state'),
  getLocalLicenseDevice: () =>
    invoke<LocalLicenseDevice | null>('get_local_license_device'),
  getLicenseServiceSettings: () =>
    invoke<LicenseServiceSettings>('get_license_service_settings'),
  beginLicenseAuthorization: (openBrowser = true) =>
    invoke<LicenseAuthorizationRequest>('begin_license_authorization', { openBrowser }),
  takeLicenseAuthorizationCallback: (authorizationState: string) =>
    invoke<LicenseAuthorizationCallbackEvent | null>('take_license_authorization_callback', {
      authorizationState,
    }),
  cancelLicenseAuthorization: (authorizationState: string) =>
    invoke<void>('cancel_license_authorization', { authorizationState }),
  completeLicenseAuthorization: (request: {
    expectedState: string;
    displayName?: string;
  }) => invoke<EntitlementSnapshot>('complete_license_authorization', request),
  refreshLicenseEntitlement: () =>
    invoke<EntitlementSnapshot>('refresh_license_entitlement'),
  reconnectLicenseDevice: () =>
    invoke<EntitlementSnapshot>('reconnect_license_device'),
  getLicenseDevices: (cursor?: string, pageSize = 50) =>
    invoke<RegisteredLicenseDevicePage>('get_license_devices', {
      cursor: cursor ?? null,
      pageSize,
    }),
  removeLicenseDevice: (deviceId: string, operationId: string) =>
    invoke<void>('remove_license_device', { deviceId, operationId }),
  getLicenseBillingSummary: () =>
    invoke<LicenseBillingSummary>('get_license_billing_summary'),
  submitLicensePaymentClaim: (submission: CustomerPaymentSubmission) =>
    invoke<ManualPaymentClaim>('submit_license_payment_claim', { submission }),
  getLicenseTeamProfile: () => invoke<TeamProfile>('get_license_team_profile'),
  getLicenseTeamMembers: (cursor?: string | null, limit = 100) =>
    invoke<TeamMemberPage>('get_license_team_members', {
      request: { cursor: cursor ?? null, limit },
    }),
  createLicenseTeamInvitation: (request: CreateTeamInvitation) =>
    invoke<TeamInvitation>('create_license_team_invitation', { request }),
  acceptLicenseTeamInvitation: (invitationToken: string, operationId: string) =>
    invoke<TeamProfile>('accept_license_team_invitation', {
      request: { invitationToken, operationId },
    }),
  updateLicenseTeamMember: (memberId: string, request: UpdateWorkspaceMember) =>
    invoke<WorkspaceMember>('update_license_team_member', { memberId, request }),
  createLicenseTeamDeviceEnrollment: (operationId: string) =>
    invoke<MemberDeviceEnrollment>('create_license_team_device_enrollment', {
      request: { operationId },
    }),
  createLicenseTeamMemberDeviceEnrollment: (memberId: string, operationId: string) =>
    invoke<MemberDeviceEnrollment>('create_license_team_member_device_enrollment', {
      memberId,
      request: { operationId },
    }),
  acceptLicenseTeamDeviceEnrollment: (enrollmentToken: string, operationId: string) =>
    invoke<TeamProfile>('accept_license_team_device_enrollment', {
      request: { enrollmentToken, operationId },
    }),
  leaveLicenseTeamWorkspace: (request: LeaveWorkspace) =>
    invoke<void>('leave_license_team_workspace', { request }),
  transferLicenseTeamOwnership: (request: TransferWorkspaceOwnership) =>
    invoke<OwnershipTransferResult>('transfer_license_team_ownership', { request }),
  getLicenseWorkspaceConfigurations: (request: SharedConfigurationPageRequest) =>
    invoke<SharedConfigurationPage>('get_license_workspace_configurations', { request }),
  getLicenseWorkspaceConfiguration: (
    documentId: string,
    request: SharedConfigurationContentRequest,
  ) => invoke<SharedConfigurationContent>('get_license_workspace_configuration', {
    documentId,
    request,
  }),
  createLicenseWorkspaceConfiguration: (request: CreateSharedConfiguration) =>
    invoke<WorkspaceMutationReceipt>('create_license_workspace_configuration', { request }),
  reviseLicenseWorkspaceConfiguration: (
    documentId: string,
    request: ReviseSharedConfiguration,
  ) => invoke<WorkspaceMutationReceipt>('revise_license_workspace_configuration', {
    documentId,
    request,
  }),
  publishLicenseWorkspaceConfiguration: (
    documentId: string,
    request: PublishSharedConfiguration,
  ) => invoke<WorkspaceMutationReceipt>('publish_license_workspace_configuration', {
    documentId,
    request,
  }),
  deleteLicenseWorkspaceConfiguration: (
    documentId: string,
    request: VersionedWorkspaceMutation,
  ) => invoke<WorkspaceMutationReceipt>('delete_license_workspace_configuration', {
    documentId,
    request,
  }),
  restoreLicenseWorkspaceConfiguration: (
    documentId: string,
    request: VersionedWorkspaceMutation,
  ) => invoke<WorkspaceMutationReceipt>('restore_license_workspace_configuration', {
    documentId,
    request,
  }),
  purgeLicenseWorkspaceConfiguration: (
    documentId: string,
    request: VersionedWorkspaceMutation,
  ) => invoke<WorkspaceMutationReceipt>('purge_license_workspace_configuration', {
    documentId,
    request,
  }),
  getLicenseWorkspaceSyncFeed: (request: WorkspaceSyncFeedRequest) =>
    invoke<WorkspaceSyncFeed>('get_license_workspace_sync_feed', { request }),
  getLicenseWorkspaceCheckpoint: () =>
    invoke<WorkspaceDeviceCheckpoint | null>('get_license_workspace_checkpoint'),
  advanceLicenseWorkspaceCheckpoint: (request: AdvanceWorkspaceCheckpoint) =>
    invoke<WorkspaceMutationReceipt>('advance_license_workspace_checkpoint', { request }),
  getLicenseWorkspaceAlertRules: (request: WorkspaceAlertRulePageRequest) =>
    invoke<WorkspaceAlertRulePage>('get_license_workspace_alert_rules', { request }),
  createLicenseWorkspaceAlertRule: (request: CreateWorkspaceAlertRule) =>
    invoke<WorkspaceMutationReceipt>('create_license_workspace_alert_rule', { request }),
  updateLicenseWorkspaceAlertRule: (ruleId: string, request: UpdateWorkspaceAlertRule) =>
    invoke<WorkspaceMutationReceipt>('update_license_workspace_alert_rule', { ruleId, request }),
  deleteLicenseWorkspaceAlertRule: (ruleId: string, request: VersionedWorkspaceMutation) =>
    invoke<WorkspaceMutationReceipt>('delete_license_workspace_alert_rule', { ruleId, request }),
  getLicenseWorkspaceAlertIncidents: (request: WorkspaceIncidentPageRequest) =>
    invoke<WorkspaceIncidentPage>('get_license_workspace_alert_incidents', { request }),
  acknowledgeLicenseWorkspaceAlertIncident: (
    incidentId: string,
    request: VersionedWorkspaceMutation,
  ) => invoke<WorkspaceMutationReceipt>('acknowledge_license_workspace_alert_incident', {
    incidentId,
    request,
  }),
  resolveLicenseWorkspaceAlertIncident: (
    incidentId: string,
    request: VersionedWorkspaceMutation,
  ) => invoke<WorkspaceMutationReceipt>('resolve_license_workspace_alert_incident', {
    incidentId,
    request,
  }),
  getLicenseWorkspaceAuditEvents: (request: WorkspaceAuditPageRequest) =>
    invoke<WorkspaceAuditPage>('get_license_workspace_audit_events', { request }),
  getLicenseWorkspaceAuditEventTypes: () =>
    invoke<WorkspaceAuditEventTypes>('get_license_workspace_audit_event_types'),
  exportLicenseWorkspaceAuditEvents: (request: WorkspaceAuditPageRequest) =>
    invoke<WorkspaceAuditExport>('export_license_workspace_audit_events', { request }),
  getLicenseWorkspaceWebhookEndpoints: () =>
    invoke<WebhookEndpoint[]>('get_license_workspace_webhook_endpoints'),
  createLicenseWorkspaceWebhookEndpoint: (request: CreateWebhookEndpoint) =>
    invoke<WebhookSecretResult>('create_license_workspace_webhook_endpoint', { request }),
  updateLicenseWorkspaceWebhookEndpoint: (endpointId: string, request: UpdateWebhookEndpoint) =>
    invoke<WebhookEndpoint>('update_license_workspace_webhook_endpoint', {
      endpointId,
      request,
    }),
  rotateLicenseWorkspaceWebhookEndpoint: (endpointId: string, request: RotateWebhookSecret) =>
    invoke<WebhookSecretResult>('rotate_license_workspace_webhook_endpoint', {
      endpointId,
      request,
    }),
  deleteLicenseWorkspaceWebhookEndpoint: (endpointId: string, request: DeleteWebhookEndpoint) =>
    invoke<WebhookDeletion>('delete_license_workspace_webhook_endpoint', {
      endpointId,
      request,
    }),
  getLicenseWorkspaceWebhookDeliveries: (endpointId: string | null, limit = 50) =>
    invoke<WebhookDeliverySummary[]>('get_license_workspace_webhook_deliveries', {
      endpointId,
      limit,
    }),
  logoutLicenseSession: () => invoke<void>('logout_license_session'),
  resetLicenseDeviceIdentity: (operationId: string) =>
    invoke<EntitlementSnapshot>('reset_license_device_identity', { operationId }),
  listPrograms: () => invoke<ProgramSummary[]>('list_programs'),
  getProgram: (programId: string) => invoke<ProgramDetail>('get_program', { programId }),
  getProgramPrivilegeAssessment: (programId: string) =>
    invoke<PrivilegeAssessment>('get_program_privilege_assessment', { programId }),
  createProgram: (request: {
    spec: ProgramSpec;
    packageSource?: string;
    initialConfig?: string;
  }) => invoke<void>('create_program', { request }),
  listInvalidPrograms: () => invoke<InvalidProgram[]>('list_invalid_programs'),
  updateProgram: (spec: ProgramSpec) => invoke<void>('update_program', { spec }),
  updateProgramAndRestart: (spec: ProgramSpec) =>
    invoke<void>('update_program_and_restart', { spec }),
  updateProgramAndRefreshConfig: (spec: ProgramSpec) =>
    invoke<ConfigUpdateResult>('update_program_and_refresh_config', { spec }),
  removeProgram: (programId: string) => invoke<void>('remove_program', { programId }),
  startProgram: (programId: string) => invoke<void>('start_program', { programId }),
  stopProgram: (programId: string) => invoke<void>('stop_program', { programId }),
  restartProgram: (programId: string) => invoke<void>('restart_program', { programId }),
  replacePackage: (programId: string, packageSource: string) =>
    invoke<void>('replace_package', { programId, packageSource }),
  listActions: (programId: string) =>
    invoke<ActionDescriptor[]>('list_actions', { programId }),
  loadConfig: (programId: string) => invoke<ConfigDocument>('load_config', { programId }),
  loadConfigurationSchema: (programId: string) =>
    invoke<ConfigurationSchemaDocument | null>('load_configuration_schema', { programId }),
  validateConfig: (programId: string, content: string, baseHash: string) =>
    invoke<ValidationResult>('validate_config', { programId, content, baseHash }),
  applyConfig: (programId: string, content: string, baseHash: string) =>
    invoke<string>('apply_config', { programId, content, baseHash }),
  refreshConfigSources: (programId: string) =>
    invoke<ConfigUpdateResult>('refresh_config_sources', { programId }),
  runAction: (programId: string, actionId: string, content: string, baseHash: string) =>
    invoke<ActionResult>('run_action', { programId, actionId, content, baseHash }),
  readLogs: (programId: string, stream: 'stdout' | 'stderr', maxBytes = 262144) =>
    invoke<LogChunk>('read_logs', { programId, stream, maxBytes }),
  clearLogs: (programId: string) => invoke<void>('clear_logs', { programId }),
  openWorkingDirectory: (programId: string) =>
    invoke<void>('open_working_directory', { programId }),
  openDataDirectory: () => invoke<void>('open_data_directory'),
  openAppLogDirectory: () => invoke<void>('open_app_log_directory'),
  openDocumentation: (programId: string) =>
    invoke<void>('open_documentation', { programId }),
  openSingBoxDashboard: (programId: string, dashboardKind: 'native' | 'clash') =>
    invoke<void>('open_sing_box_dashboard', { programId, dashboardKind }),
  openMihomoDashboard: (programId: string) =>
    invoke<void>('open_mihomo_dashboard', { programId }),
  getXrayDashboardSnapshot: (
    programId: string,
    includeRouting = false,
    includeTopology = false,
  ) =>
    invoke<XrayDashboardSnapshot>('get_xray_dashboard_snapshot', {
      programId,
      includeRouting,
      includeTopology,
    }),
  setXrayBalancerTarget: (
    programId: string,
    balancerTag: string,
    target?: string,
  ) => invoke<XrayBalancerInfo>(
    'set_xray_balancer_target',
    { programId, balancerTag, target: target ?? null },
  ),
  restartXrayLogger: (programId: string) =>
    invoke<void>('restart_xray_logger', { programId }),
  getAutostart: () => invoke<boolean>('get_autostart'),
  setAutostart: (enabled: boolean) => invoke<void>('set_autostart', { enabled }),
  getAppSettings: () => invoke<AppSettings>('get_app_settings'),
  setAppSettings: (settings: AppSettings) =>
    invoke<void>('set_app_settings', { settings }),
  frontendReady: () => invoke<UiIntent | null>('frontend_ready'),
  onManagerEvent: (handler: (event: ManagerEvent) => void): Promise<UnlistenFn> =>
    listen<ManagerEvent>('manager-event', ({ payload }) => handler(payload)),
  onAutomaticConfigUpdate: (
    handler: (event: AutomaticConfigUpdateEvent) => void,
  ): Promise<UnlistenFn> =>
    listen<AutomaticConfigUpdateEvent>('automatic-config-update', ({ payload }) => handler(payload)),
  onOpenCreateProgram: (handler: () => void): Promise<UnlistenFn> =>
    listen('open-create-program', handler),
  onSelectProgram: (handler: (programId: string) => void): Promise<UnlistenFn> =>
    listen<string>('select-program', ({ payload }) => handler(payload)),
  onOpenAbout: (handler: () => void): Promise<UnlistenFn> =>
    listen('open-about', handler),
  onLicenseAuthorizationCallback: (
    handler: (event: LicenseAuthorizationCallbackEvent) => void,
  ): Promise<UnlistenFn> =>
    listen<LicenseAuthorizationCallbackEvent>(
      'license-authorization-callback',
      ({ payload }) => handler(payload),
    ),
  onLicenseAuthorizationFailed: (
    handler: (event: LicenseAuthorizationFailedEvent) => void,
  ): Promise<UnlistenFn> =>
    listen<LicenseAuthorizationFailedEvent>(
      'license-authorization-failed',
      ({ payload }) => handler(payload),
    ),
  onLicenseStateChanged: (
    handler: (event: LicenseStateChangedEvent) => void,
  ): Promise<UnlistenFn> =>
    listen<LicenseStateChangedEvent>(
      'license-state-changed',
      ({ payload }) => handler(payload),
    ),
};

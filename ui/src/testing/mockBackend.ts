import type { InvokeArgs } from '@tauri-apps/api/core';
import { emit } from '@tauri-apps/api/event';
import { mockIPC, mockWindows } from '@tauri-apps/api/mocks';
import { installPreviewInvokeTransport } from '../api';
import { canUseProgramLifecycleAction, deriveLicenseAccess } from '../licenseAccess';
import { PROGRAM_SPEC_SCHEMA_VERSION } from '../types';
import type {
  AppSettings,
  ConfigSource,
  CustomerPaymentSubmission,
  EntitlementSnapshot,
  LicenseBillingSummary,
  LicenseStateChangedEvent,
  ManualPaymentClaim,
  ProgramDetail,
  ProgramSpec,
  ProgramState,
  ProgramSummary,
  RemoteUpdate,
  SharedConfigurationContent,
  SharedConfigurationSummary,
  TeamProfile,
  WebhookDeliverySummary,
  WebhookEndpoint,
  WorkspaceAlertIncident,
  WorkspaceAlertRule,
  WorkspaceAuditEvent,
  WorkspaceDeviceCheckpoint,
  WorkspaceMember,
  WorkspacePermission,
  WorkspaceRole,
  WorkspaceSyncChange,
  XrayBalancerInfo,
  XrayDashboardSnapshot,
  ConfigurationStateView,
  CoreCompatibilityProfile,
  CoreCompatibilityPreference,
  CoreTargetIdentity,
  GuidedProjection,
  GuidedSettingDescriptor,
  RawDraftSession,
  IntentOperation,
  ConfigurationConflict,
  ShareImportPreview,
  ProgramKind,
} from '../types';

const nowSeconds = Math.floor(Date.now() / 1_000);
const entitlement: EntitlementSnapshot = {
  generation: 1,
  entitlementState: {
    status: 'active',
    entitlement: {
      keyId: 'ui-preview-key',
      claims: {
        schemaVersion: 3,
        iss: 'https://license.example.test',
        aud: 'camellia-nexus-desktop',
        sub: 'preview-account',
        licenseId: 'license_preview_001',
        deviceId: 'device_preview_001',
        deviceKeyThumbprint: 'sha256:preview',
        plan: 'pro',
        planRevision: 2,
        policyHash: '0'.repeat(64),
        licenseStatus: 'active',
        capabilities: ['managed_config_sources', 'advanced_diagnostics', 'remote_dashboard'],
        workspacePermissions: [],
        limits: {
          max_programs: 50,
          max_config_sources_per_program: 20,
          max_team_members: 1,
          max_remote_monitors: 3,
          max_shared_programs: 0,
          max_webhook_endpoints: 0,
          max_workspace_storage_bytes: 0,
          max_alert_rules: 0,
          max_audit_export_events: 0,
        },
        licenseExpiresAt: nowSeconds + 2_592_000,
        licenseEpoch: 4,
        deviceLimit: 3,
        memberLimit: 1,
        iat: nowSeconds - 3_600,
        refreshAfter: nowSeconds + 21_600,
        exp: nowSeconds + 21_600,
        offlineAccessEndsAt: nowSeconds + 86_400,
        tokenId: 'preview-token',
        keyId: 'ui-preview-key',
        clientVersionPolicy: {
          minimumVersion: '1.0.0',
          recommendedVersion: '1.0.0',
          enforceAfter: nowSeconds + 31_536_000,
        },
      },
    },
  },
};
const unlicensedEntitlement: EntitlementSnapshot = {
  generation: 1,
  entitlementState: { status: 'unauthenticated' },
};
const previewParameters = new URLSearchParams(location.search);
const billingNeedsInformationPreview = previewParameters.has('__ui_billing_needs_information');
const teamMemberPreview = previewParameters.has('__ui_team_member');
const teamUnlinkedPreview = previewParameters.has('__ui_team_unlinked');
const teamConflictPreview = previewParameters.has('__ui_team_conflict');
const teamCloudPreview = previewParameters.has('__ui_team_cloud');
const teamLongLayoutPreview = previewParameters.has('__ui_team_long');
const xrayDenseLayoutPreview = previewParameters.has('__ui_xray_dense');
const removedLicensePreview = previewParameters.has('__ui_removed_license');
const coreTargetPreview = previewParameters.get('__ui_core_target') ?? '';
const coreEvidencePreview = previewParameters.get('__ui_core_evidence') ?? '';
const configurationSourcePreview = previewParameters.get('__ui_config_source') ?? '';
const rawAllOverridePreview = previewParameters.has('__ui_raw_all_override');
const requestedTeamRole = previewParameters.get('__ui_team_role');
const previewWorkspaceRole: WorkspaceRole = teamMemberPreview
  ? 'operator'
  : ['owner', 'admin', 'billing', 'operator', 'viewer', 'auditor'].includes(requestedTeamRole ?? '')
    ? requestedTeamRole as WorkspaceRole
    : 'owner';
const teamPreview = previewParameters.has('__ui_team')
  || teamMemberPreview
  || teamUnlinkedPreview
  || teamConflictPreview
  || teamCloudPreview
  || !!requestedTeamRole;

let previewBillingSummary: LicenseBillingSummary = billingNeedsInformationPreview
  ? {
      invoices: [{
        id: 'invoice_billing_preview',
        rowVersion: 1,
        accountId: 'preview-account',
        licenseId: 'license_preview_001',
        offerId: 'offer_billing_preview',
        plan: 'pro',
        planRevision: 2,
        seats: 0,
        durationDays: 365,
        currency: 'USD',
        amountDue: '19.99000000',
        paymentReference: 'CNX-PAY_F76430CD5208A6B2A78C01C8D4E3C190',
        status: 'open',
        dueAt: nowSeconds + 604_800,
        paidAt: null,
        createdAt: nowSeconds - 3_600,
        updatedAt: nowSeconds - 3_600,
      }],
      paymentClaims: [{
        id: 'payment_claim_billing_preview',
        rowVersion: 2,
        invoiceId: 'invoice_billing_preview',
        accountId: 'preview-account',
        paymentMethodId: 'payment_method_billing_preview',
        externalTransactionId: 'PREVIEW-RECEIPT-001',
        paidAmount: '19.99000000',
        paidAsset: 'USD',
        paidAt: nowSeconds - 1_800,
        payerName: 'Camellia Test',
        note: 'receipt identifier pending confirmation',
        status: 'needs_information',
        submittedBy: 'device:device_preview_001',
        reviewedBy: 'preview-reviewer',
        reviewReason: '请补充内部核验备注并确认回执编号。',
        submittedAt: nowSeconds - 1_700,
        createdAt: nowSeconds - 1_700,
        updatedAt: nowSeconds - 300,
      }],
      paymentMethods: [{
        id: 'payment_method_billing_preview',
        rowVersion: 1,
        nameEn: 'Test bank transfer',
        nameZh: '测试银行转账',
        instructionsEn: 'Include the invoice reference in the transfer memo.',
        instructionsZh: '请在转账附言中填写账单参考号。',
        settlementAsset: 'USD',
        destinationHint: 'Internal test destination · no real payment',
        active: true,
        createdAt: nowSeconds - 86_400,
        updatedAt: nowSeconds - 86_400,
      }],
    }
  : { invoices: [], paymentClaims: [], paymentMethods: [] };

function permissionsForRole(role: WorkspaceRole): WorkspacePermission[] {
  switch (role) {
    case 'owner': return [
      'team.read', 'team.manage', 'team.transfer_ownership', 'billing.read', 'billing.manage',
      'shared.read', 'shared.write', 'shared.publish', 'shared.purge', 'sync.read', 'sync.write',
      'remote.read', 'alerts.read', 'alerts.manage', 'alerts.ack', 'audit.read', 'audit.export',
      'webhooks.read', 'webhooks.manage',
    ];
    case 'admin': return [
      'team.read', 'team.manage', 'billing.read', 'shared.read', 'shared.write',
      'shared.publish', 'sync.read', 'sync.write', 'remote.read', 'alerts.read',
      'alerts.manage', 'alerts.ack', 'audit.read', 'audit.export', 'webhooks.read',
      'webhooks.manage',
    ];
    case 'billing': return ['billing.read', 'billing.manage'];
    case 'operator': return [
      'team.read', 'shared.read', 'shared.write', 'sync.read', 'sync.write', 'remote.read',
      'alerts.read', 'alerts.ack',
    ];
    case 'viewer': return ['team.read', 'shared.read', 'sync.read', 'remote.read', 'alerts.read'];
    case 'auditor': return [
      'team.read', 'alerts.history.read', 'audit.read', 'audit.export',
      'webhooks.delivery.read',
    ];
  }
}
let previewEntitlement: EntitlementSnapshot = previewParameters.has('__ui_unlicensed')
  ? unlicensedEntitlement
  : removedLicensePreview
    ? { generation: 1, entitlementState: { status: 'deviceDenied', state: 'removed' } }
    : teamPreview
      ? structuredClone(entitlement)
      : entitlement;
if (teamPreview && previewEntitlement.entitlementState.status === 'active') {
  const claims = previewEntitlement.entitlementState.entitlement.claims;
  claims.plan = 'team';
  const workspacePermissions = teamUnlinkedPreview ? [] : permissionsForRole(previewWorkspaceRole);
  claims.workspacePermissions = workspacePermissions;
  claims.capabilities = [
    ...claims.capabilities,
    'managed_program_packages',
    ...(workspacePermissions.includes('team.manage') ? ['team_administration' as const] : []),
    ...(workspacePermissions.includes('shared.read') ? ['shared_configurations' as const] : []),
    ...(workspacePermissions.includes('sync.read') ? ['cloud_sync' as const] : []),
    ...(workspacePermissions.some((permission) => permission.startsWith('alerts.')) ? ['alerts' as const] : []),
    ...(workspacePermissions.includes('audit.read') ? ['audit_log' as const] : []),
    ...(workspacePermissions.some((permission) => permission.startsWith('webhooks.')) ? ['webhooks' as const] : []),
  ];
  claims.limits = {
    ...claims.limits,
    max_config_sources_per_program: 50,
    max_team_members: 5,
    max_remote_monitors: 20,
    max_shared_programs: 200,
    max_webhook_endpoints: 5,
    max_workspace_storage_bytes: 2 * 1024 * 1024 * 1024,
    max_alert_rules: 50,
    max_audit_export_events: 5_000,
  };
  claims.deviceLimit = 15;
  claims.memberLimit = 5;
}
const previewOwner: WorkspaceMember = {
  id: 'member_preview',
  email: teamLongLayoutPreview
    ? 'workspace-owner-with-an-exceptionally-long-production-identity@example.test'
    : 'owner@example.test',
  displayName: teamLongLayoutPreview
    ? 'Workspace owner with an exceptionally long production identity 工作区所有者超长显示名称'
    : 'Workspace owner',
  role: 'owner',
  status: 'active',
  boundDeviceCount: 1,
  rowVersion: 1,
  createdAt: nowSeconds,
  updatedAt: nowSeconds,
};
const previewOperator: WorkspaceMember = {
  id: 'member_operator_preview',
  email: 'operator@example.test',
  displayName: 'Preview operator',
  role: 'operator',
  status: 'active',
  boundDeviceCount: 1,
  rowVersion: 5,
  createdAt: nowSeconds,
  updatedAt: nowSeconds,
};
const previewRoleMember: WorkspaceMember = previewWorkspaceRole === 'owner'
  ? previewOwner
  : previewWorkspaceRole === 'operator'
    ? previewOperator
    : {
        id: `member_${previewWorkspaceRole}_current`,
        email: `${previewWorkspaceRole}@example.test`,
        displayName: `Preview ${previewWorkspaceRole}`,
        role: previewWorkspaceRole,
        status: 'active',
        boundDeviceCount: 1,
        rowVersion: 6,
        createdAt: nowSeconds,
        updatedAt: nowSeconds,
      };
const teamMemberFixtures: WorkspaceMember[] = [
  previewOwner,
  { id: 'member_admin_preview', email: 'admin@example.test', displayName: 'Preview administrator', role: 'admin', status: 'active', boundDeviceCount: 1, rowVersion: 2, createdAt: nowSeconds, updatedAt: nowSeconds },
  {
    id: 'member_auditor_preview',
    email: teamLongLayoutPreview
      ? 'auditor-with-an-exceptionally-long-production-identity@example.test'
      : 'auditor@example.test',
    displayName: teamLongLayoutPreview
      ? 'Preview auditor with an exceptionally long production identity 审计成员超长显示名称'
      : 'Preview auditor',
    role: 'auditor',
    status: 'active',
    boundDeviceCount: 0,
    rowVersion: 3,
    createdAt: nowSeconds,
    updatedAt: nowSeconds,
  },
  { id: 'member_removed_preview', email: 'former@example.test', displayName: 'Former operator', role: 'operator', status: 'removed', boundDeviceCount: 0, rowVersion: 4, createdAt: nowSeconds, updatedAt: nowSeconds },
  previewRoleMember,
];
let previewTeamMembers: WorkspaceMember[] = teamUnlinkedPreview
  ? []
  : [...new Map(teamMemberFixtures.map((member) => [member.id, member])).values()];
let previewTeamProfile: TeamProfile = {
  enabled: true,
  member: teamUnlinkedPreview ? null : previewRoleMember,
  permissions: teamUnlinkedPreview ? [] : permissionsForRole(previewWorkspaceRole),
  memberLimit: 5,
  memberCount: teamUnlinkedPreview
    ? 0
    : previewTeamMembers.filter((member) => member.status !== 'removed').length,
};
window.addEventListener('camellia-ui-preview:team-role-downgrade', () => {
  const member = previewTeamProfile.member;
  if (!member || member.status !== 'active') return;
  const downgraded = { ...member, role: 'viewer' as const, rowVersion: member.rowVersion + 1 };
  previewTeamProfile = {
    ...previewTeamProfile,
    member: downgraded,
    permissions: permissionsForRole('viewer'),
  };
  previewTeamMembers = previewTeamMembers.map((item) => (
    item.id === downgraded.id ? downgraded : item
  ));
});
let ownershipConflictPending = teamConflictPreview;
let licenseTimeoutFailures = false;
window.addEventListener('camellia-ui-preview:license-timeout-errors', () => {
  licenseTimeoutFailures = true;
});
let licenseRequiredProgramListFailurePending = previewParameters.has('__ui_license_required_error');
let staleLogFailurePending = previewParameters.has('__ui_stale_log_error');
let configurationSchemaFailurePending = previewParameters.has('__ui_schema_error');
const slowTeamOperations = previewParameters.has('__ui_slow_team');
let teamLostResponsePending = previewParameters.has('__ui_team_lost_response');
const slowLicenseRefresh = previewParameters.has('__ui_slow_license_refresh');
const pagedTeamMembers = previewParameters.has('__ui_team_pages');
const slowExternalActions = previewParameters.has('__ui_slow_external');
const controlledProgramSelection = previewParameters.has('__ui_controlled_program_selection');
const failExternalActions = previewParameters.has('__ui_fail_external');
const failedExternalActions = new Set<string>();
let sourceSaveFailurePending = previewParameters.has('__ui_source_save_error');
let compatibilitySaveFailurePending = previewParameters.has('__ui_compatibility_save_error');

function mockTeamResult<T>(value: T): T | Promise<T> {
  if (!slowTeamOperations) return value;
  return new Promise((resolve) => window.setTimeout(() => resolve(value), 800));
}

type TeamOperationRecord = {
  command: string;
  request: string;
  result: unknown;
};

const teamOperationRecords = new Map<string, TeamOperationRecord>();

function commitTeamOperation<T>(
  command: string,
  request: Record<string, unknown>,
  mutation: () => T,
): T | Promise<T> {
  const operationId = typeof request.operationId === 'string' ? request.operationId : '';
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(operationId)) {
    throw { code: 'INVALID_SPEC', message: 'A canonical operation ID is required' };
  }
  const rowIdentity = Object.fromEntries(Object.entries({
    memberId: request.memberId,
    rowVersion: request.rowVersion,
    newOwnerMemberId: request.newOwnerMemberId,
    ownerRowVersion: request.ownerRowVersion,
    newOwnerRowVersion: request.newOwnerRowVersion,
  }).filter(([, value]) => value !== undefined));
  window.dispatchEvent(new CustomEvent('camellia-ui-preview:workspace-mutation', {
    detail: { command, operationId, rowIdentity },
  }));
  const canonicalRequest = JSON.stringify(request);
  const existing = teamOperationRecords.get(operationId);
  if (existing) {
    if (existing.command !== command || existing.request !== canonicalRequest) {
      throw { code: 'LICENSE_OPERATION_CONFLICT', message: 'License service operation failed' };
    }
    return mockTeamResult(structuredClone(existing.result) as T);
  }
  const result = mutation();
  teamOperationRecords.set(operationId, {
    command,
    request: canonicalRequest,
    result: structuredClone(result),
  });
  if (teamLostResponsePending) {
    teamLostResponsePending = false;
    throw {
      code: 'TIMEOUT',
      message: 'License service operation failed',
      details: 'the committed Team mutation response was lost',
    };
  }
  return mockTeamResult(structuredClone(result));
}

let previewSharedConfigurations: SharedConfigurationSummary[] = [
  {
    id: 'shared_config_preview',
    name: 'Production edge routing',
    programKind: 'singBox',
    rowVersion: 3,
    draftRevision: 2,
    publishedRevision: 1,
    deletedAt: null,
    contentSha256: 'a'.repeat(64),
    plaintextBytes: 1_248,
    createdAt: nowSeconds - 86_400,
    updatedAt: nowSeconds - 300,
  },
  {
    id: 'shared_config_deleted_preview',
    name: 'Retired proxy profile',
    programKind: 'xray',
    rowVersion: 5,
    draftRevision: 3,
    publishedRevision: 3,
    deletedAt: nowSeconds - 31 * 86_400,
    contentSha256: 'b'.repeat(64),
    plaintextBytes: 896,
    createdAt: nowSeconds - 60 * 86_400,
    updatedAt: nowSeconds - 31 * 86_400,
  },
];
const previewSharedContents = new Map<string, SharedConfigurationContent>([
  ['shared_config_preview', {
    ...previewSharedConfigurations[0],
    revision: 2,
    input: '--config /etc/camellia/config.json',
    content: '{\n  "log": { "level": "info" },\n  "route": { "final": "proxy-sg" }\n}\n',
    revisionCreatedAt: nowSeconds - 300,
  }],
  ['shared_config_deleted_preview', {
    ...previewSharedConfigurations[1],
    revision: 3,
    input: '',
    content: '{\n  "log": { "loglevel": "warning" }\n}\n',
    revisionCreatedAt: nowSeconds - 32 * 86_400,
  }],
]);
let previewSyncChanges: WorkspaceSyncChange[] = [
  { cursor: 11, operationId: '11111111-1111-4111-8111-111111111111', changeKind: 'configuration_revised', resourceType: 'shared_configuration', resourceId: 'shared_config_preview', rowVersion: 3, occurredAt: nowSeconds - 300, metadata: { revision: '2' } },
  { cursor: 12, operationId: '22222222-2222-4222-8222-222222222222', changeKind: 'alert_incident_acknowledged', resourceType: 'alert_incident', resourceId: 'incident_ack_preview', rowVersion: 2, occurredAt: nowSeconds - 180, metadata: {} },
];
let previewCheckpoint: WorkspaceDeviceCheckpoint | null = {
  cursor: 10,
  rowVersion: 2,
  updatedAt: nowSeconds - 600,
};
let previewAlertRules: WorkspaceAlertRule[] = [
  { id: 'alert_rule_preview', name: 'Critical sync conflicts', eventKind: 'sync_conflict', severity: 'critical', enabled: true, rowVersion: 2, createdAt: nowSeconds - 86_400, updatedAt: nowSeconds - 600 },
];
let previewAlertIncidents: WorkspaceAlertIncident[] = [
  { id: 'incident_open_preview', ruleId: 'alert_rule_preview', eventKind: 'sync_conflict', severity: 'critical', status: 'open', summary: 'A shared configuration has a concurrent revision.', metadata: { documentId: 'shared_config_preview' }, rowVersion: 1, occurredAt: nowSeconds - 180, acknowledgedAt: null, resolvedAt: null },
  { id: 'incident_ack_preview', ruleId: 'alert_rule_preview', eventKind: 'quota_warning', severity: 'warning', status: 'acknowledged', summary: 'Workspace storage is above the warning threshold.', metadata: { utilization: '82%' }, rowVersion: 2, occurredAt: nowSeconds - 3_600, acknowledgedAt: nowSeconds - 1_800, resolvedAt: null },
  { id: 'incident_resolved_preview', ruleId: 'alert_rule_preview', eventKind: 'configuration_deleted', severity: 'info', status: 'resolved', summary: 'An obsolete shared configuration was deleted.', metadata: {}, rowVersion: 3, occurredAt: nowSeconds - 7_200, acknowledgedAt: nowSeconds - 7_000, resolvedAt: nowSeconds - 6_900 },
];
const previewAuditEvents: WorkspaceAuditEvent[] = [
  { id: 'audit_preview_1', eventType: 'workspace_configuration_revised', outcome: 'succeeded', occurredAt: nowSeconds - 300, deviceId: 'device_preview_001', reasonCode: null, metadata: { documentId: 'shared_config_preview', revision: '2' } },
  { id: 'audit_preview_2', eventType: 'challenge_issued', outcome: 'succeeded', occurredAt: nowSeconds - 240, deviceId: 'device_preview_001', reasonCode: null, metadata: {} },
];
let previewWebhookEndpoints: WebhookEndpoint[] = [
  { id: 'webhook_endpoint_preview', name: 'Operations receiver', url: 'https://events.example.test/camellia', eventTypes: ['alert.incident.opened', 'sync.conflict'], active: true, secretVersion: 1, rowVersion: 2, createdAt: nowSeconds - 86_400, updatedAt: nowSeconds - 600 },
];
const previewWebhookDeliveries: WebhookDeliverySummary[] = [
  { id: 'delivery_preview_1', eventId: 'event_preview_1', endpointId: 'webhook_endpoint_preview', eventType: 'alert.incident.opened', status: 'delivered', attemptCount: 1, nextAttemptAt: nowSeconds - 120, lastHttpStatus: 204, lastErrorCategory: null, deliveredAt: nowSeconds - 120, createdAt: nowSeconds - 180, updatedAt: nowSeconds - 120 },
  { id: 'delivery_preview_2', eventId: 'event_preview_2', endpointId: 'webhook_endpoint_preview', eventType: 'sync.conflict', status: 'retry', attemptCount: 2, nextAttemptAt: nowSeconds + 120, lastHttpStatus: 503, lastErrorCategory: 'server_error', deliveredAt: null, createdAt: nowSeconds - 90, updatedAt: nowSeconds - 30 },
];
let workspaceConflictPending = previewParameters.has('__ui_workspace_conflict');
let workspaceRetryPending = previewParameters.has('__ui_workspace_retry');

function requireWorkspacePermission(permission: WorkspacePermission) {
  if (!previewTeamProfile.permissions.includes(permission)) {
    throw { code: 'LICENSE_PERMISSION_DENIED', message: 'License service operation failed' };
  }
}

function workspaceRequest(args: InvokeArgs | undefined) {
  return objectArgs(objectArgs(args).request as InvokeArgs);
}

function recordWorkspaceMutation(command: string, request: Record<string, unknown>) {
  window.dispatchEvent(new CustomEvent('camellia-ui-preview:workspace-mutation', {
    detail: { command, operationId: request.operationId },
  }));
  if (workspaceRetryPending) {
    workspaceRetryPending = false;
    throw { code: 'NETWORK', message: 'The preview network response was interrupted.' };
  }
}

function requireRowVersion(actual: number, requested: unknown) {
  if (workspaceConflictPending) {
    workspaceConflictPending = false;
    throw { code: 'LICENSE_WORKSPACE_CONFLICT', message: 'License service operation failed' };
  }
  if (requested !== actual) {
    throw { code: 'LICENSE_WORKSPACE_CONFLICT', message: 'License service operation failed' };
  }
}

function mockExternalAction(command: string): null | Promise<null> {
  window.dispatchEvent(new CustomEvent('camellia-ui-preview:external-action', { detail: command }));
  if (failExternalActions && !failedExternalActions.has(command)) {
    failedExternalActions.add(command);
    throw {
      code: 'SYSTEM_INTEGRATION',
      message: 'The preview external action failed.',
    };
  }
  if (!slowExternalActions) return null;
  return new Promise((resolve) => window.setTimeout(() => resolve(null), 300));
}

type ProgramSelectionCommand =
  | 'get_program'
  | 'get_program_privilege_assessment'
  | 'list_actions';

function mockProgramSelectionResult<T>(
  command: ProgramSelectionCommand,
  programId: string,
  value: T,
): T | Promise<T> {
  window.dispatchEvent(new CustomEvent('camellia-ui-preview:program-selection-request', {
    detail: { command, programId },
  }));
  if (controlledProgramSelection) {
    return new Promise((resolve) => {
      const release = (event: Event) => {
        const commands = (event as CustomEvent<ProgramSelectionCommand[]>).detail;
        if (!commands.includes(command)) return;
        window.removeEventListener(
          'camellia-ui-preview:release-program-selection',
          release,
        );
        resolve(value);
      };
      window.addEventListener('camellia-ui-preview:release-program-selection', release);
    });
  }
  return value;
}

function unclassifiedCoreTarget(
  program: Exclude<ProgramKind, 'generic'>,
  reportedVersion?: string,
) {
  const fingerprintSha256 = 'a'.repeat(64);
  const releaseTarget = (tag: string, normalizedVersion: string, commitSha: string, report: string) => ({
    program,
    coordinate: {
      kind: 'release' as const,
      tag,
      normalizedVersion,
      commitSha,
    },
    basis: 'binaryReported' as const,
    catalogRevision: 'core-history-v1-20260811',
    reportedVersion: report,
    fingerprintSha256,
  });
  if (coreTargetPreview === 'release-xray' && program === 'xray') {
    return releaseTarget(
      'v26.3.27',
      '26.3.27',
      'd2758a023cd7f4174a5a5fa4ff66e487d4342ba0',
      'Xray v26.3.27',
    );
  }
  if (coreTargetPreview === 'release-mihomo' && program === 'mihomo') {
    return releaseTarget(
      'v1.19.29',
      '1.19.29',
      'e26714a181ac0e2fa803453c0a8e9a9ce94e31cb',
      'Mihomo Meta v1.19.29',
    );
  }
  if (coreTargetPreview === 'release-singbox' && program === 'singBox') {
    return releaseTarget(
      'v1.13.18',
      '1.13.18',
      '45ca32dcb966f07f97fc888fe8586e359dbe8405',
      'sing-box version 1.13.18',
    );
  }
  if (coreTargetPreview === 'release-singbox-new' && program === 'singBox') {
    return releaseTarget(
      'v1.14.0-beta.2',
      '1.14.0-beta.2',
      '03c3bf4c01e7b1fd165d0c46ff376828fa878aab',
      'sing-box version 1.14.0-beta.2',
    );
  }
  if (coreTargetPreview === 'future') {
    const displayName = program === 'xray'
      ? 'Xray'
      : program === 'singBox'
        ? 'sing-box'
        : 'Mihomo';
    return {
      program,
      coordinate: {
        kind: 'uncatalogued' as const,
        normalizedVersion: '99.0.0',
        reason: 'futureVersion' as const,
      },
      basis: 'binaryReported' as const,
      catalogRevision: 'core-history-v1-20260811',
      reportedVersion: `${displayName} version 99.0.0`,
      fingerprintSha256,
    };
  }
  if (coreTargetPreview === 'unknown') {
    return {
      program,
      coordinate: { kind: 'unknown' as const },
      basis: 'unknown' as const,
      catalogRevision: 'core-history-v1-20260811',
      fingerprintSha256,
    };
  }
  return {
    program,
    coordinate: {
      kind: 'uncatalogued' as const,
      ...(reportedVersion ? { normalizedVersion: reportedVersion.replace(/^.*?([0-9]+\.[0-9]+\.[0-9]+.*)$/, '$1') } : {}),
      reason: 'notFound' as const,
    },
    basis: 'binaryReported' as const,
    catalogRevision: 'core-history-v1-20260811',
    ...(reportedVersion ? { reportedVersion } : {}),
    fingerprintSha256,
  };
}

function managedExecutable(
  path: string,
  program?: Exclude<ProgramKind, 'generic'>,
  version?: string,
) {
  return {
    mode: 'managed' as const,
    path,
    compatibility: { mode: 'automatic' as const },
    metadata: {
      fingerprint: {
        sha256: 'a'.repeat(64),
        size: 18_462_720,
        modifiedUnixMs: Date.now() - 86_400_000,
      },
      ...(program ? {
        probe: {
          revision: 'core-binary-probe-v2-20260811',
          ...(version ? { reportedVersion: version } : {}),
          ...(version ? { normalizedVersion: version } : {}),
          cliObservations: [],
        },
        coreTarget: unclassifiedCoreTarget(program, version),
      } : {}),
    },
  };
}

function mockCompatibilityProfile(
  program: ProgramKind,
  targetOverride?: CoreTargetIdentity,
): CoreCompatibilityProfile {
  const targetProgram = program === 'generic' ? 'xray' : program;
  const target = targetOverride ?? unclassifiedCoreTarget(targetProgram);
  const featureIds = [
    'core.cli.nativeValidation',
    'core.cli.generatedSchema',
    'proxy.outbound.vless',
    'proxy.outbound.shadowsocks',
    'proxy.outbound.hysteria2',
    'proxy.outbound.tuicV5',
  ];
  const releaseKnown = target.coordinate.kind === 'release';
  const generatedSchemaKnown = releaseKnown
    && target.program === 'singBox'
    && target.coordinate.kind === 'release'
    && target.coordinate.normalizedVersion.startsWith('1.14.');
  return {
    target,
    profileHash: 'preview-core-profile-hash',
    decisions: featureIds.map((featureId) => ({
      featureId,
      availability: !releaseKnown
        ? 'unknown' as const
        : featureId === 'proxy.outbound.tuicV5' && target.program === 'xray'
          ? 'unsupported' as const
          : featureId === 'core.cli.generatedSchema' && !generatedSchemaKnown
            ? 'unknown' as const
            : 'supported' as const,
      lifecycle: !releaseKnown ? 'unreviewed' as const : 'active' as const,
      evidence: releaseKnown ? 'catalogConfirmed' as const : 'binaryReported' as const,
      attemptAllowed: true,
    })),
  };
}

const specs: Record<string, ProgramSpec> = {
  'local-agent': {
    // ProgramSpec has its own storage schema; this is unrelated to entitlement schema v3.
    schemaVersion: PROGRAM_SPEC_SCHEMA_VERSION,
    id: 'local-agent',
    name: 'Local telemetry agent',
    executable: managedExecutable('bin/local-agent'),
    type: { kind: 'generic', args: ['--listen', '127.0.0.1:4400'] },
    workingDirectory: 'bin',
    environment: { RUST_LOG: 'info' },
    autoStart: false,
    restartPolicy: 'onFailure',
    privilegePolicy: { mode: 'automatic' },
  },
  'sing-box-edge': {
    schemaVersion: PROGRAM_SPEC_SCHEMA_VERSION,
    id: 'sing-box-edge',
    name: 'Singapore edge gateway',
    executable: managedExecutable('bin/sing-box/sing-box', 'singBox', 'sing-box version 1.14.0'),
    type: { kind: 'singBox', extraArgs: ['run'] },
    managedConfig: {
      sources: [
        { mode: 'local', id: 'base', name: 'Base policy', enabled: true, path: 'profiles/base.json' },
        { mode: 'remote', id: 'routes', name: 'Managed routes', enabled: true, url: 'https://config.example.test/routes.json' },
      ],
      remoteUpdate: { enabled: true, intervalMinutes: 60 },
      singBoxDashboard: { listenPort: 9090, updateInterval: '1d' },
      singBoxClashDashboard: { listenPort: 9091 },
    },
    workingDirectory: 'bin/sing-box',
    environment: {},
    autoStart: true,
    restartPolicy: 'always',
    privilegePolicy: { mode: 'automatic' },
  },
  'xray-primary': {
    schemaVersion: PROGRAM_SPEC_SCHEMA_VERSION,
    id: 'xray-primary',
    name: 'Primary Xray routing fabric',
    executable: managedExecutable('bin/xray/xray', 'xray', 'Xray 25.6.8'),
    type: { kind: 'xray', extraArgs: ['run'] },
    managedConfig: {
      sources: [{ mode: 'local', id: 'primary', name: 'Production routing', enabled: true, path: 'profiles/xray.json' }],
      xrayDashboard: { apiPort: 10085, metricsPort: 11111 },
    },
    workingDirectory: 'bin/xray',
    environment: {},
    autoStart: true,
    restartPolicy: 'onFailure',
    privilegePolicy: { mode: 'automatic' },
  },
  'mihomo-alpha': {
    schemaVersion: PROGRAM_SPEC_SCHEMA_VERSION,
    id: 'mihomo-alpha',
    name: 'Mihomo Alpha gateway',
    executable: managedExecutable('bin/mihomo/mihomo', 'mihomo', 'Mihomo Meta alpha'),
    type: { kind: 'mihomo', extraArgs: [] },
    managedConfig: {
      sources: [
        { mode: 'local', id: 'base', name: 'Base policy', enabled: true, path: 'profiles/base.yaml' },
        { mode: 'remote', id: 'providers', name: 'Proxy providers', enabled: true, url: 'https://config.example.test/mihomo.yaml' },
      ],
      remoteUpdate: { enabled: true, intervalMinutes: 60 },
      mihomoDashboard: { listenPort: 9092 },
    },
    workingDirectory: 'bin/mihomo',
    environment: {},
    autoStart: true,
    restartPolicy: 'onFailure',
    privilegePolicy: { mode: 'automatic' },
  },
};

let states: Record<string, ProgramState> = {
  'local-agent': { status: 'stopped' },
  'sing-box-edge': { status: 'running', pid: 27182, startedUnixMs: Date.now() - 7_200_000 },
  'xray-primary': { status: 'running', pid: 31415, startedUnixMs: Date.now() - 12_480_000 },
  'mihomo-alpha': { status: 'running', pid: 27183, startedUnixMs: Date.now() - 5_400_000 },
};
if (previewParameters.has('__ui_exited_program')) {
  states = { ...states, 'sing-box-edge': { status: 'exited', code: 0, success: true } };
}

let appSettings: AppSettings = {
  version: 1,
  logRetention: 'preserve',
  logLevel: 'warn',
  programStartupDelayMs: 750,
  language: 'en',
};

const xrayBalancer: XrayBalancerInfo = {
  tag: 'regional-egress',
  selectors: ['proxy-'],
  candidates: ['proxy-sg', 'proxy-jp', 'proxy-us'],
  availableCandidates: ['proxy-sg', 'proxy-jp'],
  principleTargets: ['proxy-sg'],
  strategy: 'leastPing',
  fallbackTarget: 'direct',
};

const xrayBaseSnapshot: XrayDashboardSnapshot = {
  apiUrl: '127.0.0.1:10085',
  metricsUrl: 'http://127.0.0.1:11111/debug/vars',
  metrics: {
    stats: {
      inbound: { mixed: { uplink: 183_210_040, downlink: 921_447_118 } },
      outbound: {
        'proxy-sg': { uplink: 92_441_024, downlink: 511_202_190 },
        direct: { uplink: 11_029_481, downlink: 38_901_771 },
      },
      user: { 'operator@example.test': { uplink: 32_119_882, downlink: 146_220_104 } },
    },
    observatory: {
      singapore: { outbound_tag: 'proxy-sg', alive: true, delay: 34, health_ping: { all: 48, fail: 0 } },
      japan: { outbound_tag: 'proxy-jp', alive: true, delay: 72, health_ping: { all: 48, fail: 2 } },
      america: { outbound_tag: 'proxy-us', alive: false, delay: 0, last_error_reason: 'probe timeout' },
    },
  },
  systemStats: {
    uptimeSeconds: 12_480,
    allocatedBytes: 74_220_144,
    systemBytes: 128_441_032,
    goroutines: 86,
    liveObjects: 21_908,
    garbageCollections: 318,
  },
  topology: { inboundTags: ['mixed', 'api'], outboundTags: ['proxy-sg', 'proxy-jp', 'proxy-us', 'direct'] },
  onlineUsers: {
    policyEnabled: true,
    statusAvailable: true,
    loopbackOnly: false,
    userCount: 2,
    addressCount: 3,
    users: [
      { email: 'operator@example.test', online: true, addresses: [{ ip: '10.0.0.24', lastSeenUnix: nowSeconds - 12 }], uplink: 32_119_882, downlink: 146_220_104 },
      { email: 'tablet@example.test', online: true, addresses: [{ ip: '10.0.0.51', lastSeenUnix: nowSeconds - 34 }, { ip: 'fd00::51', lastSeenUnix: nowSeconds - 40 }], uplink: 8_110_212, downlink: 54_300_182 },
    ],
  },
  balancers: [xrayBalancer],
  fetchedUnixMs: Date.now(),
};

const denseXrayOutbounds = Array.from(
  { length: 9 },
  (_, index) => `regional-premium-observatory-outbound-${String(index + 1).padStart(2, '0')}-with-long-route-name`,
);
const denseXrayUsers = Array.from(
  { length: 7 },
  (_, index) => ({
    email: `operations-user-${String(index + 1).padStart(2, '0')}-with-extended-identity@example.test`,
    online: index % 3 === 0 ? undefined : index % 3 === 1,
    addresses: [
      { ip: `10.24.${index + 1}.128`, lastSeenUnix: nowSeconds - 12 - index * 9 },
      {
        ip: `fd00:24:${String(index + 1).padStart(4, '0')}::128`,
        lastSeenUnix: nowSeconds - 18 - index * 11,
      },
    ],
    uplink: 18_000_000 + index * 7_341_127,
    downlink: 81_000_000 + index * 19_831_091,
  }),
);
const xraySnapshot: XrayDashboardSnapshot = xrayDenseLayoutPreview
  ? {
      ...xrayBaseSnapshot,
      metrics: {
        stats: {
          inbound: Object.fromEntries(Array.from(
            { length: 5 },
            (_, index) => [
              `inbound-handler-${index + 1}-with-an-extended-name`,
              { uplink: 120_000_000 + index * 9_000_000, downlink: 640_000_000 + index * 21_000_000 },
            ],
          )),
          outbound: Object.fromEntries(denseXrayOutbounds.map((tag, index) => [
            tag,
            { uplink: 90_000_000 + index * 13_123_111, downlink: 480_000_000 + index * 31_456_789 },
          ])),
          user: Object.fromEntries(denseXrayUsers.map((user) => [
            user.email,
            { uplink: user.uplink, downlink: user.downlink },
          ])),
        },
        observatory: Object.fromEntries(denseXrayOutbounds.map((tag, index) => [
          `observatory-${index + 1}`,
          {
            outbound_tag: tag,
            alive: index !== 7,
            delay: index === 7 ? 0 : 28 + index * 17,
            health_ping: { all: 12_480 + index * 37, fail: index * 3 },
            ...(index === 7 ? { last_error_reason: 'probe timeout after repeated health checks' } : {}),
          },
        ])),
      },
      topology: {
        inboundTags: Array.from(
          { length: 5 },
          (_, index) => `inbound-handler-${index + 1}-with-an-extended-name`,
        ),
        outboundTags: [...denseXrayOutbounds, 'direct-fallback-with-a-long-descriptive-tag'],
      },
      onlineUsers: {
        policyEnabled: true,
        statusAvailable: true,
        loopbackOnly: false,
        userCount: denseXrayUsers.length,
        addressCount: denseXrayUsers.reduce((count, user) => count + user.addresses.length, 0),
        users: denseXrayUsers,
      },
      balancers: Array.from(
        { length: 3 },
        (_, index) => ({
          tag: `regional-balancer-${index + 1}-with-a-long-routing-control-name`,
          selectors: [
            `regional-premium-observatory-outbound-0${index + 1}`,
            `extended-selector-for-routing-group-${index + 1}`,
          ],
          candidates: denseXrayOutbounds.slice(index, index + 6),
          availableCandidates: denseXrayOutbounds.slice(index, index + 5),
          principleTargets: denseXrayOutbounds.slice(index, index + 2),
          strategy: index === 0 ? 'leastPing' : index === 1 ? 'roundRobin' : 'leastLoad',
          fallbackTarget: 'direct-fallback-with-a-long-descriptive-tag',
        })),
    }
  : xrayBaseSnapshot;

const previewStdout = Array.from(
  { length: 240 },
  (_, index) => `2026-07-10T12:${String(Math.floor(index / 60)).padStart(2, '0')}:${String(index % 60).padStart(2, '0')}Z INFO request ${index + 1} completed`,
).join('\n');
const previewStderr = Array.from(
  { length: 96 },
  (_, index) => `2026-07-10T12:18:${String(index % 60).padStart(2, '0')}Z WARN retry sample ${index + 1}`,
).join('\n');
const logReadCounts: Record<'stdout' | 'stderr', number> = { stdout: 0, stderr: 0 };
const previewConfigurationDocuments = new Map<string, {
  content: string;
  baseHash: string;
}>();
const previewConfigurationStates = new Map<string, ConfigurationStateView>();
const previewRawDrafts = new Map<string, RawDraftSession>();

function previewGuidedSettings(kind: ProgramSpec['type']['kind']): {
  descriptors: GuidedSettingDescriptor[];
  projection: GuidedProjection[];
} {
  if (kind === 'generic') return { descriptors: [], projection: [] };
  const descriptors: GuidedSettingDescriptor[] = [{
    id: 'logging.level',
    category: 'logging',
    label: 'Log level',
    description: 'Control the Core log verbosity, or follow the source configuration.',
    control: 'select',
    allowedValues: kind === 'xray'
      ? ['debug', 'info', 'warning', 'error', 'none']
      : ['trace', 'debug', 'info', 'warn', 'error', 'fatal', 'panic'],
  }];
  const values = new Map<string, unknown>([['logging.level', kind === 'xray' ? 'warning' : 'info']]);
  if (kind === 'singBox') {
    descriptors.push(
      { id: 'dns.strategy', category: 'dns', label: 'IP strategy', description: 'Configure IP strategy, or follow the source configuration.', control: 'select', allowedValues: ['prefer_ipv4', 'prefer_ipv6', 'ipv4_only', 'ipv6_only'] },
      { id: 'routing.autoDetectInterface', category: 'routing', label: 'Auto-detect interface', description: 'Configure interface detection, or follow the source configuration.', control: 'toggle', allowedValues: [] },
    );
    values.set('dns.strategy', 'prefer_ipv4');
    values.set('routing.autoDetectInterface', true);
  } else if (kind === 'xray') {
    descriptors.push({ id: 'routing.domainStrategy', category: 'routing', label: 'Domain strategy', description: 'Configure domain strategy, or follow the source configuration.', control: 'select', allowedValues: ['AsIs', 'IPIfNonMatch', 'IPOnDemand'] });
    values.set('routing.domainStrategy', 'AsIs');
  } else {
    descriptors.push(
      { id: 'network.ipv6', category: 'network', label: 'IPv6', description: 'Configure IPv6, or follow the source configuration.', control: 'toggle', allowedValues: [] },
      { id: 'tun.enabled', category: 'tun', label: 'TUN', description: 'Configure TUN, or follow the source configuration.', control: 'toggle', allowedValues: [] },
      { id: 'tun.strictRoute', category: 'tun', label: 'Strict routing', description: 'Configure strict routing, or follow the source configuration.', control: 'toggle', allowedValues: [], enabledWhen: 'tun.enabled' },
      { id: 'dns.enabled', category: 'dns', label: 'DNS', description: 'Configure DNS, or follow the source configuration.', control: 'toggle', allowedValues: [] },
      { id: 'dns.mode', category: 'dns', label: 'DNS mode', description: 'Configure DNS mode, or follow the source configuration.', control: 'select', allowedValues: ['normal', 'fake-ip', 'redir-host'] },
      { id: 'routing.mode', category: 'routing', label: 'Routing mode', description: 'Configure routing mode, or follow the source configuration.', control: 'select', allowedValues: ['rule', 'global', 'direct'] },
    );
    values.set('network.ipv6', true);
    values.set('tun.enabled', false);
    values.set('tun.strictRoute', false);
    values.set('dns.enabled', true);
    values.set('dns.mode', 'fake-ip');
    values.set('routing.mode', 'rule');
  }
  return {
    descriptors,
    projection: descriptors.map((descriptor) => ({
      settingId: descriptor.id,
      status: 'inherited',
      value: values.get(descriptor.id),
    })),
  };
}

function configurationState(programId: string): ConfigurationStateView {
  const existing = previewConfigurationStates.get(programId);
  if (existing) return structuredClone(existing);
  const spec = specs[programId];
  const document = configDocument(programId);
  const kind = spec?.type.kind ?? 'generic';
  const format = document.language === 'yaml' ? 'yaml' : 'jsonc';
  const guided = previewGuidedSettings(kind);
  const metadataTarget = kind === 'generic'
    ? undefined
    : spec?.executable.metadata?.coreTarget;
  const compatibilityProfile = mockCompatibilityProfile(kind, metadataTarget);
  const evidenceStale = coreEvidencePreview === 'stale';
  const evidenceMismatch = coreEvidencePreview === 'profile-mismatch';
  const sourceBlocked = configurationSourcePreview === 'invalid'
    || configurationSourcePreview === 'unavailable';
  const candidateNeedsAttention = evidenceStale || evidenceMismatch || sourceBlocked;
  const desiredHash = candidateNeedsAttention
    ? 'preview-pending-desired-hash'
    : 'preview-desired-hash';
  const generation = candidateNeedsAttention ? 2 : 1;
  const diagnostics = evidenceStale
    ? [{
        code: 'CORE_VALIDATION_EVIDENCE_STALE',
        message: 'The binary, compatibility profile, or configuration changed after native validation.',
        scope: { surface: 'compatibility' as const },
      }]
    : evidenceMismatch
      ? [{
          code: 'CORE_PROFILE_MISMATCH',
          message: 'The candidate was validated for a different compatibility profile.',
          scope: { surface: 'compatibility' as const },
        }]
      : sourceBlocked
        ? [{
            code: configurationSourcePreview === 'unavailable'
              ? 'SOURCE_UNAVAILABLE'
              : 'SOURCE_INVALID',
            message: configurationSourcePreview === 'unavailable'
              ? 'No parsed snapshot is available for this source.'
              : 'The latest source content is invalid; Applied and Last Known Good were retained.',
            scope: { surface: 'sources' as const, ownerId: 'preview-source' },
          }]
        : [];
  const rawPreviewPath = kind === 'xray'
    ? [{ kind: 'key' as const, key: 'log' }, { kind: 'key' as const, key: 'loglevel' }]
    : kind === 'singBox'
      ? [{ kind: 'key' as const, key: 'log' }, { kind: 'key' as const, key: 'level' }]
      : [{ kind: 'key' as const, key: 'log-level' }];
  const rawPreviewOperation: IntentOperation = {
    operation: 'set',
    path: rawPreviewPath,
    value: 'debug',
  };
  const rawPreviewDecision = {
    decisionId: `preview-raw-${programId}`,
    semanticPath: kind === 'xray' ? '/log/loglevel' : kind === 'singBox' ? '/log/level' : '/log-level',
    operation: rawPreviewOperation,
    status: 'superseded' as const,
    origin: 'migrated' as const,
    basis: {
      upstreamGeneration: 1,
      upstreamContentHash: 'preview-upstream-hash',
      upstreamPathHash: 'preview-upstream-path-hash',
    },
    upstreamValue: 'info',
    rawValue: 'debug',
  };
  const rawPreviewConflict: ConfigurationConflict = {
    semanticPath: rawPreviewDecision.semanticPath,
    reason: 'The upstream configuration changed after this Raw decision was made',
    severity: 'error',
    messageKey: 'RAW_DECISION_SUPERSEDED',
    scope: { surface: 'configuration', ownerId: rawPreviewDecision.decisionId },
    sourceValue: rawPreviewDecision.upstreamValue,
    rawValue: rawPreviewDecision.rawValue,
    effectiveValue: rawPreviewDecision.upstreamValue,
  };
  const state: ConfigurationStateView = {
    schemaVersion: 5,
    kind,
    format,
    generation,
    compatibilityProfile,
    sourceStatuses: (spec?.managedConfig?.sources ?? []).map((source) => ({
      sourceId: source.id,
      sourceName: source.name,
      freshness: !source.enabled
        ? 'disabled'
        : configurationSourcePreview === 'stale'
          ? 'stale'
          : configurationSourcePreview === 'invalid'
            ? 'invalid'
            : configurationSourcePreview === 'unavailable'
              ? 'unavailable'
              : 'fresh',
      snapshotHash: source.enabled && configurationSourcePreview !== 'unavailable'
        ? 'preview-source-hash'
        : undefined,
      ...(configurationSourcePreview === 'stale'
        ? { message: 'Latest read failed; using the last parsed snapshot.' }
        : configurationSourcePreview === 'invalid'
          ? { message: 'Latest content could not be parsed.' }
          : configurationSourcePreview === 'unavailable'
            ? { message: 'No usable observation or snapshot is available.' }
            : {}),
    })),
    sourceParseSummaries: {},
    provenance: [],
    desired: {
      revision: { generation, contentHash: desiredHash, createdUnixMs: Date.now() },
      content: document.content,
      compatibilityProfileHash: evidenceMismatch
        ? 'preview-previous-profile-hash'
        : compatibilityProfile.profileHash,
      validation: evidenceStale ? 'pending' : evidenceMismatch || sourceBlocked || rawAllOverridePreview ? 'invalid' : 'valid',
      ...(!candidateNeedsAttention ? {
        validationEvidence: {
          binarySha256: 'a'.repeat(64),
          profileHash: compatibilityProfile.profileHash,
          configHash: desiredHash,
          validatorContractRevision: 'preview-validator-v1',
          nativeAccepted: true,
          validatedUnixMs: Date.now(),
        },
      } : {}),
      diagnostics,
      conflicts: rawAllOverridePreview ? [rawPreviewConflict] : [],
    },
    appliedRevision: { generation: 1, contentHash: 'preview-desired-hash', createdUnixMs: Date.now() },
    lastKnownGoodRevision: { generation: 1, contentHash: 'preview-desired-hash', createdUnixMs: Date.now() },
    guidedDescriptors: guided.descriptors,
    guidedProjection: guided.projection,
    managedIntegrations: kind === 'singBox'
      ? [
          {
            integrationId: 'dashboard.singBoxApi',
            status: spec?.managedConfig?.singBoxDashboard ? 'explicit' : 'inactive',
            effectiveEnabled: !!spec?.managedConfig?.singBoxDashboard,
            rawPaths: [],
            issueIds: [],
          },
          {
            integrationId: 'dashboard.singBoxClash',
            status: spec?.managedConfig?.singBoxClashDashboard ? 'explicit' : 'inactive',
            effectiveEnabled: !!spec?.managedConfig?.singBoxClashDashboard,
            rawPaths: [],
            issueIds: [],
          },
        ]
      : kind === 'xray'
        ? [{ integrationId: 'dashboard.xray', status: spec?.managedConfig?.xrayDashboard ? 'explicit' : 'inactive', effectiveEnabled: !!spec?.managedConfig?.xrayDashboard, rawPaths: [], issueIds: [] }]
        : kind === 'mihomo'
          ? [{ integrationId: 'dashboard.mihomo', status: spec?.managedConfig?.mihomoDashboard ? 'explicit' : 'inactive', effectiveEnabled: !!spec?.managedConfig?.mihomoDashboard, rawPaths: [], issueIds: [] }]
          : [],
    compatibilityReferences: [
      { kind: 'release', tag: kind === 'xray' ? 'v26.3.27' : kind === 'singBox' ? 'v1.13.18' : 'v1.19.29' },
      { kind: 'commit', commitSha: 'a'.repeat(40) },
    ],
    workspace: {
      upstreamDocument: document.content,
      finalPreviewDocument: document.content,
      editableDocument: document.content,
      layerTrace: [],
      rawDecisions: rawAllOverridePreview ? [rawPreviewDecision] : [],
      sourceConflicts: [],
      layerConflicts: [],
      rawConflicts: rawAllOverridePreview ? [rawPreviewConflict] : [],
      diagnostics,
      saveStatus: rawAllOverridePreview ? 'blocked' : candidateNeedsAttention ? 'pendingValidation' : 'saved',
      validationStatus: evidenceStale ? 'pending' : evidenceMismatch || sourceBlocked || rawAllOverridePreview ? 'invalid' : 'valid',
      canSave: !rawAllOverridePreview,
      canValidate: !rawAllOverridePreview,
      canApply: !candidateNeedsAttention && !rawAllOverridePreview,
    },
  };
  if (rawAllOverridePreview && kind !== 'generic') {
    const rawPath = rawPreviewPath;
    state.guidedProjection = state.guidedProjection.map((projection) => {
      const path = previewGuidedPath(kind, projection.settingId);
      const overlaps = path && (path.every((segment, index) => rawPath[index]?.key === segment)
        || rawPath.every((segment, index) => segment.key === path[index]));
      return overlaps ? { ...projection, status: 'rawDecision', value: undefined } : projection;
    });
  }
  previewConfigurationStates.set(programId, state);
  return structuredClone(state);
}

function rawDraftSession(programId: string): RawDraftSession {
  const existing = previewRawDrafts.get(programId);
  if (existing) return structuredClone(existing);
  const state = configurationState(programId);
  if (previewParameters.has('__ui_raw_conflict') && programId === 'xray-primary') {
    const conflictId = 'preview-route-conflict';
    const userContent = state.desired.content.replace('proxy-sg', 'mine-route');
    const draft: RawDraftSession = {
      sessionId: `preview-draft-${programId}`,
      draftRevision: 1,
      basedOnGeneration: state.generation,
      baseContent: state.workspace.upstreamDocument,
      userContent,
      workingContent: userContent,
      conflicts: [{
        conflictId,
        segments: [
          { kind: 'key', key: 'route' },
          { kind: 'key', key: 'final' },
        ],
        semanticPath: '/route/final',
        displayPath: '/route/final',
        conflictType: 'value',
        severity: 'error',
        originalBase: 'proxy-sg',
        updatedBase: 'source-route',
        userValue: 'mine-route',
        suggestedActions: ['keepMine', 'useUpdated', 'manualEdit'],
        canCombine: false,
      }],
      resolutions: {},
      unresolvedConflictIds: [conflictId],
      updatedUnixMs: Date.now(),
    };
    previewRawDrafts.set(programId, structuredClone(draft));
    return draft;
  }
  const draft: RawDraftSession = {
    sessionId: `preview-draft-${programId}`,
    draftRevision: 0,
    basedOnGeneration: state.generation,
    baseContent: state.workspace.upstreamDocument,
    userContent: state.workspace.finalPreviewDocument,
    workingContent: state.workspace.finalPreviewDocument,
    conflicts: [],
    resolutions: {},
    unresolvedConflictIds: [],
    updatedUnixMs: Date.now(),
  };
  return draft;
}

function refreshPreviewUnresolvedConflicts(draft: RawDraftSession): void {
  let document: unknown;
  try {
    document = JSON.parse(draft.workingContent);
  } catch {
    draft.unresolvedConflictIds = draft.conflicts.map((conflict) => conflict.conflictId);
    return;
  }
  draft.unresolvedConflictIds = draft.conflicts
    .filter((conflict) => {
      const resolution = draft.resolutions[conflict.conflictId];
      if (!resolution) return true;
      const expected = previewConflictResolutionValue(conflict, resolution);
      const observed = previewConflictPathValue(document, conflict.segments);
      return !expected.present
        ? observed.present
        : !observed.present || JSON.stringify(observed.value) !== JSON.stringify(expected.value);
    })
    .map((conflict) => conflict.conflictId);
}

function previewConflictResolutionValue(
  conflict: RawDraftSession['conflicts'][number],
  resolution: RawDraftSession['resolutions'][string],
): { present: boolean; value?: unknown } {
  if (resolution === 'keepMine') {
    return conflict.conflictType === 'delete-vs-modify'
      ? { present: false }
      : { present: true, value: conflict.userValue };
  }
  if (resolution === 'useUpdated') {
    return conflict.conflictType === 'modify-vs-delete'
      ? { present: false }
      : { present: true, value: conflict.updatedBase };
  }
  if (resolution === 'combine') {
    return {
      present: true,
      value: previewCombineValues(conflict.updatedBase, conflict.userValue),
    };
  }
  return { present: true, value: resolution.manualEdit.value };
}

function previewCombineValues(updated: unknown, user: unknown): unknown {
  if (
    updated && user
    && typeof updated === 'object' && !Array.isArray(updated)
    && typeof user === 'object' && !Array.isArray(user)
  ) {
    const merged = structuredClone(updated) as Record<string, unknown>;
    for (const [key, value] of Object.entries(user)) {
      merged[key] = previewCombineValues(merged[key], value);
    }
    return merged;
  }
  return structuredClone(user);
}

function previewConflictPathValue(
  root: unknown,
  segments: RawDraftSession['conflicts'][number]['segments'],
): { present: boolean; value?: unknown } {
  let current = root;
  for (const segment of segments) {
    if (segment.kind === 'key') {
      if (!current || typeof current !== 'object' || Array.isArray(current)) {
        return { present: false };
      }
      const object = current as Record<string, unknown>;
      if (!(segment.key in object)) return { present: false };
      current = object[segment.key];
      continue;
    }
    if (!Array.isArray(current)) return { present: false };
    const matches = current.filter((item) => (
      item && typeof item === 'object' && !Array.isArray(item)
      && (item as Record<string, unknown>)[segment.field] === segment.value
    ));
    if (matches.length !== 1) return { present: false };
    current = matches[0];
  }
  return { present: true, value: current };
}

function updatePreviewConflictPath(
  root: unknown,
  segments: RawDraftSession['conflicts'][number]['segments'],
  selected: { present: boolean; value?: unknown },
): void {
  if (!root || typeof root !== 'object' || segments.length === 0) return;
  let current: unknown = root;
  for (const segment of segments.slice(0, -1)) {
    const next = previewConflictPathValue(current, [segment]);
    if (!next.present) return;
    current = next.value;
  }
  const last = segments[segments.length - 1];
  if (last.kind === 'key' && current && typeof current === 'object' && !Array.isArray(current)) {
    const object = current as Record<string, unknown>;
    if (selected.present) object[last.key] = structuredClone(selected.value);
    else delete object[last.key];
    return;
  }
  if (last.kind === 'identity' && Array.isArray(current)) {
    const index = current.findIndex((item) => (
      item && typeof item === 'object' && !Array.isArray(item)
      && (item as Record<string, unknown>)[last.field] === last.value
    ));
    if (selected.present && index >= 0) current[index] = structuredClone(selected.value);
    else if (selected.present) current.push(structuredClone(selected.value));
    else if (index >= 0) current.splice(index, 1);
  }
}

function previewContentHash(content: string): string {
  // The preview backend does not need cryptographic hashes, but it does need
  // content identity to exercise the same revision/evidence rules as the
  // native backend.  FNV-1a is deterministic, fast and deliberately marked
  // as a preview value so it can never be mistaken for native evidence.
  let hash = 0x811c9dc5;
  for (let index = 0; index < content.length; index += 1) {
    hash ^= content.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return `preview-content-${(hash >>> 0).toString(16).padStart(8, '0')}`;
}

function previewGuidedPath(kind: ProgramKind, settingId: string): string[] | undefined {
  const paths: Record<string, string[]> = {
    'logging.level': kind === 'xray' ? ['log', 'loglevel'] : ['log', 'level'],
    'dns.strategy': ['dns', 'strategy'],
    'routing.autoDetectInterface': ['route', 'auto_detect_interface'],
    'routing.domainStrategy': ['routing', 'domainStrategy'],
    'network.ipv6': ['ipv6'],
    'tun.enabled': ['tun', 'enable'],
    'tun.strictRoute': ['tun', 'strict-route'],
    'dns.enabled': ['dns', 'enable'],
    'dns.mode': ['dns', 'enhanced-mode'],
    'routing.mode': ['mode'],
  };
  return paths[settingId];
}

function setPreviewGuidedPath(
  root: unknown,
  path: string[],
  value: unknown,
): void {
  if (!root || typeof root !== 'object' || Array.isArray(root) || path.length === 0) return;
  let current = root as Record<string, unknown>;
  for (const key of path.slice(0, -1)) {
    const next = current[key];
    if (!next || typeof next !== 'object' || Array.isArray(next)) {
      current[key] = {};
    }
    current = current[key] as Record<string, unknown>;
  }
  const leaf = path[path.length - 1];
  if (value === undefined) delete current[leaf];
  else current[leaf] = structuredClone(value);
}

function previewPathSegments(path: import('../types').SemanticPathSegment[]): string[] {
  return path
    .filter((segment): segment is { kind: 'key'; key: string } => segment.kind === 'key')
    .map((segment) => segment.key);
}

function previewPathsOverlap(left: string[], right: string[]): boolean {
  const shared = Math.min(left.length, right.length);
  return left.slice(0, shared).every((segment, index) => segment === right[index]);
}

function previewReadPath(root: unknown, path: string[]): unknown {
  let current: unknown = root;
  for (const segment of path) {
    if (!current || typeof current !== 'object' || Array.isArray(current)) return undefined;
    current = (current as Record<string, unknown>)[segment];
  }
  return current;
}

function previewWritePath(root: unknown, path: string[], value: unknown): void {
  if (!root || typeof root !== 'object' || Array.isArray(root) || path.length === 0) return;
  let current = root as Record<string, unknown>;
  for (const segment of path.slice(0, -1)) {
    if (!current[segment] || typeof current[segment] !== 'object' || Array.isArray(current[segment])) {
      current[segment] = {};
    }
    current = current[segment] as Record<string, unknown>;
  }
  current[path[path.length - 1]] = structuredClone(value);
}

function previewDeletePath(root: unknown, path: string[]): void {
  if (!root || typeof root !== 'object' || Array.isArray(root) || path.length === 0) return;
  let current = root as Record<string, unknown>;
  for (const segment of path.slice(0, -1)) {
    if (!current[segment] || typeof current[segment] !== 'object' || Array.isArray(current[segment])) return;
    current = current[segment] as Record<string, unknown>;
  }
  delete current[path[path.length - 1]];
}

function recomputePreviewCandidate(state: ConfigurationStateView): void {
  let document: unknown;
  try {
    document = JSON.parse(state.workspace.upstreamDocument);
  } catch {
    return;
  }
  for (const decision of state.workspace.rawDecisions) {
    if (decision.status !== 'active' && decision.status !== 'resolved') continue;
    const path = previewPathSegments(decision.operation.path);
    if (decision.operation.operation === 'set') previewWritePath(document, path, decision.rawValue);
    else if (decision.operation.operation === 'delete') previewDeletePath(document, path);
  }
  const content = `${JSON.stringify(document, null, 2)}\n`;
  state.desired.content = content;
  state.desired.revision.contentHash = previewContentHash(content);
  const preservedConflicts = state.desired.conflicts.filter(
    (conflict) => conflict.messageKey !== 'RAW_DECISION_SUPERSEDED',
  );
  const rawConflicts = state.workspace.rawDecisions
    .filter((decision) => decision.status === 'superseded')
    .map((decision) => ({
      semanticPath: decision.semanticPath,
      reason: 'The upstream configuration changed after this Raw decision was made',
      severity: 'error' as const,
      messageKey: 'RAW_DECISION_SUPERSEDED',
      scope: { surface: 'configuration' as const, ownerId: decision.decisionId },
      sourceValue: decision.upstreamValue,
      rawValue: decision.rawValue,
      effectiveValue: decision.upstreamValue,
    }));
  state.desired.conflicts = [...preservedConflicts, ...rawConflicts];
  state.desired.validation = state.desired.conflicts.some((conflict) => conflict.severity === 'error')
    ? 'invalid'
    : 'pending';
  state.desired.validationEvidence = undefined;
  state.workspace.finalPreviewDocument = content;
  state.workspace.editableDocument = content;
  state.workspace.rawConflicts = rawConflicts;
  state.workspace.validationStatus = state.desired.validation;
  state.workspace.saveStatus = rawConflicts.length > 0 ? 'blocked' : 'pendingValidation';
  state.workspace.canSave = rawConflicts.length === 0;
  state.workspace.canValidate = rawConflicts.length === 0;
  state.workspace.canApply = false;
}

function updateConfigurationState(
  programId: string,
  update: (state: ConfigurationStateView) => void,
  advanceGeneration = true,
): ConfigurationStateView {
  const state = configurationState(programId);
  const previousContent = state.desired.content;
  update(state);
  if (state.desired.content !== previousContent) {
    state.desired.revision.contentHash = previewContentHash(state.desired.content);
    // A content mutation invalidates native evidence. The preview backend
    // mirrors the real coordinator's fail-closed rule; only the explicit
    // validation command creates fresh evidence.
    state.desired.validationEvidence = undefined;
    if (state.desired.validation === 'valid') state.desired.validation = 'pending';
    const savedDocument = previewConfigurationDocuments.get(programId);
    previewConfigurationDocuments.set(programId, {
      ...savedDocument,
      content: state.desired.content,
      baseHash: state.desired.revision.contentHash,
    });
  }
  if (advanceGeneration) {
    state.generation += 1;
    state.desired.revision = {
      ...state.desired.revision,
      generation: state.generation,
      createdUnixMs: Date.now(),
    };
  }
  state.workspace.finalPreviewDocument = state.desired.content;
  state.workspace.editableDocument = state.desired.content;
  state.workspace.validationStatus = state.desired.validation;
  state.workspace.diagnostics = structuredClone(state.desired.diagnostics);
  const blocked = state.desired.conflicts.some((conflict) => conflict.severity === 'error');
  state.workspace.saveStatus = blocked
    ? 'blocked'
    : state.desired.validation === 'pending'
      ? 'pendingValidation'
      : 'saved';
  state.workspace.canSave = !blocked;
  state.workspace.canValidate = !blocked;
  state.workspace.canApply = state.desired.validation === 'valid' && !blocked;
  previewConfigurationStates.set(programId, structuredClone(state));
  return state;
}

function growingLog(stream: 'stdout' | 'stderr'): string {
  const readCount = logReadCounts[stream]++;
  const base = stream === 'stderr' ? previewStderr : previewStdout;
  if (readCount === 0) return base;
  const appended = Array.from(
    { length: readCount * 8 },
    (_, index) => `LIVE ${stream} sample ${index + 1}`,
  ).join('\n');
  return `${base}\n${appended}`;
}

function summaries(): ProgramSummary[] {
  return Object.values(specs).map((spec) => ({
    id: spec.id,
    name: spec.name,
    kind: spec.type.kind,
    autoStart: spec.autoStart,
    state: states[spec.id] ?? { status: 'stopped' },
  }));
}

function detail(programId: string): ProgramDetail {
  const spec = specs[programId];
  if (!spec) throw { code: 'NOT_FOUND', message: `Unknown preview program: ${programId}` };
  return { spec: structuredClone(spec), state: structuredClone(states[programId]), workingDirectory: spec.workingDirectory };
}

function validatePreviewCompatibility(spec: ProgramSpec): void {
  const preference = spec.executable.compatibility;
  if (spec.type.kind === 'generic' && preference.mode !== 'automatic') {
    throw { code: 'INVALID_SPEC', message: 'Generic programs only support automatic compatibility.' };
  }
  if (preference.mode === 'release' && !preference.tag.trim()) {
    throw { code: 'INVALID_SPEC', message: 'Select an exact catalogued release before saving.' };
  }
  if (
    preference.mode === 'commit'
    && !/^[0-9a-f]{40}$/.test(preference.commitSha)
  ) {
    throw { code: 'INVALID_SPEC', message: 'Enter a lowercase 40-character catalogued commit SHA.' };
  }
  if (preference.mode === 'unknown' && preference.reference) {
    if (preference.reference.kind === 'release' && !preference.reference.tag.trim()) {
      throw { code: 'INVALID_SPEC', message: 'Enter a catalogued release tag or commit SHA.' };
    }
    if (preference.reference.kind === 'commit' && !/^[0-9a-f]{40}$/.test(preference.reference.commitSha)) {
      throw { code: 'INVALID_SPEC', message: 'Enter a catalogued release tag or commit SHA.' };
    }
  }
}

function configDocument(programId: string) {
  const saved = previewConfigurationDocuments.get(programId);
  if (specs[programId]?.type.kind === 'mihomo') {
    return {
      content: saved?.content ?? 'mode: rule\nlog-level: info\nexternal-controller: 127.0.0.1:9092\nexternal-ui: camellia-nexus-mihomo-dashboard\nrules:\n  - MATCH,DIRECT\n',
      baseHash: saved?.baseHash ?? 'preview-mihomo-hash',
      language: 'yaml',
      documentationUrl: 'https://wiki.metacubex.one/config/',
    };
  }
  const singBox = specs[programId]?.type.kind === 'singBox';
  return {
    content: saved?.content ?? (singBox
      ? '{\n  "log": { "level": "info" },\n  "outbounds": [\n    { "type": "direct", "tag": "direct" },\n    { "type": "socks", "tag": "proxy-sg", "server": "127.0.0.1", "server_port": 1080 }\n  ],\n  "route": { "final": "proxy-sg" }\n}\n'
      : '{\n  "log": { "level": "info" },\n  "route": { "final": "proxy-sg" }\n}\n'),
    baseHash: saved?.baseHash ?? 'preview-hash',
    language: 'jsonc',
    documentationUrl: 'https://example.test/docs',
    ...(singBox
      ? {
          configurationSchema: {
            source: 'programBinary' as const,
            dialect: 'draft2020-12' as const,
          },
        }
      : {}),
  };
}

const singBoxConfigurationSchema = {
  $schema: 'https://json-schema.org/draft/2020-12/schema',
  type: 'object',
  properties: {
    log: {
      type: 'object',
      properties: {
        level: {
          type: 'string',
          enum: ['trace', 'debug', 'info', 'warn', 'error', 'fatal', 'panic'],
        },
      },
      additionalProperties: false,
    },
    outbounds: {
      type: 'array',
      items: { $ref: '#/$defs/outbound' },
    },
    route: {
      type: 'object',
      properties: {
        final: {
          type: 'string',
          'x-tag-reference': 'outbound',
        },
      },
      additionalProperties: false,
    },
  },
  additionalProperties: false,
  $defs: {
    outbound: {
      oneOf: [
        {
          type: 'object',
          properties: {
            type: { const: 'direct' },
            tag: { type: 'string' },
            detour: {
              type: 'string',
              'x-tag-reference': 'outbound',
            },
          },
          required: ['type', 'tag'],
          additionalProperties: false,
        },
        {
          type: 'object',
          properties: {
            type: { const: 'socks' },
            tag: { type: 'string' },
            server: { type: 'string' },
            server_port: {
              type: 'integer',
              minimum: 1,
              maximum: 65_535,
            },
            detour: {
              type: 'string',
              'x-tag-reference': 'outbound',
            },
          },
          required: ['type', 'tag', 'server', 'server_port'],
          additionalProperties: false,
        },
      ],
    },
  },
};
const singBoxConfigurationSchemaContent = JSON.stringify(singBoxConfigurationSchema);

function stringArg(args: InvokeArgs | undefined, key: string): string {
  const value = objectArgs(args)[key];
  return typeof value === 'string' ? value : '';
}

function objectArgs(args: InvokeArgs | undefined): Record<string, unknown> {
  return args && typeof args === 'object' && !Array.isArray(args)
    ? args as Record<string, unknown>
    : {};
}

function setLifecycleState(args: InvokeArgs | undefined, state: ProgramState) {
  const id = stringArg(args, 'programId');
  if (specs[id]) states = { ...states, [id]: state };
}

function requireLifecycleAccess(action: 'start' | 'restart') {
  const access = deriveLicenseAccess(previewEntitlement.entitlementState, Object.keys(specs).length);
  if (!canUseProgramLifecycleAction(access, action)) {
    throw {
      code: 'LICENSE_REQUIRED',
      message: 'An active license is required for this action.',
    };
  }
}

export function installMockBackend() {
  const recoveredEntitlement = structuredClone(previewEntitlement);
  const tauriInternals = Reflect.get(window, '__TAURI_INTERNALS__');
  const nativeWindowAvailable = !!tauriInternals
    && typeof tauriInternals === 'object'
    && Reflect.has(tauriInternals, 'metadata');
  const handlePreviewInvoke = (command: string, args?: InvokeArgs) => {
    switch (command) {
      case 'frontend_ready': return null;
      case 'log_frontend_event': return null;
      case 'get_application_info': return { name: 'Camellia Nexus', version: '1.0.0-preview', author: 'Camellia', copyright: '© Camellia', license: 'Commercial', description: 'Desktop program orchestration', signatureStatus: 'notChecked' };
      case 'get_entitlement_state': return previewEntitlement;
      case 'get_license_service_settings': return { configured: true, baseUrl: 'https://license.example.test', loopbackDevelopment: false, authorizationConfigured: true, authorizationEndpoint: 'https://license.example.test/authorize' };
      case 'get_local_license_device': return { deviceId: 'device_preview_001', displayName: 'Design workstation', platform: 'Linux' };
      case 'get_license_devices': return { devices: [{ deviceId: 'device_preview_001', displayName: 'Design workstation', platform: 'Linux', state: 'active', lastSeenAt: nowSeconds }], nextCursor: null };
      case 'get_license_billing_summary':
        window.dispatchEvent(new CustomEvent('camellia-ui-preview:billing-request'));
        if (slowLicenseRefresh) {
          return new Promise((resolve) => window.setTimeout(
            () => resolve(structuredClone(previewBillingSummary)),
            800,
          ));
        }
        return structuredClone(previewBillingSummary);
      case 'submit_license_payment_claim': {
        if (!billingNeedsInformationPreview) {
          throw { code: 'INVALID_REQUEST', message: 'No preview invoice is open.' };
        }
        const submission = objectArgs(objectArgs(args).submission as InvokeArgs) as Partial<CustomerPaymentSubmission>;
        const previous = previewBillingSummary.paymentClaims[0];
        if (
          !previous
          || typeof submission.operationId !== 'string'
          || typeof submission.invoiceId !== 'string'
          || typeof submission.paymentMethodId !== 'string'
          || typeof submission.externalTransactionId !== 'string'
          || typeof submission.paidAmount !== 'string'
          || typeof submission.paidAsset !== 'string'
          || typeof submission.paidAt !== 'number'
        ) throw { code: 'INVALID_REQUEST', message: 'Invalid preview payment submission.' };
        window.dispatchEvent(new CustomEvent('camellia-ui-preview:billing-submission', {
          detail: structuredClone(submission),
        }));
        const updated: ManualPaymentClaim = {
          ...previous,
          rowVersion: previous.rowVersion + 1,
          paymentMethodId: submission.paymentMethodId,
          externalTransactionId: submission.externalTransactionId,
          paidAmount: submission.paidAmount,
          paidAsset: submission.paidAsset,
          paidAt: submission.paidAt,
          payerName: typeof submission.payerName === 'string' ? submission.payerName : null,
          note: typeof submission.note === 'string' ? submission.note : null,
          status: 'submitted',
          reviewedBy: null,
          reviewReason: null,
          submittedAt: nowSeconds,
          updatedAt: nowSeconds,
        };
        previewBillingSummary = { ...previewBillingSummary, paymentClaims: [updated] };
        return structuredClone(updated);
      }
      case 'get_license_team_profile':
        window.dispatchEvent(new CustomEvent('camellia-ui-preview:team-profile-request'));
        if (licenseTimeoutFailures) {
          throw { code: 'TIMEOUT', message: 'License service operation failed', details: 'license operation timed out' };
        }
        return structuredClone(previewTeamProfile);
      case 'get_license_team_members': {
        window.dispatchEvent(new CustomEvent('camellia-ui-preview:team-members-request'));
        if (!previewTeamProfile.permissions.includes('team.read')) {
          throw { code: 'LICENSE_PERMISSION_DENIED', message: 'License service operation failed' };
        }
        if (pagedTeamMembers) {
          const request = objectArgs(objectArgs(args).request as InvokeArgs);
          const ordered = [...previewTeamMembers].sort((left, right) =>
            left.createdAt - right.createdAt || left.id.localeCompare(right.id));
          const offset = request.cursor === 'preview_member_page_2' ? 2 : 0;
          const members = ordered.slice(offset, offset + 2);
          const hasMore = offset + members.length < ordered.length;
          return {
            members: structuredClone(members),
            nextCursor: hasMore ? 'preview_member_page_2' : null,
            hasMore,
          };
        }
        return {
          members: structuredClone(previewTeamMembers),
          nextCursor: null,
          hasMore: false,
        };
      }
      case 'create_license_team_invitation': {
        if (!teamPreview || !previewTeamProfile.permissions.includes('team.manage')) {
          throw { code: 'LICENSE_PERMISSION_DENIED', message: 'License service operation failed' };
        }
        const request = objectArgs(objectArgs(args).request as InvokeArgs);
        return commitTeamOperation('create_invitation', request, () => {
          const member: WorkspaceMember = {
            id: 'member_invited_preview',
            email: typeof request.email === 'string' ? request.email : 'invitee@example.test',
            displayName: typeof request.displayName === 'string' ? request.displayName : 'Invited member',
            role: request.role === 'admin' || request.role === 'billing' || request.role === 'auditor' || request.role === 'viewer'
              ? request.role
              : 'operator',
            status: 'invited',
            boundDeviceCount: 0,
            rowVersion: 1,
            createdAt: nowSeconds,
            updatedAt: nowSeconds,
          };
          previewTeamMembers = [...previewTeamMembers.filter((item) => item.id !== member.id), member];
          previewTeamProfile.memberCount = previewTeamMembers.filter((item) => item.status !== 'removed').length;
          return {
            id: 'team_invite_preview',
            member,
            invitationToken: 'preview-invitation-token-0123456789abcdef',
            expiresAt: nowSeconds + 604_800,
          };
        });
      }
      case 'accept_license_team_invitation': throw {
        code: 'LICENSE_TEAM_INVITATION_INVALID',
        message: 'License service operation failed',
      };
      case 'update_license_team_member': {
        if (!teamPreview || !previewTeamProfile.permissions.includes('team.manage')) {
          throw { code: 'LICENSE_PERMISSION_DENIED', message: 'License service operation failed' };
        }
        const input = objectArgs(args);
        const memberId = typeof input.memberId === 'string' ? input.memberId : '';
        const request = objectArgs(input.request as InvokeArgs);
        return commitTeamOperation('update_member', { ...request, memberId }, () => {
          const index = previewTeamMembers.findIndex((member) => member.id === memberId);
          const member = previewTeamMembers[index];
          if (
            index < 0
            || !member
            || request.rowVersion !== member.rowVersion
            || !['active', 'suspended', 'removed'].includes(String(request.status))
          ) {
            throw { code: 'LICENSE_WORKSPACE_CONFLICT', message: 'License service operation failed' };
          }
          const updated: WorkspaceMember = {
            ...member,
            role: request.role === 'admin' || request.role === 'billing' || request.role === 'auditor' || request.role === 'viewer'
              ? request.role
              : 'operator',
            status: request.status as WorkspaceMember['status'],
            rowVersion: member.rowVersion + 1,
            updatedAt: nowSeconds,
          };
          previewTeamMembers[index] = updated;
          previewTeamMembers = [...previewTeamMembers];
          previewTeamProfile.memberCount = previewTeamMembers.filter((item) => item.status !== 'removed').length;
          return updated;
        });
      }
      case 'create_license_team_device_enrollment': {
        const member = previewTeamProfile.member;
        if (!teamPreview || !member || member.status !== 'active') {
          throw { code: 'LICENSE_PERMISSION_DENIED', message: 'License service operation failed' };
        }
        const request = objectArgs(objectArgs(args).request as InvokeArgs);
        return commitTeamOperation('create_device_enrollment', request, () => ({
          id: 'device_enrollment_preview',
          memberId: member.id,
          enrollmentToken: 'preview-device-enrollment-token-0123456789abcdef',
          expiresAt: nowSeconds + 900,
        }));
      }
      case 'create_license_team_member_device_enrollment': {
        const input = objectArgs(args);
        const request = objectArgs(input.request as InvokeArgs);
        return commitTeamOperation(
          'create_member_device_enrollment',
          { ...request, memberId: input.memberId },
          () => {
            const member = previewTeamMembers.find((item) => item.id === input.memberId);
            if (!teamPreview || !member || member.status !== 'active' || member.boundDeviceCount !== 0) {
              throw { code: 'LICENSE_PERMISSION_DENIED', message: 'License service operation failed' };
            }
            return {
              id: 'device_recovery_enrollment_preview',
              memberId: member.id,
              enrollmentToken: 'preview-recovery-enrollment-token-0123456789abcdef',
              expiresAt: nowSeconds + 900,
            };
          },
        );
      }
      case 'accept_license_team_device_enrollment': {
        const request = objectArgs(objectArgs(args).request as InvokeArgs);
        return commitTeamOperation('accept_device_enrollment', request, () => {
          if (
            !teamUnlinkedPreview
            || !!previewTeamProfile.member
            || request.enrollmentToken !== 'preview-device-enrollment-token-0123456789abcdef'
          ) {
            throw {
              code: 'LICENSE_TEAM_DEVICE_ENROLLMENT_INVALID',
              message: 'License service operation failed',
            };
          }
          const linkedMember: WorkspaceMember = {
            id: 'member_auditor_preview',
            email: 'auditor@example.test',
            displayName: 'Preview auditor',
            role: 'auditor',
            status: 'active',
            boundDeviceCount: 1,
            rowVersion: 3,
            createdAt: nowSeconds,
            updatedAt: nowSeconds,
          };
          previewTeamProfile = {
            ...previewTeamProfile,
            member: linkedMember,
            permissions: permissionsForRole('auditor'),
          };
          return previewTeamProfile;
        });
      }
      case 'leave_license_team_workspace': {
        const request = objectArgs(objectArgs(args).request as InvokeArgs);
        return commitTeamOperation('leave_workspace', request, () => {
          const member = previewTeamProfile.member;
          if (!member || member.role === 'owner' || member.status !== 'active') {
            throw { code: 'LICENSE_PERMISSION_DENIED', message: 'License service operation failed' };
          }
          if (request.rowVersion !== member.rowVersion) {
            throw { code: 'LICENSE_WORKSPACE_CONFLICT', message: 'License service operation failed' };
          }
          previewTeamProfile = { ...previewTeamProfile, member: null, permissions: [] };
          previewTeamMembers = [];
          previewEntitlement = { generation: previewEntitlement.generation + 1, entitlementState: { status: 'unauthenticated' } };
          return null;
        });
      }
      case 'transfer_license_team_ownership': {
        const request = objectArgs(objectArgs(args).request as InvokeArgs);
        return commitTeamOperation('transfer_ownership', request, () => {
          const owner = previewTeamProfile.member;
          const newOwnerIndex = previewTeamMembers.findIndex((member) =>
            member.id === request.newOwnerMemberId && member.role === 'admin' && member.status === 'active'
          );
          if (!owner || owner.role !== 'owner' || owner.status !== 'active' || newOwnerIndex < 0) {
            throw { code: 'LICENSE_PERMISSION_DENIED', message: 'License service operation failed' };
          }
          if (ownershipConflictPending) {
            ownershipConflictPending = false;
            previewTeamProfile = {
              ...previewTeamProfile,
              member: { ...owner, rowVersion: owner.rowVersion + 1 },
            };
            previewTeamMembers = previewTeamMembers.map((member, index) =>
              index === newOwnerIndex ? { ...member, rowVersion: member.rowVersion + 1 } : member
            );
            throw { code: 'LICENSE_WORKSPACE_CONFLICT', message: 'License service operation failed' };
          }
          const freshOwner = previewTeamProfile.member;
          const freshNewOwner = previewTeamMembers[newOwnerIndex];
          if (
            !freshOwner
            || request.ownerRowVersion !== freshOwner.rowVersion
            || request.newOwnerRowVersion !== freshNewOwner.rowVersion
          ) {
            throw { code: 'LICENSE_WORKSPACE_CONFLICT', message: 'License service operation failed' };
          }
          const previousOwner: WorkspaceMember = {
            ...freshOwner,
            role: 'admin',
            rowVersion: freshOwner.rowVersion + 1,
            updatedAt: nowSeconds,
          };
          const newOwner: WorkspaceMember = {
            ...freshNewOwner,
            role: 'owner',
            rowVersion: freshNewOwner.rowVersion + 1,
            updatedAt: nowSeconds,
          };
          previewTeamProfile = {
            ...previewTeamProfile,
            member: previousOwner,
            permissions: permissionsForRole('admin'),
          };
          previewTeamMembers = previewTeamMembers.map((member, index) =>
            index === newOwnerIndex ? newOwner : member
          );
          return { previousOwner, newOwner };
        });
      }
      case 'get_license_workspace_configurations': {
        requireWorkspacePermission('shared.read');
        const request = workspaceRequest(args);
        const includeDeleted = request.includeDeleted === true
          && previewTeamProfile.permissions.includes('shared.write');
        const viewer = previewTeamProfile.member?.role === 'viewer';
        const configurations = previewSharedConfigurations
          .filter((configuration) => includeDeleted || !configuration.deletedAt)
          .filter((configuration) => !viewer || !!configuration.publishedRevision)
          .map((configuration) => viewer
            ? { ...configuration, draftRevision: configuration.publishedRevision ?? configuration.draftRevision }
            : configuration);
        return mockTeamResult({
          configurations: structuredClone(configurations),
          usage: {
            activeDocumentCount: previewSharedConfigurations.filter((item) => !item.deletedAt).length,
            maxActiveDocuments: 200,
            revisionPlaintextBytes: previewSharedConfigurations.reduce((total, item) => total + item.plaintextBytes, 0),
            maxRevisionPlaintextBytes: 2_147_483_648,
            rowVersion: 4,
          },
          nextCursor: null,
          hasMore: false,
        });
      }
      case 'get_license_workspace_configuration': {
        requireWorkspacePermission('shared.read');
        const documentId = stringArg(args, 'documentId');
        const content = previewSharedContents.get(documentId);
        if (!content || (content.deletedAt && previewTeamProfile.member?.role === 'viewer')) {
          throw { code: 'LICENSE_WORKSPACE_NOT_FOUND', message: 'License service operation failed' };
        }
        return mockTeamResult(structuredClone(content));
      }
      case 'create_license_workspace_configuration': {
        requireWorkspacePermission('shared.write');
        const request = workspaceRequest(args);
        recordWorkspaceMutation(command, request);
        const id = `shared_config_${previewSharedConfigurations.length + 1}`;
        const summary: SharedConfigurationSummary = {
          id,
          name: String(request.name),
          programKind: request.programKind === 'singBox' || request.programKind === 'xray' || request.programKind === 'mihomo' ? request.programKind : 'generic',
          rowVersion: 1,
          draftRevision: 1,
          publishedRevision: null,
          deletedAt: null,
          contentSha256: 'c'.repeat(64),
          plaintextBytes: String(request.content ?? '').length,
          createdAt: nowSeconds,
          updatedAt: nowSeconds,
        };
        previewSharedConfigurations = [summary, ...previewSharedConfigurations];
        previewSharedContents.set(id, {
          ...summary,
          revision: 1,
          input: String(request.input ?? ''),
          content: String(request.content ?? ''),
          revisionCreatedAt: nowSeconds,
        });
        return mockTeamResult({ resourceType: 'shared_configuration', resourceId: id, rowVersion: 1, cursor: 13 });
      }
      case 'revise_license_workspace_configuration': {
        requireWorkspacePermission('shared.write');
        const request = workspaceRequest(args);
        recordWorkspaceMutation(command, request);
        const documentId = stringArg(args, 'documentId');
        const index = previewSharedConfigurations.findIndex((item) => item.id === documentId);
        if (index < 0) throw { code: 'LICENSE_WORKSPACE_NOT_FOUND', message: 'License service operation failed' };
        const current = previewSharedConfigurations[index];
        requireRowVersion(current.rowVersion, request.baseRowVersion);
        const next: SharedConfigurationSummary = {
          ...current,
          name: String(request.name),
          programKind: request.programKind === 'singBox' || request.programKind === 'xray' || request.programKind === 'mihomo' ? request.programKind : 'generic',
          rowVersion: current.rowVersion + 1,
          draftRevision: current.draftRevision + 1,
          contentSha256: 'd'.repeat(64),
          plaintextBytes: String(request.content ?? '').length,
          updatedAt: nowSeconds,
        };
        previewSharedConfigurations[index] = next;
        previewSharedContents.set(documentId, {
          ...next,
          revision: next.draftRevision,
          input: String(request.input ?? ''),
          content: String(request.content ?? ''),
          revisionCreatedAt: nowSeconds,
        });
        return mockTeamResult({ resourceType: 'shared_configuration', resourceId: documentId, rowVersion: next.rowVersion, cursor: 13 });
      }
      case 'publish_license_workspace_configuration': {
        requireWorkspacePermission('shared.publish');
        const request = workspaceRequest(args);
        recordWorkspaceMutation(command, request);
        const documentId = stringArg(args, 'documentId');
        const index = previewSharedConfigurations.findIndex((item) => item.id === documentId);
        if (index < 0) throw { code: 'LICENSE_WORKSPACE_NOT_FOUND', message: 'License service operation failed' };
        const current = previewSharedConfigurations[index];
        requireRowVersion(current.rowVersion, request.baseRowVersion);
        previewSharedConfigurations[index] = {
          ...current,
          publishedRevision: typeof request.revision === 'number' ? request.revision : current.draftRevision,
          rowVersion: current.rowVersion + 1,
          updatedAt: nowSeconds,
        };
        return mockTeamResult({ resourceType: 'shared_configuration', resourceId: documentId, rowVersion: current.rowVersion + 1, cursor: 13 });
      }
      case 'delete_license_workspace_configuration':
      case 'restore_license_workspace_configuration': {
        requireWorkspacePermission('shared.write');
        const request = workspaceRequest(args);
        recordWorkspaceMutation(command, request);
        const documentId = stringArg(args, 'documentId');
        const index = previewSharedConfigurations.findIndex((item) => item.id === documentId);
        if (index < 0) throw { code: 'LICENSE_WORKSPACE_NOT_FOUND', message: 'License service operation failed' };
        const current = previewSharedConfigurations[index];
        requireRowVersion(current.rowVersion, request.baseRowVersion);
        const next = {
          ...current,
          deletedAt: command.startsWith('delete_') ? nowSeconds : null,
          rowVersion: current.rowVersion + 1,
          updatedAt: nowSeconds,
        };
        previewSharedConfigurations[index] = next;
        const content = previewSharedContents.get(documentId);
        if (content) previewSharedContents.set(documentId, { ...content, ...next });
        return mockTeamResult({ resourceType: 'shared_configuration', resourceId: documentId, rowVersion: next.rowVersion, cursor: 13 });
      }
      case 'purge_license_workspace_configuration': {
        requireWorkspacePermission('shared.purge');
        const request = workspaceRequest(args);
        recordWorkspaceMutation(command, request);
        const documentId = stringArg(args, 'documentId');
        const index = previewSharedConfigurations.findIndex((item) => item.id === documentId);
        if (index < 0) throw { code: 'LICENSE_WORKSPACE_NOT_FOUND', message: 'License service operation failed' };
        const current = previewSharedConfigurations[index];
        requireRowVersion(current.rowVersion, request.baseRowVersion);
        if (!current.deletedAt || current.deletedAt > nowSeconds - 30 * 86_400) {
          throw { code: 'LICENSE_WORKSPACE_RETENTION_ACTIVE', message: 'License service operation failed' };
        }
        previewSharedConfigurations.splice(index, 1);
        previewSharedContents.delete(documentId);
        return mockTeamResult({ resourceType: 'shared_configuration', resourceId: documentId, rowVersion: current.rowVersion + 1, cursor: 13 });
      }
      case 'get_license_workspace_sync_feed': {
        requireWorkspacePermission('sync.read');
        const request = workspaceRequest(args);
        const cursor = typeof request.cursor === 'number' ? request.cursor : 0;
        return mockTeamResult({ changes: structuredClone(previewSyncChanges.filter((change) => change.cursor > cursor)), nextCursor: previewSyncChanges.at(-1)?.cursor ?? cursor, hasMore: false });
      }
      case 'get_license_workspace_checkpoint':
        requireWorkspacePermission('sync.read');
        return mockTeamResult(structuredClone(previewCheckpoint));
      case 'advance_license_workspace_checkpoint': {
        requireWorkspacePermission('sync.write');
        const request = workspaceRequest(args);
        recordWorkspaceMutation(command, request);
        requireRowVersion(previewCheckpoint?.rowVersion ?? 0, request.baseRowVersion);
        previewCheckpoint = { cursor: Number(request.cursor), rowVersion: (previewCheckpoint?.rowVersion ?? 0) + 1, updatedAt: nowSeconds };
        return mockTeamResult({ resourceType: 'device_checkpoint', resourceId: 'device_preview_001', rowVersion: previewCheckpoint.rowVersion, cursor: previewCheckpoint.cursor });
      }
      case 'get_license_workspace_alert_rules': {
        requireWorkspacePermission('alerts.read');
        return mockTeamResult({ rules: structuredClone(previewAlertRules), nextCursor: null, hasMore: false });
      }
      case 'create_license_workspace_alert_rule': {
        requireWorkspacePermission('alerts.manage');
        const request = workspaceRequest(args);
        recordWorkspaceMutation(command, request);
        const id = `alert_rule_${previewAlertRules.length + 1}`;
        previewAlertRules = [{ id, name: String(request.name), eventKind: request.eventKind as WorkspaceAlertRule['eventKind'], severity: request.severity as WorkspaceAlertRule['severity'], enabled: request.enabled === true, rowVersion: 1, createdAt: nowSeconds, updatedAt: nowSeconds }, ...previewAlertRules];
        return mockTeamResult({ resourceType: 'alert_rule', resourceId: id, rowVersion: 1, cursor: 13 });
      }
      case 'update_license_workspace_alert_rule': {
        requireWorkspacePermission('alerts.manage');
        const request = workspaceRequest(args);
        recordWorkspaceMutation(command, request);
        const ruleId = stringArg(args, 'ruleId');
        const index = previewAlertRules.findIndex((rule) => rule.id === ruleId);
        if (index < 0) throw { code: 'LICENSE_WORKSPACE_NOT_FOUND', message: 'License service operation failed' };
        const current = previewAlertRules[index];
        requireRowVersion(current.rowVersion, request.baseRowVersion);
        previewAlertRules[index] = { ...current, name: String(request.name), eventKind: request.eventKind as WorkspaceAlertRule['eventKind'], severity: request.severity as WorkspaceAlertRule['severity'], enabled: request.enabled === true, rowVersion: current.rowVersion + 1, updatedAt: nowSeconds };
        return mockTeamResult({ resourceType: 'alert_rule', resourceId: ruleId, rowVersion: current.rowVersion + 1, cursor: 13 });
      }
      case 'delete_license_workspace_alert_rule': {
        requireWorkspacePermission('alerts.manage');
        const request = workspaceRequest(args);
        recordWorkspaceMutation(command, request);
        const ruleId = stringArg(args, 'ruleId');
        const index = previewAlertRules.findIndex((rule) => rule.id === ruleId);
        if (index < 0) throw { code: 'LICENSE_WORKSPACE_NOT_FOUND', message: 'License service operation failed' };
        requireRowVersion(previewAlertRules[index].rowVersion, request.baseRowVersion);
        previewAlertRules.splice(index, 1);
        return mockTeamResult({ resourceType: 'alert_rule', resourceId: ruleId, rowVersion: 2, cursor: 13 });
      }
      case 'get_license_workspace_alert_incidents': {
        if (!previewTeamProfile.permissions.includes('alerts.read') && !previewTeamProfile.permissions.includes('alerts.history.read')) requireWorkspacePermission('alerts.read');
        const request = workspaceRequest(args);
        const canReadHistory = previewTeamProfile.permissions.includes('alerts.history.read');
        const incidents = previewAlertIncidents.filter((incident) => (canReadHistory || incident.status !== 'resolved') && (!request.status || incident.status === request.status));
        return mockTeamResult({ incidents: structuredClone(incidents), nextCursor: null, hasMore: false });
      }
      case 'acknowledge_license_workspace_alert_incident':
      case 'resolve_license_workspace_alert_incident': {
        requireWorkspacePermission(command.startsWith('acknowledge_') ? 'alerts.ack' : 'alerts.manage');
        const request = workspaceRequest(args);
        recordWorkspaceMutation(command, request);
        const incidentId = stringArg(args, 'incidentId');
        const index = previewAlertIncidents.findIndex((incident) => incident.id === incidentId);
        if (index < 0) throw { code: 'LICENSE_WORKSPACE_NOT_FOUND', message: 'License service operation failed' };
        const current = previewAlertIncidents[index];
        requireRowVersion(current.rowVersion, request.baseRowVersion);
        const status = command.startsWith('acknowledge_') ? 'acknowledged' : 'resolved';
        previewAlertIncidents[index] = { ...current, status, rowVersion: current.rowVersion + 1, acknowledgedAt: status === 'acknowledged' ? nowSeconds : current.acknowledgedAt, resolvedAt: status === 'resolved' ? nowSeconds : null };
        return mockTeamResult({ resourceType: 'alert_incident', resourceId: incidentId, rowVersion: current.rowVersion + 1, cursor: 13 });
      }
      case 'get_license_workspace_audit_events': {
        requireWorkspacePermission('audit.read');
        const request = workspaceRequest(args);
        const events = previewAuditEvents.filter((event) => !request.eventType || event.eventType === request.eventType);
        return mockTeamResult({ events: structuredClone(events), nextCursor: null, hasMore: false });
      }
      case 'get_license_workspace_audit_event_types':
        requireWorkspacePermission('audit.read');
        return mockTeamResult({
          eventTypes: [...new Set(previewAuditEvents.map((event) => event.eventType))],
        });
      case 'export_license_workspace_audit_events': {
        requireWorkspacePermission('audit.export');
        const request = workspaceRequest(args);
        window.dispatchEvent(new CustomEvent('camellia-ui-preview:audit-export-request', {
          detail: { limit: request.limit },
        }));
        const events = previewAuditEvents.filter((event) => !request.eventType || event.eventType === request.eventType);
        return mockTeamResult({ events: structuredClone(events), nextCursor: null, truncated: false });
      }
      case 'get_license_workspace_webhook_endpoints':
        requireWorkspacePermission('webhooks.read');
        return mockTeamResult(structuredClone(previewWebhookEndpoints));
      case 'create_license_workspace_webhook_endpoint': {
        requireWorkspacePermission('webhooks.manage');
        const request = workspaceRequest(args);
        recordWorkspaceMutation(command, request);
        const endpoint: WebhookEndpoint = { id: `webhook_endpoint_${previewWebhookEndpoints.length + 1}`, name: String(request.name), url: String(request.url), eventTypes: Array.isArray(request.eventTypes) ? request.eventTypes.map(String) : [], active: request.active === true, secretVersion: 1, rowVersion: 1, createdAt: nowSeconds, updatedAt: nowSeconds };
        previewWebhookEndpoints = [...previewWebhookEndpoints, endpoint];
        return mockTeamResult({ endpoint: structuredClone(endpoint), secret: 'preview-webhook-secret-0123456789abcdef' });
      }
      case 'update_license_workspace_webhook_endpoint': {
        requireWorkspacePermission('webhooks.manage');
        const request = workspaceRequest(args);
        recordWorkspaceMutation(command, request);
        const endpointId = stringArg(args, 'endpointId');
        const index = previewWebhookEndpoints.findIndex((endpoint) => endpoint.id === endpointId);
        if (index < 0) throw { code: 'LICENSE_WEBHOOK_NOT_FOUND', message: 'License service operation failed' };
        const current = previewWebhookEndpoints[index];
        requireRowVersion(current.rowVersion, request.rowVersion);
        const next = { ...current, name: String(request.name), url: String(request.url), eventTypes: Array.isArray(request.eventTypes) ? request.eventTypes.map(String) : [], active: request.active === true, rowVersion: current.rowVersion + 1, updatedAt: nowSeconds };
        previewWebhookEndpoints[index] = next;
        return mockTeamResult(structuredClone(next));
      }
      case 'rotate_license_workspace_webhook_endpoint': {
        requireWorkspacePermission('webhooks.manage');
        const request = workspaceRequest(args);
        recordWorkspaceMutation(command, request);
        const endpointId = stringArg(args, 'endpointId');
        const index = previewWebhookEndpoints.findIndex((endpoint) => endpoint.id === endpointId);
        if (index < 0) throw { code: 'LICENSE_WEBHOOK_NOT_FOUND', message: 'License service operation failed' };
        const current = previewWebhookEndpoints[index];
        requireRowVersion(current.rowVersion, request.rowVersion);
        const next = { ...current, secretVersion: current.secretVersion + 1, rowVersion: current.rowVersion + 1, updatedAt: nowSeconds };
        previewWebhookEndpoints[index] = next;
        return mockTeamResult({ endpoint: structuredClone(next), secret: 'preview-webhook-rotated-secret-0123456789abcdef' });
      }
      case 'delete_license_workspace_webhook_endpoint': {
        requireWorkspacePermission('webhooks.manage');
        const request = workspaceRequest(args);
        recordWorkspaceMutation(command, request);
        const endpointId = stringArg(args, 'endpointId');
        const index = previewWebhookEndpoints.findIndex((endpoint) => endpoint.id === endpointId);
        if (index < 0) throw { code: 'LICENSE_WEBHOOK_NOT_FOUND', message: 'License service operation failed' };
        const current = previewWebhookEndpoints[index];
        requireRowVersion(current.rowVersion, request.rowVersion);
        previewWebhookEndpoints.splice(index, 1);
        return mockTeamResult({ endpointId, deletedAt: nowSeconds, rowVersion: current.rowVersion + 1 });
      }
      case 'get_license_workspace_webhook_deliveries': {
        if (!previewTeamProfile.permissions.includes('webhooks.read') && !previewTeamProfile.permissions.includes('webhooks.delivery.read')) requireWorkspacePermission('webhooks.read');
        const endpointId = objectArgs(args).endpointId;
        return mockTeamResult(structuredClone(previewWebhookDeliveries.filter((delivery) => !endpointId || delivery.endpointId === endpointId)));
      }
      case 'refresh_license_entitlement':
        if (licenseTimeoutFailures) {
          throw { code: 'TIMEOUT', message: 'License service operation failed', details: 'license operation timed out' };
        }
        return previewEntitlement;
      case 'get_autostart': return false;
      case 'set_autostart': return null;
      case 'get_app_settings': return appSettings;
      case 'set_app_settings': {
        const next = objectArgs(args).settings;
        if (next && typeof next === 'object') appSettings = next as AppSettings;
        return null;
      }
      case 'list_programs': {
        if (licenseRequiredProgramListFailurePending) {
          licenseRequiredProgramListFailurePending = false;
          throw { code: 'LICENSE_REQUIRED', message: 'An active license is required for this action.' };
        }
        return summaries();
      }
      case 'list_invalid_programs': return [];
      case 'get_program': {
        const programId = stringArg(args, 'programId');
        return mockProgramSelectionResult(command, programId, detail(programId));
      }
      case 'get_program_privilege_assessment': {
        const programId = stringArg(args, 'programId');
        const kind = specs[programId]?.type.kind;
        const assessment = kind === 'generic'
          ? { detected: 'unknown', effective: 'standard', authoritative: false, reasons: [{ code: 'configurationUnavailable' }] }
          : { detected: 'standard', effective: 'standard', authoritative: true, reasons: [] };
        return mockProgramSelectionResult(command, programId, assessment);
      }
      case 'start_program': requireLifecycleAccess('start'); setLifecycleState(args, { status: 'running', pid: 42420, startedUnixMs: Date.now() }); return null;
      case 'stop_program': setLifecycleState(args, { status: 'stopped' }); return null;
      case 'restart_program': requireLifecycleAccess('restart'); setLifecycleState(args, { status: 'running', pid: 42421, startedUnixMs: Date.now() }); return null;
      case 'update_program': {
        const next = objectArgs(args).spec;
        if (next && typeof next === 'object') {
          const nextSpec = next as ProgramSpec;
          const previous = specs[nextSpec.id];
          const managedChanged = JSON.stringify({
            singBoxDashboard: previous?.managedConfig?.singBoxDashboard,
            singBoxClashDashboard: previous?.managedConfig?.singBoxClashDashboard,
            xrayDashboard: previous?.managedConfig?.xrayDashboard,
            mihomoDashboard: previous?.managedConfig?.mihomoDashboard,
          }) !== JSON.stringify({
            singBoxDashboard: nextSpec.managedConfig?.singBoxDashboard,
            singBoxClashDashboard: nextSpec.managedConfig?.singBoxClashDashboard,
            xrayDashboard: nextSpec.managedConfig?.xrayDashboard,
            mihomoDashboard: nextSpec.managedConfig?.mihomoDashboard,
          });
          validatePreviewCompatibility(next as ProgramSpec);
          specs[nextSpec.id] = structuredClone(nextSpec);
          previewConfigurationStates.delete(nextSpec.id);
        }
        return null;
      }
      case 'update_program_and_restart': {
        requireLifecycleAccess('restart');
        const next = objectArgs(args).spec;
        if (next && typeof next === 'object') {
          const nextSpec = next as ProgramSpec;
          validatePreviewCompatibility(next as ProgramSpec);
          specs[nextSpec.id] = structuredClone(nextSpec);
          previewConfigurationStates.delete(nextSpec.id);
        }
        setLifecycleState(args, { status: 'running', pid: 42421, startedUnixMs: Date.now() });
        return null;
      }
      case 'update_configuration_compatibility': {
        const programId = stringArg(args, 'programId');
        const request = objectArgs(args).request as {
          preference?: CoreCompatibilityPreference;
          expectedGeneration?: number;
        } | undefined;
        const spec = specs[programId];
        if (!spec || !request?.preference) {
          throw { code: 'INVALID_SPEC', message: 'Compatibility update request is incomplete.' };
        }
        const preference = request.preference;
        if (compatibilitySaveFailurePending) {
          compatibilitySaveFailurePending = false;
          throw new Error(JSON.stringify({
            code: 'INVALID_SPEC',
            messageKey: 'CORE_COMPATIBILITY_INVALID',
            message: 'Selected Core reference is not catalogued for this program',
          }));
        }
        if (states[programId]?.status === 'running') {
          throw {
            code: 'INVALID_STATE',
            messageKey: 'CORE_COMPATIBILITY_PROGRAM_ACTIVE',
            message: 'Stop the program before changing its Core compatibility baseline',
          };
        }
        const current = configurationState(programId);
        if (request.expectedGeneration !== current.generation) {
          throw {
            code: 'CONFIG_CONFLICT',
            messageKey: 'CONFIGURATION_GENERATION_STALE',
            message: 'Configuration changed since the compatibility target was loaded',
          };
        }
        const candidate = structuredClone(spec) as ProgramSpec;
        candidate.executable.compatibility = structuredClone(preference);
        validatePreviewCompatibility(candidate);
        specs[programId] = candidate;
        return updateConfigurationState(programId, (state) => {
          if (candidate.type.kind === 'generic') return;
          const previousTarget = candidate.executable.metadata?.coreTarget;
          const reference = preference.mode === 'unknown'
            ? preference.reference
            : preference.mode === 'release'
              ? { kind: 'release' as const, tag: preference.tag }
              : preference.mode === 'commit'
                ? { kind: 'commit' as const, commitSha: preference.commitSha }
                : undefined;
          const automaticTarget = unclassifiedCoreTarget(
            candidate.type.kind,
            candidate.executable.metadata?.probe?.reportedVersion,
          );
          const target: CoreTargetIdentity = {
            program: candidate.type.kind,
            coordinate: preference.mode === 'automatic'
              ? automaticTarget.coordinate
              : reference?.kind === 'release'
              ? {
                  kind: 'release',
                  tag: reference.tag,
                  normalizedVersion: reference.tag.replace(/^v/, ''),
                  commitSha: 'a'.repeat(40),
                }
              : reference?.kind === 'commit'
                ? { kind: 'commit', commitSha: reference.commitSha }
                : { kind: 'unknown' },
            basis: preference.mode === 'unknown'
              ? 'unknown'
              : preference.mode === 'automatic'
                ? automaticTarget.basis
                : 'userDeclared',
            catalogRevision: previousTarget?.catalogRevision ?? 'core-history-v1-20260811',
            reportedVersion: previousTarget?.reportedVersion,
            fingerprintSha256: candidate.executable.metadata?.fingerprint.sha256,
          };
          if (candidate.executable.metadata) candidate.executable.metadata.coreTarget = target;
          state.compatibilityProfile = mockCompatibilityProfile(candidate.type.kind, target);
          state.desired.compatibilityProfileHash = state.compatibilityProfile.profileHash;
          state.desired.validation = 'valid';
          state.desired.validationEvidence = {
            binarySha256: candidate.executable.metadata?.fingerprint.sha256 ?? 'a'.repeat(64),
            profileHash: state.compatibilityProfile.profileHash,
            configHash: state.desired.revision.contentHash,
            validatorContractRevision: 'preview-validator-v1',
            nativeAccepted: true,
            validatedUnixMs: Date.now(),
          };
        });
      }
      case 'remove_program': delete specs[stringArg(args, 'programId')]; return null;
      case 'list_actions': {
        const programId = stringArg(args, 'programId');
        const kind = specs[programId]?.type.kind;
        if (kind === 'xray') {
          return mockProgramSelectionResult(command, programId, [
            { id: 'dump-config', label: 'Dump parsed configuration', allowedStates: ['stopped', 'running'], confirmation: false },
          ]);
        }
        if (kind === 'singBox') {
          return mockProgramSelectionResult(command, programId, [
            { id: 'format-config', label: 'Format with sing-box', allowedStates: ['stopped', 'running'], confirmation: false },
          ]);
        }
        return mockProgramSelectionResult(command, programId, []);
      }
      case 'load_config': return configDocument(stringArg(args, 'programId'));
      case 'get_configuration_state':
      case 'get_configuration_workspace':
        return configurationState(stringArg(args, 'programId'));
      case 'get_configuration_editor_session':
        return rawDraftSession(stringArg(args, 'programId'));
      case 'preview_configuration_import': {
        const programId = stringArg(args, 'programId');
        const targetCore = specs[programId]?.type.kind ?? 'xray';
        const target = specs[programId]?.executable.metadata?.coreTarget
          ?? unclassifiedCoreTarget(targetCore === 'generic' ? 'xray' : targetCore);
        const preview: ShareImportPreview = {
          normalized: {
            text: 'vless://preview-redacted',
            envelope: 'plain',
            payload: 'singleShareLink',
            contentHash: 'preview-share-source-hash',
          },
          summary: {
            envelope: 'plain',
            payload: 'singleShareLink',
            parserRevision: 'share-v1-preview',
            totalItems: 1,
            acceptedItems: 1,
            rejectedItems: 0,
            warningCount: 0,
            protocols: { VLESS: 1 },
            fidelity: 'exact',
            collectionStatus: 'success',
            issues: [],
          },
          items: [{
            itemId: 'share-preview-item',
            semantic: { protocol: 'VLESS', name: 'Preview node' },
            fragment: { type: 'vless', server: 'example.com', server_port: 443 },
            summary: {
              target,
              translatorRevision: 'translator-v2-preview',
              profileHash: 'preview-core-profile-hash',
              catalogRevision: 'core-history-v1-20260811',
              featureDecisions: [],
              fidelity: 'equivalent',
              warnings: [],
              blockingIssues: [],
            },
            provenance: {},
          }],
        };
        return preview;
      }
      case 'save_configuration_draft': {
        const programId = stringArg(args, 'programId');
        const request = objectArgs(args).request as {
          draft?: RawDraftSession;
          expectedRevision?: number;
        } | undefined;
        if (!request?.draft) throw { code: 'INVALID_SPEC', message: 'Draft is required' };
        const draft = structuredClone(request.draft);
        const persisted = previewRawDrafts.get(programId);
        if (
          request.expectedRevision !== draft.draftRevision
          || (persisted && (
            persisted.draftRevision !== request.expectedRevision
            || persisted.sessionId !== draft.sessionId
          ))
          || (!persisted && request.expectedRevision !== 0)
        ) {
          throw { code: 'CONFIG_CONFLICT', message: 'Preview Raw draft revision is stale' };
        }
        if (persisted) {
          draft.baseContent = persisted.baseContent;
          draft.basedOnGeneration = persisted.basedOnGeneration;
          draft.conflicts = structuredClone(persisted.conflicts);
          draft.resolutions = structuredClone(persisted.resolutions);
          draft.unresolvedConflictIds = [...persisted.unresolvedConflictIds];
        }
        draft.draftRevision += 1;
        draft.updatedUnixMs = Date.now();
        refreshPreviewUnresolvedConflicts(draft);
        previewRawDrafts.set(programId, structuredClone(draft));
        return draft;
      }
      case 'rebase_configuration_draft': {
        const programId = stringArg(args, 'programId');
        const draft = rawDraftSession(programId);
        draft.basedOnGeneration = configurationState(programId).generation;
        draft.resolutions = {};
        draft.unresolvedConflictIds = draft.conflicts.map((conflict) => conflict.conflictId);
        draft.draftRevision += 1;
        draft.updatedUnixMs = Date.now();
        previewRawDrafts.set(programId, structuredClone(draft));
        return draft;
      }
      case 'resolve_configuration_conflict': {
        const programId = stringArg(args, 'programId');
        const request = objectArgs(args).request as {
          conflictId?: string;
          resolution?: RawDraftSession['resolutions'][string];
        } | undefined;
        const draft = rawDraftSession(programId);
        const conflict = draft.conflicts.find((item) => item.conflictId === request?.conflictId);
        if (!conflict || !request?.resolution) {
          throw { code: 'NOT_FOUND', message: 'Preview configuration conflict was not found' };
        }
        const document = JSON.parse(draft.workingContent) as unknown;
        updatePreviewConflictPath(
          document,
          conflict.segments,
          previewConflictResolutionValue(conflict, request.resolution),
        );
        draft.workingContent = `${JSON.stringify(document, null, 2)}\n`;
        draft.resolutions[conflict.conflictId] = request.resolution;
        refreshPreviewUnresolvedConflicts(draft);
        draft.draftRevision += 1;
        draft.updatedUnixMs = Date.now();
        previewRawDrafts.set(programId, structuredClone(draft));
        return draft;
      }
      case 'resolve_raw_decision': {
        const programId = stringArg(args, 'programId');
        const request = objectArgs(args).request as {
          decisionId?: string;
          resolution?: import('../types').RawDecisionResolution;
        } | undefined;
        return updateConfigurationState(programId, (state) => {
          const decision = state.workspace.rawDecisions.find(
            (item) => item.decisionId === request?.decisionId,
          );
          if (!decision || !request?.resolution) {
            throw { code: 'NOT_FOUND', message: 'Preview Raw decision was not found' };
          }
          if (request.resolution === 'acceptUpstream') {
            decision.status = 'dormant';
          } else if (request.resolution === 'keepRaw') {
            decision.status = 'resolved';
          } else {
            decision.status = 'resolved';
            decision.operation = {
              operation: 'set',
              path: decision.operation.path,
              value: request.resolution.manualEdit.value,
            };
            decision.rawValue = request.resolution.manualEdit.value;
          }
          recomputePreviewCandidate(state);
          const decisionPath = previewPathSegments(decision.operation.path);
          state.guidedProjection = state.guidedProjection.map((projection) => {
            const guidedPath = previewGuidedPath(state.kind, projection.settingId);
            if (!guidedPath || !previewPathsOverlap(guidedPath, decisionPath)) return projection;
            if (decision.status === 'dormant') {
              return {
                ...projection,
                status: projection.intentValue === undefined ? 'inherited' : 'explicit',
              };
            }
            return { ...projection, status: 'rawDecision' };
          });
        });
      }
      case 'discard_configuration_draft':
        previewRawDrafts.delete(stringArg(args, 'programId'));
        return null;
      case 'commit_configuration_draft': {
        const programId = stringArg(args, 'programId');
        const draft = rawDraftSession(programId);
        refreshPreviewUnresolvedConflicts(draft);
        if (draft.unresolvedConflictIds.length > 0) {
          throw { code: 'CONFIG_CONFLICT', message: 'Resolve all preview conflicts before saving' };
        }
        const currentState = configurationState(programId);
        let committedContent = draft.workingContent;
        if (currentState.format === 'jsonc') {
          committedContent = `${JSON.stringify(JSON.parse(draft.workingContent), null, 2)}\n`;
        }
        const state = updateConfigurationState(programId, (current) => {
          current.desired.content = committedContent;
          current.desired.validation = 'pending';
          current.desired.validationEvidence = undefined;
          current.desired.diagnostics = [];
          current.desired.conflicts = [];
        });
        state.workspace.validationStatus = 'pending';
        state.workspace.saveStatus = 'pendingValidation';
        state.workspace.canSave = true;
        state.workspace.canValidate = true;
        state.workspace.canApply = false;
        previewConfigurationStates.set(programId, structuredClone(state));
        previewRawDrafts.delete(programId);
        return state;
      }
      case 'validate_configuration_candidate': {
        const programId = stringArg(args, 'programId');
        const current = configurationState(programId);
        if (current.desired.conflicts.some((conflict) => conflict.severity === 'error')) {
          throw {
            code: 'CONFIG_CONFLICT',
            messageKey: 'CONFIGURATION_CONFLICT',
            message: 'Resolve all blocking conflicts before validation.',
          };
        }
        return updateConfigurationState(programId, (state) => {
          state.desired.validation = 'valid';
          state.desired.validationEvidence = {
            binarySha256: 'a'.repeat(64),
            profileHash: state.compatibilityProfile.profileHash,
            configHash: state.desired.revision.contentHash,
            validatorContractRevision: 'preview-validator-v1',
            nativeAccepted: true,
            validatedUnixMs: Date.now(),
          };
          state.desired.diagnostics = [];
          state.workspace.validationStatus = 'valid';
          state.workspace.saveStatus = 'saved';
          state.workspace.canApply = true;
        }, false);
      }
      case 'set_guided_intent': {
        const programId = stringArg(args, 'programId');
        const request = objectArgs(args).request;
        const nextState = updateConfigurationState(programId, (state) => {
          const guided = request && typeof request === 'object' ? request as { settingId?: string; value?: unknown; replaceRawOverride?: boolean } : {};
          const settingId = guided.settingId;
          if (!settingId) return;
          const projection = state.guidedProjection.find((item) => item.settingId === settingId);
          const descriptor = state.guidedDescriptors.find((item) => item.id === settingId);
          if (!projection || !descriptor) {
            throw { code: 'NOT_FOUND', message: `Preview Guided setting was not found: ${settingId}` };
          }
          let nextValue = guided.value;
          if (nextValue !== undefined && descriptor.control === 'select'
            && !descriptor.allowedValues.includes(String(nextValue))) {
            throw { code: 'INVALID_SPEC', message: `Preview Guided value is not allowed: ${settingId}` };
          }
          if (nextValue === undefined) {
            // Following source restores the fixture's source-owned value.  It
            // is intentionally resolved from a fresh descriptor projection,
            // not from the possibly overridden current projection.
            nextValue = previewGuidedSettings(state.kind).projection
              .find((item) => item.settingId === settingId)?.value;
          }
          let upstreamDocument: unknown;
          try {
            upstreamDocument = JSON.parse(state.workspace.upstreamDocument);
          } catch {
            upstreamDocument = {};
          }
          setPreviewGuidedPath(
            upstreamDocument,
            previewGuidedPath(state.kind, settingId) ?? [],
            nextValue,
          );
          state.workspace.upstreamDocument = `${JSON.stringify(upstreamDocument, null, 2)}\n`;
          if (projection) {
            projection.status = guided.value === undefined ? 'inherited' : 'explicit';
            projection.value = nextValue;
            projection.intentValue = guided.value;
          }
          for (const decision of state.workspace.rawDecisions) {
            const decisionPath = previewPathSegments(decision.operation.path);
            const guidedPath = previewGuidedPath(state.kind, settingId) ?? [];
            if ((decision.status === 'active' || decision.status === 'resolved')
              && previewPathsOverlap(decisionPath, guidedPath)) {
              decision.status = 'superseded';
              decision.upstreamValue = previewReadPath(upstreamDocument, decisionPath);
            }
          }
          recomputePreviewCandidate(state);
          const changedPath = previewGuidedPath(state.kind, settingId) ?? [];
          const hasRawDecision = state.workspace.rawDecisions.some((decision) =>
            decision.status !== 'dormant'
            && previewPathsOverlap(previewPathSegments(decision.operation.path), changedPath),
          );
          state.guidedProjection = state.guidedProjection.map((item) => item.settingId === settingId
            ? { ...item, status: hasRawDecision ? 'rawDecision' : guided.value === undefined ? 'inherited' : 'explicit', value: nextValue, intentValue: guided.value }
            : item);
        });
        const existingDraft = previewRawDrafts.get(programId);
        if (existingDraft && existingDraft.draftRevision === 0) {
          existingDraft.baseContent = nextState.desired.content;
          existingDraft.userContent = nextState.desired.content;
          existingDraft.workingContent = nextState.desired.content;
          existingDraft.basedOnGeneration = nextState.generation;
          existingDraft.updatedUnixMs = Date.now();
          previewRawDrafts.set(programId, structuredClone(existingDraft));
        }
        return nextState;
      }
      case 'apply_configuration_candidate': {
        const programId = stringArg(args, 'programId');
        const current = configurationState(programId);
        if (current.desired.validation !== 'valid' || !current.desired.validationEvidence
          || current.desired.conflicts.some((conflict) => conflict.severity === 'error')) {
          throw {
            code: 'CONFIG_INVALID',
            messageKey: 'CORE_INVALID',
            message: 'Save and validate the candidate before applying it.',
          };
        }
        return updateConfigurationState(programId, (state) => {
          state.appliedRevision = { ...state.desired.revision };
          state.lastKnownGoodRevision = { ...state.desired.revision };
        }, false);
      }
      case 'load_configuration_schema': {
        const programId = stringArg(args, 'programId');
        if (specs[programId]?.type.kind !== 'singBox') return null;
        if (configurationSchemaFailurePending) {
          configurationSchemaFailurePending = false;
          throw {
            code: 'CONFIGURATION_SCHEMA_INVALID',
            message: 'The program configuration schema is unavailable.',
          };
        }
        return {
          source: 'programBinary',
          dialect: 'draft2020-12',
          content: singBoxConfigurationSchemaContent,
          contentHash: '0'.repeat(64),
        };
      }
      case 'run_action': return { stdout: 'Diagnostic completed successfully.', stderr: '' };
      case 'read_logs': {
        const programId = stringArg(args, 'programId');
        const stream = stringArg(args, 'stream') === 'stderr' ? 'stderr' : 'stdout';
        if (staleLogFailurePending && programId === 'xray-primary' && stream === 'stdout') {
          staleLogFailurePending = false;
          return new Promise((_, reject) => window.setTimeout(() => reject({
            code: 'STORAGE',
            message: 'Stale preview log failure.',
          }), 500));
        }
        return {
          content: growingLog(stream),
          truncated: false,
        };
      }
      case 'clear_logs': return null;
      case 'get_xray_dashboard_snapshot': return { ...xraySnapshot, fetchedUnixMs: Date.now() };
      case 'set_xray_balancer_target': {
        const target = objectArgs(args).target;
        return { ...xrayBalancer, currentTarget: typeof target === 'string' && target ? target : undefined };
      }
      case 'restart_xray_logger': return null;
      case 'open_working_directory':
      case 'open_data_directory':
      case 'open_app_log_directory':
      case 'open_documentation':
      case 'open_sing_box_dashboard':
      case 'open_mihomo_dashboard':
        return mockExternalAction(command);
      case 'refresh_configuration_sources':
      case 'update_configuration_sources': {
        const programId = stringArg(args, 'programId');
        if (command === 'update_configuration_sources') {
          if (sourceSaveFailurePending) {
            sourceSaveFailurePending = false;
            throw {
              code: 'PROGRAM_BUSY',
              messageKey: 'CONFIGURATION_OPERATION_BUSY',
              message: 'Another configuration operation is still in progress',
              details: 'source transaction lease is currently held',
            };
          }
          const request = objectArgs(args).request;
          const spec = specs[programId];
          if (spec?.managedConfig && request && typeof request === 'object') {
            const value = request as { sources?: ConfigSource[]; remoteUpdate?: RemoteUpdate };
            if (Array.isArray(value.sources)) {
              spec.managedConfig.sources = structuredClone(value.sources) as typeof spec.managedConfig.sources;
            }
            spec.managedConfig.remoteUpdate = value.remoteUpdate as typeof spec.managedConfig.remoteUpdate;
          }
        }
        return updateConfigurationState(programId, (state) => {
          for (const source of state.sourceStatuses) {
            if (source.freshness !== 'disabled') source.freshness = 'fresh';
          }
        });
      }
      case 'replace_package':
        return null;
      default:
        if (command.startsWith('plugin:')) return null;
        throw { code: 'MOCK_COMMAND_UNIMPLEMENTED', message: `No UI preview response for ${command}` };
    }
  };
  if (nativeWindowAvailable) {
    installPreviewInvokeTransport(handlePreviewInvoke);
  } else {
    mockWindows('main');
    mockIPC(handlePreviewInvoke, { shouldMockEvents: true });
  }

  window.addEventListener('camellia-ui-preview:automatic-config-update', (event) => {
    const detail = (event as CustomEvent<{ programId?: string }>).detail;
    const programId = detail?.programId ?? 'xray-primary';
    const content = '{\n  "log": { "loglevel": "debug" },\n  "automatic": true\n}\n';
    const state = updateConfigurationState(programId, (current) => {
      current.desired.content = content;
      current.desired.validation = 'valid';
      current.desired.diagnostics = [];
      current.desired.conflicts = [];
      const logging = current.guidedProjection.find(
        (projection) => projection.settingId === 'logging.level',
      );
      if (logging) {
        logging.status = 'inherited';
        logging.value = 'debug';
      }
      if (current.sourceStatuses[0]) {
        current.sourceStatuses[0].sourceName = 'Automatically refreshed source';
        current.sourceStatuses[0].freshness = 'fresh';
      }
    });
    state.desired.revision.contentHash = 'preview-automatic-hash';
    state.appliedRevision = { ...state.desired.revision };
    state.lastKnownGoodRevision = { ...state.desired.revision };
    previewConfigurationStates.set(programId, structuredClone(state));
    previewConfigurationDocuments.set(programId, {
      content,
      baseHash: state.desired.revision.contentHash,
    });
    void emit('automatic-config-update', { programId, succeeded: true }).catch(() => null);
  });

  if (previewParameters.has('__ui_revalidation_notice')) {
    window.setTimeout(() => {
      previewEntitlement = {
        generation: previewEntitlement.generation + 1,
        entitlementState: { status: 'revalidationRequired', reason: 'obsolete_epoch' },
      };
      const event: LicenseStateChangedEvent = {
        ...previewEntitlement,
        reason: 'license_policy_updated',
        runtimeImpact: 'hardInactive',
        stoppedPrograms: 0,
        failedPrograms: 0,
        failedProgramIds: [],
      };
      void emit('license-state-changed', event).catch(() => null);
    }, 400);

    if (previewParameters.has('__ui_revalidation_recovery')) {
      window.setTimeout(() => {
        previewEntitlement = {
          ...recoveredEntitlement,
          generation: previewEntitlement.generation + 1,
        };
        const event: LicenseStateChangedEvent = {
          ...previewEntitlement,
          reason: 'license_refresh',
          runtimeImpact: 'active',
          stoppedPrograms: 0,
          failedPrograms: 0,
          failedProgramIds: [],
        };
        void emit('license-state-changed', event).catch(() => null);
      }, 1_200);
    }
  }
}

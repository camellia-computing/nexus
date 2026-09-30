import type { InvokeArgs } from '@tauri-apps/api/core';
import { parse as parseYaml } from 'yaml';
import { parse as parseJsonc, type ParseError } from 'jsonc-parser';
import { emit } from '@tauri-apps/api/event';
import { mockIPC, mockWindows } from '@tauri-apps/api/mocks';
import { installPreviewInvokeTransport } from '../api';
import { canUseProgramLifecycleAction, deriveLicenseAccess } from '../licenseAccess';
import { semanticPathSegments } from '../editor/configurationMarkerModel';
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
  ConfigurationMutationContext,
  ConfigurationOperationResult,
  CoreCompatibilityProfile,
  CoreAdmissionReport,
  CoreTargetIdentity,
  GuidedProjection,
  GuidedSettingDescriptor,
  FinalEditorSession,
  FinalConflictProjection,
  FinalChangeProjection,
  ConfigurationConflict,
  ShareImportPreview,
  ProgramKind,
} from '../types';

const nowSeconds = Math.floor(Date.now() / 1_000);

function parsePreviewJsonc(content: string): unknown {
  const errors: ParseError[] = [];
  const value = parseJsonc(content, errors, { allowTrailingComma: true, disallowComments: false });
  if (errors.length || value === undefined) throw new SyntaxError('Invalid JSON configuration');
  return value;
}
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
const platformIssuePreview = previewParameters.has('__ui_platform_issue');
const finalMergeConflictPreview = previewParameters.has('__ui_final_merge_conflict');
let conflictWriteFailurePending = previewParameters.has('__ui_conflict_fail_once');
let adoptWriteFailurePending = previewParameters.has('__ui_adopt_fail_once');
let conflictChoiceRejectedPending = previewParameters.has('__ui_conflict_rejected_once');
let conflictResponseFailurePending = previewParameters.has('__ui_conflict_response_lost');
const guidedFinalEditPreview = previewParameters.has('__ui_guided_final_edit');
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
let finalDraftDiscardRacePending = previewParameters.has('__ui_final_draft_discard_race');
let configurationResponseLostPending = previewParameters.has('__ui_configuration_response_lost');
let configurationSaveResponseLostPending = previewParameters.has('__ui_configuration_save_response_lost');
let configurationWorkspaceReads = 0;
let configurationMetadataFailurePending = previewParameters.has('__ui_config_metadata_error_once');
let configurationResponseHeldPending = previewParameters.has('__ui_configuration_hold_response');
let finalDraftWriteFailurePending = previewParameters.has('__ui_final_draft_write_failure');
let finalDraftResponseHeldPending = previewParameters.has('__ui_final_draft_hold_response');
let configurationNativeRejectionPending = previewParameters.has('__ui_configuration_native_rejection');
let finalDraftDiscardRaceSeeded = false;

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
    knowledgeHash: 'preview-knowledge-digest',
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
      'v1.14.0',
      '1.14.0',
      '0b8995879f29a9b98ee027bc17b75e101445b238',
      'sing-box version 1.14.0',
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
      coordinate: { kind: 'unknown' as const },
      basis: 'binaryReported' as const,
      knowledgeHash: 'preview-knowledge-digest',
      reportedVersion: `${displayName} version 99.0.0`,
      fingerprintSha256,
    };
  }
  if (coreTargetPreview === 'unknown') {
    return {
      program,
      coordinate: { kind: 'unknown' as const },
      basis: 'unknown' as const,
      knowledgeHash: 'preview-knowledge-digest',
      fingerprintSha256,
    };
  }
  const baseline = program === 'xray'
    ? { version: '26.3.27', commit: 'd2758a023cd7f4174a5a5fa4ff66e487d4342ba0' }
    : program === 'mihomo'
      ? { version: '1.19.30', commit: 'ac017cdd246ce8bd547653d927e7bf77d7ee73d5' }
      : { version: '1.14.0', commit: '0b8995879f29a9b98ee027bc17b75e101445b238' };
  return releaseTarget(`v${baseline.version}`, baseline.version, baseline.commit, reportedVersion ?? baseline.version);
}

function mockCoreAdmission(program: ProgramKind, target?: CoreTargetIdentity): CoreAdmissionReport | null {
  if (program === 'generic') return null;
  const release = target?.coordinate.kind === 'release' ? target.coordinate : null;
  const status = release ? 'admitted' : coreTargetPreview === 'future' ? 'notMaintained' : 'unrecognized';
  return {
    program,
    status,
    messageKey: status === 'admitted' ? 'CORE_ADMISSION_ACCEPTED'
      : status === 'notMaintained' ? 'CORE_VERSION_NOT_MAINTAINED' : 'CORE_VERSION_UNRECOGNIZED',
    maintainedFamilies: program === 'xray' ? ['26.3', '26.2']
      : program === 'mihomo' ? ['1.19', '1.18'] : ['1.14', '1.13'],
    baseline: release ? {
      tag: release.tag,
      version: release.normalizedVersion,
      commitSha: release.commitSha,
      modulePath: `example.test/fixture/${program}`,
      dependencies: [],
      publishedAt: 'preview-fixture',
      sourceTimestamp: 'preview-fixture',
      sourceUrl: 'https://example.test/release',
    } : null,
    knowledgeHash: 'preview-knowledge-digest',
  };
}

function managedExecutable(
  path: string,
  program?: Exclude<ProgramKind, 'generic'>,
  version?: string,
) {
  const target = program ? unclassifiedCoreTarget(program, version) : undefined;
  return {
    mode: 'managed' as const,
    path,
    metadata: {
      fingerprint: {
        sha256: 'a'.repeat(64),
        size: 18_462_720,
        modifiedUnixMs: Date.now() - 86_400_000,
      },
      ...(program ? {
        probe: {
          revision: 'preview-probe-digest',
          prerelease: false,
          hasBuildMetadata: false,
          ...(target?.reportedVersion ? { reportedVersion: target.reportedVersion } : {}),
          ...(target?.coordinate.kind === 'release' ? { normalizedVersion: target.coordinate.normalizedVersion } : {}),
          cliObservations: [],
        },
        coreTarget: target,
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
    'proxy.outbound.vless',
    'proxy.outbound.shadowsocks',
    'proxy.outbound.hysteria2',
    'proxy.outbound.tuicV5',
  ];
  const releaseKnown = target.coordinate.kind === 'release';
  return {
    target,
    profileHash: 'preview-core-profile-hash',
    decisions: featureIds.map((featureId) => ({
      featureId,
      availability: !releaseKnown
        ? 'unconfirmed' as const
        : featureId === 'proxy.outbound.tuicV5' && target.program === 'xray'
          ? 'sourceUnavailable' as const
          : 'sourceDeclared' as const,
      buildConditions: [],
      evidence: [],
    })),
  };
}

const specs: Record<string, ProgramSpec> = {
  'local-agent': {
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
    id: 'xray-primary',
    name: 'Primary Xray routing fabric',
    executable: managedExecutable('bin/xray/xray', 'xray', 'Xray 25.6.8'),
    type: { kind: 'xray', extraArgs: ['run'] },
    managedConfig: {
      sources: [
        { mode: 'local', id: 'primary', name: 'Production routing', enabled: true, path: 'profiles/xray.json' },
        ...(configurationSourcePreview === 'conflict'
          ? [{ mode: 'inline' as const, id: 'alternate', name: 'Alternative routing', enabled: true, content: '{"log":{"loglevel":"debug"}}' }]
          : []),
      ],
      xrayDashboard: { apiPort: 10085, metricsPort: 11111 },
    },
    workingDirectory: 'bin/xray',
    environment: {},
    autoStart: true,
    restartPolicy: 'onFailure',
    privilegePolicy: { mode: 'automatic' },
  },
  'mihomo-alpha': {
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
let identityReadReady = false;
window.addEventListener('camellia-ui-preview:identity-ready', () => { identityReadReady = true; });
const previewUpstreamDocuments = new Map<string, string>();
const previewUpstreamContributions = new Map<string, {
  source: string;
  writes: Array<{ owner: string; change: FinalChangeProjection }>;
}>();
const previewFinalDrafts = new Map<string, FinalEditorSession>();
const previewConflictOperations = new Map<string, {
  serialized: string;
  conflict: FinalConflictProjection;
  resolution: import('../types').FinalConflictResolution;
  undone: boolean;
}>();
const previewAdoptedChanges = new Map<string, string>();
const previewConfigurationOperations = new Map<string, {
  request: ConfigurationMutationContext;
  result: ConfigurationOperationResult;
}>();

function previewOperationResult(programId: string, request: ConfigurationMutationContext) {
  const receipt = previewConfigurationOperations.get(`${programId}:${request.operationId}`);
  if (receipt && JSON.stringify(receipt.request) !== JSON.stringify(request)) {
    throw { code: 'CONFIG_CONFLICT', messageKey: 'CONFIGURATION_OPERATION_MISMATCH', message: 'Request changed' };
  }
  return receipt?.result;
}

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
  const sourceConflict = configurationSourcePreview === 'conflict' && programId === 'xray-primary';
  const candidateNeedsAttention = evidenceStale || evidenceMismatch || sourceBlocked || sourceConflict;
  let desiredHash = candidateNeedsAttention
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
  const finalPreviewPath = kind === 'xray'
    ? [{ kind: 'key' as const, key: 'log' }, { kind: 'key' as const, key: 'loglevel' }]
    : kind === 'singBox'
      ? [{ kind: 'key' as const, key: 'log' }, { kind: 'key' as const, key: 'level' }]
      : [{ kind: 'key' as const, key: 'log-level' }];
  const finalPreviewConflict: FinalConflictProjection = {
    conflictId: `preview-final-conflict-${programId}`,
    reference: { origin: 'candidate', conflictId: `preview-final-conflict-${programId}`, fingerprint: `preview-${programId}-initial` },
    semanticPath: kind === 'xray' ? '/log/loglevel' : kind === 'singBox' ? '/log/level' : '/log-level',
    segments: finalPreviewPath,
    kind: 'modifyVsModify',
    baseValue: { state: 'present', value: 'warn' },
    upstreamValue: { state: 'present', value: 'info' },
    userValue: { state: 'present', value: 'debug' },
    canMerge: false,
  };
  const finalCandidateConflict: ConfigurationConflict = {
    semanticPath: finalPreviewConflict.semanticPath,
    reason: 'The upstream configuration and final editor changed the same path',
    severity: 'error',
    messageKey: 'FINAL_EDIT_CONFLICT',
    scope: { surface: 'configuration', ownerId: finalPreviewConflict.conflictId },
    sourceValue: 'info',
    userValue: 'debug',
    effectiveValue: 'info',
  };
  let upstreamContent = document.content;
  let effectiveContent = document.content;
  if (guidedFinalEditPreview && kind === 'singBox' && format === 'jsonc') {
    const upstreamDocument = parsePreviewJsonc(document.content);
    setPreviewGuidedPath(upstreamDocument, ['log', 'level'], 'fatal');
    upstreamContent = `${JSON.stringify(upstreamDocument, null, 2)}\n`;
    const finalDocument = structuredClone(upstreamDocument);
    setPreviewGuidedPath(finalDocument, ['log', 'level'], 'error');
    effectiveContent = `${JSON.stringify(finalDocument, null, 2)}\n`;
    desiredHash = previewContentHash(effectiveContent);
    const logging = guided.projection.find(
      (projection) => projection.settingId === 'logging.level',
    );
    if (logging) {
      logging.status = 'finalEdit';
      logging.value = 'fatal';
      logging.intentValue = 'fatal';
    }
  }
  if (finalMergeConflictPreview && format === 'jsonc') {
    const effectiveDocument = parsePreviewJsonc(document.content);
    updatePreviewConflictPath(effectiveDocument, finalPreviewPath, {
      present: true,
      value: 'info',
    });
    effectiveContent = `${JSON.stringify(effectiveDocument, null, 2)}\n`;
    upstreamContent = effectiveContent;
    desiredHash = previewContentHash(effectiveContent);
  }
  if (previewParameters.has('__ui_final_draft_conflict') && programId === 'xray-primary') {
    effectiveContent = effectiveContent.replace('proxy-sg', 'source-route');
    upstreamContent = effectiveContent;
    desiredHash = previewContentHash(effectiveContent);
  }
  const state: ConfigurationStateView = {
    kind,
    format,
    stateRevision: generation,
    generation,
    compatibilityProfile,
    coreAdmission: mockCoreAdmission(kind, metadataTarget),
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
        ? { messageKey: 'SOURCE_DOWNLOAD_FAILED' }
        : configurationSourcePreview === 'invalid'
          ? { messageKey: 'SOURCE_INVALID' }
          : configurationSourcePreview === 'unavailable'
            ? { messageKey: 'SOURCE_FILE_NOT_FOUND' }
            : {}),
    })),
    sourceParseSummaries: {},
    provenance: [],
    desired: {
      revision: { generation, contentHash: desiredHash, createdUnixMs: Date.now() },
      content: effectiveContent,
      compatibilityProfileHash: evidenceMismatch
        ? 'preview-previous-profile-hash'
        : compatibilityProfile.profileHash,
      validation: evidenceStale ? 'pending' : evidenceMismatch || sourceBlocked || finalMergeConflictPreview ? 'invalid' : 'valid',
      ...(!candidateNeedsAttention ? {
        validationEvidence: {
          binarySha256: 'a'.repeat(64),
          profileHash: compatibilityProfile.profileHash,
          configHash: desiredHash,
          candidateGeneration: generation,
          validatorContractRevision: 'preview-validator-test',
          nativeAccepted: true,
          validatedUnixMs: Date.now(),
        },
      } : {}),
      diagnostics,
      conflicts: finalMergeConflictPreview ? [finalCandidateConflict] : sourceConflict ? [{
        semanticPath: '/log/loglevel',
        reason: 'Two sources provide different values',
        severity: 'error',
        sourceIds: ['primary', 'alternate'],
        messageKey: 'SOURCE_VALUE_CONFLICT',
        scope: { surface: 'sources', ownerId: 'alternate' },
        sourceValue: 'info',
        userValue: 'debug',
        effectiveValue: 'info',
      }] : [],
    },
    appliedRevision: { generation: 1, contentHash: 'preview-desired-hash', createdUnixMs: Date.now() },
    lastKnownGoodRevision: { generation: 1, contentHash: 'preview-desired-hash', createdUnixMs: Date.now() },
    guidedDescriptors: guided.descriptors,
    guidedProjection: guided.projection,
    managedIntegrations: kind === 'singBox'
      ? [
          {
            integrationId: 'dashboard.singBoxApi',
            settings: [],
            status: spec?.managedConfig?.singBoxDashboard ? 'explicit' : 'inactive',
            effectiveEnabled: !!spec?.managedConfig?.singBoxDashboard,
            finalPaths: [],
            issueIds: [],
          },
          {
            integrationId: 'dashboard.singBoxClash',
            settings: [],
            status: spec?.managedConfig?.singBoxClashDashboard ? 'explicit' : 'inactive',
            effectiveEnabled: !!spec?.managedConfig?.singBoxClashDashboard,
            finalPaths: [],
            issueIds: [],
          },
        ]
      : kind === 'xray'
        ? [{ integrationId: 'dashboard.xray', settings: [], status: spec?.managedConfig?.xrayDashboard ? 'explicit' : 'inactive', effectiveEnabled: !!spec?.managedConfig?.xrayDashboard, finalPaths: [], issueIds: [] }]
        : kind === 'mihomo'
          ? [{ integrationId: 'dashboard.mihomo', settings: [], status: spec?.managedConfig?.mihomoDashboard ? 'explicit' : 'inactive', effectiveEnabled: !!spec?.managedConfig?.mihomoDashboard, finalPaths: [], issueIds: [] }]
          : [],
    workspace: {
      editor: {
        document: {
          content: effectiveContent,
          revision: { generation, contentHash: desiredHash, createdUnixMs: Date.now() },
        },
        editStatus: finalMergeConflictPreview ? 'conflict' : 'clean',
        candidateStatus: finalMergeConflictPreview ? 'unsaved' : candidateNeedsAttention ? 'invalid' : 'applied',
        changes: [],
        conflicts: finalMergeConflictPreview ? [finalPreviewConflict] : [],
        diagnostics: [],
        blockers: [],
        canSave: !finalMergeConflictPreview,
        canValidate: !finalMergeConflictPreview,
        canApply: !candidateNeedsAttention && !finalMergeConflictPreview,
      },
    },
  };
  if (previewParameters.has('__ui_deleted_final_edit') && programId === 'xray-primary') {
    state.workspace.editor.changes.push({
      editId: 'preview-deleted-log-timestamp',
      semanticPath: '/log/timestamp',
      segments: [{ kind: 'key', key: 'log' }, { kind: 'key', key: 'timestamp' }],
      kind: 'deleted',
      upstreamValue: { state: 'present', value: true },
      finalValue: { state: 'missing' },
      issues: [],
    });
  }
  if (previewParameters.has('__ui_two_final_conflicts') && programId === 'xray-primary') {
    const second: FinalConflictProjection = {
      ...finalPreviewConflict,
      conflictId: 'preview-final-conflict-route',
      reference: { origin: 'candidate', conflictId: 'preview-final-conflict-route', fingerprint: 'preview-route-initial' },
      semanticPath: '/route/final',
      segments: [{ kind: 'key', key: 'route' }, { kind: 'key', key: 'final' }],
      baseValue: { state: 'present', value: 'original-route' },
      upstreamValue: { state: 'present', value: 'proxy-sg' },
      userValue: { state: 'present', value: 'mine-route' },
    };
    state.workspace.editor.conflicts.push(second);
    state.desired.conflicts.push({
      ...finalCandidateConflict,
      semanticPath: second.semanticPath,
      scope: { surface: 'configuration', ownerId: second.conflictId },
    });
  }
  previewUpstreamDocuments.set(programId, upstreamContent);
  previewUpstreamContributions.set(programId, { source: upstreamContent, writes: [] });
  refreshPreviewWorkspaceGates(state);
  syncPreviewEditor(state);
  previewConfigurationStates.set(programId, state);
  return structuredClone(state);
}

let previewEditorSessionSequence = 0;

function finalEditorSession(programId: string): FinalEditorSession {
  const existing = previewFinalDrafts.get(programId);
  if (existing) {
    const state = configurationState(programId);
    const rebased = rebasePreviewFinalEditorSession(existing, state);
    previewFinalDrafts.set(programId, structuredClone(rebased));
    return rebased;
  }
  const state = configurationState(programId);
  if (
    previewParameters.has('__ui_final_draft_discard_race')
    && programId === 'xray-primary'
    && !finalDraftDiscardRaceSeeded
  ) {
    finalDraftDiscardRaceSeeded = true;
    const draft: FinalEditorSession = {
      sessionId: `preview-draft-${programId}`,
      draftRevision: 1,
      basedOnStateRevision: state.stateRevision,
      basedOnCandidateGeneration: state.generation,
      baseContent: state.desired.content,
      workingContent: state.desired.content,
      conflicts: [],
      resolutions: {},
      unresolvedConflictIds: [],
      rebaseRequired: false,
      updatedUnixMs: Date.now(),
    };
    previewFinalDrafts.set(programId, structuredClone(draft));
    return draft;
  }
  if (previewParameters.has('__ui_final_draft_conflict') && programId === 'xray-primary') {
    const conflictId = 'preview-route-conflict';
    const workingContent = state.desired.content;
    const draft: FinalEditorSession = {
      sessionId: `preview-draft-${programId}`,
      draftRevision: 1,
      basedOnStateRevision: state.stateRevision,
      basedOnCandidateGeneration: state.generation,
      baseContent: previewUpstreamDocuments.get(programId) ?? state.workspace.editor.document.content,
      workingContent,
      conflicts: [{
        conflictId,
        segments: [
          { kind: 'key', key: 'route' },
          { kind: 'key', key: 'final' },
        ],
        semanticPath: '/route/final',
        kind: 'modifyVsModify',
        baseValue: { state: 'present', value: 'proxy-sg' },
        upstreamValue: { state: 'present', value: 'source-route' },
        userValue: { state: 'present', value: 'mine-route' },
        canMerge: false,
      }],
      resolutions: {},
      unresolvedConflictIds: [conflictId],
      rebaseRequired: false,
      updatedUnixMs: Date.now(),
    };
    previewFinalDrafts.set(programId, structuredClone(draft));
    return draft;
  }
  const draft: FinalEditorSession = {
    sessionId: `preview-draft-${programId}-${++previewEditorSessionSequence}`,
    draftRevision: 0,
    basedOnStateRevision: state.stateRevision,
    basedOnCandidateGeneration: state.generation,
    baseContent: state.workspace.editor.document.content,
    workingContent: state.workspace.editor.document.content,
    conflicts: [],
    resolutions: {},
    unresolvedConflictIds: [],
    rebaseRequired: false,
    updatedUnixMs: Date.now(),
  };
  return draft;
}

function previewSemanticEqual(left: unknown, right: unknown): boolean {
  const normalize = (value: unknown): unknown => {
    if (Array.isArray(value)) return value.map(normalize);
    if (value && typeof value === 'object') {
      return Object.fromEntries(Object.entries(value).sort(([a], [b]) => a.localeCompare(b))
        .map(([key, child]) => [key, normalize(child)]));
    }
    return value;
  };
  return JSON.stringify(normalize(left)) === JSON.stringify(normalize(right));
}

function rebasePreviewFinalEditorSession(
  current: FinalEditorSession,
  state: ConfigurationStateView,
): FinalEditorSession {
  const draft = structuredClone(current);
  if (draft.baseContent === state.desired.content) {
    draft.rebaseRequired = false;
    draft.basedOnStateRevision = state.stateRevision;
    draft.basedOnCandidateGeneration = state.generation;
    return draft;
  }
  let base: unknown;
  let mine: unknown;
  let updated: unknown;
  try {
    const parse = (content: string): unknown => state.format === 'yaml' ? parseYaml(content) : parsePreviewJsonc(content);
    base = parse(draft.baseContent);
    updated = parse(state.desired.content);
    if (previewSemanticEqual(base, updated)) {
      draft.baseContent = state.desired.content;
      draft.basedOnStateRevision = state.stateRevision;
      draft.basedOnCandidateGeneration = state.generation;
      draft.rebaseRequired = false;
      return draft;
    }
    mine = parse(draft.workingContent);
  } catch {
    draft.rebaseRequired = true;
    return draft;
  }

  for (const conflict of draft.conflicts) {
    if (!draft.unresolvedConflictIds.includes(conflict.conflictId)) continue;
    const original = conflict.baseValue.state === 'present'
      ? { present: true, value: conflict.baseValue.value } : { present: false };
    const user = conflict.userValue.state === 'present'
      ? { present: true, value: conflict.userValue.value } : { present: false };
    if (conflict.segments.length === 0) {
      base = original.value;
      mine = user.value;
    } else {
      updatePreviewConflictPath(base, conflict.segments, original);
      updatePreviewConflictPath(mine, conflict.segments, user);
    }
  }

  const conflicts: FinalEditorSession['conflicts'] = [];
  const merge = (
    baseValue: { present: boolean; value?: unknown },
    mineValue: { present: boolean; value?: unknown },
    updatedValue: { present: boolean; value?: unknown },
    segments: FinalEditorSession['conflicts'][number]['segments'],
  ): { present: boolean; value?: unknown } => {
    const equal = (
      left: { present: boolean; value?: unknown },
      right: { present: boolean; value?: unknown },
    ) => left.present === right.present
      && (!left.present || previewSemanticEqual(left.value, right.value));
    if (equal(mineValue, baseValue)) return structuredClone(updatedValue);
    if (equal(updatedValue, baseValue)) return structuredClone(mineValue);
    if (equal(mineValue, updatedValue)) return structuredClone(updatedValue);

    const baseObject = baseValue.present && baseValue.value && typeof baseValue.value === 'object'
      && !Array.isArray(baseValue.value) ? baseValue.value as Record<string, unknown> : null;
    const mineObject = mineValue.present && mineValue.value && typeof mineValue.value === 'object'
      && !Array.isArray(mineValue.value) ? mineValue.value as Record<string, unknown> : null;
    const updatedObject = updatedValue.present && updatedValue.value && typeof updatedValue.value === 'object'
      && !Array.isArray(updatedValue.value) ? updatedValue.value as Record<string, unknown> : null;
    if (baseObject && mineObject && updatedObject) {
      const result: Record<string, unknown> = {};
      const keys = new Set([
        ...Object.keys(baseObject),
        ...Object.keys(mineObject),
        ...Object.keys(updatedObject),
      ]);
      for (const key of keys) {
        const child = merge(
          { present: Object.hasOwn(baseObject, key), value: baseObject[key] },
          { present: Object.hasOwn(mineObject, key), value: mineObject[key] },
          { present: Object.hasOwn(updatedObject, key), value: updatedObject[key] },
          [...segments, { kind: 'key', key }],
        );
        if (child.present) result[key] = child.value;
      }
      return { present: true, value: result };
    }

    const semanticPath = segments.length
      ? `/${segments.map((segment) => segment.kind === 'key'
          ? segment.key.replaceAll('~', '~0').replaceAll('/', '~1')
          : `${segment.field}=${segment.value}`
        ).join('/')}`
      : '/';
    conflicts.push({
      conflictId: `preview-draft-conflict:${semanticPath}`,
      segments: structuredClone(segments),
      semanticPath,
      kind: !baseValue.present ? 'addVsAdd' : !mineValue.present ? 'deleteVsModify' : !updatedValue.present ? 'modifyVsDelete' : 'modifyVsModify',
      baseValue: baseValue.present ? { state: 'present', value: structuredClone(baseValue.value) } : { state: 'missing' },
      upstreamValue: updatedValue.present ? { state: 'present', value: structuredClone(updatedValue.value) } : { state: 'missing' },
      userValue: mineValue.present ? { state: 'present', value: structuredClone(mineValue.value) } : { state: 'missing' },
      canMerge: false,
    });
    return structuredClone(updatedValue);
  };

  const merged = merge(
    { present: true, value: base },
    { present: true, value: mine },
    { present: true, value: updated },
    [],
  );
  draft.baseContent = state.desired.content;
  draft.workingContent = `${JSON.stringify(merged.value, null, 2)}\n`;
  draft.basedOnStateRevision = state.stateRevision;
  draft.basedOnCandidateGeneration = state.generation;
  draft.conflicts = conflicts;
  draft.resolutions = {};
  draft.unresolvedConflictIds = conflicts.map((conflict) => conflict.conflictId);
  draft.rebaseRequired = false;
  draft.draftRevision += 1;
  draft.updatedUnixMs = Date.now();
  return draft;
}

function configurationWorkspaceSnapshot(programId: string): {
  state: ConfigurationStateView;
  editorSession: FinalEditorSession;
} {
  const state = configurationState(programId);
  state.managedIntegrations = state.managedIntegrations?.map((integration) => {
    const settings = previewManagedSettingPaths(specs[programId]).filter((setting) => setting.settingId.startsWith(`${integration.integrationId}.`)).map((setting) => {
      let effectiveValue: import('../types').SemanticValue = { state: 'missing' };
      try {
        const content = previewUpstreamDocuments.get(programId) ?? state.desired.content;
        const document: unknown = state.format === 'yaml' ? parseYaml(content) : parsePreviewJsonc(content);
        const found = previewConflictPathValue(document, setting.segments);
        if (found.present) {
          const value = setting.settingId.endsWith('Port') && typeof found.value === 'string'
            ? Number(found.value.split(':').at(-1)) : found.value;
          effectiveValue = { state: 'present', value };
        }
      } catch { effectiveValue = { state: 'missing' }; }
      const savedValue: import('../types').SemanticValue = setting.value === undefined ? { state: 'missing' } : { state: 'present', value: setting.value };
      return { settingId: setting.settingId, savedValue, effectiveValue, canUseSavedValue: savedValue.state === 'present' && !previewSemanticEqual(savedValue, effectiveValue) };
    });
    return { ...integration, settings, status: settings.some((setting) => setting.canUseSavedValue) ? 'latestSettings' : integration.status };
  });
  const editorSession = finalEditorSession(programId);
  const unresolved = new Set(editorSession.unresolvedConflictIds);
  state.workspace.editor.conflicts.push(...editorSession.conflicts
    .filter((item) => unresolved.has(item.conflictId))
    .map((item) => ({
      ...item,
      reference: {
        origin: 'draft' as const,
        conflictId: item.conflictId,
        fingerprint: JSON.stringify(item),
      },
    })));
  state.workspace.editor.conflicts = state.workspace.editor.conflicts.map((conflict) => ({
    ...conflict,
    conflictId: `${conflict.reference.origin}:${conflict.reference.conflictId}`,
  }));
  // This fixture exercises authoritative diagnostics without mirroring source capability rules.
  if (platformIssuePreview && programId === 'sing-box-edge') {
    try {
      const content = editorSession.draftRevision > 0 && !editorSession.rebaseRequired
        ? editorSession.workingContent : state.desired.content;
      const document = parsePreviewJsonc(content) as { inbounds?: { auto_redirect?: boolean }[] };
      if (document.inbounds?.[0]?.auto_redirect === true) {
        const code = 'CONFIGURATION_PLATFORM_UNSUPPORTED';
        const location = { semanticPath: '/inbounds/0/auto_redirect', documentPath: ['inbounds', '0', 'auto_redirect'] };
        state.workspace.editor.diagnostics.push({ code, message: code, messageKey: code, location, scope: { surface: 'configuration' } });
        state.workspace.editor.blockers.push({ code, messageKey: code, semanticPath: location.semanticPath,
          scope: { surface: 'configuration' }, blocks: ['validate', 'apply'], recoveryAction: 'reviewCandidate' });
        state.workspace.editor.candidateStatus = 'invalid';
        state.workspace.editor.canValidate = false;
        state.workspace.editor.canApply = false;
      }
    } catch { /* Incomplete text is handled by editor syntax diagnostics. */ }
  }
  return { state, editorSession };
}

function refreshPreviewUnresolvedConflicts(draft: FinalEditorSession): void {
  let document: unknown;
  try {
    document = parsePreviewJsonc(draft.workingContent);
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
  conflict: FinalEditorSession['conflicts'][number],
  resolution: FinalEditorSession['resolutions'][string],
): { present: boolean; value?: unknown } {
  const selected = resolution === 'keepMine' ? conflict.userValue
    : resolution === 'acceptUpstream' ? conflict.upstreamValue
    : resolution.manualEdit.value;
  return selected.state === 'missing' ? { present: false } : { present: true, value: selected.value };
}

function previewConflictPathValue(
  root: unknown,
  segments: FinalEditorSession['conflicts'][number]['segments'],
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
  segments: FinalEditorSession['conflicts'][number]['segments'],
  selected: { present: boolean; value?: unknown },
): void {
  if (!root || typeof root !== 'object' || segments.length === 0) return;
  let current: unknown = root;
  for (const [index, segment] of segments.slice(0, -1).entries()) {
    const next = previewConflictPathValue(current, [segment]);
    if (!next.present || !next.value || typeof next.value !== 'object') {
      if (!selected.present) return;
      const container = segments[index + 1].kind === 'identity' ? [] : {};
      updatePreviewConflictPath(current, [segment], { present: true, value: container });
    }
    current = previewConflictPathValue(current, [segment]).value;
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

function previewManagedIntegrationContent(
  spec: ProgramSpec,
  fallback: string,
): string {
  if (spec.type.kind === 'generic') return fallback;
  let document: Record<string, unknown>;
  try {
    const parsed = parsePreviewJsonc(fallback);
    document = parsed && typeof parsed === 'object' && !Array.isArray(parsed)
      ? parsed as Record<string, unknown>
      : {};
  } catch {
    return fallback;
  }
  if (spec.type.kind === 'singBox') {
    const services = Array.isArray(document.services)
      ? document.services.filter((service) => !(
          service && typeof service === 'object' && !Array.isArray(service)
          && (service as Record<string, unknown>).tag === 'camellia-nexus-api'
        ))
      : [];
    const apiDashboard = spec.managedConfig?.singBoxDashboard;
    if (apiDashboard) {
      services.push({
        type: 'api',
        tag: 'camellia-nexus-api',
        listen: '127.0.0.1',
        listen_port: apiDashboard.listenPort,
        dashboard: {
          enabled: true,
          update_interval: apiDashboard.updateInterval,
        },
      });
    }
    if (services.length > 0) document.services = services;
    else delete document.services;
    const experimental = document.experimental && typeof document.experimental === 'object'
      && !Array.isArray(document.experimental)
      ? document.experimental as Record<string, unknown>
      : {};
    const clashApi = experimental.clash_api && typeof experimental.clash_api === 'object'
      && !Array.isArray(experimental.clash_api)
      ? experimental.clash_api as Record<string, unknown>
      : {};
    const clashDashboard = spec.managedConfig?.singBoxClashDashboard;
    if (clashDashboard) {
      clashApi.external_controller = `127.0.0.1:${clashDashboard.listenPort}`;
      clashApi.external_ui = 'clash-dashboard';
      if (clashDashboard.downloadUrl) {
        clashApi.external_ui_download_url = clashDashboard.downloadUrl;
      } else {
        delete clashApi.external_ui_download_url;
      }
      experimental.clash_api = clashApi;
      document.experimental = experimental;
    } else {
      delete clashApi.external_controller;
      delete clashApi.external_ui;
      delete clashApi.external_ui_download_url;
      if (Object.keys(clashApi).length > 0) experimental.clash_api = clashApi;
      else delete experimental.clash_api;
      if (Object.keys(experimental).length > 0) document.experimental = experimental;
      else delete document.experimental;
    }
  } else if (spec.type.kind === 'xray') {
    const dashboard = spec.managedConfig?.xrayDashboard;
    if (dashboard) {
      document.api = {
        ...(
          document.api && typeof document.api === 'object' && !Array.isArray(document.api)
            ? document.api as Record<string, unknown>
            : {}
        ),
        tag: 'camellia-nexus-api',
        listen: `127.0.0.1:${dashboard.apiPort}`,
        services: [
          'HandlerService',
          'LoggerService',
          'StatsService',
          'RoutingService',
          'ReflectionService',
        ],
      };
      document.metrics = {
        ...(
          document.metrics && typeof document.metrics === 'object' && !Array.isArray(document.metrics)
            ? document.metrics as Record<string, unknown>
            : {}
        ),
        tag: 'camellia-nexus-metrics',
        listen: `127.0.0.1:${dashboard.metricsPort}`,
      };
      if (!document.stats || typeof document.stats !== 'object' || Array.isArray(document.stats)) {
        document.stats = {};
      }
    } else {
      if (
        document.api && typeof document.api === 'object' && !Array.isArray(document.api)
        && (document.api as Record<string, unknown>).tag === 'camellia-nexus-api'
      ) delete document.api;
      if (
        document.metrics && typeof document.metrics === 'object' && !Array.isArray(document.metrics)
        && (document.metrics as Record<string, unknown>).tag === 'camellia-nexus-metrics'
      ) delete document.metrics;
    }
  } else if (spec.type.kind === 'mihomo') {
    const dashboard = spec.managedConfig?.mihomoDashboard;
    if (dashboard) {
      document['external-controller'] = `127.0.0.1:${dashboard.listenPort}`;
      document['external-ui'] = 'camellia-nexus-mihomo-dashboard';
      if (dashboard.downloadUrl) document['external-ui-url'] = dashboard.downloadUrl;
      else delete document['external-ui-url'];
    } else {
      delete document['external-controller'];
      delete document['external-ui'];
      delete document['external-ui-url'];
    }
  }
  return `${JSON.stringify(document, null, 2)}\n`;
}

function previewInlineSourceContent(spec: ProgramSpec): string | null {
  const inline = spec.managedConfig?.sources.filter(
    (source): source is Extract<ConfigSource, { mode: 'inline' }> => (
      source.mode === 'inline' && source.enabled
    ),
  ) ?? [];
  if (inline.length === 0) return null;
  let merged: unknown = {};
  const merge = (current: unknown, next: unknown): unknown => {
    if (
      current && next
      && typeof current === 'object' && !Array.isArray(current)
      && typeof next === 'object' && !Array.isArray(next)
    ) {
      const result = structuredClone(current) as Record<string, unknown>;
      for (const [key, value] of Object.entries(next)) {
        result[key] = key in result ? merge(result[key], value) : structuredClone(value);
      }
      return result;
    }
    return structuredClone(next);
  };
  for (const source of inline) merged = merge(merged, parsePreviewJsonc(source.content));
  return `${JSON.stringify(merged, null, 2)}\n`;
}

function recordPreviewContributions(programId: string, owner: string, changes: FinalChangeProjection[]): string {
  const contributions = previewUpstreamContributions.get(programId)!;
  for (const change of changes) {
    contributions.writes = contributions.writes.filter((write) =>
      write.owner !== owner || write.change.segments.length < change.segments.length
      || !change.segments.every((segment, index) => previewSemanticEqual(segment, write.change.segments[index])));
    contributions.writes.push({ owner, change: structuredClone(change) });
  }
  let document: unknown = parsePreviewJsonc(contributions.source);
  const source: unknown = structuredClone(document);
  for (const { owner, change } of contributions.writes) {
    const value = owner === 'sources'
      ? previewConflictPathValue(source, change.segments)
      : change.finalValue.state === 'present'
        ? { present: true, value: change.finalValue.value } : { present: false };
    if (change.segments.length === 0) document = value.present ? structuredClone(value.value) : {};
    else updatePreviewConflictPath(document, change.segments, value);
  }
  return `${JSON.stringify(document, null, 2)}\n`;
}

function previewSourceUpdate(programId: string, sourceContent: string, explicit: boolean): string {
  const contributions = previewUpstreamContributions.get(programId)!;
  const changes = explicit ? previewFinalChanges(contributions.source, sourceContent) : [];
  contributions.source = sourceContent;
  const expand = (change: FinalChangeProjection): FinalChangeProjection[] => {
    const value = change.finalValue.state === 'present' ? change.finalValue.value : undefined;
    if (change.kind !== 'added' || !value || typeof value !== 'object' || Array.isArray(value)) return [change];
    return Object.entries(value).flatMap(([key, child]) => expand({
      ...change, semanticPath: `${change.semanticPath}/${key}`,
      segments: [...change.segments, { kind: 'key', key }], finalValue: { state: 'present', value: child },
    }));
  };
  return recordPreviewContributions(programId, 'sources', changes.flatMap(expand));
}

function previewManagedSettingPaths(spec: ProgramSpec) {
  const managed = spec.managedConfig;
  const values: [string, string, unknown][] = spec.type.kind === 'singBox' ? [
    ['dashboard.singBoxApi.listenPort', '/services[tag=camellia-nexus-api]/listen_port', managed?.singBoxDashboard?.listenPort],
    ['dashboard.singBoxApi.updateInterval', '/services[tag=camellia-nexus-api]/dashboard/update_interval', managed?.singBoxDashboard?.updateInterval],
    ['dashboard.singBoxClash.listenPort', '/experimental/clash_api/external_controller', managed?.singBoxClashDashboard?.listenPort],
    ['dashboard.singBoxClash.downloadUrl', '/experimental/clash_api/external_ui_download_url', managed?.singBoxClashDashboard?.downloadUrl],
  ] : spec.type.kind === 'xray' ? [
    ['dashboard.xray.apiPort', '/api/listen', managed?.xrayDashboard?.apiPort],
    ['dashboard.xray.metricsPort', '/metrics/listen', managed?.xrayDashboard?.metricsPort],
  ] : spec.type.kind === 'mihomo' ? [
    ['dashboard.mihomo.listenPort', '/external-controller', managed?.mihomoDashboard?.listenPort],
    ['dashboard.mihomo.downloadUrl', '/external-ui-url', managed?.mihomoDashboard?.downloadUrl],
  ] : [];
  return values.map(([settingId, path, value]) => ({ settingId, path, segments: semanticPathSegments(path), value }));
}

function previewManagedUpdate(previous: ProgramSpec, next: ProgramSpec, current: string, claimedSettings: string[] = []): string {
  const before = previewManagedIntegrationContent(previous, current);
  const after = previewManagedIntegrationContent(next, current);
  const changes = previewFinalChanges(before, after);
  for (const setting of previewManagedSettingPaths(next).filter((setting) => claimedSettings.includes(setting.settingId))) {
    const upstream = previewConflictPathValue(parsePreviewJsonc(current), setting.segments);
    const final = previewConflictPathValue(parsePreviewJsonc(after), setting.segments);
    if (!final.present || previewSemanticEqual(upstream, final)) continue;
    changes.push({ editId: `preview-details:${setting.settingId}`, semanticPath: setting.path, segments: setting.segments,
      kind: 'modified', upstreamValue: upstream.present ? { state: 'present', value: upstream.value } : { state: 'missing' },
      finalValue: { state: 'present', value: final.value }, issues: [] });
  }
  return recordPreviewContributions(next.id, 'details', changes);
}

function previewFinalChanges(
  upstreamContent: string,
  finalContent: string,
): FinalChangeProjection[] {
  let upstream: unknown;
  let finalValue: unknown;
  try {
    upstream = parsePreviewJsonc(upstreamContent);
    finalValue = parsePreviewJsonc(finalContent);
  } catch {
    return [];
  }
  const changes: FinalChangeProjection[] = [];
  const walk = (
    upstreamEntry: { present: boolean; value?: unknown },
    finalEntry: { present: boolean; value?: unknown },
    segments: FinalChangeProjection['segments'],
  ) => {
    if (
      upstreamEntry.present === finalEntry.present
      && (!upstreamEntry.present || previewSemanticEqual(upstreamEntry.value, finalEntry.value))
    ) return;
    const upstreamObject = upstreamEntry.present && upstreamEntry.value
      && typeof upstreamEntry.value === 'object' && !Array.isArray(upstreamEntry.value)
      ? upstreamEntry.value as Record<string, unknown>
      : null;
    const finalObject = finalEntry.present && finalEntry.value
      && typeof finalEntry.value === 'object' && !Array.isArray(finalEntry.value)
      ? finalEntry.value as Record<string, unknown>
      : null;
    if (upstreamObject && finalObject) {
      for (const key of new Set([...Object.keys(upstreamObject), ...Object.keys(finalObject)])) {
        walk(
          { present: Object.hasOwn(upstreamObject, key), value: upstreamObject[key] },
          { present: Object.hasOwn(finalObject, key), value: finalObject[key] },
          [...segments, { kind: 'key', key }],
        );
      }
      return;
    }
    const semanticPath = segments.length
      ? `/${segments.map((segment) => segment.kind === 'key'
          ? segment.key.replaceAll('~', '~0').replaceAll('/', '~1')
          : `${segment.field}=${segment.value}`
        ).join('/')}`
      : '/';
    changes.push({
      editId: `preview-final-edit:${semanticPath}`,
      semanticPath,
      segments: structuredClone(segments),
      kind: !finalEntry.present ? 'deleted' : !upstreamEntry.present ? 'added' : 'modified',
      upstreamValue: upstreamEntry.present
        ? { state: 'present', value: structuredClone(upstreamEntry.value) }
        : { state: 'missing' },
      finalValue: finalEntry.present
        ? { state: 'present', value: structuredClone(finalEntry.value) }
        : { state: 'missing' },
      issues: [],
    });
  };
  walk(
    { present: true, value: upstream },
    { present: true, value: finalValue },
    [],
  );
  return changes;
}

function refreshPreviewWorkspaceGates(state: ConfigurationStateView): void {
  const conflictBlockers = state.desired.conflicts
    .filter((conflict) => conflict.severity === 'error')
    .map((conflict) => ({
      code: conflict.messageKey ?? 'CONFIGURATION_CONFLICT',
      messageKey: conflict.messageKey ?? 'CONFIGURATION_CONFLICT',
      scope: conflict.scope ?? { surface: 'configuration' as const },
      semanticPath: conflict.semanticPath,
      blocks: ['save', 'validate', 'apply'] as const,
      recoveryAction: conflict.messageKey === 'SOURCE_VALUE_CONFLICT'
        ? 'resolveSourceConflict' as const
        : conflict.messageKey === 'LAYER_OWNERSHIP_CONFLICT'
          ? 'resolveLayerConflict' as const
          : 'openFinalConfiguration' as const,
    }));
  const diagnosticBlockers = state.desired.diagnostics.map((diagnostic) => {
    const sourceOwned = diagnostic.scope?.surface === 'sources';
    return {
      code: diagnostic.messageKey ?? diagnostic.code,
      messageKey: diagnostic.messageKey ?? diagnostic.code,
      scope: diagnostic.scope ?? { surface: 'configuration' as const },
      details: !diagnostic.scope || diagnostic.scope.surface === 'configuration'
        ? diagnostic.details
        : undefined,
      blocks: sourceOwned
        ? ['save' as const, 'validate' as const, 'apply' as const]
        : ['apply' as const],
      recoveryAction: sourceOwned ? 'resolveSourceConflict' as const : 'reviewCandidate' as const,
    };
  });
  const needsValidation = state.desired.validation !== 'valid'
    && conflictBlockers.length === 0
    && diagnosticBlockers.length === 0;
  const candidateUnsaved = state.workspace.editor.candidateStatus === 'unsaved';
  state.workspace.editor.blockers = [
    ...(state.coreAdmission && state.coreAdmission.status !== 'admitted' ? [{
      code: state.coreAdmission.messageKey,
      messageKey: state.coreAdmission.messageKey,
      scope: { surface: 'compatibility' as const },
      blocks: ['validate' as const, 'apply' as const],
      recoveryAction: 'openCompatibility' as const,
    }] : []),
    ...conflictBlockers.map((blocker) => ({ ...blocker, blocks: [...blocker.blocks] })),
    ...diagnosticBlockers.map((blocker) => ({ ...blocker, blocks: [...blocker.blocks] })),
    ...(candidateUnsaved && ![...conflictBlockers, ...diagnosticBlockers].some((blocker) => blocker.blocks.includes('save')) ? [{
      code: 'CONFIGURATION_CANDIDATE_UNSAVED',
      messageKey: 'CONFIGURATION_CANDIDATE_UNSAVED',
      scope: { surface: 'configuration' as const },
      blocks: ['validate' as const, 'apply' as const],
      recoveryAction: 'openFinalConfiguration' as const,
    }] : []),
    ...(needsValidation && !candidateUnsaved ? [{
      code: 'CORE_VALIDATION_REQUIRED',
      messageKey: 'CORE_VALIDATION_REQUIRED',
      scope: { surface: 'configuration' as const },
      blocks: ['apply' as const],
      recoveryAction: 'validateCandidate' as const,
    }] : []),
  ];
  state.workspace.editor.canSave = state.workspace.editor.blockers.every(
    (blocker) => !blocker.blocks.includes('save'),
  );
  state.workspace.editor.canValidate = state.workspace.editor.blockers.every(
    (blocker) => !blocker.blocks.includes('validate'),
  );
  state.workspace.editor.canApply = state.workspace.editor.blockers.every(
    (blocker) => !blocker.blocks.includes('apply'),
  );
}

function syncPreviewEditor(state: ConfigurationStateView): void {
  state.workspace.editor.diagnostics = structuredClone(state.desired.diagnostics);
  state.workspace.editor.document = {
    content: state.desired.content,
    revision: structuredClone(state.desired.revision),
  };
  if (state.workspace.editor.conflicts.length > 0) {
    state.workspace.editor.editStatus = 'conflict';
  } else if (state.workspace.editor.changes.length > 0) {
    state.workspace.editor.editStatus = 'modified';
  } else {
    state.workspace.editor.editStatus = 'clean';
  }
  refreshPreviewWorkspaceGates(state);
}

function updateConfigurationState(
  programId: string,
  update: (state: ConfigurationStateView) => void,
  advanceGeneration = true,
): ConfigurationStateView {
  const state = configurationState(programId);
  const before = JSON.stringify(state);
  const previousContent = state.desired.content;
  const previousProfile = state.compatibilityProfile.profileHash;
  update(state);
  if (state.desired.content !== previousContent) {
    state.desired.revision.contentHash = previewContentHash(state.desired.content);
    // A content mutation invalidates native evidence. The preview backend
    // mirrors the real coordinator's fail-closed rule; only the explicit
    // validation command creates fresh evidence.
    state.desired.validationEvidence = undefined;
    if (state.desired.validation === 'valid') state.desired.validation = 'pending';
    state.workspace.editor.candidateStatus = 'unsaved';
    const savedDocument = previewConfigurationDocuments.get(programId);
    previewConfigurationDocuments.set(programId, {
      ...savedDocument,
      content: state.desired.content,
      baseHash: state.desired.revision.contentHash,
    });
  }
  if (advanceGeneration && (state.desired.content !== previousContent
    || state.compatibilityProfile.profileHash !== previousProfile)) {
    state.generation += 1;
    state.desired.revision = {
      ...state.desired.revision,
      generation: state.generation,
      createdUnixMs: Date.now(),
    };
  }
  refreshPreviewWorkspaceGates(state);
  syncPreviewEditor(state);
  if (JSON.stringify(state) !== before) state.stateRevision += 1;
  previewConfigurationStates.set(programId, structuredClone(state));
  return state;
}

function updatePreviewUpstream(
  programId: string,
  content: string,
  update: (state: ConfigurationStateView) => void = () => undefined,
): ConfigurationStateView {
  const current = configurationState(programId);
  const previousUpstream = previewUpstreamDocuments.get(programId) ?? current.desired.content;
  const existingDraft = previewFinalDrafts.get(programId);
  const existingConflicts = structuredClone(current.workspace.editor.conflicts);
  const existingDesiredConflicts = structuredClone(current.desired.conflicts);
  let nextContent = content;
  let rebasedCommitted: FinalEditorSession | null = null;

  if (!existingDraft && (current.workspace.editor.changes.length > 0 || existingConflicts.length > 0)) {
    const committedSession: FinalEditorSession = {
      sessionId: `preview-rebase-${programId}`,
      draftRevision: 0,
      basedOnStateRevision: current.stateRevision,
      basedOnCandidateGeneration: current.generation,
      baseContent: previousUpstream,
      workingContent: current.desired.content,
      conflicts: existingConflicts,
      resolutions: {},
      unresolvedConflictIds: existingConflicts.map((conflict) => conflict.conflictId),
      rebaseRequired: false,
      updatedUnixMs: Date.now(),
    };
    const target = structuredClone(current);
    target.stateRevision += 1;
    target.generation += 1;
    target.desired.content = content;
    rebasedCommitted = rebasePreviewFinalEditorSession(committedSession, target);
    nextContent = rebasedCommitted.workingContent;
  }

  previewUpstreamDocuments.set(programId, content);
  const state = updateConfigurationState(programId, (next) => {
    next.desired.content = nextContent;
    if (previousUpstream !== content) {
      next.desired.validation = 'pending';
      next.desired.validationEvidence = undefined;
      next.desired.diagnostics = [];
    }
    next.desired.conflicts = existingConflicts.length > 0 ? existingDesiredConflicts : [];
    next.workspace.editor.conflicts = existingConflicts;
    next.workspace.editor.changes = previewFinalChanges(content, nextContent);
    if (next.format === 'jsonc') {
      const upstream = parsePreviewJsonc(content);
      next.guidedProjection = next.guidedProjection.map((projection) => {
        const path = previewGuidedPath(next.kind, projection.settingId);
        if (!path) return projection;
        const value = previewConflictPathValue(upstream, path.map((key) => ({ kind: 'key', key })));
        return { ...projection, value: value.value };
      });
    }
    update(next);
  });

  if (rebasedCommitted?.unresolvedConflictIds.length) {
    rebasedCommitted.basedOnStateRevision = state.stateRevision;
    rebasedCommitted.basedOnCandidateGeneration = state.generation;
    rebasedCommitted.baseContent = content;
    rebasedCommitted.draftRevision = Math.max(1, rebasedCommitted.draftRevision);
    previewFinalDrafts.set(programId, structuredClone(rebasedCommitted));
  }
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
      ? (platformIssuePreview ? '{\n  "inbounds": [{ "type": "tun", "tag": "tun-in", "auto_route": true, "auto_redirect": true }]\n}\n' : '{\n  "log": { "level": "info" },\n  "outbounds": [\n    { "type": "direct", "tag": "direct" },\n    { "type": "socks", "tag": "proxy-sg", "server": "127.0.0.1", "server_port": 1080 }\n  ],\n  "route": { "final": "proxy-sg" }\n}\n')
      : '{\n  "log": { "loglevel": "warning" },\n  "route": { "final": "proxy-sg" }\n}\n'),
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
    inbounds: {
      type: 'array', items: { type: 'object', properties: {
        type: { type: 'string' }, tag: { type: 'string' }, auto_route: { type: 'boolean' }, auto_redirect: { type: 'boolean' },
      }, additionalProperties: false },
    },
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
      case 'create_program':
        if (previewParameters.has('__ui_create_prerelease')) {
          throw {
            code: 'UNSUPPORTED_BINARY',
            messageKey: 'CORE_PRERELEASE_NOT_SUPPORTED',
            message: 'Unsupported program',
          };
        }
        throw { code: 'MOCK_COMMAND_UNIMPLEMENTED', message: `No UI preview response for ${command}` };
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
        const claimedSettings = (objectArgs(args).claimedManagedSettings ?? []) as string[];
        let programId = '';
        if (next && typeof next === 'object') {
          const nextSpec = next as ProgramSpec;
          programId = nextSpec.id;
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
          specs[nextSpec.id] = structuredClone(nextSpec);
          if ((managedChanged || claimedSettings.length > 0) && nextSpec.type.kind !== 'generic') {
            const state = configurationState(nextSpec.id);
            const upstream = previewManagedUpdate(
              previous, nextSpec,
              previewUpstreamDocuments.get(nextSpec.id) ?? state.desired.content,
              claimedSettings,
            );
            updatePreviewUpstream(nextSpec.id, upstream, (nextState) => {
              nextState.managedIntegrations = (nextState.managedIntegrations ?? []).map((integration) => {
                const enabled = integration.integrationId === 'dashboard.singBoxApi'
                  ? !!nextSpec.managedConfig?.singBoxDashboard
                  : integration.integrationId === 'dashboard.singBoxClash'
                    ? !!nextSpec.managedConfig?.singBoxClashDashboard
                    : integration.integrationId === 'dashboard.xray'
                      ? !!nextSpec.managedConfig?.xrayDashboard
                      : !!nextSpec.managedConfig?.mihomoDashboard;
                return { ...integration, status: enabled ? 'explicit' : 'inactive', effectiveEnabled: enabled };
              });
            });
          }
        }
        return programId && specs[programId]?.type.kind !== 'generic'
          ? configurationWorkspaceSnapshot(programId)
          : null;
      }
      case 'update_program_and_restart': {
        requireLifecycleAccess('restart');
        const next = objectArgs(args).spec;
        const claimedSettings = (objectArgs(args).claimedManagedSettings ?? []) as string[];
        let programId = '';
        if (next && typeof next === 'object') {
          const nextSpec = next as ProgramSpec;
          programId = nextSpec.id;
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
          specs[nextSpec.id] = structuredClone(nextSpec);
          if ((managedChanged || claimedSettings.length > 0) && nextSpec.type.kind !== 'generic') {
            const state = configurationState(nextSpec.id);
            const upstream = previewManagedUpdate(
              previous, nextSpec,
              previewUpstreamDocuments.get(nextSpec.id) ?? state.desired.content,
              claimedSettings,
            );
            updatePreviewUpstream(nextSpec.id, upstream, (nextState) => {
              nextState.managedIntegrations = (nextState.managedIntegrations ?? []).map((integration) => {
                const enabled = integration.integrationId === 'dashboard.singBoxApi'
                  ? !!nextSpec.managedConfig?.singBoxDashboard
                  : integration.integrationId === 'dashboard.singBoxClash'
                    ? !!nextSpec.managedConfig?.singBoxClashDashboard
                    : integration.integrationId === 'dashboard.xray'
                      ? !!nextSpec.managedConfig?.xrayDashboard
                      : !!nextSpec.managedConfig?.mihomoDashboard;
                return { ...integration, status: enabled ? 'explicit' : 'inactive', effectiveEnabled: enabled };
              });
            });
          }
        }
        setLifecycleState(args, { status: 'running', pid: 42421, startedUnixMs: Date.now() });
        return programId && specs[programId]?.type.kind !== 'generic'
          ? configurationWorkspaceSnapshot(programId)
          : null;
      }
      case 'remove_program': delete specs[stringArg(args, 'programId')]; return null;
      case 'list_actions': {
        const programId = stringArg(args, 'programId');
        const kind = specs[programId]?.type.kind;
        if (kind === 'singBox') {
          return mockProgramSelectionResult(command, programId, [
            { id: 'format-config', label: 'Format with sing-box', allowedStates: ['stopped', 'running'], confirmation: false },
          ]);
        }
        return mockProgramSelectionResult(command, programId, []);
      }
      case 'load_config': {
        const document = configDocument(stringArg(args, 'programId'));
        if (configurationMetadataFailurePending) {
          configurationMetadataFailurePending = false;
          throw new Error('Optional editor metadata is unavailable.');
        }
        if (previewParameters.has('__ui_slow_config_metadata')) {
          return new Promise((resolve) => window.setTimeout(() => resolve(document), 4500));
        }
        return document;
      }
      case 'get_configuration_workspace_preview': {
        return configurationWorkspaceSnapshot(stringArg(args, 'programId'));
      }
      case 'get_configuration_workspace': {
        if (new URLSearchParams(window.location.search).has('__ui_identity_read_error') && !identityReadReady) {
          throw new Error(JSON.stringify({
            code: 'TIMEOUT', message: 'Program identity probe exceeded its deadline',
            details: 'bounded identity read timed out',
          }));
        }
        const programId = stringArg(args, 'programId');
        const snapshot = configurationWorkspaceSnapshot(programId);
        configurationWorkspaceReads += 1;
        if (previewParameters.has('__ui_slow_workspace_after_first') && configurationWorkspaceReads > 1) {
          return new Promise((resolve) => window.setTimeout(() => resolve(snapshot), 2500));
        }
        if (previewParameters.has('__ui_slow_verified_workspace')) {
          return new Promise((resolve) => window.setTimeout(() => resolve(snapshot), 1800));
        }
        return snapshot;
      }
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
            parserRevision: 'share-parser-preview',
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
              translatorRevision: 'share-translator-preview',
              profileHash: 'preview-core-profile-hash',
              knowledgeHash: 'preview-knowledge-digest',
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
      case 'update_final_configuration_draft': {
        const programId = stringArg(args, 'programId');
        window.dispatchEvent(new CustomEvent('camellia-ui-preview:draft-write', { detail: programId }));
        if (finalDraftWriteFailurePending) {
          finalDraftWriteFailurePending = false;
          return new Promise((_, reject) => {
            window.addEventListener('camellia-ui-preview:reject-draft-write', () => reject({
              code: 'STORAGE', message: 'Draft storage is temporarily unavailable.',
            }), { once: true });
          });
        }
        const request = objectArgs(args).request as {
          draft?: FinalEditorSession;
          expectedRevision?: number;
        } | undefined;
        if (!request?.draft) throw { code: 'INVALID_SPEC', message: 'Draft is required' };
        const draft = structuredClone(request.draft);
        const persisted = previewFinalDrafts.get(programId);
        if (
          request.expectedRevision !== draft.draftRevision
          || (persisted && (
            persisted.draftRevision !== request.expectedRevision
            || persisted.sessionId !== draft.sessionId
          ))
          || (!persisted && request.expectedRevision !== 0)
        ) {
          throw { code: 'CONFIG_CONFLICT', messageKey: 'CONFIGURATION_DRAFT_STALE', message: 'Preview Final editor draft revision is stale' };
        }
        if (persisted) {
          draft.conflicts = structuredClone(persisted.conflicts);
          draft.resolutions = structuredClone(persisted.resolutions);
          draft.unresolvedConflictIds = [...persisted.unresolvedConflictIds];
        }
        draft.draftRevision += 1;
        draft.updatedUnixMs = Date.now();
        refreshPreviewUnresolvedConflicts(draft);
        previewFinalDrafts.set(programId, structuredClone(draft));
        if (finalDraftResponseHeldPending) {
          finalDraftResponseHeldPending = false;
          const snapshot = configurationWorkspaceSnapshot(programId);
          return new Promise((resolve) => {
            window.addEventListener('camellia-ui-preview:release-draft-response', () => resolve(snapshot), { once: true });
          });
        }
        return configurationWorkspaceSnapshot(programId);
      }
      case 'rebase_final_configuration_draft': {
        const programId = stringArg(args, 'programId');
        const state = configurationState(programId);
        const draft = finalEditorSession(programId);
        draft.basedOnStateRevision = state.stateRevision;
        draft.basedOnCandidateGeneration = state.generation;
        draft.resolutions = {};
        draft.unresolvedConflictIds = draft.conflicts.map((conflict) => conflict.conflictId);
        draft.rebaseRequired = false;
        draft.draftRevision += 1;
        draft.updatedUnixMs = Date.now();
        previewFinalDrafts.set(programId, structuredClone(draft));
        return configurationWorkspaceSnapshot(programId);
      }
      case 'adopt_upstream_change': {
        const programId = stringArg(args, 'programId');
        const request = objectArgs(args).request as import('../types').AdoptUpstreamChangeRequest;
        const key = `${programId}:${request.operationId}`;
        const previous = previewAdoptedChanges.get(key);
        if (previous !== undefined) {
          if (previous !== JSON.stringify(request)) throw { code: 'CONFIG_CONFLICT', messageKey: 'CONFIGURATION_OPERATION_MISMATCH' };
          return configurationWorkspaceSnapshot(programId);
        }
        const current = configurationState(programId);
        if (current.stateRevision !== request.expectedStateRevision) throw { code: 'CONFIG_CONFLICT', messageKey: 'CONFIGURATION_STATE_STALE' };
        if (adoptWriteFailurePending) {
          adoptWriteFailurePending = false;
          throw { code: 'STORAGE', message: 'Injected adoption write failure' };
        }
        const draft = previewFinalDrafts.get(programId);
        if (draft && (draft.sessionId !== request.editorSessionId || draft.draftRevision !== request.expectedDraftRevision)) {
          throw { code: 'CONFIG_CONFLICT', messageKey: 'CONFIGURATION_DRAFT_STALE' };
        }
        if (draft && (draft.rebaseRequired || draft.workingContent !== current.desired.content)) {
          throw { code: 'CONFIG_CONFLICT', messageKey: 'CONFIGURATION_DRAFT_UNCOMMITTED' };
        }
        const edit = current.workspace.editor.changes.find((item) => item.editId === request.editId && JSON.stringify(item.segments) === JSON.stringify(request.path));
        if (!edit) throw { code: 'CONFIG_CONFLICT', messageKey: 'CONFIGURATION_STATE_STALE' };
        updateConfigurationState(programId, (state) => {
          const document = parsePreviewJsonc(state.desired.content);
          updatePreviewConflictPath(document, edit.segments, edit.upstreamValue.state === 'missing'
            ? { present: false }
            : { present: true, value: edit.upstreamValue.value });
          state.desired.content = `${JSON.stringify(document, null, 2)}\n`;
          state.workspace.editor.changes = state.workspace.editor.changes.filter((item) => item.editId !== edit.editId);
        });
        previewAdoptedChanges.set(key, JSON.stringify(request));
        return configurationWorkspaceSnapshot(programId);
      }
      case 'resolve_configuration_conflict': {
        const programId = stringArg(args, 'programId');
        const request = objectArgs(args).request as import('../types').ResolveConfigurationConflictRequest;
        const receiptKey = `${programId}:${request.operationId}`;
        const prior = previewConflictOperations.get(receiptKey);
        if (prior) {
          if (prior.serialized !== JSON.stringify(request)) throw { code: 'CONFIG_CONFLICT', messageKey: 'CONFIGURATION_OPERATION_MISMATCH' };
          return configurationWorkspaceSnapshot(programId);
        }
        const current = configurationState(programId);
        if (request.expectedStateRevision !== current.stateRevision) {
          throw { code: 'CONFIG_CONFLICT', messageKey: 'CONFIGURATION_STATE_STALE' };
        }
        if (conflictWriteFailurePending) {
          conflictWriteFailurePending = false;
          throw { code: 'STORAGE', message: 'Injected conflict write failure' };
        }
        if (conflictChoiceRejectedPending) {
          conflictChoiceRejectedPending = false;
          throw { code: 'CONFIG_INVALID', message: 'The selected value cannot be used' };
        }
        if (request.action.kind !== 'resolve') {
          const original = previewConflictOperations.get(`${programId}:${request.action.resolutionOperationId}`);
          if (!original || original.undone === (request.action.kind === 'undo')) {
            throw { code: 'CONFIG_CONFLICT', messageKey: 'CONFIGURATION_CONFLICT_STALE' };
          }
          const { conflict: previousConflict, resolution: previousResolution } = original;
          if (request.action.kind === 'undo') {
            if (previousConflict.reference.origin === 'draft') {
              const draft = finalEditorSession(programId);
              const document = parsePreviewJsonc(draft.workingContent);
              updatePreviewConflictPath(document, previousConflict.segments, previewConflictResolutionValue(previousConflict, 'acceptUpstream'));
              draft.workingContent = `${JSON.stringify(document, null, 2)}\n`;
              draft.conflicts.push(structuredClone(previousConflict));
              delete draft.resolutions[previousConflict.conflictId];
              draft.unresolvedConflictIds.push(previousConflict.conflictId);
              draft.draftRevision += 1;
              current.stateRevision += 1;
              draft.basedOnStateRevision = current.stateRevision;
              previewFinalDrafts.set(programId, structuredClone(draft));
              previewConfigurationStates.set(programId, structuredClone(current));
            } else {
              updateConfigurationState(programId, (state) => {
                const document = parsePreviewJsonc(state.desired.content);
                updatePreviewConflictPath(document, previousConflict.segments, previewConflictResolutionValue(previousConflict, 'acceptUpstream'));
                state.desired.content = `${JSON.stringify(document, null, 2)}\n`;
                state.workspace.editor.conflicts.push(structuredClone(previousConflict));
                state.workspace.editor.changes = state.workspace.editor.changes.filter((item) => item.semanticPath !== previousConflict.semanticPath);
                state.desired.conflicts.push({ semanticPath: previousConflict.semanticPath, reason: 'A final edit needs a choice', severity: 'error', messageKey: 'FINAL_EDIT_CONFLICT', scope: { surface: 'configuration', ownerId: previousConflict.conflictId } });
              });
            }
          } else {
            const document = parsePreviewJsonc(previousConflict.reference.origin === 'draft' ? finalEditorSession(programId).workingContent : current.desired.content);
            updatePreviewConflictPath(document, previousConflict.segments, previewConflictResolutionValue(previousConflict, previousResolution));
            if (previousConflict.reference.origin === 'draft') {
              const draft = finalEditorSession(programId);
              draft.workingContent = `${JSON.stringify(document, null, 2)}\n`;
              draft.resolutions[previousConflict.conflictId] = previousResolution;
              refreshPreviewUnresolvedConflicts(draft);
              draft.draftRevision += 1;
              current.stateRevision += 1;
              draft.basedOnStateRevision = current.stateRevision;
              previewFinalDrafts.set(programId, structuredClone(draft));
              previewConfigurationStates.set(programId, structuredClone(current));
            } else {
              updateConfigurationState(programId, (state) => {
                state.desired.content = `${JSON.stringify(document, null, 2)}\n`;
                state.workspace.editor.conflicts = state.workspace.editor.conflicts.filter((item) => item.conflictId !== previousConflict.conflictId);
                state.desired.conflicts = state.desired.conflicts.filter((item) => item.scope?.ownerId !== previousConflict.conflictId);
              });
            }
          }
          original.undone = request.action.kind === 'undo';
          previewConflictOperations.set(receiptKey, { ...original, serialized: JSON.stringify(request) });
          return configurationWorkspaceSnapshot(programId);
        }
        const { reference, resolution } = request.action;
        const draft = finalEditorSession(programId);
        if (request.expectedDraftRevision !== undefined && request.expectedDraftRevision !== draft.draftRevision) {
          throw { code: 'CONFIG_CONFLICT', messageKey: 'CONFIGURATION_DRAFT_STALE' };
        }
        if (reference.origin === 'draft') {
          const conflict = draft.conflicts.find((item) => item.conflictId === reference.conflictId);
          if (!conflict || JSON.stringify(conflict) !== reference.fingerprint) {
            throw { code: 'CONFIG_CONFLICT', messageKey: 'CONFIGURATION_CONFLICT_STALE' };
          }
          const document = parsePreviewJsonc(draft.workingContent);
          updatePreviewConflictPath(document, conflict.segments, previewConflictResolutionValue(conflict, resolution));
          draft.workingContent = `${JSON.stringify(document, null, 2)}\n`;
          draft.resolutions[conflict.conflictId] = resolution;
          refreshPreviewUnresolvedConflicts(draft);
          draft.draftRevision += 1;
          draft.updatedUnixMs = Date.now();
          current.stateRevision += 1;
          draft.basedOnStateRevision = current.stateRevision;
          previewConfigurationStates.set(programId, structuredClone(current));
          previewFinalDrafts.set(programId, structuredClone(draft));
          previewConflictOperations.set(receiptKey, { serialized: JSON.stringify(request), conflict: {
            ...conflict, reference,
          }, resolution, undone: false });
          if (conflictResponseFailurePending) {
            conflictResponseFailurePending = false;
            throw { code: 'UNKNOWN', message: 'Conflict response unavailable' };
          }
          return configurationWorkspaceSnapshot(programId);
        }
        const conflict = current.workspace.editor.conflicts.find((item) => item.conflictId === reference.conflictId);
        if (!conflict || JSON.stringify(conflict.reference) !== JSON.stringify(reference)) {
          throw { code: 'CONFIG_CONFLICT', messageKey: 'CONFIGURATION_CONFLICT_STALE' };
        }
        updateConfigurationState(programId, (state) => {
          const selected = resolution === 'acceptUpstream'
            ? conflict.upstreamValue
            : resolution === 'keepMine'
              ? conflict.userValue
              : resolution.manualEdit.value;
          const document = parsePreviewJsonc(state.desired.content);
          updatePreviewConflictPath(document, conflict.segments, {
            present: selected.state === 'present',
            ...(selected.state === 'present' ? { value: selected.value } : {}),
          });
          state.desired.content = `${JSON.stringify(document, null, 2)}\n`;
          state.desired.conflicts = state.desired.conflicts.filter(
            (item) => item.scope?.ownerId !== conflict.conflictId,
          );
          state.workspace.editor.conflicts = state.workspace.editor.conflicts.filter(
            (item) => item.conflictId !== conflict.conflictId,
          );
          state.workspace.editor.changes = selected.state === conflict.upstreamValue.state
            && (selected.state === 'missing'
              || JSON.stringify(selected.value) === JSON.stringify(
                conflict.upstreamValue.state === 'present' ? conflict.upstreamValue.value : undefined,
              ))
            ? state.workspace.editor.changes.filter((item) => item.semanticPath !== conflict.semanticPath)
            : [...state.workspace.editor.changes.filter((item) => item.semanticPath !== conflict.semanticPath), {
                editId: `preview-final-edit-${programId}`,
                semanticPath: conflict.semanticPath,
                segments: conflict.segments,
                kind: selected.state === 'missing' ? 'deleted' : 'modified',
                upstreamValue: structuredClone(conflict.upstreamValue),
                finalValue: structuredClone(selected),
                issues: [],
              } satisfies FinalChangeProjection];
          state.desired.validation = 'pending';
          state.desired.validationEvidence = undefined;
          state.workspace.editor.candidateStatus = 'unsaved';
        });
        previewConflictOperations.set(receiptKey, { serialized: JSON.stringify(request), conflict, resolution, undone: false });
        if (conflictResponseFailurePending) {
          conflictResponseFailurePending = false;
          throw { code: 'UNKNOWN', message: 'Conflict response unavailable' };
        }
        return configurationWorkspaceSnapshot(programId);
      }
      case 'discard_final_configuration_draft': {
        const programId = stringArg(args, 'programId');
        const expectedRevision = objectArgs(args).expectedRevision;
        const currentDraft = previewFinalDrafts.get(programId);
        const currentRevision = currentDraft?.draftRevision ?? 0;
        if (finalDraftDiscardRacePending && currentDraft) {
          finalDraftDiscardRacePending = false;
          const conflictId = 'preview-discard-race-conflict';
          previewFinalDrafts.set(programId, {
            ...structuredClone(currentDraft),
            draftRevision: currentDraft.draftRevision + 1,
            conflicts: [{
              conflictId,
              segments: [
                { kind: 'key', key: 'log' },
                { kind: 'key', key: 'loglevel' },
              ],
              semanticPath: '/log/loglevel',
              kind: 'modifyVsModify',
              baseValue: { state: 'present', value: 'warning' },
              upstreamValue: { state: 'present', value: 'error' },
              userValue: { state: 'present', value: 'debug' },
              canMerge: false,
            }],
            unresolvedConflictIds: [conflictId],
            updatedUnixMs: Date.now(),
          });
          throw {
            code: 'CONFIG_CONFLICT',
            messageKey: 'CONFIGURATION_DRAFT_STALE',
            message: 'Final configuration draft changed before it could be discarded',
          };
        }
        if (expectedRevision !== currentRevision) {
          throw {
            code: 'CONFIG_CONFLICT',
            messageKey: 'CONFIGURATION_DRAFT_STALE',
            message: 'Final configuration draft changed before it could be discarded',
          };
        }
        previewFinalDrafts.delete(programId);
        return configurationWorkspaceSnapshot(programId);
      }
      case 'save_configuration_candidate': {
        const programId = stringArg(args, 'programId');
        const request = objectArgs(args).request as ConfigurationMutationContext;
        if (request.kind !== 'save') {
          throw { code: 'CONFIG_CONFLICT', messageKey: 'CONFIGURATION_OPERATION_MISMATCH', message: 'Action changed' };
        }
        const recorded = previewOperationResult(programId, request);
        if (recorded) return { ...configurationWorkspaceSnapshot(programId), operationResult: recorded };
        const currentState = configurationState(programId);
        if (request.expectedStateRevision !== currentState.stateRevision) {
          throw { code: 'CONFIG_CONFLICT', messageKey: 'CONFIGURATION_STATE_STALE', message: 'Configuration changed' };
        }
        const draft = finalEditorSession(programId);
        if (draft.draftRevision > 0 && (request.editorSessionId !== draft.sessionId
          || request.expectedDraftRevision !== draft.draftRevision)) {
          throw { code: 'CONFIG_CONFLICT', messageKey: 'CONFIGURATION_DRAFT_STALE', message: 'Draft changed' };
        }
        refreshPreviewUnresolvedConflicts(draft);
        if (draft.rebaseRequired || draft.unresolvedConflictIds.length > 0) {
          throw { code: 'CONFIG_CONFLICT', messageKey: 'FINAL_EDIT_CONFLICT', message: 'Resolve all preview conflicts before saving' };
        }
        let committedContent = draft.workingContent;
        if (currentState.format === 'jsonc') {
          committedContent = `${JSON.stringify(parsePreviewJsonc(draft.workingContent), null, 2)}\n`;
        }
        const contentChanged = committedContent !== currentState.desired.content;
        const upstreamContent = previewUpstreamDocuments.get(programId)
          ?? currentState.desired.content;
        const state = updateConfigurationState(programId, (current) => {
          current.desired.content = committedContent;
          current.desired.validation = 'pending';
          current.desired.validationEvidence = undefined;
          current.desired.diagnostics = [];
          current.desired.conflicts = [];
          current.workspace.editor.conflicts = [];
          current.workspace.editor.changes = previewFinalChanges(upstreamContent, committedContent);
        }, contentChanged);
        state.workspace.editor.candidateStatus = 'pendingValidation';
        state.desired.validation = 'pending';
        state.desired.validationEvidence = undefined;
        syncPreviewEditor(state);
        previewConfigurationStates.set(programId, structuredClone(state));
        previewFinalDrafts.delete(programId);
        const result: ConfigurationOperationResult = {
          operationId: request.operationId,
          status: 'saved',
          candidateGeneration: state.generation,
          savedCandidate: structuredClone(state.desired.revision),
        };
        previewConfigurationOperations.set(`${programId}:${request.operationId}`, {
          request: structuredClone(request), result,
        });
        window.dispatchEvent(new CustomEvent('camellia-ui-preview:configuration-save', { detail: request.operationId }));
        if (configurationSaveResponseLostPending) {
          configurationSaveResponseLostPending = false;
          throw { code: 'TIMEOUT', message: 'The save response was not received.' };
        }
        return { ...configurationWorkspaceSnapshot(programId), operationResult: result };
      }
      case 'set_guided_intent': {
        const programId = stringArg(args, 'programId');
        const request = objectArgs(args).request;
        const guided = request && typeof request === 'object'
          ? request as { settingId?: string; value?: unknown }
          : {};
        const settingId = guided.settingId;
        const current = configurationState(programId);
        if (!settingId) throw { code: 'INVALID_SPEC', message: 'Preview Guided setting is required' };
        const projection = current.guidedProjection.find((item) => item.settingId === settingId);
        const descriptor = current.guidedDescriptors.find((item) => item.id === settingId);
        if (!projection || !descriptor) {
          throw { code: 'NOT_FOUND', message: `Preview Guided setting was not found: ${settingId}` };
        }
        let nextValue = guided.value;
        if (nextValue !== undefined && descriptor.control === 'select'
          && !descriptor.allowedValues.includes(String(nextValue))) {
          throw { code: 'INVALID_SPEC', message: `Preview Guided value is not allowed: ${settingId}` };
        }
        if (nextValue === undefined) {
          nextValue = previewGuidedSettings(current.kind).projection
            .find((item) => item.settingId === settingId)?.value;
        }
        let upstreamDocument: unknown;
        try {
          upstreamDocument = parsePreviewJsonc(
            previewUpstreamDocuments.get(programId) ?? current.workspace.editor.document.content,
          );
        } catch {
          upstreamDocument = {};
        }
        setPreviewGuidedPath(
          upstreamDocument,
          previewGuidedPath(current.kind, settingId) ?? [],
          nextValue,
        );
        const upstreamContent = previewUpstreamDocuments.get(programId) ?? current.desired.content;
        const contributions = previewUpstreamContributions.get(programId)!;
        const owner = `intent:${settingId}`;
        if (guided.value === undefined) contributions.writes = contributions.writes.filter((write) => write.owner !== owner);
        const changes = guided.value === undefined ? []
          : previewFinalChanges(upstreamContent, JSON.stringify(upstreamDocument));
        updatePreviewUpstream(
          programId,
          recordPreviewContributions(programId, owner, changes),
          (state) => {
            state.guidedProjection = state.guidedProjection.map((item) => item.settingId === settingId
              ? {
                  ...item,
                  status: guided.value === undefined ? 'inherited' : 'explicit',
                  value: nextValue,
                  intentValue: guided.value,
                }
              : item);
          },
        );
        return configurationWorkspaceSnapshot(programId);
      }
      case 'get_configuration_operation': {
        const programId = stringArg(args, 'programId');
        const request = objectArgs(args).request as ConfigurationMutationContext;
        return { ...configurationWorkspaceSnapshot(programId), operationResult: previewOperationResult(programId, request) };
      }
      case 'activate_configuration_candidate': {
        const programId = stringArg(args, 'programId');
        const admission = configurationState(programId).coreAdmission;
        if (admission && admission.status !== 'admitted') {
          throw { code: 'UNSUPPORTED_BINARY', messageKey: admission.messageKey, message: 'Program admission rejected' };
        }
        const request = objectArgs(args).request as ConfigurationMutationContext;
        if (request.kind !== 'apply') {
          throw { code: 'CONFIG_CONFLICT', messageKey: 'CONFIGURATION_OPERATION_MISMATCH', message: 'Action changed' };
        }
        const recorded = previewOperationResult(programId, request);
        if (recorded) return { ...configurationWorkspaceSnapshot(programId), operationResult: recorded };
        let current = configurationState(programId);
        if (request.expectedStateRevision !== current.stateRevision) {
          throw {
            code: 'CONFIG_CONFLICT',
            messageKey: 'CONFIGURATION_STATE_STALE',
            message: 'Configuration changed since it was loaded',
          };
        }
        const draft = finalEditorSession(programId);
        if (draft.draftRevision > 0 && (request.editorSessionId !== draft.sessionId
          || request.expectedDraftRevision !== draft.draftRevision)) {
          throw { code: 'CONFIG_CONFLICT', messageKey: 'CONFIGURATION_DRAFT_STALE', message: 'Draft changed' };
        }
        refreshPreviewUnresolvedConflicts(draft);
        if (draft.rebaseRequired || draft.unresolvedConflictIds.length > 0) {
          throw {
            code: 'CONFIG_CONFLICT',
            messageKey: 'FINAL_EDIT_CONFLICT',
            message: 'Resolve final configuration conflicts before applying',
          };
        }
        if (draft.draftRevision > 0) {
          const committedContent = `${JSON.stringify(parsePreviewJsonc(draft.workingContent), null, 2)}\n`;
          const upstreamContent = previewUpstreamDocuments.get(programId)
            ?? current.desired.content;
          current = updateConfigurationState(programId, (state) => {
            state.desired.content = committedContent;
            state.desired.validation = 'pending';
            state.desired.validationEvidence = undefined;
            state.desired.diagnostics = [];
            state.desired.conflicts = [];
            state.workspace.editor.conflicts = [];
            state.workspace.editor.changes = previewFinalChanges(upstreamContent, committedContent);
            state.workspace.editor.candidateStatus = 'pendingValidation';
          }, committedContent !== current.desired.content);
        }
        if (configurationNativeRejectionPending) {
          configurationNativeRejectionPending = false;
          current = updateConfigurationState(programId, (state) => {
            state.desired.validation = 'invalid';
            state.desired.validationEvidence = undefined;
            state.desired.diagnostics = [{
              code: 'CORE_INVALID',
              message: 'The program reported a value with the wrong type.',
              messageKey: 'CORE_NATIVE_TYPE_REJECTED',
              scope: { surface: 'configuration' },
              details: JSON.stringify({
                messageKey: 'CORE_NATIVE_TYPE_REJECTED',
                exitCode: 1,
                stdoutBytes: 240,
                stderrBytes: 240,
              }),
            }];
            state.workspace.editor.candidateStatus = 'invalid';
          }, false);
          const result: ConfigurationOperationResult = {
            operationId: request.operationId,
            status: 'rejected',
            messageKey: 'CORE_INVALID',
            candidateGeneration: current.generation,
            savedCandidate: structuredClone(current.desired.revision),
          };
          previewConfigurationOperations.set(`${programId}:${request.operationId}`, {
            request: structuredClone(request), result,
          });
          return { ...configurationWorkspaceSnapshot(programId), operationResult: result };
        }
        current = updateConfigurationState(programId, (state) => {
          state.desired.validation = 'valid';
          state.desired.validationEvidence = {
            binarySha256: 'a'.repeat(64),
            profileHash: state.compatibilityProfile.profileHash,
            configHash: state.desired.revision.contentHash,
            candidateGeneration: state.generation,
            validatorContractRevision: 'preview-validator-test',
            nativeAccepted: true,
            validatedUnixMs: Date.now(),
          };
          state.desired.diagnostics = [];
          state.workspace.editor.candidateStatus = 'validated';
        }, false);
        updateConfigurationState(programId, (state) => {
          state.appliedRevision = { ...state.desired.revision };
          state.lastKnownGoodRevision = { ...state.desired.revision };
          state.workspace.editor.candidateStatus = 'applied';
        }, false);
        previewFinalDrafts.delete(programId);
        const result: ConfigurationOperationResult = {
          operationId: request.operationId,
          status: 'applied',
          candidateGeneration: configurationState(programId).generation,
          savedCandidate: structuredClone(current.desired.revision),
        };
        previewConfigurationOperations.set(`${programId}:${request.operationId}`, { request: structuredClone(request), result });
        window.dispatchEvent(new CustomEvent('camellia-ui-preview:configuration-activation', { detail: request.operationId }));
        if (configurationResponseLostPending) {
          configurationResponseLostPending = false;
          throw { code: 'TIMEOUT', message: 'The apply response was not received.' };
        }
        return { ...configurationWorkspaceSnapshot(programId), operationResult: result };
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
      case 'run_action': return { report: { messageKey: 'CORE_CHECK_COMPLETED', exitCode: 0, stdoutBytes: 0, stderrBytes: 0 } };
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
        const spec = specs[programId];
        const current = configurationState(programId);
        const sourceContent = spec ? previewInlineSourceContent(spec) : null;
        const updateSources = (state: ConfigurationStateView) => {
          state.sourceStatuses = (spec?.managedConfig?.sources ?? []).map((source) => ({
            sourceId: source.id,
            sourceName: source.name,
            freshness: source.enabled ? 'fresh' : 'disabled',
            ...(source.enabled ? { snapshotHash: previewContentHash(sourceContent ?? source.id) } : {}),
          }));
          if (!spec?.managedConfig?.sources.some((source) => source.id === 'alternate' && source.enabled)) {
            state.desired.conflicts = state.desired.conflicts.filter(
              (conflict) => !(conflict.messageKey === 'SOURCE_VALUE_CONFLICT' && conflict.scope?.ownerId === 'alternate'),
            );
          }
        };
        if (spec && sourceContent) {
          const upstream = previewSourceUpdate(programId, sourceContent, command === 'update_configuration_sources');
          updatePreviewUpstream(programId, upstream, updateSources);
        } else {
          updateConfigurationState(programId, updateSources, false);
        }
        return configurationWorkspaceSnapshot(programId);
      }
      case 'replace_package': {
        const request = objectArgs(args);
        const programId = String(request.programId);
        const spec = specs[programId];
        if (!spec || spec.executable.mode !== 'managed') {
          throw { code: 'INVALID_STATE', message: 'A managed program is required.' };
        }
        if (states[programId]?.status === 'running') {
          throw { code: 'INVALID_STATE', message: 'Stop the program before replacement.' };
        }
        if (spec.type.kind !== 'generic' && request.expectedStateRevision !== configurationState(programId).stateRevision) {
          throw { code: 'CONFIG_CONFLICT', messageKey: 'CONFIGURATION_STATE_STALE', message: 'Workspace changed.' };
        }
        const metadata = spec.executable.metadata;
        if (metadata) {
          metadata.fingerprint.sha256 = previewContentHash(String(request.packageSource));
          metadata.fingerprint.modifiedUnixMs = Date.now();
          if (metadata.coreTarget) metadata.coreTarget.fingerprintSha256 = metadata.fingerprint.sha256;
        }
        if (spec.type.kind === 'generic') return null;
        updateConfigurationState(programId, (state) => {
          state.compatibilityProfile.profileHash = previewContentHash(JSON.stringify(metadata));
          if (metadata?.coreTarget) state.compatibilityProfile.target = structuredClone(metadata.coreTarget);
          state.desired.compatibilityProfileHash = state.compatibilityProfile.profileHash;
          state.desired.validation = 'pending';
          state.desired.validationEvidence = undefined;
          state.desired.diagnostics = [];
        });
        return configurationWorkspaceSnapshot(programId);
      }
      default:
        if (command.startsWith('plugin:')) return null;
        throw { code: 'MOCK_COMMAND_UNIMPLEMENTED', message: `No UI preview response for ${command}` };
    }
  };
  const invokeWithResponseControl = (command: string, args?: InvokeArgs) => {
    const result = handlePreviewInvoke(command, args);
    if (configurationResponseHeldPending
      && ['save_configuration_candidate', 'activate_configuration_candidate'].includes(command)) {
      configurationResponseHeldPending = false;
      window.dispatchEvent(new Event('camellia-ui-preview:configuration-response-held'));
      return new Promise((resolve) => {
        window.addEventListener('camellia-ui-preview:release-configuration-response', () => resolve(result), { once: true });
      });
    }
    return result;
  };
  if (nativeWindowAvailable) {
    installPreviewInvokeTransport(invokeWithResponseControl);
  } else {
    mockWindows('main');
    mockIPC(invokeWithResponseControl, { shouldMockEvents: true });
  }
  window.addEventListener('camellia-ui-preview:automatic-config-update', (event) => {
    const detail = (event as CustomEvent<{ programId?: string }>).detail;
    const programId = detail?.programId ?? 'xray-primary';
    const content = '{\n  "log": { "loglevel": "debug" },\n  "automatic": true\n}\n';
    configurationState(programId);
    updatePreviewUpstream(programId, previewSourceUpdate(programId, content, false), (current) => {
      if (current.sourceStatuses[0]) {
        current.sourceStatuses[0].sourceName = 'Automatically refreshed source';
        current.sourceStatuses[0].freshness = 'fresh';
      }
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

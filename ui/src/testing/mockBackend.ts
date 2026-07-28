import type { InvokeArgs } from '@tauri-apps/api/core';
import { emit } from '@tauri-apps/api/event';
import { mockIPC, mockWindows } from '@tauri-apps/api/mocks';
import { canUseProgramLifecycleAction, deriveLicenseAccess } from '../licenseAccess';
import type {
  AppSettings,
  CustomerPaymentSubmission,
  EntitlementSnapshot,
  LicenseBillingSummary,
  LicenseStateChangedEvent,
  ManualPaymentClaim,
  ProgramDetail,
  ProgramSpec,
  ProgramState,
  ProgramSummary,
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

function managedExecutable(path: string, version: string) {
  return {
    mode: 'managed' as const,
    path,
    metadata: { size: 18_462_720, modifiedUnixMs: Date.now() - 86_400_000, detectedVersion: version },
  };
}

const specs: Record<string, ProgramSpec> = {
  'local-agent': {
    // ProgramSpec has its own storage schema; this is unrelated to entitlement schema v3.
    schemaVersion: 3,
    id: 'local-agent',
    name: 'Local telemetry agent',
    executable: managedExecutable('bin/local-agent', '2.8.1'),
    type: { kind: 'generic', args: ['--listen', '127.0.0.1:4400'] },
    workingDirectory: 'bin',
    environment: { RUST_LOG: 'info' },
    autoStart: false,
    restartPolicy: 'onFailure',
    privilegePolicy: { mode: 'automatic' },
  },
  'sing-box-edge': {
    schemaVersion: 3,
    id: 'sing-box-edge',
    name: 'Singapore edge gateway',
    executable: managedExecutable('bin/sing-box/sing-box', '1.14.0'),
    type: { kind: 'singBox', mainConfig: 'config.json', extraArgs: ['run'] },
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
    schemaVersion: 3,
    id: 'xray-primary',
    name: 'Primary Xray routing fabric',
    executable: managedExecutable('bin/xray/xray', '25.6.8'),
    type: { kind: 'xray', mainConfig: 'config.json', extraArgs: ['run'] },
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
    schemaVersion: 3,
    id: 'mihomo-alpha',
    name: 'Mihomo Alpha gateway',
    executable: managedExecutable('bin/mihomo/mihomo', 'Mihomo Meta alpha'),
    type: { kind: 'mihomo', mainConfig: 'config/managed.yaml', extraArgs: [] },
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
  mockWindows('main');
  mockIPC((command, args) => {
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
        if (next && typeof next === 'object') specs[(next as ProgramSpec).id] = structuredClone(next as ProgramSpec);
        return null;
      }
      case 'update_program_and_restart': {
        requireLifecycleAccess('restart');
        const next = objectArgs(args).spec;
        if (next && typeof next === 'object') specs[(next as ProgramSpec).id] = structuredClone(next as ProgramSpec);
        setLifecycleState(args, { status: 'running', pid: 42421, startedUnixMs: Date.now() });
        return null;
      }
      case 'update_program_and_refresh_config': {
        const next = objectArgs(args).spec;
        if (next && typeof next === 'object') specs[(next as ProgramSpec).id] = structuredClone(next as ProgramSpec);
        const id = next && typeof next === 'object' ? (next as ProgramSpec).id : '';
        return { sourceCount: 1, document: configDocument(id) };
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
      case 'validate_config': return { valid: true, stdout: 'Configuration is valid.', stderr: '' };
      case 'apply_config': {
        const programId = stringArg(args, 'programId');
        const baseHash = stringArg(args, 'baseHash');
        const nextHash = `${baseHash || 'preview-hash'}-next`;
        previewConfigurationDocuments.set(programId, {
          content: stringArg(args, 'content'),
          baseHash: nextHash,
        });
        return nextHash;
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
      case 'refresh_config_sources':
        return { sourceCount: 2, document: configDocument(stringArg(args, 'programId')) };
      case 'replace_package':
        return null;
      default:
        if (command.startsWith('plugin:')) return null;
        throw { code: 'MOCK_COMMAND_UNIMPLEMENTED', message: `No UI preview response for ${command}` };
    }
  }, { shouldMockEvents: true });

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

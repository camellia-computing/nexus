<script lang="ts">
  import { onDestroy, tick } from 'svelte';
  import ErrorNotice from './ErrorNotice.svelte';
  import { saveJsonFile } from './fileExport';
  import {
    isTransientErrorInfo,
    publicErrorInfo,
    sameUserFacingError,
    TRANSIENT_ERROR_DISMISS_MS,
  } from './errors';
  import { t, uiLanguage, type UiLanguage } from './i18n';
  import { api, errorInfoOf, type ErrorInfo } from './api';
  import type {
    CreateSharedConfiguration,
    CreateTeamInvitation,
    CreateWebhookEndpoint,
    LeaveWorkspace,
    SharedConfigurationContent,
    SharedConfigurationPage,
    SharedConfigurationSummary,
    MemberDeviceEnrollment,
    TeamInvitation,
    TeamProfile,
    TransferWorkspaceOwnership,
    UpdateWorkspaceMember,
    WebhookDeliverySummary,
    WebhookEndpoint,
    WebhookSecretResult,
    WorkspaceAlertEventKind,
    WorkspaceAlertIncident,
    WorkspaceAlertRule,
    WorkspaceAlertSeverity,
    WorkspaceAuditEvent,
    WorkspaceAuditPage,
    WorkspaceDeviceCheckpoint,
    WorkspaceIncidentPage,
    WorkspaceIncidentStatus,
    WorkspaceMember,
    WorkspaceRole,
    WorkspaceSyncFeed,
  } from './types';

  type WorkspaceView = 'members' | 'shared' | 'sync' | 'alerts' | 'audit' | 'webhooks';
  type RetryableMutation = { label: string; run: () => Promise<void> };

  const AUDIT_EXPORT_UI_SAFETY_LIMIT = 5_000;
  const TEAM_TOKEN_MIN_LENGTH = 32;
  const TEAM_TOKEN_MAX_LENGTH = 256;

  const webhookEventTypes = [
    'alert.incident.acknowledged',
    'alert.incident.opened',
    'alert.incident.resolved',
    'configuration.created',
    'configuration.deleted',
    'configuration.published',
    'configuration.purged',
    'configuration.restored',
    'configuration.revised',
    'sync.conflict',
  ];

  const alertEventKinds: WorkspaceAlertEventKind[] = [
    'sync_conflict',
    'quota_warning',
    'configuration_created',
    'configuration_revised',
    'configuration_published',
    'configuration_deleted',
    'configuration_restored',
  ];

  export let profile: TeamProfile | null = null;
  export let members: WorkspaceMember[] = [];
  export let membersHasMore = false;
  export let membersLoadingMore = false;
  export let invitation: TeamInvitation | null = null;
  export let deviceEnrollment: MemberDeviceEnrollment | null = null;
  export let secretGeneration = 0;
  export let auditExportLimit = 0;
  export let error: ErrorInfo | null = null;
  export let busy = false;
  export let busyAction = '';
  export let canRefresh = false;
  export let onRefresh: () => void;
  export let onLoadMoreMembers: () => void;
  export let onCreateInvitation: (request: CreateTeamInvitation) => Promise<void>;
  export let onDismissInvitation: () => void;
  export let onAcceptInvitation: (token: string, operationId: string) => Promise<void>;
  export let onUpdateMember: (
    memberId: string,
    request: UpdateWorkspaceMember,
  ) => Promise<void>;
  export let onCreateDeviceEnrollment: (operationId: string) => Promise<void>;
  export let onCreateMemberDeviceEnrollment: (
    memberId: string,
    operationId: string,
  ) => Promise<void>;
  export let onDismissDeviceEnrollment: () => void;
  export let onAcceptDeviceEnrollment: (
    token: string,
    operationId: string,
  ) => Promise<void>;
  export let onLeaveWorkspace: (request: LeaveWorkspace) => Promise<void>;
  export let onTransferOwnership: (request: TransferWorkspaceOwnership) => Promise<void>;
  export let onConfirmAction: (
    title: string,
    message: string,
    confirmLabel: string,
    danger?: boolean,
  ) => Promise<boolean>;
  export let onDismissError: () => void;

  $: canReadTeam = !!profile?.permissions.includes('team.read');
  $: canManageTeam = !!profile?.permissions.includes('team.manage');
  $: canReadShared = !!profile?.permissions.includes('shared.read');
  $: canWriteShared = !!profile?.permissions.includes('shared.write');
  $: canPublishShared = !!profile?.permissions.includes('shared.publish');
  $: canPurgeShared = !!profile?.permissions.includes('shared.purge');
  $: canReadSync = !!profile?.permissions.includes('sync.read');
  $: canWriteSync = !!profile?.permissions.includes('sync.write');
  $: canReadAlerts = !!profile?.permissions.includes('alerts.read');
  $: canReadAlertHistory = !!profile?.permissions.includes('alerts.history.read');
  $: canManageAlerts = !!profile?.permissions.includes('alerts.manage');
  $: canAcknowledgeAlerts = !!profile?.permissions.includes('alerts.ack');
  $: canReadAudit = !!profile?.permissions.includes('audit.read');
  $: canExportAudit = !!profile?.permissions.includes('audit.export');
  $: canReadWebhooks = !!profile?.permissions.includes('webhooks.read');
  $: canManageWebhooks = !!profile?.permissions.includes('webhooks.manage');
  $: canReadWebhookDeliveries = canReadWebhooks
    || !!profile?.permissions.includes('webhooks.delivery.read');
  $: effectiveAuditExportLimit = Number.isSafeInteger(auditExportLimit) && auditExportLimit > 0
    ? Math.min(auditExportLimit, AUDIT_EXPORT_UI_SAFETY_LIMIT)
    : 0;
  $: visibleViews = [
    { id: 'members' as const, label: 'Members', visible: true },
    { id: 'shared' as const, label: 'Shared configurations', visible: canReadShared },
    { id: 'sync' as const, label: 'Sync activity', visible: canReadSync },
    { id: 'alerts' as const, label: 'Alerts', visible: canReadAlerts || canReadAlertHistory },
    { id: 'audit' as const, label: 'Audit log', visible: canReadAudit },
    { id: 'webhooks' as const, label: 'Webhooks', visible: canReadWebhooks || canReadWebhookDeliveries },
  ].filter((view) => view.visible);
  $: currentMember = profile?.member ?? null;
  $: activeAdminCandidates = members.filter((member) =>
    member.role === 'admin' && member.status === 'active' && member.id !== currentMember?.id
  );
  $: if (
    ownershipTargetMemberId
    && !activeAdminCandidates.some((member) => member.id === ownershipTargetMemberId)
  ) ownershipTargetMemberId = '';
  $: if (!visibleViews.some((view) => view.id === activeView)) activeView = 'members';

  let inviteEmail = '';
  let inviteName = '';
  let inviteRole: Exclude<WorkspaceRole, 'owner'> = 'operator';
  let invitationToken = '';
  let formError = '';
  let deviceEnrollmentToken = '';
  let deviceEnrollmentFormError = '';
  let ownershipTargetMemberId = '';
  let lastDisplayedInvitationToken = '';
  let invitationCopyStatus: 'copied' | 'failed' | '' = '';
  let lastDisplayedDeviceEnrollmentToken = '';
  let deviceEnrollmentCopyStatus: 'copied' | 'failed' | '' = '';
  let activeView: WorkspaceView = 'members';
  let viewNavigation: HTMLElement;
  let workspaceBusyAction = '';
  let workspaceError: ErrorInfo | null = null;
  let workspaceNotice = '';
  let workspaceNoticeTimer: number | undefined;
  let retryableMutation: RetryableMutation | null = null;
  let errorRegion: HTMLElement;

  $: publicExternalError = publicErrorInfo(error);
  $: publicWorkspaceError = publicErrorInfo(workspaceError);
  $: workspaceErrorDuplicatesExternal = sameUserFacingError(publicExternalError, publicWorkspaceError);
  $: visibleExternalError = workspaceErrorDuplicatesExternal ? null : publicExternalError;
  $: visibleWorkspaceError = publicWorkspaceError;

  $: if (currentMember?.role !== 'owner' && inviteRole === 'admin') inviteRole = 'operator';

  let sharedPage: SharedConfigurationPage | null = null;
  let sharedConfigurations: SharedConfigurationSummary[] = [];
  let sharedLoaded = false;
  let sharedIncludeDeleted = false;
  let selectedSharedContent: SharedConfigurationContent | null = null;
  let sharedFormOpen = false;
  let sharedFormName = '';
  let sharedFormProgramKind: CreateSharedConfiguration['programKind'] = 'generic';
  let sharedFormInput = '';
  let sharedFormContent = '';
  let sharedFormError = '';

  let syncFeed: WorkspaceSyncFeed | null = null;
  let syncChanges: WorkspaceSyncFeed['changes'] = [];
  let syncCheckpoint: WorkspaceDeviceCheckpoint | null = null;
  let syncLoaded = false;

  let alertRules: WorkspaceAlertRule[] = [];
  let alertRulesNextCursor: string | null = null;
  let alertRulesHasMore = false;
  let alertIncidents: WorkspaceAlertIncident[] = [];
  let alertIncidentsNextCursor: string | null = null;
  let alertIncidentsHasMore = false;
  let alertsLoaded = false;
  let incidentStatusFilter: WorkspaceIncidentStatus | '' = '';
  let alertFormOpen = false;
  let editingAlertRule: WorkspaceAlertRule | null = null;
  let alertRuleName = '';
  let alertRuleEventKind: WorkspaceAlertEventKind = 'sync_conflict';
  let alertRuleSeverity: WorkspaceAlertSeverity = 'warning';
  let alertRuleEnabled = true;
  let alertFormError = '';

  let auditPage: WorkspaceAuditPage | null = null;
  let auditEvents: WorkspaceAuditEvent[] = [];
  let auditLoaded = false;
  let auditEventTypes: string[] = [];
  let auditEventTypesLoaded = false;
  let auditEventType = '';
  let auditExportStatus = '';

  let webhookEndpoints: WebhookEndpoint[] = [];
  let webhookDeliveries: WebhookDeliverySummary[] = [];
  let webhooksLoaded = false;
  let webhookDeliveryEndpointId = '';
  let webhookFormOpen = false;
  let editingWebhookEndpoint: WebhookEndpoint | null = null;
  let webhookName = '';
  let webhookUrl = '';
  let selectedWebhookEventTypes: string[] = [];
  let webhookActive = true;
  let webhookFormError = '';
  let webhookSecretResult: WebhookSecretResult | null = null;
  let webhookSecretCopyStatus: 'copied' | 'failed' | '' = '';
  let observedSecretGeneration = secretGeneration;
  let observedWorkspaceScope = workspaceScopeKey(profile);
  let workspaceScopeGeneration = 0;
  let destroyed = false;

  $: {
    const nextWorkspaceScope = workspaceScopeKey(profile);
    if (nextWorkspaceScope !== observedWorkspaceScope) {
      observedWorkspaceScope = nextWorkspaceScope;
      workspaceScopeGeneration += 1;
      resetWorkspaceSurface();
    }
  }

  $: if (secretGeneration !== observedSecretGeneration) {
    observedSecretGeneration = secretGeneration;
    invitationToken = '';
    deviceEnrollmentToken = '';
    formError = '';
    deviceEnrollmentFormError = '';
    invitationCopyStatus = '';
    deviceEnrollmentCopyStatus = '';
    clearWebhookSecret();
  }

  $: if ((invitation?.invitationToken ?? '') !== lastDisplayedInvitationToken) {
    lastDisplayedInvitationToken = invitation?.invitationToken ?? '';
    invitationCopyStatus = '';
  }

  $: if ((deviceEnrollment?.enrollmentToken ?? '') !== lastDisplayedDeviceEnrollmentToken) {
    lastDisplayedDeviceEnrollmentToken = deviceEnrollment?.enrollmentToken ?? '';
    deviceEnrollmentCopyStatus = '';
  }

  function workspaceScopeKey(value: TeamProfile | null) {
    const member = value?.member;
    return member
      ? [
          value.enabled ? 'enabled' : 'disabled',
          member.id,
          member.status,
          member.role,
          member.rowVersion,
          ...[...value.permissions].sort(),
        ].join('\u0000')
      : `${value?.enabled ? 'enabled' : 'disabled'}\u0000unlinked`;
  }

  function resetWorkspaceSurface() {
    activeView = 'members';
    retryableMutation = null;
    workspaceError = null;
    clearWorkspaceNotice();
    invitationToken = '';
    deviceEnrollmentToken = '';
    formError = '';
    deviceEnrollmentFormError = '';
    inviteEmail = '';
    inviteName = '';
    inviteRole = 'operator';
    ownershipTargetMemberId = '';
    invitationCopyStatus = '';
    deviceEnrollmentCopyStatus = '';

    sharedPage = null;
    sharedConfigurations = [];
    sharedLoaded = false;
    sharedIncludeDeleted = false;
    selectedSharedContent = null;
    sharedFormOpen = false;
    sharedFormName = '';
    sharedFormProgramKind = 'generic';
    sharedFormInput = '';
    sharedFormContent = '';
    sharedFormError = '';

    syncFeed = null;
    syncChanges = [];
    syncCheckpoint = null;
    syncLoaded = false;

    alertRules = [];
    alertRulesNextCursor = null;
    alertRulesHasMore = false;
    alertIncidents = [];
    alertIncidentsNextCursor = null;
    alertIncidentsHasMore = false;
    alertsLoaded = false;
    incidentStatusFilter = '';
    alertFormOpen = false;
    editingAlertRule = null;
    alertRuleName = '';
    alertRuleEventKind = 'sync_conflict';
    alertRuleSeverity = 'warning';
    alertRuleEnabled = true;
    alertFormError = '';

    auditPage = null;
    auditEvents = [];
    auditLoaded = false;
    auditEventTypes = [];
    auditEventTypesLoaded = false;
    auditEventType = '';
    auditExportStatus = '';

    webhookEndpoints = [];
    webhookDeliveries = [];
    webhooksLoaded = false;
    webhookDeliveryEndpointId = '';
    webhookFormOpen = false;
    editingWebhookEndpoint = null;
    webhookName = '';
    webhookUrl = '';
    selectedWebhookEventTypes = [];
    webhookActive = true;
    webhookFormError = '';
    clearWebhookSecret();
  }

  function formatMinuteDate(seconds: number, language: UiLanguage) {
    return new Intl.DateTimeFormat(language === 'zh-CN' ? 'zh-CN' : 'en-US', {
      dateStyle: 'medium',
      timeStyle: 'short',
    }).format(new Date(seconds * 1_000));
  }

  function workspaceRoleLabel(role: WorkspaceRole) {
    switch (role) {
      case 'owner': return 'Owner';
      case 'admin': return 'Administrator';
      case 'billing': return 'Billing manager';
      case 'operator': return 'Operator';
      case 'auditor': return 'Auditor';
      default: return 'Viewer';
    }
  }

  function ownershipCandidateTitle(memberId: string) {
    const member = activeAdminCandidates.find((candidate) => candidate.id === memberId);
    return member ? `${member.displayName} · ${member.email}` : $t('Select an administrator');
  }

  function workspaceMemberStatusLabel(status: WorkspaceMember['status']) {
    switch (status) {
      case 'active': return 'Active member';
      case 'invited': return 'Invitation pending';
      case 'suspended': return 'Access suspended';
      default: return 'Removed member';
    }
  }

  async function submitInvitation() {
    formError = '';
    const email = inviteEmail.trim();
    const displayName = inviteName.trim();
    if (!displayName || !email || !email.includes('@')) {
      formError = $t('Enter the member name and email address.');
      return;
    }
    const request: CreateTeamInvitation = {
      operationId: operationId(),
      email,
      displayName,
      role: inviteRole,
    };
    await executeMutation(
      'license-team-invitation',
      async () => {
        await onCreateInvitation(request);
        inviteEmail = '';
        inviteName = '';
      },
      parentMutationRefresh,
      'Team invitation created',
    );
  }

  async function acceptInvitationToken() {
    formError = '';
    const token = invitationToken.trim();
    if (token.length < TEAM_TOKEN_MIN_LENGTH || token.length > TEAM_TOKEN_MAX_LENGTH) {
      formError = $t('Enter a valid invitation token.');
      return;
    }
    const requestOperationId = operationId();
    await executeMutation(
      'license-team-invitation-accept',
      async () => {
        await onAcceptInvitation(token, requestOperationId);
        invitationToken = '';
      },
      parentMutationRefresh,
      'Team invitation accepted',
    );
  }

  async function acceptDeviceEnrollmentToken() {
    deviceEnrollmentFormError = '';
    const token = deviceEnrollmentToken.trim();
    if (token.length < TEAM_TOKEN_MIN_LENGTH || token.length > TEAM_TOKEN_MAX_LENGTH) {
      deviceEnrollmentFormError = $t('Enter a valid device enrollment token.');
      return;
    }
    const requestOperationId = operationId();
    await executeMutation(
      'license-team-device-enrollment-accept',
      async () => {
        await onAcceptDeviceEnrollment(token, requestOperationId);
        deviceEnrollmentToken = '';
      },
      parentMutationRefresh,
      'Team device enrollment consumed',
    );
  }

  function updateMember(
    member: WorkspaceMember,
    role: WorkspaceRole,
    status: 'active' | 'suspended',
  ) {
    if (
      !canManageTeam
      || role === 'owner'
      || member.role === 'owner'
      || member.id === profile?.member?.id
      || member.status === 'invited'
      || member.status === 'removed'
      || (currentMember?.role === 'admin' && member.role === 'admin')
    ) return;
    const request: UpdateWorkspaceMember = {
      operationId: operationId(),
      role,
      status,
      rowVersion: member.rowVersion,
    };
    void executeMutation(
      'license-team-member-update',
      () => onUpdateMember(member.id, request),
      parentMutationRefresh,
      'Team member updated',
    );
  }

  function canManageMember(member: WorkspaceMember) {
    return canManageTeam
      && member.role !== 'owner'
      && member.id !== currentMember?.id
      && member.status !== 'removed'
      && !(currentMember?.role === 'admin' && member.role === 'admin');
  }

  async function removeMember(member: WorkspaceMember) {
    if (!canManageMember(member) || member.role === 'owner') return;
    const pendingInvitation = member.status === 'invited';
    const confirmed = await onConfirmAction(
      $t(pendingInvitation ? 'Revoke pending invitation?' : 'Remove team member?'),
      $t(pendingInvitation
        ? 'The invitation token will stop working and the reserved seat will be released.'
        : 'All linked devices and refresh sessions will be revoked. Invite this person again to restore access.'),
      $t(pendingInvitation ? 'Revoke invitation' : 'Remove member'),
      true,
    );
    if (!confirmed) return;
    const request: UpdateWorkspaceMember = {
      operationId: operationId(),
      role: member.role,
      status: 'removed',
      rowVersion: member.rowVersion,
    };
    await executeMutation(
      'license-team-member-update',
      () => onUpdateMember(member.id, request),
      parentMutationRefresh,
      'Team member updated',
    );
  }

  async function createDeviceEnrollment() {
    const requestOperationId = operationId();
    await executeMutation(
      'license-team-device-enrollment-create',
      () => onCreateDeviceEnrollment(requestOperationId),
      parentMutationRefresh,
      'Team device enrollment created',
    );
  }

  async function createMemberDeviceEnrollment(memberId: string) {
    const requestOperationId = operationId();
    await executeMutation(
      'license-team-member-device-enrollment-create',
      () => onCreateMemberDeviceEnrollment(memberId, requestOperationId),
      parentMutationRefresh,
      'Team member device recovery created',
    );
  }

  async function leaveWorkspace() {
    if (!currentMember || currentMember.status !== 'active' || currentMember.role === 'owner') {
      return;
    }
    const confirmed = await onConfirmAction(
      $t('Leave this workspace?'),
      $t('This device will lose team access and managed programs will stop.'),
      $t('Leave workspace'),
      true,
    );
    if (!confirmed) return;
    const member = currentMember;
    if (!member || member.status !== 'active' || member.role === 'owner') return;
    const request: LeaveWorkspace = {
      operationId: operationId(),
      memberId: member.id,
      rowVersion: member.rowVersion,
    };
    await executeMutation(
      'license-team-leave',
      () => onLeaveWorkspace(request),
      parentMutationRefresh,
      'Team member left',
    );
  }

  async function transferOwnership() {
    const member = activeAdminCandidates.find(
      (candidate) => candidate.id === ownershipTargetMemberId,
    );
    if (!member || currentMember?.role !== 'owner' || currentMember.status !== 'active') return;
    const confirmed = await onConfirmAction(
      $t('Transfer workspace ownership?'),
      `${$t('The selected administrator will become the workspace owner:')} ${member.displayName}. ${$t('You will remain an administrator.')}`,
      $t('Transfer ownership'),
      true,
    );
    if (!confirmed) return;
    const owner = currentMember;
    const target = activeAdminCandidates.find(
      (candidate) => candidate.id === member.id,
    );
    if (!owner || owner.role !== 'owner' || owner.status !== 'active' || !target) return;
    const request: TransferWorkspaceOwnership = {
      operationId: operationId(),
      newOwnerMemberId: target.id,
      ownerRowVersion: owner.rowVersion,
      newOwnerRowVersion: target.rowVersion,
    };
    await executeMutation(
      'license-team-ownership-transfer',
      () => onTransferOwnership(request),
      parentMutationRefresh,
      'Team ownership transferred',
    );
  }

  async function copyInvitationToken() {
    const token = invitation?.invitationToken;
    if (!token) return;
    try {
      await navigator.clipboard.writeText(token);
      invitationCopyStatus = 'copied';
    } catch {
      invitationCopyStatus = 'failed';
    }
  }

  async function copyDeviceEnrollmentToken() {
    const token = deviceEnrollment?.enrollmentToken;
    if (!token) return;
    try {
      await navigator.clipboard.writeText(token);
      deviceEnrollmentCopyStatus = 'copied';
    } catch {
      deviceEnrollmentCopyStatus = 'failed';
    }
  }

  function operationId() {
    return crypto.randomUUID();
  }

  async function parentMutationRefresh() {
    return true;
  }

  function isWorkspaceConflict(value: ErrorInfo) {
    return value.code === 'LICENSE_WORKSPACE_CONFLICT'
      || value.code === 'LICENSE_OPERATION_CONFLICT';
  }

  function isRetryableWorkspaceError(value: ErrorInfo) {
    return value.code === 'NETWORK'
      || value.code === 'TIMEOUT'
      || value.code === 'RATE_LIMITED'
      || value.code === 'INTERNAL';
  }

  async function focusWorkspaceError() {
    await tick();
    errorRegion?.focus();
  }

  function clearWorkspaceNotice() {
    if (workspaceNoticeTimer !== undefined) {
      window.clearTimeout(workspaceNoticeTimer);
      workspaceNoticeTimer = undefined;
    }
    workspaceNotice = '';
  }

  function transientDismissDelay(errorInfo: ErrorInfo | null) {
    return isTransientErrorInfo(errorInfo) ? TRANSIENT_ERROR_DISMISS_MS : 0;
  }

  function dismissWorkspaceError() {
    workspaceError = null;
    retryableMutation = null;
    if (workspaceErrorDuplicatesExternal) onDismissError();
  }

  function dismissExternalError() {
    onDismissError();
  }

  function showWorkspaceNotice(message: string, timeoutMs = 5_000) {
    clearWorkspaceNotice();
    workspaceNotice = message;
    workspaceNoticeTimer = window.setTimeout(() => {
      workspaceNotice = '';
      workspaceNoticeTimer = undefined;
    }, timeoutMs);
  }

  async function readWorkspaceData(
    action: string,
    operation: () => Promise<void>,
    preserveError = false,
  ) {
    if (workspaceBusyAction) return false;
    const requestScopeGeneration = workspaceScopeGeneration;
    workspaceBusyAction = action;
    if (!preserveError) {
      clearWorkspaceNotice();
      workspaceError = null;
    }
    try {
      await operation();
      if (requestScopeGeneration !== workspaceScopeGeneration) {
        resetWorkspaceSurface();
        return false;
      }
      return true;
    } catch (value) {
      if (requestScopeGeneration !== workspaceScopeGeneration) {
        resetWorkspaceSurface();
        return false;
      }
      workspaceError = errorInfoOf(value);
      await focusWorkspaceError();
      return false;
    } finally {
      if (workspaceBusyAction === action) workspaceBusyAction = '';
    }
  }

  async function executeMutation(
    label: string,
    operation: () => Promise<void>,
    refresh: () => Promise<boolean | undefined>,
    successMessage: string,
  ) {
    if (workspaceBusyAction) return;
    const requestScopeGeneration = workspaceScopeGeneration;
    workspaceBusyAction = label;
    workspaceError = null;
    clearWorkspaceNotice();
    let succeeded = false;
    try {
      await operation();
      if (requestScopeGeneration !== workspaceScopeGeneration) {
        resetWorkspaceSurface();
        return;
      }
      succeeded = true;
      retryableMutation = null;
    } catch (value) {
      if (requestScopeGeneration !== workspaceScopeGeneration) {
        resetWorkspaceSurface();
        return;
      }
      const info = errorInfoOf(value);
      if (isWorkspaceConflict(info)) {
        retryableMutation = null;
        invalidateStaleEditor(label);
        workspaceBusyAction = '';
        const refreshed = await refresh();
        if (requestScopeGeneration !== workspaceScopeGeneration || refreshed === false) return;
        workspaceError = info;
      } else if (isRetryableWorkspaceError(info)) {
        retryableMutation = {
          label,
          run: () => executeMutation(label, operation, refresh, successMessage),
        };
        workspaceError = info;
      } else {
        retryableMutation = null;
        workspaceError = info;
      }
      await focusWorkspaceError();
    } finally {
      if (workspaceBusyAction === label) workspaceBusyAction = '';
    }
    if (!succeeded) return;
    if (!workspaceNotice) showWorkspaceNotice($t(successMessage));
    await refresh();
  }

  async function retryLastMutation() {
    const retry = retryableMutation;
    if (!retry || workspaceBusyAction) return;
    await retry.run();
  }

  function invalidateStaleEditor(label: string) {
    if (label.startsWith('shared-')) {
      sharedFormOpen = false;
      selectedSharedContent = null;
    }
    if (label.startsWith('alert-rule-')) {
      alertFormOpen = false;
      editingAlertRule = null;
    }
    if (label.startsWith('webhook-')) {
      webhookFormOpen = false;
      editingWebhookEndpoint = null;
      clearWebhookSecret();
    }
  }

  async function selectView(view: WorkspaceView) {
    if (activeView === 'webhooks' && view !== 'webhooks') clearWebhookSecret();
    activeView = view;
    workspaceError = null;
    clearWorkspaceNotice();
    retryableMutation = null;
    await tick();
    if (view === 'shared' && !sharedLoaded) await loadSharedConfigurations();
    if (view === 'sync' && !syncLoaded) await loadSyncActivity();
    if (view === 'alerts' && !alertsLoaded) await loadAlerts();
    if (view === 'audit' && !auditLoaded) await loadAuditEvents();
    if (view === 'webhooks' && !webhooksLoaded) await loadWebhooks();
  }

  function navigateViews(event: KeyboardEvent) {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    const current = visibleViews.findIndex((view) => view.id === activeView);
    const index = event.key === 'Home'
      ? 0
      : event.key === 'End'
        ? visibleViews.length - 1
        : (current + (event.key === 'ArrowRight' ? 1 : -1) + visibleViews.length)
          % visibleViews.length;
    const next = visibleViews[index];
    if (!next) return;
    void selectView(next.id).then(() => {
      viewNavigation?.querySelector<HTMLElement>(`[data-workspace-view="${next.id}"]`)?.focus();
    });
  }

  async function loadSharedConfigurations(append = false, preserveError = false) {
    if (!canReadShared) return;
    const cursor = append ? sharedPage?.nextCursor ?? null : null;
    return readWorkspaceData('shared-list', async () => {
      const page = await api.getLicenseWorkspaceConfigurations({
        cursor,
        limit: 50,
        includeDeleted: canWriteShared && sharedIncludeDeleted,
      });
      sharedPage = page;
      if (append) {
        const merged = new Map(sharedConfigurations.map((item) => [item.id, item]));
        for (const item of page.configurations) merged.set(item.id, item);
        sharedConfigurations = [...merged.values()];
      } else {
        sharedConfigurations = page.configurations;
      }
      if (
        selectedSharedContent
        && !sharedConfigurations.some((item) => item.id === selectedSharedContent?.id)
      ) selectedSharedContent = null;
      sharedLoaded = true;
    }, preserveError);
  }

  async function loadSharedContent(configuration: SharedConfigurationSummary) {
    if (!canReadShared || workspaceBusyAction) return;
    return readWorkspaceData('shared-content', async () => {
      selectedSharedContent = await api.getLicenseWorkspaceConfiguration(configuration.id, {});
      sharedFormOpen = false;
    });
  }

  async function loadSyncActivity(append = false, preserveError = false) {
    if (!canReadSync) return;
    return readWorkspaceData('sync-list', async () => {
      if (!append) syncCheckpoint = await api.getLicenseWorkspaceCheckpoint();
      const cursor = append
        ? syncFeed?.nextCursor ?? 0
        : syncCheckpoint?.cursor ?? 0;
      const page = await api.getLicenseWorkspaceSyncFeed({ cursor, limit: 50 });
      syncFeed = page;
      syncChanges = append ? [...syncChanges, ...page.changes] : page.changes;
      syncLoaded = true;
    }, preserveError);
  }

  async function loadAlerts(appendRules = false, appendIncidents = false, preserveError = false) {
    if (!canReadAlerts && !canReadAlertHistory) return;
    return readWorkspaceData('alerts-list', async () => {
      const rulesPage = canReadAlerts
        ? await api.getLicenseWorkspaceAlertRules({
          cursor: appendRules ? alertRulesNextCursor : null,
          limit: 50,
        })
        : null;
      const incidentsPage = await api.getLicenseWorkspaceAlertIncidents({
        cursor: appendIncidents ? alertIncidentsNextCursor : null,
        limit: 50,
        status: incidentStatusFilter || null,
      });
      if (rulesPage) {
        alertRules = appendRules ? [...alertRules, ...rulesPage.rules] : rulesPage.rules;
        alertRulesNextCursor = rulesPage.nextCursor ?? null;
        alertRulesHasMore = rulesPage.hasMore;
      } else {
        alertRules = [];
        alertRulesNextCursor = null;
        alertRulesHasMore = false;
      }
      alertIncidents = appendIncidents
        ? [...alertIncidents, ...incidentsPage.incidents]
        : incidentsPage.incidents;
      alertIncidentsNextCursor = incidentsPage.nextCursor ?? null;
      alertIncidentsHasMore = incidentsPage.hasMore;
      alertsLoaded = true;
    }, preserveError);
  }

  async function loadAuditEvents(append = false, preserveError = false) {
    if (!canReadAudit) return;
    return readWorkspaceData('audit-list', async () => {
      const [page, eventTypes] = await Promise.all([
        api.getLicenseWorkspaceAuditEvents({
          cursor: append ? auditPage?.nextCursor ?? null : null,
          limit: 100,
          eventType: auditEventType || null,
        }),
        append && auditEventTypesLoaded ? null : api.getLicenseWorkspaceAuditEventTypes(),
      ]);
      if (eventTypes) {
        auditEventTypes = eventTypes.eventTypes;
        auditEventTypesLoaded = true;
      }
      auditPage = page;
      auditEvents = append ? [...auditEvents, ...page.events] : page.events;
      auditLoaded = true;
    }, preserveError);
  }

  async function loadWebhooks(preserveError = false) {
    if (!canReadWebhooks && !canReadWebhookDeliveries) return;
    return readWorkspaceData('webhook-list', async () => {
      const nextEndpoints = canReadWebhooks
        ? await api.getLicenseWorkspaceWebhookEndpoints()
        : [];
      if (
        webhookDeliveryEndpointId
        && !nextEndpoints.some((endpoint) => endpoint.id === webhookDeliveryEndpointId)
      ) {
        webhookDeliveryEndpointId = '';
      }
      const nextDeliveries = canReadWebhookDeliveries
        ? await api.getLicenseWorkspaceWebhookDeliveries(
            webhookDeliveryEndpointId || null,
            50,
          )
        : [];
      webhookEndpoints = nextEndpoints;
      webhookDeliveries = nextDeliveries;
      webhooksLoaded = true;
    }, preserveError);
  }

  function beginCreateSharedConfiguration() {
    selectedSharedContent = null;
    sharedFormName = '';
    sharedFormProgramKind = 'generic';
    sharedFormInput = '';
    sharedFormContent = '';
    sharedFormError = '';
    sharedFormOpen = true;
  }

  async function beginReviseSharedConfiguration(configuration: SharedConfigurationSummary) {
    if (!canWriteShared || configuration.deletedAt) return;
    await loadSharedContent(configuration);
    if (!selectedSharedContent || selectedSharedContent.id !== configuration.id) return;
    sharedFormName = selectedSharedContent.name;
    sharedFormProgramKind = selectedSharedContent.programKind;
    sharedFormInput = selectedSharedContent.input;
    sharedFormContent = selectedSharedContent.content;
    sharedFormError = '';
    sharedFormOpen = true;
  }

  async function saveSharedConfiguration() {
    sharedFormError = '';
    const name = sharedFormName.trim();
    if (!name || !sharedFormContent.trim()) {
      sharedFormError = $t('Enter a name and configuration content.');
      return;
    }
    const editing = selectedSharedContent;
    if (editing) {
      const request = {
        baseRowVersion: editing.rowVersion,
        name,
        programKind: sharedFormProgramKind,
        input: sharedFormInput,
        content: sharedFormContent,
        operationId: operationId(),
      };
      await executeMutation(
        'shared-revise',
        async () => {
          await api.reviseLicenseWorkspaceConfiguration(editing.id, request);
          sharedFormOpen = false;
          selectedSharedContent = null;
        },
        () => loadSharedConfigurations(false, true),
        'Shared configuration revision saved.',
      );
      return;
    }
    const request = {
      name,
      programKind: sharedFormProgramKind,
      input: sharedFormInput,
      content: sharedFormContent,
      operationId: operationId(),
    };
    await executeMutation(
      'shared-create',
      async () => {
        await api.createLicenseWorkspaceConfiguration(request);
        sharedFormOpen = false;
      },
      () => loadSharedConfigurations(false, true),
      'Shared configuration created.',
    );
  }

  async function publishSharedConfiguration(configuration: SharedConfigurationSummary) {
    if (!canPublishShared || configuration.deletedAt) return;
    const request = {
      baseRowVersion: configuration.rowVersion,
      revision: null,
      operationId: operationId(),
    };
    await executeMutation(
      'shared-publish',
      () => api.publishLicenseWorkspaceConfiguration(configuration.id, request).then(() => {}),
      () => loadSharedConfigurations(false, true),
      'Shared configuration published.',
    );
  }

  async function setSharedConfigurationDeleted(
    configuration: SharedConfigurationSummary,
    restore: boolean,
  ) {
    if (!canWriteShared) return;
    if (!restore) {
      const confirmed = await onConfirmAction(
        $t('Delete shared configuration?'),
        $t('The configuration will be hidden from viewers and can be restored before permanent removal.'),
        $t('Delete configuration'),
        true,
      );
      if (!confirmed) return;
    }
    const request = {
      baseRowVersion: configuration.rowVersion,
      operationId: operationId(),
    };
    await executeMutation(
      restore ? 'shared-restore' : 'shared-delete',
      () => (restore
        ? api.restoreLicenseWorkspaceConfiguration(configuration.id, request)
        : api.deleteLicenseWorkspaceConfiguration(configuration.id, request)).then(() => {}),
      () => loadSharedConfigurations(false, true),
      restore ? 'Shared configuration restored.' : 'Shared configuration deleted.',
    );
  }

  function canPurgeConfiguration(configuration: SharedConfigurationSummary) {
    return canPurgeShared
      && configuration.deletedAt !== null;
  }

  async function purgeSharedConfiguration(configuration: SharedConfigurationSummary) {
    if (!canPurgeConfiguration(configuration)) return;
    const confirmed = await onConfirmAction(
      $t('Permanently remove shared configuration?'),
      $t('All encrypted revisions will be permanently removed. This action cannot be undone. The service rejects removal until its trusted 30-day recovery period has ended.'),
      $t('Permanently remove'),
      true,
    );
    if (!confirmed) return;
    const request = {
      baseRowVersion: configuration.rowVersion,
      operationId: operationId(),
    };
    await executeMutation(
      'shared-purge',
      () => api.purgeLicenseWorkspaceConfiguration(configuration.id, request).then(() => {}),
      () => loadSharedConfigurations(false, true),
      'Shared configuration permanently removed.',
    );
  }

  async function exportSharedConfiguration(configuration: SharedConfigurationSummary) {
    await readWorkspaceData('shared-export', async () => {
      const content = await api.getLicenseWorkspaceConfiguration(configuration.id, {});
      const saved = await saveJsonFile(`${safeFilename(content.name)}-r${content.revision}.json`, {
        id: content.id,
        name: content.name,
        programKind: content.programKind,
        revision: content.revision,
        input: content.input,
        content: content.content,
        contentSha256: content.contentSha256,
        exportedAt: new Date().toISOString(),
      });
      if (saved) showWorkspaceNotice($t('Shared configuration exported.'));
    });
  }

  async function advanceSyncCheckpoint() {
    if (!canWriteSync || !syncFeed) return;
    const request = {
      cursor: syncFeed.nextCursor,
      baseRowVersion: syncCheckpoint?.rowVersion ?? 0,
      operationId: operationId(),
    };
    await executeMutation(
      'sync-checkpoint',
      () => api.advanceLicenseWorkspaceCheckpoint(request).then(() => {}),
      () => loadSyncActivity(false, true),
      'This device checkpoint was advanced.',
    );
  }

  function beginCreateAlertRule() {
    editingAlertRule = null;
    alertRuleName = '';
    alertRuleEventKind = 'sync_conflict';
    alertRuleSeverity = 'warning';
    alertRuleEnabled = true;
    alertFormError = '';
    alertFormOpen = true;
  }

  function beginEditAlertRule(rule: WorkspaceAlertRule) {
    if (!canManageAlerts) return;
    editingAlertRule = rule;
    alertRuleName = rule.name;
    alertRuleEventKind = rule.eventKind;
    alertRuleSeverity = rule.severity;
    alertRuleEnabled = rule.enabled;
    alertFormError = '';
    alertFormOpen = true;
  }

  async function saveAlertRule() {
    alertFormError = '';
    const name = alertRuleName.trim();
    if (!name) {
      alertFormError = $t('Enter an alert rule name.');
      return;
    }
    const editing = editingAlertRule;
    const request = {
      ...(editing ? { baseRowVersion: editing.rowVersion } : {}),
      name,
      eventKind: alertRuleEventKind,
      severity: alertRuleSeverity,
      enabled: alertRuleEnabled,
      operationId: operationId(),
    };
    await executeMutation(
      editing ? 'alert-rule-update' : 'alert-rule-create',
      () => (editing
        ? api.updateLicenseWorkspaceAlertRule(editing.id, {
            ...request,
            baseRowVersion: editing.rowVersion,
          })
        : api.createLicenseWorkspaceAlertRule(request)).then(() => {
          alertFormOpen = false;
          editingAlertRule = null;
        }),
      () => loadAlerts(false, false, true),
      editing ? 'Alert rule updated.' : 'Alert rule created.',
    );
  }

  async function deleteAlertRule(rule: WorkspaceAlertRule) {
    if (!canManageAlerts) return;
    const confirmed = await onConfirmAction(
      $t('Delete alert rule?'),
      $t('Existing incidents remain in history, but this rule will stop creating new incidents.'),
      $t('Delete alert rule'),
      true,
    );
    if (!confirmed) return;
    const request = { baseRowVersion: rule.rowVersion, operationId: operationId() };
    await executeMutation(
      'alert-rule-delete',
      () => api.deleteLicenseWorkspaceAlertRule(rule.id, request).then(() => {}),
      () => loadAlerts(false, false, true),
      'Alert rule deleted.',
    );
  }

  async function acknowledgeIncident(incident: WorkspaceAlertIncident) {
    if (!canAcknowledgeAlerts || incident.status !== 'open') return;
    const request = { baseRowVersion: incident.rowVersion, operationId: operationId() };
    await executeMutation(
      'incident-acknowledge',
      () => api.acknowledgeLicenseWorkspaceAlertIncident(incident.id, request).then(() => {}),
      () => loadAlerts(false, false, true),
      'Incident acknowledged.',
    );
  }

  async function resolveIncident(incident: WorkspaceAlertIncident) {
    if (!canManageAlerts || incident.status !== 'acknowledged') return;
    const confirmed = await onConfirmAction(
      $t('Resolve this incident?'),
      $t('Resolved incidents are terminal and remain available only to roles with alert history access.'),
      $t('Resolve incident'),
      false,
    );
    if (!confirmed) return;
    const request = { baseRowVersion: incident.rowVersion, operationId: operationId() };
    await executeMutation(
      'incident-resolve',
      () => api.resolveLicenseWorkspaceAlertIncident(incident.id, request).then(() => {}),
      () => loadAlerts(false, false, true),
      'Incident resolved.',
    );
  }

  async function exportAuditEvents() {
    if (!canExportAudit || !effectiveAuditExportLimit) return;
    auditExportStatus = '';
    await readWorkspaceData('audit-export', async () => {
      const result = await api.exportLicenseWorkspaceAuditEvents({
        cursor: null,
        limit: effectiveAuditExportLimit,
        eventType: auditEventType.trim() || null,
      });
      const saved = await saveJsonFile(`camellia-nexus-audit-${new Date().toISOString().slice(0, 10)}.json`, {
        exportedAt: new Date().toISOString(),
        truncated: result.truncated,
        nextCursor: result.nextCursor ?? null,
        events: result.events,
      });
      if (saved) {
        auditExportStatus = result.truncated
          ? `${$t('Exported')} ${result.events.length}. ${$t('The bounded export was truncated; narrow the event filter to export a smaller set.')}`
          : `${$t('Exported')} ${result.events.length} ${$t('audit events')}`;
      }
    });
  }

  function beginCreateWebhookEndpoint() {
    if (webhookSecretResult) return;
    editingWebhookEndpoint = null;
    webhookName = '';
    webhookUrl = '';
    selectedWebhookEventTypes = [];
    webhookActive = true;
    webhookFormError = '';
    webhookFormOpen = true;
  }

  function beginEditWebhookEndpoint(endpoint: WebhookEndpoint) {
    if (!canManageWebhooks || webhookSecretResult) return;
    editingWebhookEndpoint = endpoint;
    webhookName = endpoint.name;
    webhookUrl = endpoint.url;
    selectedWebhookEventTypes = [...endpoint.eventTypes];
    webhookActive = endpoint.active;
    webhookFormError = '';
    webhookFormOpen = true;
  }

  function toggleWebhookEventType(eventType: string, checked: boolean) {
    selectedWebhookEventTypes = checked
      ? [...new Set([...selectedWebhookEventTypes, eventType])]
      : selectedWebhookEventTypes.filter((value) => value !== eventType);
  }

  async function saveWebhookEndpoint() {
    webhookFormError = '';
    const name = webhookName.trim();
    const url = webhookUrl.trim();
    if (!name || !url.startsWith('https://') || !selectedWebhookEventTypes.length) {
      webhookFormError = $t('Enter a name, a public HTTPS URL, and at least one event type.');
      return;
    }
    const editing = editingWebhookEndpoint;
    const requestSecretGeneration = secretGeneration;
    const request: CreateWebhookEndpoint = {
      operationId: operationId(),
      name,
      url,
      eventTypes: [...selectedWebhookEventTypes].sort(),
      active: webhookActive,
    };
    await executeMutation(
      editing ? 'webhook-update' : 'webhook-create',
      async () => {
        if (editing) {
          await api.updateLicenseWorkspaceWebhookEndpoint(editing.id, {
            ...request,
            rowVersion: editing.rowVersion,
          });
        } else {
          const result = await api.createLicenseWorkspaceWebhookEndpoint(request);
          if (result.secret) {
            if (!destroyed && activeView === 'webhooks' && requestSecretGeneration === secretGeneration) {
              webhookSecretResult = result;
              webhookSecretCopyStatus = '';
            } else {
              result.secret = null;
            }
          } else {
            showWorkspaceNotice($t('The endpoint already exists, but its one-time secret cannot be displayed again. Rotate the secret if a new value is required.'), 12_000);
          }
        }
        webhookFormOpen = false;
        editingWebhookEndpoint = null;
      },
      () => loadWebhooks(true),
      editing ? 'Webhook endpoint updated.' : 'Webhook endpoint created.',
    );
  }

  async function rotateWebhookSecret(endpoint: WebhookEndpoint) {
    if (!canManageWebhooks || webhookSecretResult) return;
    const confirmed = await onConfirmAction(
      $t('Rotate webhook secret?'),
      $t('The current signing secret will stop working immediately. Update the receiver before the next delivery.'),
      $t('Rotate secret'),
      true,
    );
    if (!confirmed) return;
    const request = { operationId: operationId(), rowVersion: endpoint.rowVersion };
    const requestSecretGeneration = secretGeneration;
    await executeMutation(
      'webhook-rotate',
      async () => {
        const result = await api.rotateLicenseWorkspaceWebhookEndpoint(endpoint.id, request);
        if (result.secret) {
          if (!destroyed && activeView === 'webhooks' && requestSecretGeneration === secretGeneration) {
            webhookSecretResult = result;
            webhookSecretCopyStatus = '';
          } else {
            result.secret = null;
          }
        } else {
          showWorkspaceNotice($t('This rotation was already completed and its one-time secret cannot be displayed again. Rotate again only if the receiver still needs a new secret.'), 12_000);
        }
      },
      () => loadWebhooks(true),
      'Webhook secret rotated.',
    );
  }

  async function deleteWebhookEndpoint(endpoint: WebhookEndpoint) {
    if (!canManageWebhooks) return;
    const confirmed = await onConfirmAction(
      $t('Delete webhook endpoint?'),
      $t('Pending deliveries for this endpoint will be stopped. Delivery metadata remains available for audit.'),
      $t('Delete endpoint'),
      true,
    );
    if (!confirmed) return;
    const request = { operationId: operationId(), rowVersion: endpoint.rowVersion };
    await executeMutation(
      'webhook-delete',
      () => api.deleteLicenseWorkspaceWebhookEndpoint(endpoint.id, request).then(() => {}),
      () => loadWebhooks(true),
      'Webhook endpoint deleted.',
    );
  }

  async function copyWebhookSecret() {
    const secret = webhookSecretResult?.secret;
    if (!secret) return;
    try {
      await navigator.clipboard.writeText(secret);
      webhookSecretCopyStatus = 'copied';
    } catch {
      webhookSecretCopyStatus = 'failed';
    }
  }

  function clearWebhookSecret() {
    if (webhookSecretResult?.secret) webhookSecretResult.secret = null;
    webhookSecretResult = null;
    webhookSecretCopyStatus = '';
  }

  function safeFilename(value: string) {
    const filename = value.trim().replace(/[^a-z0-9._-]+/gi, '-').replace(/^-+|-+$/g, '');
    return filename || 'shared-configuration';
  }

  function alertEventKindLabel(kind: WorkspaceAlertEventKind) {
    switch (kind) {
      case 'sync_conflict': return 'Sync conflict';
      case 'quota_warning': return 'Quota warning';
      case 'configuration_created': return 'Configuration created';
      case 'configuration_revised': return 'Configuration revised';
      case 'configuration_published': return 'Configuration published';
      case 'configuration_deleted': return 'Configuration deleted';
      case 'configuration_restored': return 'Configuration restored';
    }
  }

  function sharedProgramKindLabel(kind: SharedConfigurationSummary['programKind']) {
    switch (kind) {
      case 'singBox': return 'sing-box';
      case 'xray': return 'Xray';
      case 'mihomo': return 'Mihomo';
      default: return 'Generic';
    }
  }

  function auditOutcomeLabel(outcome: string) {
    switch (outcome) {
      case 'succeeded': return 'Succeeded';
      case 'denied': return 'Denied';
      case 'failed': return 'Failed';
      default: return outcome;
    }
  }

  function auditEventLabel(eventType: string) {
    switch (eventType) {
      case 'account_created': return 'Account created';
      case 'activation_code_issue': return 'Activation code issued';
      case 'activation_proof_issued': return 'Activation proof issued';
      case 'authorization_code_denied': return 'Authorization denied';
      case 'authorization_code_issued': return 'Authorization approved';
      case 'challenge_denied': return 'Device verification challenge denied';
      case 'challenge_issued': return 'Device verification challenge issued';
      case 'client_version_denied': return 'Client version denied';
      case 'device_activation_confirmed': return 'Device activation confirmed';
      case 'device_activation_denied': return 'Device activation denied';
      case 'device_registered': return 'Device registered';
      case 'device_registration_denied': return 'Device registration denied';
      case 'device_removed': return 'Device removed';
      case 'device_removal_denied': return 'Device removal denied';
      case 'device_session_issued': return 'Device session issued';
      case 'entitlement_refreshed': return 'License refreshed';
      case 'entitlement_status_denied': return 'License status refresh denied';
      case 'license_created': return 'License created';
      case 'rate_limit_denied': return 'Request rate limit reached';
      case 'refresh_session_reuse_detected': return 'Session reuse detected';
      case 'session_authorization_denied': return 'Session authorization denied';
      case 'session_logout': return 'Device signed out';
      case 'session_recovered': return 'Device session recovered';
      case 'session_recovery_challenge_denied': return 'Session recovery challenge denied';
      case 'session_recovery_challenge_issued': return 'Session recovery challenge issued';
      case 'session_recovery_denied': return 'Session recovery denied';
      case 'team_invitation_created': return 'Team invitation created';
      case 'team_invitation_accepted': return 'Team invitation accepted';
      case 'team_invitation_accept_denied': return 'Team invitation acceptance denied';
      case 'team_member_updated': return 'Team member updated';
      case 'team_member_left': return 'Team member left';
      case 'team_ownership_transferred': return 'Team ownership transferred';
      case 'team_member_device_enrollment_created': return 'Team device enrollment created';
      case 'team_member_device_recovery_created': return 'Team member device recovery created';
      case 'team_member_device_enrollment_consumed': return 'Team device enrollment consumed';
      case 'team_member_device_enrollment_accept_denied': return 'Team device enrollment denied';
      case 'workspace_configuration_created': return 'Configuration created';
      case 'workspace_configuration_revised': return 'Configuration revised';
      case 'workspace_configuration_published': return 'Configuration published';
      case 'workspace_configuration_deleted': return 'Configuration deleted';
      case 'workspace_configuration_restored': return 'Configuration restored';
      case 'workspace_configuration_purged': return 'Configuration permanently removed';
      case 'workspace_configuration_limit_denied': return 'Workspace configuration creation denied';
      case 'workspace_configuration_purge_denied': return 'Workspace configuration removal denied';
      case 'workspace_sync_checkpoint_advanced': return 'Workspace checkpoint advanced';
      case 'workspace_sync_conflict': return 'Sync conflict';
      case 'workspace_alert_rule_created': return 'Alert rule created';
      case 'workspace_alert_rule_updated': return 'Alert rule updated';
      case 'workspace_alert_rule_deleted': return 'Alert rule deleted';
      case 'workspace_alert_rule_limit_denied': return 'Alert rule creation denied';
      case 'workspace_alert_incident_opened': return 'Alert incident opened';
      case 'workspace_alert_incident_updated': return 'Alert incident updated';
      case 'workspace_alert_incident_acknowledged': return 'Alert incident acknowledged';
      case 'workspace_alert_incident_resolved': return 'Alert incident resolved';
      case 'workspace_server_alert_event_received': return 'Workspace alert received';
      case 'workspace_webhook_endpoint_created': return 'Webhook endpoint created';
      case 'workspace_webhook_endpoint_updated': return 'Webhook endpoint updated';
      case 'workspace_webhook_endpoint_deleted': return 'Webhook endpoint deleted';
      case 'workspace_webhook_secret_rotated': return 'Webhook secret rotated';
      default: return eventType;
    }
  }

  function auditReasonLabel(reasonCode: string) {
    switch (reasonCode) {
      case 'admin_operation': return 'Administrative operation';
      case 'device_already_bound': return 'Device is already linked to a member';
      case 'device_bound_to_other_member': return 'Device is linked to another member';
      case 'device_enrollment_already_consumed': return 'Device enrollment token was already used';
      case 'device_enrollment_expired': return 'Device enrollment token expired';
      case 'device_enrollment_not_found': return 'Device enrollment token was not found';
      case 'device_enrollment_revoked': return 'Device enrollment token was revoked';
      case 'invitation_already_consumed': return 'Invitation was already used';
      case 'invitation_expired': return 'Invitation token expired';
      case 'invitation_not_found': return 'Invitation was not found';
      case 'invitation_revoked': return 'Invitation token was revoked';
      case 'member_device_limit_reached': return 'Member device limit reached';
      case 'member_not_active': return 'Member is not active';
      case 'member_not_invited': return 'Member is not awaiting invitation';
      case 'member_suspended': return 'Member is suspended';
      case 'retention_active': return 'Retention period is active';
      case 'version_conflict': return 'Workspace version changed';
      case 'workspace_alert_rule_limit_reached': return 'Workspace alert-rule limit reached';
      default: return reasonCode;
    }
  }

  function auditMetadataLabel(key: string) {
    switch (key) {
      case 'cursor': return 'Cursor';
      case 'document_id': return 'Document ID';
      case 'event_kind': return 'Event kind';
      case 'incident_count': return 'Incident count';
      case 'incident_id': return 'Incident ID';
      case 'member_id': return 'Member ID';
      case 'max_alert_rules': return 'Maximum alert rules';
      case 'released_bytes': return 'Released bytes';
      case 'resource_id': return 'Resource ID';
      case 'resource_type': return 'Resource type';
      case 'revision': return 'Revision';
      case 'row_version': return 'Row version';
      case 'rule_id': return 'Rule ID';
      case 'severity': return 'Severity';
      case 'status': return 'Status';
      case 'target_device_id': return 'Device ID';
      case 'usage_bytes': return 'Usage bytes';
      default: return key;
    }
  }

  function webhookEventLabel(eventType: string) {
    switch (eventType) {
      case 'alert.incident.acknowledged': return 'Alert incident acknowledged';
      case 'alert.incident.opened': return 'Alert incident opened';
      case 'alert.incident.resolved': return 'Alert incident resolved';
      case 'configuration.created': return 'Configuration created';
      case 'configuration.deleted': return 'Configuration deleted';
      case 'configuration.published': return 'Configuration published';
      case 'configuration.purged': return 'Configuration permanently removed';
      case 'configuration.restored': return 'Configuration restored';
      case 'configuration.revised': return 'Configuration revised';
      case 'sync.conflict': return 'Sync conflict';
      default: return eventType;
    }
  }

  function webhookDeliveryStatusLabel(status: WebhookDeliverySummary['status']) {
    switch (status) {
      case 'pending': return 'Pending delivery';
      case 'inFlight': return 'Delivery in progress';
      case 'delivered': return 'Delivered';
      case 'retry': return 'Waiting to retry';
      case 'dead': return 'Delivery stopped';
    }
  }

  function webhookErrorLabel(category: string | null | undefined) {
    if (!category) return 'No error';
    switch (category) {
      case 'dns': return 'DNS error';
      case 'network': return 'Network error';
      case 'timeout': return 'Operation timed out';
      case 'tls': return 'TLS error';
      case 'http_status': return 'HTTP error';
      default: return category;
    }
  }

  function bytesLabel(bytes: number) {
    if (bytes >= 1_073_741_824) return `${(bytes / 1_073_741_824).toFixed(1)} GiB`;
    if (bytes >= 1_048_576) return `${(bytes / 1_048_576).toFixed(1)} MiB`;
    if (bytes >= 1_024) return `${(bytes / 1_024).toFixed(1)} KiB`;
    return `${bytes} B`;
  }

  onDestroy(() => {
    destroyed = true;
    invitationToken = '';
    deviceEnrollmentToken = '';
    clearWorkspaceNotice();
    clearWebhookSecret();
  });
</script>

<section class="team-workspace-panel">
  <header>
    <div>
      <h3>{$t('Team workspace')}</h3>
      <p>{$t('Membership and permissions are enforced by the license service.')}</p>
    </div>
    <button class="secondary-action" type="button" on:click={onRefresh} disabled={busy || !canRefresh}>
      {$t('Refresh team')}
    </button>
  </header>

  {#if visibleExternalError}<div class="team-error"><ErrorNotice error={visibleExternalError} dismissible autoDismissMs={transientDismissDelay(visibleExternalError)} onDismiss={dismissExternalError} /></div>{/if}
  {#if profile}
    <div class="team-summary">
      <div><span>{$t('Current member')}</span><strong class="team-summary-identity" title={profile.member?.displayName ?? $t('Not linked')}>{profile.member?.displayName ?? $t('Not linked')}</strong></div>
      <div><span>{$t('Workspace role')}</span><strong>{profile.member ? $t(workspaceRoleLabel(profile.member.role)) : $t('No workspace role')}</strong></div>
      <div><span>{$t('Member capacity')}</span><strong>{profile.memberCount}/{profile.memberLimit}</strong></div>
    </div>
  {/if}

  {#if currentMember?.status === 'active' && visibleViews.length > 1}
    <div
      class="workspace-view-tabs"
      role="tablist"
      tabindex="-1"
      aria-label={$t('Team workspace sections')}
      bind:this={viewNavigation}
      on:keydown={navigateViews}
    >
      {#each visibleViews as view (view.id)}
        <button
          type="button"
          role="tab"
          data-workspace-view={view.id}
          aria-selected={activeView === view.id}
          aria-controls={`team-workspace-${view.id}`}
          tabindex={activeView === view.id ? 0 : -1}
          on:click={() => void selectView(view.id)}
        >{$t(view.label)}</button>
      {/each}
    </div>
  {/if}

  {#if visibleWorkspaceError}
    <div class="workspace-error" bind:this={errorRegion} tabindex="-1">
      <ErrorNotice error={visibleWorkspaceError} dismissible autoDismissMs={retryableMutation ? 0 : transientDismissDelay(visibleWorkspaceError)} onDismiss={dismissWorkspaceError} />
      {#if retryableMutation}
        <button type="button" on:click={() => void retryLastMutation()} disabled={!!workspaceBusyAction}>
          {$t('Retry same request')}
        </button>
      {/if}
    </div>
  {/if}
  {#if workspaceNotice}<p class="workspace-notice" role="status" aria-live="polite">{workspaceNotice}</p>{/if}

  {#if activeView === 'members'}
  <div id="team-workspace-members" role="tabpanel" aria-label={$t('Members')}>
  {#if !currentMember}
    <div class="team-linking-actions">
      <form class="team-accept" aria-busy={!!busy} on:submit|preventDefault={() => void acceptInvitationToken()}>
        <div><strong>{$t('Join this workspace')}</strong><small>{$t('After activating this device with a code for the same Team license, paste the one-time invitation token supplied by a workspace administrator.')}</small></div>
        <input bind:value={invitationToken} maxlength={TEAM_TOKEN_MAX_LENGTH} autocomplete="off" autocapitalize="none" spellcheck="false" placeholder={$t('Invitation token')} disabled={busy} aria-invalid={!!formError} aria-describedby={formError ? 'team-form-error' : undefined} />
        <button class="primary" type="submit" disabled={busy}>{$t('Join workspace')}</button>
      </form>
      <form class="team-accept team-device-accept" aria-busy={!!busy} on:submit|preventDefault={() => void acceptDeviceEnrollmentToken()}>
        <div><strong>{$t('Link this device to an existing member')}</strong><small>{$t('Use the one-time device enrollment token created on an already linked device.')}</small></div>
        <input bind:value={deviceEnrollmentToken} maxlength={TEAM_TOKEN_MAX_LENGTH} autocomplete="off" autocapitalize="none" spellcheck="false" placeholder={$t('Device enrollment token')} disabled={busy} aria-label={$t('Device enrollment token')} aria-invalid={!!deviceEnrollmentFormError} aria-describedby={deviceEnrollmentFormError ? 'device-enrollment-form-error' : undefined} />
        <button class="primary" type="submit" disabled={busy}>{$t(busyAction === 'license-team-device-enrollment-accept' ? 'Linking device' : 'Link device')}</button>
        {#if deviceEnrollmentFormError}<p id="device-enrollment-form-error" class="form-error team-inline-error" role="alert" aria-live="assertive">{deviceEnrollmentFormError}</p>{/if}
      </form>
    </div>
  {:else if currentMember.status === 'active'}
    <div class="team-device-enrollment" aria-busy={!!busy}>
      <div class="team-device-enrollment-heading">
        <div><strong>{$t('Add another device')}</strong><small>{$t('Create a one-time token that expires after 15 minutes.')}</small></div>
        <button type="button" on:click={() => void createDeviceEnrollment()} disabled={busy || !!deviceEnrollment}>{$t(busyAction === 'license-team-device-enrollment-create' ? 'Creating token' : 'Create device token')}</button>
      </div>
      {#if deviceEnrollment}
        <div class="team-secret">
          <div class="team-secret-heading">
            <strong>{$t('Device enrollment token')}</strong>
            <div class="team-secret-actions">
              <button type="button" on:click={() => void copyDeviceEnrollmentToken()} disabled={busy}>{$t('Copy device enrollment token')}</button>
              <button type="button" on:click={onDismissDeviceEnrollment} disabled={busy}>{$t('Dismiss')}</button>
            </div>
          </div>
          <code>{deviceEnrollment.enrollmentToken}</code>
          <small>{$t('Expires')} {formatMinuteDate(deviceEnrollment.expiresAt, $uiLanguage)} · {$t('This token is shown only once.')}</small>
          {#if deviceEnrollmentCopyStatus}<span class:failed={deviceEnrollmentCopyStatus === 'failed'} class="copy-status" role="status" aria-live="polite">
            {$t(deviceEnrollmentCopyStatus === 'copied' ? 'Device enrollment token copied' : 'Unable to copy device enrollment token')}
          </span>{/if}
        </div>
      {/if}
    </div>
  {/if}

  {#if canReadTeam}
    <div class="team-members" aria-label={$t('Team members')}>
      {#each members as member (member.id)}
        <article class:readonly-member={!canManageMember(member)}>
          <div class="team-member-identity"><strong title={member.displayName}>{member.displayName}</strong><small title={member.email.endsWith('@local.invalid') ? $t('Primary workspace owner') : member.email}>{member.email.endsWith('@local.invalid') ? $t('Primary workspace owner') : member.email}</small></div>
          <span class={`team-member-status ${member.status}`}>{$t(workspaceMemberStatusLabel(member.status))}</span>
          <span class="team-member-device-count">{member.boundDeviceCount} {$t(member.boundDeviceCount === 1 ? 'linked device' : 'linked devices')}</span>
          {#if !canManageMember(member)}
            <span class="team-member-role">{$t(workspaceRoleLabel(member.role))}</span>
          {:else}
            <div class="team-member-controls" class:invitation-controls={member.status === 'invited'}>
              {#if member.status === 'invited'}
                <span class="team-member-role">{$t(workspaceRoleLabel(member.role))}</span>
                <button class="danger" type="button" on:click={() => void removeMember(member)} disabled={busy}>{$t('Revoke invitation')}</button>
              {:else}
                <select class="option-align-center" data-team-select="enum" data-control-size="md" value={member.role} aria-label={$t('Workspace role')} on:change={(event) => updateMember(member, (event.currentTarget as HTMLSelectElement).value as Exclude<WorkspaceRole, 'owner'>, member.status === 'suspended' ? 'suspended' : 'active')} disabled={busy}>
                  {#if profile?.member?.role === 'owner'}<option value="admin">{$t('Administrator')}</option>{/if}
                  <option value="billing">{$t('Billing manager')}</option>
                  <option value="operator">{$t('Operator')}</option>
                  <option value="auditor">{$t('Auditor')}</option>
                  <option value="viewer">{$t('Viewer')}</option>
                </select>
                <button type="button" on:click={() => updateMember(member, member.role, member.status === 'suspended' ? 'active' : 'suspended')} disabled={busy}>{$t(member.status === 'suspended' ? 'Restore access' : 'Suspend access')}</button>
                {#if member.status === 'active' && member.boundDeviceCount === 0}
                  <button type="button" on:click={() => void createMemberDeviceEnrollment(member.id)} disabled={busy || !!deviceEnrollment}>{$t('Create recovery token')}</button>
                {/if}
                <button class="danger" type="button" on:click={() => void removeMember(member)} disabled={busy}>{$t('Remove member')}</button>
              {/if}
            </div>
          {/if}
        </article>
      {/each}
      <div class="team-member-pagination">
        <small role="status" aria-live="polite">{members.length} {$t('member records loaded')}</small>
        {#if membersHasMore}
          <button type="button" on:click={onLoadMoreMembers} disabled={busy || membersLoadingMore}>
            {$t(membersLoadingMore ? 'Loading more members' : 'Load more members')}
          </button>
        {/if}
      </div>
    </div>
  {/if}

  {#if canManageTeam}
    <form class="team-invite-form" aria-busy={!!busy} on:submit|preventDefault={submitInvitation}>
      <div class="form-heading"><strong>{$t('Invite team member')}</strong><small>{$t('Invitation tokens are shown once and expire after seven days.')}</small></div>
      <label>{$t('Member name')}<input bind:value={inviteName} maxlength="160" autocomplete="off" disabled={busy} aria-invalid={!!formError} aria-describedby={formError ? 'team-form-error' : undefined} /></label>
      <label>{$t('Email address')}<input bind:value={inviteEmail} maxlength="254" type="email" autocomplete="off" disabled={busy} aria-invalid={!!formError} aria-describedby={formError ? 'team-form-error' : undefined} /></label>
      <label>{$t('Workspace role')}
        <select class="option-align-center" data-team-select="enum" bind:value={inviteRole} disabled={busy}>
          {#if currentMember?.role === 'owner'}<option value="admin">{$t('Administrator')}</option>{/if}
          <option value="billing">{$t('Billing manager')}</option>
          <option value="operator">{$t('Operator')}</option>
          <option value="auditor">{$t('Auditor')}</option>
          <option value="viewer">{$t('Viewer')}</option>
        </select>
      </label>
      <button class="primary team-invite-submit" type="submit" disabled={busy || (profile?.memberCount ?? 0) >= (profile?.memberLimit ?? 0)}>{$t('Create invitation')}</button>
      {#if invitation}
        <div class="team-secret">
          <div class="team-secret-heading">
            <strong>{$t('Invitation token')}</strong>
            <div class="team-secret-actions">
              <button type="button" on:click={() => void copyInvitationToken()} disabled={busy}>{$t('Copy invitation token')}</button>
              <button type="button" on:click={onDismissInvitation} disabled={busy}>{$t('Dismiss')}</button>
            </div>
          </div>
          <code>{invitation.invitationToken}</code>
          <small>{$t('Copy this token now. It cannot be retrieved again after this view is closed.')}</small>
          {#if invitationCopyStatus}<span class:failed={invitationCopyStatus === 'failed'} class="copy-status" role="status" aria-live="polite">
            {$t(invitationCopyStatus === 'copied' ? 'Invitation token copied' : 'Unable to copy invitation token')}
          </span>{/if}
        </div>
      {/if}
    </form>

    {#if currentMember?.role === 'owner' && currentMember.status === 'active'}
      <div class="team-governance" aria-busy={!!busy}>
        <div><strong>{$t('Transfer workspace ownership')}</strong><small>{$t('Only an active administrator can become the new owner. You will remain an administrator.')}</small></div>
        {#if activeAdminCandidates.length}
          <label>{$t('New workspace owner')}
            <select class="option-align-start" data-team-select="entity" bind:value={ownershipTargetMemberId} disabled={busy} title={ownershipCandidateTitle(ownershipTargetMemberId)}>
              <option value="">{$t('Select an administrator')}</option>
              {#each activeAdminCandidates as member (member.id)}
                <option value={member.id}>{member.displayName} · {member.email}</option>
              {/each}
            </select>
          </label>
          <button class="danger" type="button" on:click={() => void transferOwnership()} disabled={busy || !ownershipTargetMemberId}>{$t(busyAction === 'license-team-ownership-transfer' ? 'Transferring ownership' : 'Transfer ownership')}</button>
        {:else}
          <small class="team-governance-empty">{$t('No active administrator is eligible for ownership transfer.')}</small>
        {/if}
      </div>
    {/if}
  {/if}

  {#if currentMember && currentMember.role !== 'owner' && currentMember.status === 'active'}
    <div class="team-governance team-leave-workspace" aria-busy={!!busy}>
      <div><strong>{$t('Leave workspace')}</strong><small>{$t('Leaving removes this device from the team and stops protected managed programs.')}</small></div>
      <button class="danger" type="button" on:click={() => void leaveWorkspace()} disabled={busy}>{$t(busyAction === 'license-team-leave' ? 'Leaving workspace' : 'Leave workspace')}</button>
    </div>
  {/if}

  {#if formError}<p id="team-form-error" class="form-error team-form-error" role="alert" aria-live="assertive">{formError}</p>{/if}
  </div>
  {/if}

  {#if activeView === 'shared'}
    <div id="team-workspace-shared" class="cloud-panel" role="tabpanel" aria-label={$t('Shared configurations')} aria-busy={workspaceBusyAction.startsWith('shared-')}>
      <div class="cloud-toolbar shared-toolbar">
        <div class="shared-toolbar-heading">
          <div class="shared-toolbar-copy">
            <strong>{$t('Shared configurations')}</strong>
            <small>{$t('Encrypted revisions are synchronized through the Team workspace.')}</small>
          </div>
          <div class="toolbar-actions shared-toolbar-actions">
            {#if canWriteShared}
              <button class="primary" type="button" on:click={beginCreateSharedConfiguration} disabled={!!workspaceBusyAction}>{$t('New configuration')}</button>
            {/if}
            <button type="button" on:click={() => void loadSharedConfigurations()} disabled={!!workspaceBusyAction}>{$t('Refresh')}</button>
          </div>
        </div>
        <div class:stats-only={!canWriteShared} class="shared-toolbar-context">
          {#if canWriteShared}
            <label class="inline-check shared-filter-toggle"><input type="checkbox" bind:checked={sharedIncludeDeleted} on:change={() => void loadSharedConfigurations()} disabled={!!workspaceBusyAction} /> {$t('Show deleted')}</label>
          {/if}
          <div class="shared-usage-summary" aria-live="polite">
            {#if sharedPage}
              <span>{$t('Documents')} <strong>{sharedPage.usage.activeDocumentCount}/{sharedPage.usage.maxActiveDocuments}</strong></span>
              <span>{$t('Revision storage')} <strong>{bytesLabel(sharedPage.usage.revisionPlaintextBytes)} / {bytesLabel(sharedPage.usage.maxRevisionPlaintextBytes)}</strong></span>
            {/if}
          </div>
        </div>
      </div>

      {#if sharedFormOpen && canWriteShared}
        <form class="cloud-form shared-form" on:submit|preventDefault={() => void saveSharedConfiguration()} aria-busy={!!workspaceBusyAction}>
          <div class="form-heading">
            <strong>{$t(selectedSharedContent ? 'Revise shared configuration' : 'Create shared configuration')}</strong>
            <small>{$t('Saving creates an immutable revision. Publishing is a separate action.')}</small>
          </div>
          <label>{$t('Name')}<input bind:value={sharedFormName} maxlength="160" disabled={!!workspaceBusyAction} aria-invalid={!!sharedFormError} aria-describedby={sharedFormError ? 'shared-form-error' : undefined} /></label>
          <label>{$t('Program type')}
            <select class="option-align-center" data-team-select="enum" bind:value={sharedFormProgramKind} disabled={!!workspaceBusyAction}>
              <option value="generic">{$t('Generic')}</option>
              <option value="singBox">sing-box</option>
              <option value="xray">Xray</option>
              <option value="mihomo">Mihomo</option>
            </select>
          </label>
          <label class="wide-field">{$t('Input arguments')}<textarea bind:value={sharedFormInput} rows="3" spellcheck="false" disabled={!!workspaceBusyAction}></textarea></label>
          <label class="wide-field">{$t('Configuration content')}<textarea class="code-input" bind:value={sharedFormContent} rows="12" spellcheck="false" disabled={!!workspaceBusyAction} aria-invalid={!!sharedFormError} aria-describedby={sharedFormError ? 'shared-form-error' : undefined}></textarea></label>
          {#if sharedFormError}<p id="shared-form-error" class="form-error wide-field" role="alert">{sharedFormError}</p>{/if}
          <div class="form-actions wide-field">
            <button class="primary" type="submit" disabled={!!workspaceBusyAction}>{$t(selectedSharedContent ? 'Save revision' : 'Create configuration')}</button>
            <button type="button" on:click={() => { sharedFormOpen = false; selectedSharedContent = null; }} disabled={!!workspaceBusyAction}>{$t('Cancel')}</button>
          </div>
        </form>
      {/if}

      {#if workspaceBusyAction === 'shared-list' && !sharedLoaded}
        <p class="cloud-empty">{$t('Loading shared configurations')}…</p>
      {:else if sharedLoaded && !sharedConfigurations.length}
        <p class="cloud-empty">{$t('No shared configurations match this view.')}</p>
      {:else}
        <div class="resource-list shared-list">
          {#each sharedConfigurations as configuration (configuration.id)}
            <article class:deleted={!!configuration.deletedAt}>
              <div class="resource-main">
                <div class="resource-title">
                  <strong>{configuration.name}</strong>
                  <span>{$t(sharedProgramKindLabel(configuration.programKind))}</span>
                  {#if configuration.deletedAt}<span class="status danger">{$t('Deleted')}</span>{/if}
                </div>
                <small>
                  {$t('Draft revision')} {configuration.draftRevision}
                  · {$t('Published revision')} {configuration.publishedRevision ?? '—'}
                  · {bytesLabel(configuration.plaintextBytes)}
                  · {$t('Updated')} {formatMinuteDate(configuration.updatedAt, $uiLanguage)}
                </small>
                <code class="hash">sha256:{configuration.contentSha256}</code>
              </div>
              <div class="resource-actions">
                <button type="button" on:click={() => void loadSharedContent(configuration)} disabled={!!workspaceBusyAction}>{$t('View')}</button>
                <button type="button" on:click={() => void exportSharedConfiguration(configuration)} disabled={!!workspaceBusyAction}>{$t('Export')}</button>
                {#if canWriteShared && !configuration.deletedAt}
                  <button type="button" on:click={() => void beginReviseSharedConfiguration(configuration)} disabled={!!workspaceBusyAction}>{$t('Revise')}</button>
                  <button class="danger" type="button" on:click={() => void setSharedConfigurationDeleted(configuration, false)} disabled={!!workspaceBusyAction}>{$t('Delete')}</button>
                {:else if canWriteShared && configuration.deletedAt}
                  <button type="button" on:click={() => void setSharedConfigurationDeleted(configuration, true)} disabled={!!workspaceBusyAction}>{$t('Restore')}</button>
                  {#if canPurgeConfiguration(configuration)}
                    <button class="danger" type="button" on:click={() => void purgeSharedConfiguration(configuration)} disabled={!!workspaceBusyAction}>{$t('Permanently remove')}</button>
                  {/if}
                {/if}
                {#if canPublishShared && !configuration.deletedAt && configuration.draftRevision !== configuration.publishedRevision}
                  <button class="primary" type="button" on:click={() => void publishSharedConfiguration(configuration)} disabled={!!workspaceBusyAction}>{$t('Publish draft')}</button>
                {/if}
              </div>
            </article>
          {/each}
        </div>
        {#if sharedPage?.hasMore}
          <button class="load-more" type="button" on:click={() => void loadSharedConfigurations(true)} disabled={!!workspaceBusyAction}>{$t('Load more')}</button>
        {/if}
      {/if}

      {#if selectedSharedContent && !sharedFormOpen}
        <section class="content-preview" aria-label={$t('Shared configuration content')}>
          <div>
            <strong>{selectedSharedContent.name} · {$t('Revision')} {selectedSharedContent.revision}</strong>
            <button type="button" on:click={() => { selectedSharedContent = null; }}>{$t('Close')}</button>
          </div>
          {#if selectedSharedContent.input}<pre>{selectedSharedContent.input}</pre>{/if}
          <pre>{selectedSharedContent.content}</pre>
        </section>
      {/if}
    </div>
  {/if}

  {#if activeView === 'sync'}
    <div id="team-workspace-sync" class="cloud-panel" role="tabpanel" aria-label={$t('Sync activity')} aria-busy={workspaceBusyAction.startsWith('sync-')}>
      <div class="cloud-toolbar">
        <div><strong>{$t('Sync activity')}</strong><small>{$t('Changes are ordered by a monotonic workspace cursor.')}</small></div>
        <button type="button" on:click={() => void loadSyncActivity()} disabled={!!workspaceBusyAction}>{$t('Refresh')}</button>
      </div>
      <div class="checkpoint-summary">
        <div><span>{$t('Device checkpoint')}</span><strong>{syncCheckpoint?.cursor ?? 0}</strong><small>{syncCheckpoint ? `${$t('Updated')} ${formatMinuteDate(syncCheckpoint.updatedAt, $uiLanguage)}` : $t('No checkpoint has been recorded for this device.')}</small></div>
        <div><span>{$t('Latest loaded cursor')}</span><strong>{syncFeed?.nextCursor ?? syncCheckpoint?.cursor ?? 0}</strong><small>{$t('Advancing confirms that this device processed the loaded changes.')}</small></div>
        {#if canWriteSync}
          <button class="primary" type="button" on:click={() => void advanceSyncCheckpoint()} disabled={!!workspaceBusyAction || !syncFeed || syncFeed.nextCursor <= (syncCheckpoint?.cursor ?? 0)}>{$t('Advance checkpoint')}</button>
        {/if}
      </div>
      {#if workspaceBusyAction === 'sync-list' && !syncLoaded}
        <p class="cloud-empty">{$t('Loading sync activity')}…</p>
      {:else if syncLoaded && !syncChanges.length}
        <p class="cloud-empty">{$t('This device is caught up. No changes follow its checkpoint.')}</p>
      {:else}
        <div class="timeline-list">
          {#each syncChanges as change (`${change.cursor}:${change.operationId}`)}
            <article>
              <span class="cursor">{change.cursor}</span>
              <div><strong>{change.changeKind}</strong><small>{change.resourceType} · {change.resourceId} · rv{change.rowVersion}</small><small>{formatMinuteDate(change.occurredAt, $uiLanguage)}</small></div>
            </article>
          {/each}
        </div>
        {#if syncFeed?.hasMore}<button class="load-more" type="button" on:click={() => void loadSyncActivity(true)} disabled={!!workspaceBusyAction}>{$t('Load more')}</button>{/if}
      {/if}
    </div>
  {/if}

  {#if activeView === 'alerts'}
    <div id="team-workspace-alerts" class="cloud-panel" role="tabpanel" aria-label={$t('Alerts')} aria-busy={workspaceBusyAction.startsWith('alerts-') || workspaceBusyAction.startsWith('alert-') || workspaceBusyAction.startsWith('incident-')}>
      <div class="cloud-toolbar">
        <div><strong>{$t('Alerts')}</strong><small>{$t('Rules create incidents; incident state moves only from open to acknowledged to resolved.')}</small></div>
        <div class="toolbar-actions">
          {#if canManageAlerts}<button class="primary" type="button" on:click={beginCreateAlertRule} disabled={!!workspaceBusyAction}>{$t('New alert rule')}</button>{/if}
          <button type="button" on:click={() => void loadAlerts()} disabled={!!workspaceBusyAction}>{$t('Refresh')}</button>
        </div>
      </div>

      {#if workspaceBusyAction === 'alerts-list' && !alertsLoaded}<p class="cloud-empty">{$t('Loading alerts')}…</p>{/if}

      {#if alertFormOpen && canManageAlerts}
        <form class="cloud-form" on:submit|preventDefault={() => void saveAlertRule()} aria-busy={!!workspaceBusyAction}>
          <div class="form-heading"><strong>{$t(editingAlertRule ? 'Edit alert rule' : 'Create alert rule')}</strong><small>{$t('The event kind and severity are validated by the workspace service.')}</small></div>
          <label>{$t('Rule name')}<input bind:value={alertRuleName} maxlength="160" disabled={!!workspaceBusyAction} aria-invalid={!!alertFormError} aria-describedby={alertFormError ? 'alert-form-error' : undefined} /></label>
          <label>{$t('Event kind')}
            <select class="option-align-center" data-team-select="enum" bind:value={alertRuleEventKind} disabled={!!workspaceBusyAction}>
              {#each alertEventKinds as kind}<option value={kind}>{$t(alertEventKindLabel(kind))}</option>{/each}
            </select>
          </label>
          <label>{$t('Severity')}
            <select class="option-align-center" data-team-select="enum" bind:value={alertRuleSeverity} disabled={!!workspaceBusyAction}>
              <option value="info">{$t('Info')}</option>
              <option value="warning">{$t('Warning')}</option>
              <option value="critical">{$t('Critical')}</option>
            </select>
          </label>
          <label class="inline-check form-checkbox"><input type="checkbox" bind:checked={alertRuleEnabled} disabled={!!workspaceBusyAction} /> {$t('Rule enabled')}</label>
          {#if alertFormError}<p id="alert-form-error" class="form-error wide-field" role="alert">{alertFormError}</p>{/if}
          <div class="form-actions wide-field"><button class="primary" type="submit" disabled={!!workspaceBusyAction}>{$t(editingAlertRule ? 'Save rule' : 'Create rule')}</button><button type="button" on:click={() => { alertFormOpen = false; editingAlertRule = null; }} disabled={!!workspaceBusyAction}>{$t('Cancel')}</button></div>
        </form>
      {/if}

      {#if canReadAlerts}
        <section class="cloud-subsection">
          <header><div><h4>{$t('Alert rules')}</h4><p>{$t('Rules are evaluated by the service, not by this client.')}</p></div></header>
          {#if alertsLoaded && !alertRules.length}<p class="cloud-empty">{$t('No alert rules have been created.')}</p>{/if}
          <div class="resource-list">
            {#each alertRules as rule (rule.id)}
              <article>
                <div class="resource-main"><div class="resource-title"><strong>{rule.name}</strong><span class={`status ${rule.severity}`}>{$t(rule.severity === 'critical' ? 'Critical' : rule.severity === 'warning' ? 'Warning' : 'Info')}</span>{#if !rule.enabled}<span class="status">{$t('Disabled')}</span>{/if}</div><small>{$t(alertEventKindLabel(rule.eventKind))} · rv{rule.rowVersion} · {$t('Updated')} {formatMinuteDate(rule.updatedAt, $uiLanguage)}</small></div>
                {#if canManageAlerts}<div class="resource-actions"><button type="button" on:click={() => beginEditAlertRule(rule)} disabled={!!workspaceBusyAction}>{$t('Edit')}</button><button class="danger" type="button" on:click={() => void deleteAlertRule(rule)} disabled={!!workspaceBusyAction}>{$t('Delete')}</button></div>{/if}
              </article>
            {/each}
          </div>
          {#if alertRulesHasMore}<button class="load-more" type="button" on:click={() => void loadAlerts(true, false)} disabled={!!workspaceBusyAction}>{$t('Load more rules')}</button>{/if}
        </section>
      {/if}

      <section class="cloud-subsection incidents-section">
        <header>
          <div><h4>{$t('Incidents')}</h4><p>{$t(canReadAlertHistory ? 'Resolved history is available for this role.' : 'Only current incidents are available for this role.')}</p></div>
          <label class="compact-filter">{$t('Status')}<select class="option-align-center" data-team-select="enum" data-control-size="md" bind:value={incidentStatusFilter} on:change={() => void loadAlerts(false, false)} disabled={!!workspaceBusyAction}><option value="">{$t('All available')}</option><option value="open">{$t('Open')}</option><option value="acknowledged">{$t('Acknowledged')}</option><option value="resolved">{$t('Resolved')}</option></select></label>
        </header>
        {#if alertsLoaded && !alertIncidents.length}<p class="cloud-empty">{$t('No incidents match this filter.')}</p>{/if}
        <div class="incident-list">
          {#each alertIncidents as incident (incident.id)}
            <article>
              <div class="incident-heading"><span class={`status ${incident.severity}`}>{$t(incident.severity === 'critical' ? 'Critical' : incident.severity === 'warning' ? 'Warning' : 'Info')}</span><strong>{incident.summary}</strong><span class="status">{$t(incident.status === 'open' ? 'Open' : incident.status === 'acknowledged' ? 'Acknowledged' : 'Resolved')}</span></div>
              <small>{$t(alertEventKindLabel(incident.eventKind))} · {formatMinuteDate(incident.occurredAt, $uiLanguage)} · rv{incident.rowVersion}</small>
              {#if Object.keys(incident.metadata).length}<dl class="metadata-list">{#each Object.entries(incident.metadata) as [key, value]}<div><dt>{key}</dt><dd>{value}</dd></div>{/each}</dl>{/if}
              <div class="resource-actions">
                {#if canAcknowledgeAlerts && incident.status === 'open'}<button type="button" on:click={() => void acknowledgeIncident(incident)} disabled={!!workspaceBusyAction}>{$t('Acknowledge')}</button>{/if}
                {#if canManageAlerts && incident.status === 'acknowledged'}<button class="primary" type="button" on:click={() => void resolveIncident(incident)} disabled={!!workspaceBusyAction}>{$t('Resolve')}</button>{/if}
              </div>
            </article>
          {/each}
        </div>
        {#if alertIncidentsHasMore}<button class="load-more" type="button" on:click={() => void loadAlerts(false, true)} disabled={!!workspaceBusyAction}>{$t('Load more incidents')}</button>{/if}
      </section>
    </div>
  {/if}

  {#if activeView === 'audit'}
    <div id="team-workspace-audit" class="cloud-panel" role="tabpanel" aria-label={$t('Audit log')} aria-busy={workspaceBusyAction.startsWith('audit-')}>
      <div class="cloud-toolbar audit-toolbar">
        <div><strong>{$t('Audit log')}</strong><small>{$t('Tenant-scoped, redacted events are append-only product evidence.')}</small></div>
        <div class="toolbar-actions audit-toolbar-actions">
          <label>{$t('Event type')}<select class="option-align-start" data-team-select="entity" data-control-size="md" bind:value={auditEventType} disabled={!!workspaceBusyAction}><option value="">{$t('All event types')}</option>{#each auditEventTypes as eventType}<option value={eventType}>{$t(auditEventLabel(eventType))}</option>{/each}</select></label>
          <button type="button" on:click={() => void loadAuditEvents()} disabled={!!workspaceBusyAction}>{$t('Apply filter')}</button>
          {#if canExportAudit}<button type="button" on:click={() => void exportAuditEvents()} disabled={!!workspaceBusyAction || !effectiveAuditExportLimit}>{$t('Export up to')} {effectiveAuditExportLimit.toLocaleString($uiLanguage === 'zh-CN' ? 'zh-CN' : 'en-US')}</button>{/if}
        </div>
      </div>
      <p class="export-status" role="status" aria-live="polite">{auditExportStatus}</p>
      {#if workspaceBusyAction === 'audit-list' && !auditLoaded}<p class="cloud-empty">{$t('Loading audit events')}…</p>{:else if auditLoaded && !auditEvents.length}<p class="cloud-empty">{$t('No audit events match this filter.')}</p>{/if}
      <!-- The bounded audit viewport must be focusable so keyboard users can scroll it. -->
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <div class="audit-list" role="feed" tabindex="0" aria-label={$t('Audit events')}>
        {#each auditEvents as event (event.id)}
          <article>
            <div class="audit-heading"><strong>{$t(auditEventLabel(event.eventType))}</strong><span class={`status ${event.outcome === 'succeeded' ? 'success' : 'danger'}`}>{$t(auditOutcomeLabel(event.outcome))}</span></div>
            <small>{formatMinuteDate(event.occurredAt, $uiLanguage)} · {event.deviceId ?? $t('No device')} {event.reasonCode ? `· ${$t(auditReasonLabel(event.reasonCode))}` : ''}</small>
            <details class="audit-details">
              <summary>{$t('Details')}</summary>
              <dl class="metadata-list">
                <div><dt>{$t('Event code')}</dt><dd><code>{event.eventType}</code></dd></div>
                {#if event.reasonCode}<div><dt>{$t('Reason code')}</dt><dd><code>{event.reasonCode}</code></dd></div>{/if}
                {#each Object.entries(event.metadata) as [key, value]}<div><dt>{$t(auditMetadataLabel(key))}</dt><dd>{value}</dd></div>{/each}
              </dl>
            </details>
          </article>
        {/each}
      </div>
      {#if auditPage?.hasMore}<button class="load-more" type="button" on:click={() => void loadAuditEvents(true)} disabled={!!workspaceBusyAction}>{$t('Load more')}</button>{/if}
    </div>
  {/if}

  {#if activeView === 'webhooks'}
    <div id="team-workspace-webhooks" class="cloud-panel" role="tabpanel" aria-label={$t('Webhooks')} aria-busy={workspaceBusyAction.startsWith('webhook-')}>
      <div class="cloud-toolbar">
        <div><strong>{$t('Webhooks')}</strong><small>{$t(canReadWebhooks ? 'Manage HTTPS event receivers and inspect delivery metadata.' : 'This role can inspect delivery metadata but cannot view endpoint URLs or secrets.')}</small></div>
        <div class="toolbar-actions">
          {#if canManageWebhooks}<button class="primary" type="button" on:click={beginCreateWebhookEndpoint} disabled={!!workspaceBusyAction || !!webhookSecretResult}>{$t('New endpoint')}</button>{/if}
          <button type="button" on:click={() => void loadWebhooks()} disabled={!!workspaceBusyAction}>{$t('Refresh')}</button>
        </div>
      </div>

      {#if workspaceBusyAction === 'webhook-list' && !webhooksLoaded}<p class="cloud-empty">{$t('Loading webhooks')}…</p>{/if}

      {#if webhookSecretResult?.secret}
        <section class="webhook-secret" aria-label={$t('One-time webhook secret')}>
          <div><strong>{$t('One-time webhook secret')}</strong><button type="button" on:click={clearWebhookSecret}>{$t('Dismiss secret')}</button></div>
          <code>{webhookSecretResult.secret}</code>
          <small>{$t('Copy this secret now. It cannot be retrieved after it is dismissed or this view is closed.')}</small>
          <button type="button" on:click={() => void copyWebhookSecret()}>{$t('Copy webhook secret')}</button>
          {#if webhookSecretCopyStatus}<span class:failed={webhookSecretCopyStatus === 'failed'} class="copy-status" role="status" aria-live="polite">{$t(webhookSecretCopyStatus === 'copied' ? 'Webhook secret copied' : 'Unable to copy webhook secret')}</span>{/if}
        </section>
      {/if}

      {#if webhookFormOpen && canManageWebhooks}
        <form class="cloud-form webhook-form" on:submit|preventDefault={() => void saveWebhookEndpoint()} aria-busy={!!workspaceBusyAction}>
          <div class="form-heading"><strong>{$t(editingWebhookEndpoint ? 'Edit webhook endpoint' : 'Create webhook endpoint')}</strong><small>{$t('Only public HTTPS targets are accepted. Redirects and private network destinations are rejected.')}</small></div>
          <label>{$t('Endpoint name')}<input bind:value={webhookName} maxlength="128" disabled={!!workspaceBusyAction} aria-invalid={!!webhookFormError} aria-describedby={webhookFormError ? 'webhook-form-error' : undefined} /></label>
          <label>{$t('HTTPS URL')}<input type="url" bind:value={webhookUrl} maxlength="2048" placeholder="https://events.example.com/camellia" disabled={!!workspaceBusyAction} aria-invalid={!!webhookFormError} aria-describedby={webhookFormError ? 'webhook-form-error' : undefined} /></label>
          <fieldset class="wide-field event-types"><legend>{$t('Event types')}</legend>{#each webhookEventTypes as eventType}<label title={eventType}><input type="checkbox" checked={selectedWebhookEventTypes.includes(eventType)} on:change={(event) => toggleWebhookEventType(eventType, (event.currentTarget as HTMLInputElement).checked)} disabled={!!workspaceBusyAction} /> <span>{$t(webhookEventLabel(eventType))}</span></label>{/each}</fieldset>
          <label class="inline-check form-checkbox"><input type="checkbox" bind:checked={webhookActive} disabled={!!workspaceBusyAction} /> {$t('Endpoint active')}</label>
          {#if webhookFormError}<p id="webhook-form-error" class="form-error wide-field" role="alert">{webhookFormError}</p>{/if}
          <div class="form-actions wide-field"><button class="primary" type="submit" disabled={!!workspaceBusyAction}>{$t(editingWebhookEndpoint ? 'Save endpoint' : 'Create endpoint')}</button><button type="button" on:click={() => { webhookFormOpen = false; editingWebhookEndpoint = null; }} disabled={!!workspaceBusyAction}>{$t('Cancel')}</button></div>
        </form>
      {/if}

      {#if canReadWebhooks}
        <section class="cloud-subsection">
          <header><div><h4>{$t('Endpoints')}</h4><p>{$t('Secrets are never returned by endpoint list operations.')}</p></div></header>
          {#if webhooksLoaded && !webhookEndpoints.length}<p class="cloud-empty">{$t('No webhook endpoints have been configured.')}</p>{/if}
          <div class="resource-list webhook-endpoints">
            {#each webhookEndpoints as endpoint (endpoint.id)}
              <article>
                <div class="resource-main"><div class="resource-title"><strong>{endpoint.name}</strong><span class={`status ${endpoint.active ? 'success' : ''}`}>{$t(endpoint.active ? 'Active' : 'Disabled')}</span></div><code class="endpoint-url">{endpoint.url}</code><small>{$t('Secret version')} {endpoint.secretVersion} · rv{endpoint.rowVersion} · {$t('Updated')} {formatMinuteDate(endpoint.updatedAt, $uiLanguage)}</small><small>{endpoint.eventTypes.map((eventType) => $t(webhookEventLabel(eventType))).join(', ')}</small><details class="event-code-details"><summary>{$t('Event codes')}</summary><code>{endpoint.eventTypes.join(', ')}</code></details></div>
                {#if canManageWebhooks}<div class="resource-actions"><button type="button" on:click={() => beginEditWebhookEndpoint(endpoint)} disabled={!!workspaceBusyAction || !!webhookSecretResult}>{$t('Edit')}</button><button type="button" on:click={() => void rotateWebhookSecret(endpoint)} disabled={!!workspaceBusyAction || !!webhookSecretResult}>{$t('Rotate secret')}</button><button class="danger" type="button" on:click={() => void deleteWebhookEndpoint(endpoint)} disabled={!!workspaceBusyAction}>{$t('Delete')}</button></div>{/if}
              </article>
            {/each}
          </div>
        </section>
      {/if}

      {#if canReadWebhookDeliveries}
        <section class="cloud-subsection deliveries-section">
          <header>
            <div><h4>{$t('Delivery metadata')}</h4><p>{$t('Payloads and signing secrets are not included in this view.')}</p></div>
            {#if canReadWebhooks && webhookEndpoints.length}<label class="compact-filter delivery-filter">{$t('Endpoint')}<select class="option-align-start" data-team-select="entity" data-control-size="md" bind:value={webhookDeliveryEndpointId} on:change={() => void loadWebhooks()} disabled={!!workspaceBusyAction}><option value="">{$t('All endpoints')}</option>{#each webhookEndpoints as endpoint (endpoint.id)}<option value={endpoint.id}>{endpoint.name}</option>{/each}</select></label>{/if}
          </header>
          {#if webhooksLoaded && !webhookDeliveries.length}<p class="cloud-empty">{$t('No webhook deliveries are available.')}</p>{/if}
          <div class="delivery-list">
            {#each webhookDeliveries as delivery (delivery.id)}
              <article>
                <div class="delivery-heading"><strong title={delivery.eventType}>{$t(webhookEventLabel(delivery.eventType))}</strong><span class={`status ${delivery.status === 'delivered' ? 'success' : delivery.status === 'dead' ? 'danger' : delivery.status === 'retry' ? 'warning' : ''}`}>{$t(webhookDeliveryStatusLabel(delivery.status))}</span></div>
                <small>{delivery.endpointId} · {$t('Attempts')} {delivery.attemptCount} · {formatMinuteDate(delivery.createdAt, $uiLanguage)}</small>
                <small>{$t('HTTP status')} {delivery.lastHttpStatus ?? '—'} · {$t('Last error')} {$t(webhookErrorLabel(delivery.lastErrorCategory))} · {$t('Next attempt')} {formatMinuteDate(delivery.nextAttemptAt, $uiLanguage)}</small>
              </article>
            {/each}
          </div>
        </section>
      {/if}
    </div>
  {/if}
</section>

<style>
  .team-workspace-panel {
    --team-control-font-size: var(--ui-font-size-sm);
    --team-control-font-weight: var(--ui-weight-medium);
    --team-list-title-font-weight: var(--ui-weight-semibold);
    min-width: 0;
    overflow: hidden;
    border: 1px solid var(--ui-border-subtle);
    border-radius: var(--ui-radius-lg);
    background: var(--ui-surface-2);
    box-shadow: var(--ui-shadow-xs);
  }
  .team-workspace-panel select[data-team-select] {
    font-size: var(--team-control-font-size);
    font-weight: var(--team-control-font-weight);
    letter-spacing: normal;
  }
  .team-workspace-panel select[data-team-select] option { font-size: var(--team-control-font-size); font-weight: var(--ui-weight-regular); }
  .team-workspace-panel select[data-team-select] option:checked { font-weight: var(--ui-weight-semibold); }
  .team-workspace-panel > header {
    display: flex;
    min-width: 0;
    align-items: center;
    justify-content: space-between;
    gap: var(--ui-gap-md);
    padding: 14px 16px;
    border-bottom: 1px solid var(--ui-divider);
  }
  header h3,
  header p { margin: 0; }
  header p { margin-top: 2px; color: var(--ui-text-secondary); font-size: var(--ui-font-size-sm); }
  .team-error { padding: 12px 16px 0; }
  .team-summary { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 1px; border-bottom: 1px solid var(--ui-divider); background: var(--ui-divider); }
  .team-summary > div { display: grid; min-width: 0; gap: 4px; padding: 13px 16px; background: var(--ui-surface-2); }
  .team-summary span { color: var(--ui-text-secondary); font-size: var(--ui-font-size-xs); }
  .team-summary strong { min-width: 0; overflow-wrap: anywhere; }
  .team-summary-identity { display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .team-linking-actions { display: grid; grid-auto-rows: minmax(120px, 1fr); }
  .team-accept { display: grid; grid-template-columns: minmax(0, 1.2fr) minmax(180px, 1fr) minmax(124px, .42fr); align-items: center; gap: var(--ui-gap-md); padding: 16px; }
  .team-accept > div { display: grid; min-width: 0; align-content: center; gap: 3px; }
  .team-accept small { color: var(--ui-text-secondary); }
  .team-accept > :is(input, button) { width: 100%; min-width: 0; }
  .team-accept > button { height: var(--ui-control-lg); white-space: normal; }
  .team-device-accept { border-top: 1px solid var(--ui-divider); }
  .team-inline-error { grid-column: 1 / -1; }
  .team-device-enrollment { display: grid; gap: var(--ui-gap-md); padding: 16px; border-bottom: 1px solid var(--ui-divider); background: color-mix(in srgb, var(--ui-brand-soft) 12%, var(--ui-surface-1)); }
  .team-device-enrollment-heading { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: var(--ui-gap-md); }
  .team-device-enrollment-heading > div { display: grid; min-width: 0; gap: 3px; }
  .team-device-enrollment-heading small { color: var(--ui-text-secondary); }
  .team-device-enrollment-heading button { white-space: normal; }
  .team-members { display: grid; gap: 7px; padding: 12px 16px; }
  .team-members article { display: grid; grid-template-columns: minmax(180px, 1fr) auto auto minmax(240px, 1.2fr); align-items: center; gap: var(--ui-gap-md); padding: 10px 12px; border: 1px solid var(--ui-divider); border-radius: var(--ui-radius-sm); background: var(--ui-surface-1); }
  .team-members article.readonly-member { grid-template-columns: minmax(180px, 1fr) auto auto auto; }
  .team-member-identity { display: grid; min-width: 0; gap: 2px; }
  .team-member-identity strong { display: -webkit-box; min-width: 0; overflow: hidden; overflow-wrap: anywhere; -webkit-box-orient: vertical; -webkit-line-clamp: 2; line-clamp: 2; }
  .team-member-identity small { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .team-members small { color: var(--ui-text-secondary); }
  .team-member-status,
  .team-member-role { display: inline-flex; width: fit-content; max-width: 100%; min-width: 0; min-height: var(--ui-control-sm); align-items: center; justify-content: center; justify-self: start; padding: 4px 9px; border: 1px solid var(--ui-divider); border-radius: var(--ui-radius-round); background: var(--ui-surface-2); font-size: var(--ui-font-size-xs); font-weight: var(--ui-weight-bold); line-height: var(--ui-line-height-tight); text-align: center; }
  .team-member-status.active { color: var(--ui-success); }
  .team-member-status.invited { color: var(--ui-warning); }
  .team-member-status.suspended { color: var(--ui-danger); }
  .team-member-status.removed { color: var(--ui-text-tertiary); }
  .team-member-device-count { color: var(--ui-text-secondary); font-size: var(--ui-font-size-xs); white-space: nowrap; }
  .team-member-controls { display: grid; width: min(100%, 320px); min-width: 0; grid-template-columns: repeat(2, minmax(0, 1fr)); align-items: stretch; justify-self: end; gap: var(--ui-gap-sm); }
  .team-member-controls select { width: 100%; min-width: 0; grid-column: 1 / -1; }
  .team-member-controls button { width: 100%; min-width: 0; min-height: var(--ui-control-md); white-space: normal; }
  .team-member-controls.invitation-controls { align-items: center; }
  .team-member-pagination { display: flex; align-items: center; justify-content: space-between; gap: var(--ui-gap-md); padding-top: 3px; }
  .team-member-pagination small { color: var(--ui-text-secondary); }
  .team-invite-form { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: var(--ui-gap-md); padding: 16px; border-top: 1px solid var(--ui-divider); background: color-mix(in srgb, var(--ui-brand-soft) 20%, var(--ui-surface-1)); }
  .team-invite-form label { display: grid; gap: 6px; color: var(--ui-text-secondary); font-size: var(--ui-font-size-sm); font-weight: var(--ui-weight-semibold); }
  .team-invite-form :is(input, select) { width: 100%; min-width: 0; }
  .form-heading { display: grid; grid-column: 1 / -1; gap: 2px; }
  .form-heading small { color: var(--ui-text-secondary); }
  .team-invite-submit { align-self: end; justify-self: start; }
  .team-secret { display: grid; grid-column: 1 / -1; gap: 5px; padding: 12px; border: 1px solid color-mix(in srgb, var(--ui-warning) 30%, var(--ui-divider)); border-radius: var(--ui-radius-sm); background: var(--ui-warning-soft); }
  .team-secret-heading { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: var(--ui-gap-sm); }
  .team-secret-heading button { min-height: var(--ui-control-sm); }
  .team-secret-actions { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: var(--ui-gap-xs); }
  .team-secret code { overflow-wrap: anywhere; user-select: all; }
  .copy-status { min-height: 1.2em; color: var(--ui-success); font-size: var(--ui-font-size-sm); }
  .copy-status.failed { color: var(--ui-danger); }
  .team-governance { display: grid; grid-template-columns: minmax(0, 1fr) auto; align-items: end; gap: var(--ui-gap-md); padding: 16px; border-top: 1px solid var(--ui-divider); }
  .team-governance > div { display: grid; min-width: 0; grid-column: 1 / -1; gap: 3px; }
  .team-governance > div small,
  .team-governance-empty { color: var(--ui-text-secondary); }
  .team-governance label { display: grid; min-width: 0; gap: 6px; color: var(--ui-text-secondary); font-size: var(--ui-font-size-sm); font-weight: var(--ui-weight-semibold); }
  .team-governance select { width: 100%; min-width: 0; }
  .team-governance button { white-space: normal; }
  .team-governance-empty { grid-column: 1 / -1; align-self: center; }
  .team-leave-workspace { grid-template-columns: minmax(0, 1fr) auto; background: color-mix(in srgb, var(--ui-danger-soft) 22%, var(--ui-surface-1)); }
  .team-leave-workspace > div { grid-column: auto; }
  .form-error { margin: 0; color: var(--ui-danger); font-size: var(--ui-font-size-sm); }
  .team-form-error { padding: 0 16px 14px; }
  .workspace-view-tabs { display: flex; min-width: 0; gap: 4px; overflow-x: auto; padding: 8px 12px; border-bottom: 1px solid var(--ui-divider); scrollbar-width: thin; }
  .workspace-view-tabs button { flex: 0 0 auto; border-color: transparent; background: transparent; color: var(--ui-text-secondary); }
  .workspace-view-tabs button[aria-selected='true'] { border-color: var(--ui-brand); background: var(--ui-brand-soft); color: var(--ui-text-primary); }
  .workspace-error { display: grid; min-width: 0; gap: var(--ui-gap-sm); margin: 12px 16px 0; border-radius: var(--ui-radius-md); }
  .workspace-error:focus { outline: 3px solid color-mix(in srgb, var(--ui-brand) 48%, transparent); outline-offset: 2px; }
  .workspace-error button { justify-self: start; }
  .workspace-notice { min-height: 1.35em; margin: 6px 16px 0; color: var(--ui-success); font-size: var(--ui-font-size-sm); }
  .cloud-panel { min-width: 0; border-top: 1px solid var(--ui-divider); }
  .cloud-toolbar { display: flex; min-width: 0; align-items: center; justify-content: space-between; gap: var(--ui-gap-md); padding: 14px 16px; background: color-mix(in srgb, var(--ui-brand-soft) 10%, var(--ui-surface-2)); }
  .cloud-toolbar > div:first-child { display: grid; min-width: 0; gap: 3px; }
  .shared-toolbar { display: grid; grid-template-columns: minmax(0, 1fr); align-items: stretch; }
  .shared-toolbar-heading { display: flex !important; min-width: 0; align-items: center; justify-content: space-between; gap: var(--ui-gap-md); }
  .shared-toolbar-copy { display: grid; min-width: 0; gap: 3px; }
  .toolbar-actions.shared-toolbar-actions { width: auto; flex: 0 0 auto; align-items: center; justify-content: flex-end; }
  .shared-toolbar-actions > button { width: auto; min-width: 112px; min-height: var(--ui-control-md); }
  .shared-toolbar-context { display: flex !important; min-width: 0; min-height: var(--ui-control-md); align-items: center; justify-content: space-between; gap: var(--ui-gap-md); padding-top: 10px; border-top: 1px solid var(--ui-divider); }
  .shared-toolbar-context.stats-only { justify-content: flex-end; }
  .shared-filter-toggle { flex: 0 0 auto; color: var(--ui-text-primary); font-weight: var(--ui-weight-medium); }
  .shared-usage-summary { display: flex; min-width: 0; flex-wrap: wrap; justify-content: flex-end; gap: var(--ui-gap-lg); color: var(--ui-text-secondary); font-size: var(--ui-font-size-sm); }
  .shared-usage-summary strong { color: var(--ui-text-primary); }
  .audit-toolbar { display: grid; grid-template-columns: minmax(0, 1fr); }
  .audit-toolbar-actions { width: 100%; justify-content: center; }
  .audit-toolbar-actions label { flex: 1 1 240px; max-width: 360px; }
  .audit-toolbar-actions button { width: 180px; min-width: 180px; min-height: var(--ui-control-lg); }
  .cloud-toolbar small,
  .cloud-subsection header p,
  .resource-main small,
  .incident-list article > small,
  .audit-list article > small,
  .delivery-list article > small,
  .checkpoint-summary small { color: var(--ui-text-secondary); }
  .toolbar-actions { display: flex; min-width: 0; flex-wrap: wrap; align-items: end; justify-content: flex-end; gap: var(--ui-gap-sm); }
  .toolbar-actions label { display: grid; min-width: 160px; gap: 4px; color: var(--ui-text-secondary); font-size: var(--ui-font-size-xs); }
  .toolbar-actions > button { min-width: 112px; }
  .inline-check { display: flex !important; width: auto !important; min-width: 0 !important; flex-direction: row !important; align-items: center; justify-content: flex-start; gap: 6px; color: var(--ui-text-secondary); font-size: var(--ui-font-size-sm); }
  .inline-check input { width: auto !important; }
  .cloud-form { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: var(--ui-gap-md); padding: 16px; border-block: 1px solid var(--ui-divider); background: color-mix(in srgb, var(--ui-brand-soft) 18%, var(--ui-surface-1)); }
  .cloud-form > label { display: grid; min-width: 0; gap: 6px; color: var(--ui-text-secondary); font-size: var(--ui-font-size-sm); font-weight: var(--ui-weight-semibold); }
  .cloud-form :is(input, select, textarea) { width: 100%; min-width: 0; }
  .wide-field { grid-column: 1 / -1; }
  .form-checkbox { align-self: end; padding-block: 9px; }
  .form-actions { display: flex; flex-wrap: wrap; gap: var(--ui-gap-sm); }
  .code-input,
  .content-preview pre { font-family: var(--ui-font-mono); font-size: var(--ui-font-size-sm); line-height: 1.55; tab-size: 2; }
  .cloud-empty { margin: 0; padding: 24px 16px; color: var(--ui-text-secondary); text-align: center; }
  .resource-list,
  .incident-list,
  .audit-list,
  .delivery-list,
  .timeline-list { display: grid; gap: 8px; padding: 12px 16px; }
  .resource-list > article { display: grid; min-width: 0; grid-template-columns: minmax(0, 1fr) auto; align-items: center; gap: var(--ui-gap-md); padding: 12px; border: 1px solid var(--ui-divider); border-radius: var(--ui-radius-sm); background: var(--ui-surface-1); }
  .resource-list > article.deleted { opacity: .78; }
  .resource-main { display: grid; min-width: 0; gap: 4px; }
  .resource-title,
  .audit-heading,
  .delivery-heading,
  .incident-heading { display: flex; min-width: 0; flex-wrap: wrap; align-items: center; gap: var(--ui-gap-sm); }
  :is(.resource-title, .audit-heading, .delivery-heading, .incident-heading) > strong { min-width: 0; overflow-wrap: anywhere; font-size: var(--ui-font-size-sm); font-weight: var(--team-list-title-font-weight); }
  .resource-title > span:not(.status) { color: var(--ui-text-tertiary); font-size: var(--ui-font-size-xs); }
  .resource-actions { display: flex; min-width: 0; flex-wrap: wrap; justify-content: flex-end; gap: 6px; }
  .resource-actions button { min-height: var(--ui-control-sm); white-space: normal; }
  .hash,
  .endpoint-url { max-width: 100%; overflow-wrap: anywhere; color: var(--ui-text-secondary); font-size: var(--ui-font-size-xs); }
  .status { color: var(--ui-text-tertiary); font-size: var(--ui-font-size-xs); font-weight: var(--ui-weight-bold); text-transform: capitalize; }
  .status.success { color: var(--ui-success); }
  .status.info { color: var(--ui-brand); }
  .status.warning { color: var(--ui-warning); }
  .status.critical,
  .status.danger { color: var(--ui-danger); }
  .load-more { display: flex; margin: 0 auto 14px; }
  .content-preview { display: grid; gap: var(--ui-gap-sm); margin: 0 16px 16px; padding: 12px; border: 1px solid var(--ui-divider); border-radius: var(--ui-radius-sm); background: var(--ui-surface-1); }
  .content-preview > div { display: flex; min-width: 0; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: var(--ui-gap-sm); }
  .content-preview pre { max-height: 360px; margin: 0; overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; }
  .checkpoint-summary { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)) auto; align-items: center; gap: var(--ui-gap-md); padding: 14px 16px; border-block: 1px solid var(--ui-divider); }
  .checkpoint-summary > div { display: grid; min-width: 0; gap: 3px; }
  .checkpoint-summary span { color: var(--ui-text-secondary); font-size: var(--ui-font-size-xs); }
  .timeline-list article { display: grid; grid-template-columns: auto minmax(0, 1fr); gap: var(--ui-gap-md); padding: 10px 12px; border: 1px solid var(--ui-divider); border-radius: var(--ui-radius-sm); background: var(--ui-surface-1); }
  .timeline-list .cursor { display: grid; min-width: 42px; place-items: center; border-radius: var(--ui-radius-sm); background: var(--ui-brand-soft); color: var(--ui-brand); font-family: var(--ui-font-mono); font-weight: var(--ui-weight-bold); }
  .timeline-list article > div { display: grid; min-width: 0; gap: 2px; }
  .timeline-list small { overflow-wrap: anywhere; color: var(--ui-text-secondary); }
  .cloud-subsection { min-width: 0; border-top: 1px solid var(--ui-divider); }
  .cloud-subsection > header { display: flex; min-width: 0; align-items: end; justify-content: space-between; gap: var(--ui-gap-md); padding: 12px 16px 0; }
  .cloud-subsection header h4,
  .cloud-subsection header p { margin: 0; }
  .cloud-subsection header p { margin-top: 2px; font-size: var(--ui-font-size-sm); }
  .cloud-subsection header label { display: grid; min-width: 150px; gap: 4px; color: var(--ui-text-secondary); font-size: var(--ui-font-size-xs); }
  .cloud-subsection header .compact-filter { width: clamp(180px, 22vw, 220px); min-width: 0; justify-items: center; text-align: center; }
  .incident-list article,
  .delivery-list article { display: grid; min-width: 0; gap: 5px; padding: 12px; border: 1px solid var(--ui-divider); border-radius: var(--ui-radius-sm); background: var(--ui-surface-1); }
  .audit-list { max-height: min(480px, 56vh); gap: 0; overflow: auto; overscroll-behavior: contain; scrollbar-gutter: stable; }
  .audit-list article { display: grid; min-width: 0; gap: 4px; padding: 9px 10px; border-bottom: 1px solid var(--ui-divider); background: var(--ui-surface-1); }
  .audit-list article:last-child { border-bottom: 0; }
  .audit-heading strong { flex: 1; color: var(--ui-text-primary); }
  .audit-details summary { width: fit-content; cursor: pointer; color: var(--ui-text-link); font-size: var(--ui-font-size-xs); }
  .audit-details .metadata-list { margin-top: 6px; }
  .audit-details code,
  .event-code-details code { overflow-wrap: anywhere; color: var(--ui-text-secondary); font-size: var(--ui-font-size-xs); }
  .incident-heading strong,
  .delivery-heading strong { flex: 1; }
  .metadata-list { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 3px 12px; margin: 3px 0 0; padding-top: 7px; border-top: 1px solid var(--ui-divider); font-size: var(--ui-font-size-xs); }
  .metadata-list div { display: grid; min-width: 0; grid-template-columns: minmax(80px, .45fr) minmax(0, 1fr); gap: 6px; }
  .metadata-list dt { color: var(--ui-text-tertiary); }
  .metadata-list dd { margin: 0; overflow-wrap: anywhere; }
  .export-status { min-height: 1.35em; margin: 0; padding: 6px 16px 0; color: var(--ui-success); font-size: var(--ui-font-size-sm); }
  .event-types { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(220px, 100%), 1fr)); gap: 6px 12px; min-width: 0; margin: 0; padding: 10px 12px; border: 1px solid var(--ui-divider); border-radius: var(--ui-radius-sm); }
  .event-types legend { padding-inline: 4px; color: var(--ui-text-secondary); font-size: var(--ui-font-size-sm); font-weight: var(--ui-weight-semibold); }
  .event-types label { display: grid; min-width: 0; min-height: 34px; grid-template-columns: 17px minmax(0, 1fr); align-items: center; gap: 8px; padding: 6px 8px; border-radius: var(--ui-radius-sm); font-size: var(--ui-font-size-xs); line-height: var(--ui-line-height-body); cursor: pointer; }
  .event-types label:hover { background: var(--ui-state-hover); }
  .event-types label:has(input:focus-visible) { background: var(--ui-state-hover); }
  .event-types input { width: 17px !important; min-width: 17px; min-height: 17px; block-size: 17px; margin: 0; align-self: center; }
  .event-types span { min-width: 0; overflow-wrap: anywhere; }
  .event-types label:has(input:disabled) { cursor: default; opacity: .64; }
  .event-code-details summary { width: fit-content; cursor: pointer; color: var(--ui-text-link); font-size: var(--ui-font-size-xs); }
  .webhook-secret { display: grid; gap: 7px; margin: 12px 16px; padding: 12px; border: 1px solid color-mix(in srgb, var(--ui-warning) 40%, var(--ui-divider)); border-radius: var(--ui-radius-sm); background: var(--ui-warning-soft); }
  .webhook-secret > div { display: flex; min-width: 0; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: var(--ui-gap-sm); }
  .webhook-secret code { overflow-wrap: anywhere; user-select: all; }
  .webhook-secret > button { justify-self: start; }

  :is(button, input, select, textarea):focus-visible { outline: 3px solid color-mix(in srgb, var(--ui-brand) 48%, transparent); outline-offset: 2px; }

  @media (prefers-reduced-motion: reduce) {
    .workspace-view-tabs { scroll-behavior: auto; }
  }

  @media (max-width: 760px) {
    .team-members article,
    .team-members article.readonly-member { grid-template-columns: minmax(0, 1fr) auto; align-items: start; }
    .team-member-controls { width: min(100%, 480px); grid-column: 1 / -1; justify-self: start; }
    .team-member-role { justify-self: start; }
  }

  @media (max-width: 640px) {
    .team-workspace-panel > header { align-items: flex-start; flex-direction: column; }
    .team-workspace-panel > header button { width: 100%; }
    .team-summary,
    .team-accept,
    .team-invite-form,
    .team-members article,
    .team-members article.readonly-member,
    .team-governance,
    .team-leave-workspace { grid-template-columns: minmax(0, 1fr); align-items: stretch; }
    .team-linking-actions { grid-auto-rows: auto; }
    .team-members article { align-items: start; }
    .team-member-controls { justify-self: stretch; }
    .team-governance-empty { grid-column: auto; }
    .cloud-toolbar,
    .cloud-subsection > header { align-items: stretch; flex-direction: column; }
    .toolbar-actions { align-items: stretch; justify-content: flex-start; }
    .toolbar-actions > :is(button, label) { width: 100%; }
    .shared-toolbar-heading { align-items: stretch; flex-direction: column; }
    .toolbar-actions.shared-toolbar-actions { display: grid; width: 100%; grid-template-columns: repeat(auto-fit, minmax(min(132px, 100%), 1fr)); justify-content: stretch; }
    .shared-toolbar-actions > button { width: 100%; min-width: 0; }
    .shared-toolbar-context,
    .shared-toolbar-context.stats-only { align-items: flex-start; flex-direction: column; justify-content: flex-start; }
    .shared-usage-summary { justify-content: flex-start; }
    .toolbar-actions > button,
    .audit-toolbar-actions button { min-width: 0; }
    .audit-toolbar-actions label { max-width: none; }
    .cloud-form,
    .checkpoint-summary,
    .resource-list > article,
    .metadata-list,
    .event-types { grid-template-columns: minmax(0, 1fr); }
    .wide-field { grid-column: auto; }
    .resource-actions { justify-content: flex-start; }
    .resource-actions button { flex: 1 1 130px; }
    .cloud-subsection header label,
    .cloud-subsection header .compact-filter { width: 100%; max-width: none; }
    .workspace-error,
    .content-preview,
    .webhook-secret { margin-inline: 12px; }
    .cloud-toolbar,
    .cloud-form,
    .resource-list,
    .incident-list,
    .audit-list,
    .delivery-list,
    .timeline-list,
    .checkpoint-summary { padding-inline: 12px; }
  }
</style>

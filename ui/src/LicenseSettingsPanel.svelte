<script lang="ts">
  import { onMount } from 'svelte';
  import ErrorNotice from './ErrorNotice.svelte';
  import TeamWorkspacePanel from './TeamWorkspacePanel.svelte';
  import { compactPaymentReference, formatBillingAmount } from './billingPresentation';
  import {
    isTransientErrorInfo,
    publicErrorInfo,
    sameUserFacingError,
    TRANSIENT_ERROR_DISMISS_MS,
  } from './errors';
  import { t, uiLanguage, type UiLanguage } from './i18n';
  import {
    clientVersionAdvisory,
    clientVersionNotice,
    hasRefreshableLicenseSession,
    signedLicenseStatusPresentation,
  } from './license';
  import type {
    BillingInvoice,
    CustomerPaymentSubmission,
    CreateTeamInvitation,
    EntitlementState,
    LicenseAuthorizationRequest,
    LocalLicenseDevice,
    LicenseServiceSettings,
    LicenseBillingSummary,
    ManualPaymentClaim,
    MemberDeviceEnrollment,
    LeaveWorkspace,
    Plan,
    TeamInvitation,
    TeamProfile,
    TransferWorkspaceOwnership,
    WorkspaceMember,
    UpdateWorkspaceMember,
    RegisteredLicenseDevice,
  } from './types';
  import type { ErrorInfo } from './api';

  export let entitlementState: EntitlementState | null = null;
  export let appVersion = '';
  export let serviceSettings: LicenseServiceSettings | null = null;
  export let authorizationRequest: LicenseAuthorizationRequest | null = null;
  export let localDevice: LocalLicenseDevice | null = null;
  export let devices: RegisteredLicenseDevice[] = [];
  export let billingSummary: LicenseBillingSummary | null = null;
  export let billingError: ErrorInfo | null = null;
  export let billingLoading = false;
  export let billingLastUpdatedAt = 0;
  export let dataSyncing = false;
  export let lastSyncedAt = 0;
  export let teamProfile: TeamProfile | null = null;
  export let teamMembers: WorkspaceMember[] = [];
  export let teamMembersHasMore = false;
  export let teamMembersLoadingMore = false;
  export let teamInvitation: TeamInvitation | null = null;
  export let teamDeviceEnrollment: MemberDeviceEnrollment | null = null;
  export let teamSecretGeneration = 0;
  export let teamError: ErrorInfo | null = null;
  export let hasMoreDevices = false;
  export let error: ErrorInfo | null = null;
  export let busy = false;
  export let busyAction = '';
  export let displayName = '';
  export let onBeginAuthorization: () => void;
  export let onRefresh: () => void;
  export let onReconnect: () => void;
  export let onLoadDevices: () => void;
  export let onLoadMoreDevices: () => void;
  export let onLoadBilling: () => void;
  export let onSubmitPayment: (submission: CustomerPaymentSubmission) => void;
  export let onLoadTeam: () => void;
  export let onLoadMoreTeamMembers: () => void;
  export let onCreateTeamInvitation: (request: CreateTeamInvitation) => Promise<void>;
  export let onDismissTeamInvitation: () => void;
  export let onAcceptTeamInvitation: (token: string, operationId: string) => Promise<void>;
  export let onUpdateTeamMember: (
    memberId: string,
    request: UpdateWorkspaceMember,
  ) => Promise<void>;
  export let onCreateTeamDeviceEnrollment: (operationId: string) => Promise<void>;
  export let onCreateTeamMemberDeviceEnrollment: (
    memberId: string,
    operationId: string,
  ) => Promise<void>;
  export let onDismissTeamDeviceEnrollment: () => void;
  export let onAcceptTeamDeviceEnrollment: (
    token: string,
    operationId: string,
  ) => Promise<void>;
  export let onLeaveTeamWorkspace: (request: LeaveWorkspace) => Promise<void>;
  export let onTransferTeamOwnership: (
    request: TransferWorkspaceOwnership,
  ) => Promise<void>;
  export let onConfirmTeamAction: (
    title: string,
    message: string,
    confirmLabel: string,
    danger?: boolean,
  ) => Promise<boolean>;
  export let onRemoveDevice: (deviceId: string) => void;
  export let onCancelAuthorization: () => void;
  export let onLogout: () => void;
  export let onUseAnotherLicense: () => void;
  export let onDismissError: () => void;
  export let onDismissBillingError: () => void;
  export let onDismissTeamError: () => void;

  $: activeEntitlement = entitlementState && 'entitlement' in entitlementState
    ? entitlementState.entitlement
    : null;
  $: clientVersionPolicy = entitlementState?.status === 'clientUpgradeRequired'
    ? entitlementState.policy
    : activeEntitlement?.claims.clientVersionPolicy;
  $: versionAdvisory = clientVersionAdvisory(entitlementState, appVersion);
  $: versionNotice = clientVersionNotice(entitlementState, appVersion);
  $: plan = activeEntitlement?.claims.plan;
  $: signedLicenseStatus = activeEntitlement
    ? signedLicenseStatusPresentation(activeEntitlement.claims.licenseStatus)
    : null;
  $: maxPrograms = activeEntitlement?.claims.limits.max_programs;
  $: maxSources = activeEntitlement?.claims.limits.max_config_sources_per_program;
  $: issuedAt = activeEntitlement ? formatMinuteDate(activeEntitlement.claims.iat, $uiLanguage) : '';
  $: licenseExpiresAt = activeEntitlement
    ? (activeEntitlement.claims.licenseExpiresAt
      ? formatMinuteDate(activeEntitlement.claims.licenseExpiresAt, $uiLanguage)
      : $t('No fixed expiry'))
    : '';
  $: leaseValidUntil = activeEntitlement ? formatMinuteDate(activeEntitlement.claims.exp, $uiLanguage) : '';
  $: offlineAccessUntil = activeEntitlement
    ? formatMinuteDate(activeEntitlement.claims.offlineAccessEndsAt, $uiLanguage)
    : '';
  $: currentDeviceId = activeEntitlement?.claims.deviceId ?? localDevice?.deviceId ?? '';
  $: currentDevice = currentDeviceId
    ? devices.find((device) => device.deviceId === currentDeviceId)
    : undefined;
  $: currentDeviceStatus = entitlementState?.status === 'deviceDenied'
    ? entitlementState.state
    : currentDevice?.state ?? (activeEntitlement ? 'active' : localDevice ? 'local' : '');
  $: currentDeviceName = currentDevice?.displayName
    || localDevice?.displayName
    || localDevice?.platform
    || currentDevice?.platform
    || $t(activeEntitlement ? 'This device' : 'Not available');
  $: registeredDeviceSummary = devices.length
    ? hasMoreDevices
      ? `${devices.length}+ ${$t('loaded')}`
      : `${devices.filter((device) => device.state === 'active').length}/${devices.length} ${$t('active')}`
    : $t('Not loaded');
  $: publicError = publicErrorInfo(error);
  $: publicBillingError = publicErrorInfo(billingError);
  $: publicTeamError = publicErrorInfo(teamError);
  $: visibleBillingError = sameUserFacingError(publicError, publicBillingError)
    ? null
    : publicBillingError;
  $: visibleTeamError = sameUserFacingError(publicError, publicTeamError)
    || sameUserFacingError(visibleBillingError, publicTeamError)
    ? null
    : publicTeamError;
  $: statusLabel = entitlementState ? licenseStatusLabel(entitlementState) : 'Loading';
  $: statusTone = entitlementState ? licenseStatusTone(entitlementState) : 'neutral';
  $: hasLicenseSession = hasRefreshableLicenseSession(entitlementState);
  $: canLoadDevices = hasLicenseSession
    && entitlementState?.status !== 'sessionOnly'
    && entitlementState?.status !== 'activationPending'
    && entitlementState?.status !== 'deviceDenied'
    && entitlementState?.status !== 'licenseInactive'
    && entitlementState?.status !== 'revalidationRequired'
    && entitlementState?.status !== 'clientUpgradeRequired';
  $: canBeginAuthorization = !busy
    && !!serviceSettings?.authorizationConfigured
    && !authorizationRequest
    && entitlementState?.status !== 'sessionOnly'
    && !hasLicenseSession
    && (entitlementState?.status !== 'deviceDenied' || entitlementState.state === 'removed');
  $: canCancelAuthorization = !!authorizationRequest && !busy;
  $: canRefreshStatus = !busy && !!entitlementState;
  $: canReconnect = !busy
    && !!serviceSettings?.configured
    && !!localDevice
    && entitlementState?.status === 'unauthenticated';
  $: refreshLabel = hasLicenseSession ? 'Refresh entitlement' : 'Refresh status';
  $: openInvoices = billingSummary?.invoices.filter((invoice) => {
    if (invoice.status !== 'open') return false;
    const claim = latestClaim(invoice.id);
    return !claim || claim.status === 'rejected' || claim.status === 'withdrawn' || claim.status === 'needs_information';
  }) ?? [];
  $: if (openInvoices.length && !openInvoices.some((invoice) => invoice.id === paymentInvoiceId)) {
    paymentInvoiceId = openInvoices[0].id;
  }
  $: selectedInvoice = openInvoices.find((invoice) => invoice.id === paymentInvoiceId);
  $: selectedPaymentClaim = selectedInvoice ? latestClaim(selectedInvoice.id) : undefined;
  $: paymentNeedsInformation = selectedPaymentClaim?.status === 'needs_information';
  $: compatiblePaymentMethods = selectedInvoice
    ? billingSummary?.paymentMethods.filter((method) => method.settlementAsset === selectedInvoice.currency) ?? []
    : [];
  $: if (compatiblePaymentMethods.length && !compatiblePaymentMethods.some((method) => method.id === paymentMethodId)) {
    paymentMethodId = compatiblePaymentMethods[0].id;
  }
  $: billingAccessDenied = !!teamProfile?.enabled && !teamProfile.permissions.includes('billing.read');
  $: canSubmitPayments = plan !== 'team' || !!teamProfile?.permissions.includes('billing.manage');
  $: heroMessage = entitlementState?.status === 'active'
    ? 'This device is licensed and ready to use.'
    : entitlementState?.status === 'restrictedOffline'
      ? 'This device is using verified offline access.'
      : authorizationRequest
        ? 'Complete authorization in your browser.'
        : entitlementState?.status === 'deviceDenied' && entitlementState.state === 'removed'
          ? 'Reactivate this removed device with a code for its existing license, or replace its identity before using another license.'
        : localDevice
          ? 'Reconnect this registered device, or replace its identity before using another license.'
          : 'Activate this device with a valid license.';
  let paymentInvoiceId = '';
  let paymentMethodId = '';
  let paymentTransactionId = '';
  let paymentPaidAtInput = localDateTimeInput(new Date());
  let paymentPayerName = '';
  let paymentNote = '';
  let paymentFormError = '';
  let paymentOperationId = '';
  let paymentOperationFingerprint = '';
  let paymentPaidAt = 0;
  let paymentFormContextKey = '';
  let syncClock = Date.now();

  function planLabel(value: Plan) {
    switch (value) {
      case 'free': return 'Free';
      case 'pro': return 'Pro';
      case 'team': return 'Team';
    }
  }

  function transientDismissDelay(errorInfo: ErrorInfo | null) {
    return isTransientErrorInfo(errorInfo) ? TRANSIENT_ERROR_DISMISS_MS : 0;
  }

  function dismissPrimaryError() {
    if (sameUserFacingError(publicError, publicBillingError)) onDismissBillingError();
    if (sameUserFacingError(publicError, publicTeamError)) onDismissTeamError();
    onDismissError();
  }

  onMount(() => {
    const timer = window.setInterval(() => (syncClock = Date.now()), 10_000);
    return () => window.clearInterval(timer);
  });

  $: {
    const claim = paymentNeedsInformation ? selectedPaymentClaim : undefined;
    const contextKey = selectedInvoice
      ? `${selectedInvoice.id}:${claim?.id ?? 'new'}:${claim?.updatedAt ?? 0}`
      : '';
    if (contextKey !== paymentFormContextKey) {
      paymentFormContextKey = contextKey;
      if (contextKey) populatePaymentForm(claim);
    }
  }

  function licenseStatusLabel(state: EntitlementState) {
    switch (state.status) {
      case 'active': return 'Licensed';
      case 'restrictedOffline': return 'Offline access';
      case 'expired': return 'Offline credential expired';
      case 'sessionOnly': return 'Session only';
      case 'activationPending': return 'Completing activation';
      case 'revalidationRequired': return 'Revalidation required';
      case 'clientUpgradeRequired': return 'Client update required';
      case 'deviceDenied': return 'Device disabled';
      case 'licenseInactive': return licenseInactiveLabel(state.reason);
      default: return 'Not activated';
    }
  }

  function licenseStatusTone(state: EntitlementState) {
    switch (state.status) {
      case 'active': return 'success';
      case 'restrictedOffline': return 'warning';
      case 'activationPending': return 'warning';
      case 'expired':
      case 'revalidationRequired':
      case 'clientUpgradeRequired':
      case 'deviceDenied':
      case 'licenseInactive':
        return 'danger';
      default:
        return 'neutral';
    }
  }

  function licenseInactiveLabel(reason: Extract<EntitlementState, { status: 'licenseInactive' }>['reason']) {
    switch (reason) {
      case 'account_suspended': return 'Account suspended';
      case 'account_denylisted': return 'Account disabled';
      case 'license_past_due': return 'Payment past due';
      case 'license_canceled': return 'License canceled';
      case 'license_expired': return 'License expired';
      case 'license_unavailable': return 'License unavailable';
    }
  }

  function formatMinuteDate(seconds: number, language: UiLanguage) {
    return new Date(seconds * 1000).toLocaleString(language === 'zh-CN' ? 'zh-CN' : 'en-US', {
      year: 'numeric',
      month: 'short',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      hour12: language !== 'zh-CN',
    });
  }

  function relativeSyncAge(milliseconds: number, language: UiLanguage) {
    const elapsedSeconds = Math.max(0, Math.floor((syncClock - milliseconds) / 1_000));
    if (elapsedSeconds < 5) return language === 'zh-CN' ? '刚刚' : 'just now';
    if (elapsedSeconds < 60) {
      return language === 'zh-CN'
        ? `${elapsedSeconds} 秒前`
        : `${elapsedSeconds} seconds ago`;
    }
    const elapsedMinutes = Math.floor(elapsedSeconds / 60);
    if (elapsedMinutes < 60) {
      return language === 'zh-CN'
        ? `${elapsedMinutes} 分钟前`
        : `${elapsedMinutes} minute${elapsedMinutes === 1 ? '' : 's'} ago`;
    }
    return formatMinuteDate(Math.floor(milliseconds / 1_000), language);
  }

  function shortDeviceId(value: string) {
    return value.length > 18 ? `${value.slice(0, 8)}…${value.slice(-6)}` : value;
  }

  function deviceStateLabel(state: string) {
    switch (state) {
      case 'active': return 'Active';
      case 'pending_activation': return 'Pending activation';
      case 'removed': return 'Removed';
      case 'revoked': return 'Revoked';
      case 'suspicious': return 'Suspicious';
      case 'local': return 'Registered locally';
      default: return 'Unknown';
    }
  }

  function deviceCanBeRemoved(state: string) {
    return state === 'active' || state === 'pending_activation';
  }

  function invoiceStatusLabel(status: BillingInvoice['status']) {
    switch (status) {
      case 'open': return 'Awaiting payment';
      case 'paid': return 'Paid';
      case 'overdue': return 'Payment overdue';
      case 'void': return 'Invoice void';
      case 'refunded': return 'Refunded';
    }
  }

  function claimStatusLabel(status: string) {
    switch (status) {
      case 'submitted': return 'Evidence submitted';
      case 'under_review': return 'Payment under review';
      case 'needs_information': return 'More information required';
      case 'verified': return 'Payment verified';
      case 'rejected': return 'Payment rejected';
      case 'withdrawn': return 'Submission withdrawn';
      default: return 'Unknown';
    }
  }

  function billingTone(status: string) {
    if (status === 'paid' || status === 'verified') return 'success';
    if (status === 'open' || status === 'submitted' || status === 'under_review' || status === 'needs_information') return 'warning';
    if (status === 'void' || status === 'withdrawn') return 'neutral';
    return 'danger';
  }

  function latestClaim(invoiceId: string) {
    return billingSummary?.paymentClaims.find((claim) => claim.invoiceId === invoiceId);
  }

  function localizedPaymentMethod(method: NonNullable<LicenseBillingSummary>['paymentMethods'][number]) {
    return $uiLanguage === 'zh-CN'
      ? { name: method.nameZh, instructions: method.instructionsZh }
      : { name: method.nameEn, instructions: method.instructionsEn };
  }

  function populatePaymentForm(claim?: ManualPaymentClaim) {
    paymentMethodId = claim?.paymentMethodId ?? '';
    paymentTransactionId = claim?.externalTransactionId ?? '';
    paymentPaidAtInput = localDateTimeInput(new Date(claim ? claim.paidAt * 1000 : Date.now()));
    paymentPayerName = claim?.payerName ?? '';
    paymentNote = claim?.note ?? '';
    paymentFormError = '';
    paymentOperationId = '';
    paymentOperationFingerprint = '';
    paymentPaidAt = 0;
  }

  function submitPayment() {
    paymentFormError = '';
    const parsedPaidAt = Math.floor(new Date(paymentPaidAtInput).getTime() / 1000);
    if (
      !selectedInvoice
      || !paymentMethodId
      || !paymentTransactionId.trim()
      || !Number.isSafeInteger(parsedPaidAt)
      || parsedPaidAt <= 0
    ) {
      paymentFormError = $t('Complete the required payment details.');
      return;
    }
    if (parsedPaidAt > Math.floor(Date.now() / 1000) + 300) {
      paymentFormError = $t('Payment completion time cannot be in the future.');
      return;
    }
    const fingerprint = JSON.stringify([
      selectedInvoice.id,
      paymentMethodId,
      paymentTransactionId.trim(),
      selectedInvoice.amountDue,
      selectedInvoice.currency,
      paymentPaidAtInput,
      paymentPayerName.trim(),
      paymentNote.trim(),
    ]);
    if (fingerprint !== paymentOperationFingerprint) {
      paymentOperationFingerprint = fingerprint;
      paymentOperationId = crypto.randomUUID();
      paymentPaidAt = parsedPaidAt;
    }
    onSubmitPayment({
      operationId: paymentOperationId,
      invoiceId: selectedInvoice.id,
      paymentMethodId,
      externalTransactionId: paymentTransactionId.trim(),
      paidAmount: selectedInvoice.amountDue,
      paidAsset: selectedInvoice.currency,
      paidAt: paymentPaidAt,
      payerName: paymentPayerName.trim() || null,
      note: paymentNote.trim() || null,
    });
  }

  function localDateTimeInput(value: Date) {
    const local = new Date(value.getTime() - value.getTimezoneOffset() * 60_000);
    return local.toISOString().slice(0, 16);
  }

</script>

<div class="license-panel">
  <div class="license-hero">
    <div class={`license-orb ${statusTone}`} aria-hidden="true">
      <svg viewBox="0 0 64 64" focusable="false">
        <path class="license-shield" d="M32 6.5 50 14v14.2c0 12.2-7.2 22.1-18 28.6-10.8-6.5-18-16.4-18-28.6V14L32 6.5Z" />
        <path class="license-shield-inset" d="M32 12.2 44.5 17v11.1c0 8.8-4.8 16.2-12.5 21.5-7.7-5.3-12.5-12.7-12.5-21.5V17L32 12.2Z" />
        {#if statusTone === 'success'}
          <path class="license-status-mark" d="m24.2 31.7 5.2 5.1 10.7-11" />
        {:else if statusTone === 'warning'}
          <circle class="license-status-mark" cx="32" cy="31.5" r="8.2" />
          <path class="license-status-mark" d="M32 26.9v5.2l3.4 2" />
        {:else if statusTone === 'danger'}
          <path class="license-status-mark" d="M32 24.5v10" />
          <circle class="license-status-fill" cx="32" cy="39.5" r="1.9" />
        {:else}
          <circle class="license-status-fill" cx="32" cy="29.5" r="4.2" />
          <path class="license-status-mark" d="M32 33.7v6.1" />
        {/if}
      </svg>
    </div>
    <div class="license-identity">
      <span class={`license-status ${statusTone}`}>{$t(statusLabel)}</span>
      <h3>{$t('Camellia Nexus License')}</h3>
      <p>{$t(heroMessage)}</p>
    </div>
    <div class="license-actions">
      {#if canBeginAuthorization || authorizationRequest}<button class="primary" type="button" on:click={onBeginAuthorization} disabled={!canBeginAuthorization}>{busy ? `${$t('Working')}…` : authorizationRequest ? $t('Activation pending') : $t('Activate device')}</button>{/if}
      {#if canReconnect}<button type="button" on:click={onReconnect}>{$t('Reconnect registered device')}</button>{/if}
      <button type="button" on:click={onRefresh} disabled={!canRefreshStatus}>{$t(refreshLabel)}</button>
      <button class="danger subtle-danger" type="button" on:click={onLogout} disabled={busy || !hasLicenseSession}>{$t('Sign out')}</button>
    </div>
  </div>

  {#if dataSyncing || lastSyncedAt || entitlementState?.status === 'restrictedOffline'}
    <div class:syncing={dataSyncing} class="license-sync-state" role="status" aria-live="polite">
      <span aria-hidden="true"></span>
      <strong>
        {#if dataSyncing}
          {$t('Syncing license data')}
        {:else if entitlementState?.status === 'restrictedOffline'}
          {$t('Using verified offline license data')}
        {:else}
          {$t('Last synced')} {relativeSyncAge(lastSyncedAt, $uiLanguage)}
        {/if}
      </strong>
      {#if lastSyncedAt}<time datetime={new Date(lastSyncedAt).toISOString()}>{formatMinuteDate(Math.floor(lastSyncedAt / 1_000), $uiLanguage)}</time>{/if}
    </div>
  {/if}

  {#if publicError}<ErrorNotice error={publicError} dismissible autoDismissMs={transientDismissDelay(publicError)} onDismiss={dismissPrimaryError} />{/if}

  {#if entitlementState?.status === 'sessionOnly'}
    <section class="license-storage-warning">
      <strong>{$t('Secure credential storage unavailable')}</strong>
      <span>{$t('Device activation is disabled because this system cannot persist the device key safely. Repair the operating-system credential store and restart Camellia Nexus.')}</span>
    </section>
  {/if}

  {#if versionAdvisory && versionNotice}
    <section
      class="license-version-warning"
      class:danger={versionAdvisory.kind === 'required'}
    >
      <strong>{$t(versionNotice.title)}</strong>
      <div class="license-version-copy"><span>{$t(versionNotice.message)}</span><span>{$t(versionNotice.suggestion)}</span></div>
    </section>
  {/if}

  <div class="license-grid">
    <section class="license-card">
      <header><span>{$t('Status')}</span><strong>{$t(statusLabel)}</strong></header>
      <dl>
        <div><dt>{$t('Plan')}</dt><dd>{plan ? $t(planLabel(plan)) : $t('Not available')}</dd></div>
        {#if activeEntitlement}<div><dt>{$t('Plan policy')}</dt><dd>{$t('Revision')} {activeEntitlement.claims.planRevision}</dd></div>{/if}
        <div><dt>{$t('Signed license status')}</dt><dd>{#if signedLicenseStatus}<span class={`license-standing-pill ${signedLicenseStatus.tone}`}>{$t(signedLicenseStatus.label)}</span>{:else}{$t('Not available')}{/if}</dd></div>
        <div><dt>{$t('License ID')}</dt><dd>{#if activeEntitlement}<span class="mono" title={activeEntitlement.claims.licenseId}>{shortDeviceId(activeEntitlement.claims.licenseId)}</span>{:else}{$t('Not available')}{/if}</dd></div>
        <div><dt>{$t('License expires')}</dt><dd>{licenseExpiresAt || $t('Not available')}</dd></div>
        <div><dt>{$t('Entitlement issued')}</dt><dd>{issuedAt || $t('Not available')}</dd></div>
        <div><dt>{$t('Local credential')}</dt><dd>{leaseValidUntil || $t('Not available')}</dd></div>
        {#if offlineAccessUntil}<div><dt>{$t('Offline access until')}</dt><dd>{offlineAccessUntil}</dd></div>{/if}
        {#if clientVersionPolicy}
          <div><dt>{$t('Client version')}</dt><dd>{#if appVersion}<span class="version-value">{appVersion}</span>{:else}{$t('Not available')}{/if}</dd></div>
          <div><dt>{$t('Minimum client')}</dt><dd><span class="version-value">{clientVersionPolicy.minimumVersion}</span></dd></div>
          <div><dt>{$t('Recommended client')}</dt><dd><span class="version-value">{clientVersionPolicy.recommendedVersion}</span></dd></div>
          <div><dt>{$t('Version enforcement')}</dt><dd>{formatMinuteDate(clientVersionPolicy.enforceAfter, $uiLanguage)}</dd></div>
        {/if}
        <div><dt>{$t('Program limit')}</dt><dd>{maxPrograms ?? $t('Not available')}</dd></div>
        <div><dt>{$t('Config sources')}</dt><dd>{maxSources ?? $t('Not available')}</dd></div>
        {#if activeEntitlement}
          <div><dt>{$t('Device limit')}</dt><dd>{activeEntitlement.claims.deviceLimit}</dd></div>
          <div><dt>{$t('Member limit')}</dt><dd>{activeEntitlement.claims.memberLimit}</dd></div>
        {/if}
      </dl>
      {#if signedLicenseStatus}<p class="license-claim-note">{$t('This is a signed snapshot from when the entitlement was issued; a newer service response can supersede it.')}</p>{/if}
    </section>

    <section class="license-card">
      <header><span>{$t('Current device')}</span><strong>{$t(currentDeviceStatus ? deviceStateLabel(currentDeviceStatus) : 'Not available')}</strong></header>
      <dl>
        <div><dt>{$t('Name')}</dt><dd>{currentDeviceName}</dd></div>
        <div><dt>{$t('Device ID')}</dt><dd title={currentDeviceId}>{#if currentDeviceId}<span class="mono">{shortDeviceId(currentDeviceId)}</span>{:else}{$t('Not available')}{/if}</dd></div>
        <div><dt>{$t('Status')}</dt><dd><span class={`device-state-pill ${currentDeviceStatus || 'unknown'}`}>{$t(deviceStateLabel(currentDeviceStatus))}</span></dd></div>
        <div><dt>{$t('Registered')}</dt><dd>{registeredDeviceSummary}</dd></div>
      </dl>
    </section>
  </div>

  <section class="license-billing">
    <header>
      <div><h3>{$t('Billing and payment')}</h3><p>{$t('Review invoices and submit an external payment for verification.')}</p></div>
      <div class="billing-sync-actions">
        {#if billingLastUpdatedAt}<small role="status">{$t('Last synced')} {relativeSyncAge(billingLastUpdatedAt, $uiLanguage)}</small>{/if}
        <button class="license-secondary-action" type="button" on:click={onLoadBilling} disabled={busy || billingLoading || !canLoadDevices}>{$t(billingLoading ? 'Syncing billing' : 'Refresh billing')}</button>
      </div>
    </header>
    {#if billingAccessDenied}
      <p class="license-empty">{$t('Billing details are available only to workspace owners, administrators and billing managers.')}</p>
    {:else}
    {#if visibleBillingError}<div class="billing-error"><ErrorNotice error={visibleBillingError} dismissible autoDismissMs={transientDismissDelay(visibleBillingError)} onDismiss={onDismissBillingError} /></div>{/if}
    {#if !billingSummary}
      <p class="license-empty">{$t(hasLicenseSession ? 'Billing information has not been loaded.' : 'Activate this device to view billing information.')}</p>
    {:else if !billingSummary.invoices.length}
      <p class="license-empty">{$t('No invoices are associated with this account.')}</p>
    {:else}
      <div class="billing-invoices">
        {#each billingSummary.invoices as invoice (invoice.id)}
          {@const claim = latestClaim(invoice.id)}
          <article class="billing-invoice">
            <div class="billing-invoice-main">
              <span class={`billing-status ${billingTone(claim?.status ?? invoice.status)}`}>{$t(claim ? claimStatusLabel(claim.status) : invoiceStatusLabel(invoice.status))}</span>
              <strong>{invoice.currency} {formatBillingAmount(invoice.amountDue)} · {$t(planLabel(invoice.plan))}</strong>
              <small>{$t('Due')} {formatMinuteDate(invoice.dueAt, $uiLanguage)} · {invoice.seats} {$t(invoice.seats === 1 ? 'seat' : 'seats')}</small>
            </div>
            <div class="billing-reference"><span>{$t('Payment reference')}</span><strong class="mono">{invoice.paymentReference}</strong></div>
            {#if claim?.reviewReason}
              <div class={`billing-review-note ${billingTone(claim.status)}`}>
                <span>{$t('Review note')}</span>
                <p>{claim.reviewReason}</p>
              </div>
            {/if}
          </article>
        {/each}
      </div>

      {#if openInvoices.length && canSubmitPayments}
        <form class="billing-payment-form" aria-busy={!!busy} on:submit|preventDefault={submitPayment}>
          <div class="billing-form-heading">
            <strong>{$t(paymentNeedsInformation ? 'Complete requested payment information' : 'Submit payment for verification')}</strong>
            <small>{$t(paymentNeedsInformation ? 'Update the requested details below. Resubmitting updates the existing payment claim.' : 'The payment amount and asset must match the selected invoice.')}</small>
          </div>
          <label>{$t('Invoice')}
            <select class="option-align-start" bind:value={paymentInvoiceId} disabled={busy}>
              {#each openInvoices as invoice (invoice.id)}<option value={invoice.id}>{invoice.currency} {formatBillingAmount(invoice.amountDue)} · {compactPaymentReference(invoice.paymentReference)}</option>{/each}
            </select>
          </label>
          {#if compatiblePaymentMethods.length}
            <label>{$t('Payment method')}
              <select class="option-align-start" bind:value={paymentMethodId} disabled={busy}>
                {#each compatiblePaymentMethods as method (method.id)}<option value={method.id}>{localizedPaymentMethod(method).name}</option>{/each}
              </select>
            </label>
            {@const method = compatiblePaymentMethods.find((candidate) => candidate.id === paymentMethodId)}
            {#if method}
              <div class="billing-method-note"><strong>{localizedPaymentMethod(method).name}</strong><span>{localizedPaymentMethod(method).instructions}</span><code>{method.destinationHint}</code></div>
            {/if}
            <label class="billing-transaction">{$t('Transaction or receipt ID')}<input bind:value={paymentTransactionId} maxlength="256" autocomplete="off" disabled={busy} aria-invalid={!!paymentFormError} aria-describedby={paymentFormError ? 'payment-form-error' : undefined} /></label>
            <label>{$t('Payment completed at')}<input type="datetime-local" bind:value={paymentPaidAtInput} disabled={busy} required /><small>{$t('Use the completion time shown by your payment provider.')}</small></label>
            <label><span class="billing-field-label">{$t('Payer name')} <span class="optional">{$t('(Optional)')}</span></span><input bind:value={paymentPayerName} maxlength="160" autocomplete="name" disabled={busy} /></label>
            <label class="billing-note"><span class="billing-field-label">{$t('Note')} <span class="optional">{$t('(Optional)')}</span></span><textarea bind:value={paymentNote} maxlength="1000" rows="3" disabled={busy}></textarea></label>
            {#if paymentFormError}<p id="payment-form-error" class="billing-form-error" role="alert" aria-live="assertive">{paymentFormError}</p>{/if}
            <button class="primary billing-submit" type="submit" disabled={busy}>{$t(paymentNeedsInformation ? 'Resubmit for review' : 'Submit for review')}</button>
          {:else}
            <p class="billing-form-error" role="alert" aria-live="assertive">{$t('No payment method supports this invoice currency. Contact support before paying.')}</p>
          {/if}
        </form>
      {:else if openInvoices.length}
        <p class="license-empty">{$t('Billing is read-only for your workspace role. Ask an owner or billing manager to submit payment evidence.')}</p>
      {/if}
    {/if}
    {/if}
  </section>

  {#if plan === 'team'}
    <TeamWorkspacePanel
      profile={teamProfile}
      members={teamMembers}
      membersHasMore={teamMembersHasMore}
      membersLoadingMore={teamMembersLoadingMore}
      invitation={teamInvitation}
      deviceEnrollment={teamDeviceEnrollment}
      secretGeneration={teamSecretGeneration}
      auditExportLimit={activeEntitlement?.claims.limits.max_audit_export_events ?? 0}
      error={visibleTeamError}
      {busy}
      {busyAction}
      canRefresh={canLoadDevices}
      onRefresh={onLoadTeam}
      onLoadMoreMembers={onLoadMoreTeamMembers}
      onCreateInvitation={onCreateTeamInvitation}
      onDismissInvitation={onDismissTeamInvitation}
      onAcceptInvitation={onAcceptTeamInvitation}
      onUpdateMember={onUpdateTeamMember}
      onCreateDeviceEnrollment={onCreateTeamDeviceEnrollment}
      onCreateMemberDeviceEnrollment={onCreateTeamMemberDeviceEnrollment}
      onDismissDeviceEnrollment={onDismissTeamDeviceEnrollment}
      onAcceptDeviceEnrollment={onAcceptTeamDeviceEnrollment}
      onLeaveWorkspace={onLeaveTeamWorkspace}
      onTransferOwnership={onTransferTeamOwnership}
      onConfirmAction={onConfirmTeamAction}
      onDismissError={onDismissTeamError}
    />
  {/if}

  {#if authorizationRequest}
    <section
      class="license-flow"
      data-e2e-authorization-url={import.meta.env.MODE === 'e2e'
        ? authorizationRequest.authorizationUrl
        : undefined}
    >
      <div class="license-flow-step"><span>1</span><div><strong>{$t('Complete device activation in your browser')}</strong><small>{$t('Enter the activation code issued for your license.')}</small></div></div>
      <div class="license-auto-callback">
        <span aria-hidden="true"></span>
        <div>
          <strong>{$t('Waiting for device activation')}</strong>
          <small>{$t('Return here after the browser confirms activation.')}</small>
        </div>
      </div>
      <label>{$t('Device name')}
        <input bind:value={displayName} placeholder="Windows workstation" />
      </label>
      <div class="license-inline-actions">
        <button type="button" on:click={onBeginAuthorization} disabled={busy}>{$t('Restart activation')}</button>
        <button type="button" class="text-button" on:click={onCancelAuthorization} disabled={!canCancelAuthorization}>{$t('Cancel activation')}</button>
      </div>
    </section>
  {/if}

  <section class="license-devices">
    <header>
      <div><h3>{$t('Registered devices')}</h3><p>{$t('Devices attached to this license.')}</p></div>
      <button class="license-secondary-action" type="button" on:click={onLoadDevices} disabled={busy || !serviceSettings?.configured || !canLoadDevices}>{$t('Refresh devices')}</button>
    </header>
    {#if devices.length}
      <div class="license-device-list">
        {#each devices as device (device.deviceId)}
          <article class:current={device.deviceId === currentDeviceId} class:actionable={deviceCanBeRemoved(device.state)}>
            <span class={`device-dot ${device.state}`}></span>
            <div>
              <strong>{device.displayName || device.platform}{device.deviceId === currentDeviceId ? ` · ${$t('This device')}` : ''}</strong>
              <small class="mono" title={device.deviceId}>{shortDeviceId(device.deviceId)}</small>
            </div>
            <span class={`device-state-pill ${device.state}`}>{$t(deviceStateLabel(device.state))}</span>
            {#if deviceCanBeRemoved(device.state)}
              <button class="license-danger-action" type="button" on:click={() => onRemoveDevice(device.deviceId)} disabled={busy}>{$t('Remove')}</button>
            {/if}
          </article>
        {/each}
      </div>
      {#if hasMoreDevices}
        <div class="license-device-pagination">
          <button class="license-secondary-action" type="button" on:click={onLoadMoreDevices} disabled={busy}>{$t('Load more devices')}</button>
        </div>
      {/if}
    {:else}
      <p class="license-empty">{$t('No devices loaded')}</p>
    {/if}
  </section>

  <section class="license-switch">
    <div>
      <strong>{$t('Use another license')}</strong>
      <small>{$t('Replace this local device identity before activating a different license.')}</small>
    </div>
    <button class="license-danger-action" type="button" on:click={onUseAnotherLicense} disabled={busy || !!authorizationRequest || entitlementState?.status === 'sessionOnly' || !serviceSettings?.authorizationConfigured}>{$t('Use another license')}</button>
  </section>

</div>

<style>
  .license-panel {
    display: grid;
    min-width: 0;
    gap: var(--ui-gap-lg);
    color: var(--ui-text-primary);
  }

  .license-panel :is(p, span, strong, small, dt, dd, button) {
    max-width: 100%;
    overflow-wrap: anywhere;
  }

  .license-hero {
    position: relative;
    display: grid;
    min-width: 0;
    grid-template-columns: auto minmax(0, 1fr) minmax(210px, auto);
    align-items: center;
    gap: var(--ui-gap-lg);
    overflow: hidden;
    padding: clamp(16px, 2.3vw, 24px);
    border: 1px solid var(--ui-border-subtle);
    border-radius: var(--ui-radius-lg);
    background:
      radial-gradient(circle at 8% 10%, color-mix(in srgb, var(--ui-brand) 16%, transparent), transparent 36%),
      linear-gradient(135deg, color-mix(in srgb, var(--ui-brand-soft) 42%, var(--ui-surface-1)), var(--ui-surface-2));
    box-shadow: var(--ui-shadow-sm), var(--ui-shadow-inset);
  }

  .license-orb {
    --license-tone: var(--ui-text-tertiary);
    --license-soft: var(--ui-surface-3);
    position: relative;
    display: grid;
    width: 72px;
    height: 72px;
    flex: 0 0 auto;
    place-items: center;
    border: 1px solid color-mix(in srgb, var(--license-tone) 24%, var(--ui-border-subtle));
    border-radius: 24px;
    background:
      linear-gradient(145deg, color-mix(in srgb, var(--ui-surface-raised) 78%, transparent), transparent 48%),
      radial-gradient(circle at 28% 20%, color-mix(in srgb, var(--license-tone) 30%, transparent), transparent 46%),
      color-mix(in srgb, var(--license-soft) 88%, var(--ui-surface-1));
    color: var(--license-tone);
    box-shadow:
      0 12px 28px color-mix(in srgb, var(--license-tone) 14%, transparent),
      inset 0 1px color-mix(in srgb, white 48%, transparent),
      inset 0 -1px color-mix(in srgb, var(--license-tone) 10%, transparent);
  }

  .license-orb::before {
    position: absolute;
    inset: 7px;
    border: 1px solid color-mix(in srgb, var(--license-tone) 12%, transparent);
    border-radius: 18px;
    content: '';
  }

  .license-orb.success,
  .license-status.success { --license-tone: var(--ui-success); --license-soft: var(--ui-success-soft); }
  .license-orb.warning,
  .license-status.warning { --license-tone: var(--ui-warning); --license-soft: var(--ui-warning-soft); }
  .license-orb.danger,
  .license-status.danger { --license-tone: var(--ui-danger); --license-soft: var(--ui-danger-soft); }
  .license-orb.neutral,
  .license-status.neutral { --license-tone: var(--ui-text-secondary); --license-soft: var(--ui-surface-3); }

  .license-orb svg { position: relative; width: 52px; height: 52px; overflow: visible; filter: drop-shadow(0 4px 8px color-mix(in srgb, currentColor 18%, transparent)); }
  .license-shield { fill: color-mix(in srgb, var(--license-soft) 72%, var(--ui-surface-raised)); stroke: currentColor; stroke-width: 2.4; stroke-linejoin: round; }
  .license-shield-inset { fill: none; stroke: color-mix(in srgb, currentColor 32%, transparent); stroke-width: 1.4; stroke-linejoin: round; }
  .license-status-mark { fill: none; stroke: currentColor; stroke-width: 3.2; stroke-linecap: round; stroke-linejoin: round; }
  .license-status-fill { fill: currentColor; }

  .license-identity { min-width: 0; }
  .license-identity h3 {
    margin: 5px 0 3px;
    font-family: var(--ui-font-display);
    font-size: var(--ui-font-size-xl);
    letter-spacing: var(--ui-letter-spacing-heading);
    line-height: var(--ui-line-height-tight);
  }
  .license-identity p {
    margin: 0;
    color: var(--ui-text-secondary);
    font-size: var(--ui-font-size-sm);
    line-height: var(--ui-line-height-body);
  }

  .license-status {
    --license-tone: var(--ui-text-secondary);
    --license-soft: var(--ui-surface-3);
    display: inline-flex;
    width: fit-content;
    align-items: center;
    gap: 6px;
    padding: 4px 9px;
    border: 1px solid color-mix(in srgb, var(--license-tone) 22%, transparent);
    border-radius: var(--ui-radius-round);
    background: var(--license-soft);
    color: var(--license-tone);
    font-size: var(--ui-font-size-xs);
    font-weight: var(--ui-weight-bold);
    letter-spacing: var(--ui-letter-spacing-label);
  }

  .license-status::before {
    width: 7px;
    height: 7px;
    flex: 0 0 auto;
    border-radius: 50%;
    background: currentColor;
    box-shadow: 0 0 0 3px color-mix(in srgb, currentColor 12%, transparent);
    content: '';
  }

  .license-actions {
    display: grid;
    min-width: 210px;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: var(--ui-gap-sm);
  }

  .license-actions button {
    width: 100%;
    min-height: var(--ui-control-lg);
    padding-inline: 11px;
    white-space: normal;
  }

  .license-actions .primary { grid-column: 1 / -1; }
  .license-actions .subtle-danger { background: color-mix(in srgb, var(--ui-danger-soft) 58%, transparent); }

  .license-sync-state {
    display: flex;
    min-width: 0;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 9px;
    margin-top: calc(-1 * var(--ui-gap-sm));
    padding-inline: 4px;
    color: var(--ui-text-secondary);
    font-size: var(--ui-font-size-xs);
  }

  .license-sync-state > span {
    width: 7px;
    height: 7px;
    flex: 0 0 auto;
    border-radius: 50%;
    background: var(--ui-success);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--ui-success) 12%, transparent);
  }

  .license-sync-state.syncing > span {
    background: var(--ui-brand);
    animation: license-pulse 1.2s var(--ui-ease-standard) infinite;
  }

  .license-sync-state strong { color: var(--ui-text-primary); }
  .license-sync-state time { overflow-wrap: anywhere; }

  .license-storage-warning,
  .license-version-warning {
    display: grid;
    gap: 3px;
    padding: 12px 14px;
    border: 1px solid color-mix(in srgb, var(--ui-warning) 24%, var(--ui-border-subtle));
    border-left: 3px solid var(--ui-warning);
    border-radius: var(--ui-radius-sm);
    background: color-mix(in srgb, var(--ui-warning-soft) 82%, var(--ui-surface-1));
    color: color-mix(in srgb, var(--ui-warning) 78%, var(--ui-text-primary));
    line-height: var(--ui-line-height-body);
  }

  .license-storage-warning span,
  .license-version-warning span { font-size: var(--ui-font-size-sm); }
  .license-version-copy { display: grid; gap: 1px; }

  .license-version-warning.danger {
    border-color: color-mix(in srgb, var(--ui-danger) 26%, var(--ui-border-subtle));
    border-left-color: var(--ui-danger);
    background: color-mix(in srgb, var(--ui-danger-soft) 84%, var(--ui-surface-1));
    color: color-mix(in srgb, var(--ui-danger) 78%, var(--ui-text-primary));
  }

  .license-grid {
    display: grid;
    min-width: 0;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    align-items: start;
    gap: var(--ui-gap-md);
  }

  .license-card {
    min-width: 0;
    overflow: hidden;
    padding: 0;
    border: 1px solid var(--ui-border-subtle);
    border-radius: var(--ui-radius-lg);
    background: var(--ui-surface-2);
    box-shadow: var(--ui-shadow-xs);
  }

  .license-card > header {
    display: flex;
    min-width: 0;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--ui-gap-md);
    padding: 14px 16px;
    border-bottom: 1px solid var(--ui-divider);
    background: color-mix(in srgb, var(--ui-surface-raised) 55%, transparent);
  }

  .license-card > header span {
    color: var(--ui-text-secondary);
    font-size: var(--ui-font-size-sm);
    font-weight: var(--ui-weight-medium);
  }

  .license-card > header strong {
    min-width: 0;
    text-align: right;
    line-height: var(--ui-line-height-body);
  }

  .license-card dl { margin: 0; padding: 6px 16px 10px; }
  .license-card dl > div {
    display: grid;
    min-width: 0;
    grid-template-columns: minmax(105px, .8fr) minmax(0, 1.2fr);
    align-items: start;
    gap: var(--ui-gap-md);
    padding: 9px 0;
    border-bottom: 1px solid var(--ui-divider);
  }
  .license-card dl > div:last-child { border-bottom: 0; }
  .license-card dt {
    color: var(--ui-text-secondary);
    font-size: var(--ui-font-size-sm);
    font-weight: var(--ui-weight-medium);
  }
  .license-card dd {
    min-width: 0;
    margin: 0;
    text-align: right;
    font-size: var(--ui-font-size-sm);
    font-weight: var(--ui-weight-semibold);
    line-height: var(--ui-line-height-body);
  }

  .mono {
    font-family: var(--ui-font-mono);
    font-size: var(--ui-font-size-xs);
    font-weight: var(--ui-weight-medium);
    word-break: break-word;
  }
  .version-value {
    font-family: var(--ui-font-mono);
    font-variant-numeric: tabular-nums;
    font-size: inherit;
    font-weight: inherit;
    overflow-wrap: anywhere;
  }

  .license-standing-pill,
  .device-state-pill {
    --pill-tone: var(--ui-text-secondary);
    --pill-soft: var(--ui-surface-3);
    display: inline-flex;
    width: fit-content;
    max-width: 100%;
    align-items: center;
    justify-content: center;
    padding: 4px 8px;
    border: 1px solid color-mix(in srgb, var(--pill-tone) 22%, transparent);
    border-radius: var(--ui-radius-round);
    background: var(--pill-soft);
    color: var(--pill-tone);
    font-size: var(--ui-font-size-xs);
    font-weight: var(--ui-weight-bold);
    line-height: 1.25;
    text-align: center;
  }

  .license-card dd .license-standing-pill,
  .license-card dd .device-state-pill { margin-left: auto; }
  .license-standing-pill.success,
  .device-state-pill.active { --pill-tone: var(--ui-success); --pill-soft: var(--ui-success-soft); }
  .license-standing-pill.warning,
  .device-state-pill.pending_activation { --pill-tone: var(--ui-warning); --pill-soft: var(--ui-warning-soft); }
  .license-standing-pill.danger,
  .device-state-pill.removed,
  .device-state-pill.revoked,
  .device-state-pill.suspicious { --pill-tone: var(--ui-danger); --pill-soft: var(--ui-danger-soft); }
  .device-state-pill.local { --pill-tone: var(--ui-info); --pill-soft: var(--ui-info-soft); }

  .license-claim-note {
    margin: 0;
    padding: 11px 16px 13px;
    border-top: 1px solid color-mix(in srgb, var(--ui-info) 18%, var(--ui-divider));
    background: color-mix(in srgb, var(--ui-info-soft) 52%, transparent);
    color: color-mix(in srgb, var(--ui-info) 72%, var(--ui-text-primary));
    font-size: var(--ui-font-size-xs);
    line-height: var(--ui-line-height-relaxed);
  }

  .license-flow,
  .license-billing,
  .license-devices,
  .license-switch {
    min-width: 0;
    border: 1px solid var(--ui-border-subtle);
    border-radius: var(--ui-radius-lg);
    background: var(--ui-surface-2);
    box-shadow: var(--ui-shadow-xs);
  }

  .license-flow { display: grid; gap: var(--ui-gap-md); padding: 16px; }
  .license-billing > header,
  .license-devices > header {
    display: flex;
    min-width: 0;
    align-items: center;
    justify-content: space-between;
    gap: var(--ui-gap-md);
    padding: 14px 16px;
    border-bottom: 1px solid var(--ui-divider);
  }
  .license-billing > header h3,
  .license-billing > header p { margin: 0; }
  .license-billing > header p { margin-top: 2px; color: var(--ui-text-secondary); font-size: var(--ui-font-size-sm); }
  .billing-sync-actions { display: grid; flex: 0 1 auto; justify-items: end; gap: 5px; text-align: right; }
  .billing-sync-actions small { color: var(--ui-text-tertiary); font-size: var(--ui-font-size-xs); }
  .billing-error { padding: 12px 16px 0; }
  .billing-invoices { display: grid; gap: 8px; padding: 12px 16px; }
  .billing-invoice {
    display: grid;
    min-width: 0;
    grid-template-columns: minmax(0, 1fr) minmax(150px, auto);
    align-items: center;
    gap: var(--ui-gap-md);
    padding: 12px;
    border: 1px solid var(--ui-divider);
    border-radius: var(--ui-radius-md);
    background: var(--ui-surface-1);
  }
  .billing-invoice-main { display: grid; min-width: 0; gap: 3px; }
  .billing-invoice-main small { color: var(--ui-text-secondary); }
  .billing-status { font-size: var(--ui-font-size-xs); font-weight: var(--ui-weight-bold); }
  .billing-status.success { color: var(--ui-success); }
  .billing-status.warning { color: var(--ui-warning); }
  .billing-status.danger { color: var(--ui-danger); }
  .billing-status.neutral { color: var(--ui-text-tertiary); }
  .billing-reference { display: grid; min-width: 0; gap: 3px; text-align: right; }
  .billing-reference span { color: var(--ui-text-secondary); font-size: var(--ui-font-size-xs); }
  .billing-review-note { display: grid; grid-column: 1 / -1; gap: 3px; padding-top: 8px; border-top: 1px solid var(--ui-divider); }
  .billing-review-note span { color: var(--ui-text-secondary); font-size: var(--ui-font-size-xs); font-weight: var(--ui-weight-bold); }
  .billing-review-note p { margin: 0; color: var(--ui-text-primary); font-size: var(--ui-font-size-sm); line-height: var(--ui-line-height-body); }
  .billing-review-note.success span { color: var(--ui-success); }
  .billing-review-note.warning span { color: var(--ui-warning); }
  .billing-review-note.danger span { color: var(--ui-danger); }
  .billing-payment-form {
    display: grid;
    grid-template-columns: minmax(0, 1.05fr) minmax(0, .95fr);
    align-items: start;
    gap: var(--ui-gap-md);
    padding: 16px;
    border-top: 1px solid var(--ui-divider);
    background: color-mix(in srgb, var(--ui-brand-soft) 24%, var(--ui-surface-1));
  }
  .billing-form-heading { display: grid; grid-column: 1 / -1; gap: 2px; }
  .billing-form-heading small { color: var(--ui-text-secondary); }
  .billing-payment-form label { display: grid; min-width: 0; gap: 6px; color: var(--ui-text-secondary); font-size: var(--ui-font-size-sm); font-weight: var(--ui-weight-semibold); }
  .billing-field-label { display: flex; min-width: 0; flex-wrap: wrap; align-items: baseline; gap: 6px; }
  .billing-payment-form :is(input, select, textarea) { width: 100%; min-width: 0; }
  .billing-payment-form select { overflow: hidden; padding-inline-end: 36px; text-overflow: ellipsis; white-space: nowrap; }
  .billing-transaction,
  .billing-note,
  .billing-method-note,
  .billing-form-error { grid-column: 1 / -1; }
  .billing-method-note { display: grid; gap: 4px; padding: 11px 12px; border: 1px solid var(--ui-divider); border-radius: var(--ui-radius-sm); background: var(--ui-surface-2); }
  .billing-method-note span { color: var(--ui-text-secondary); font-size: var(--ui-font-size-sm); }
  .billing-method-note code { overflow-wrap: anywhere; color: var(--ui-text-primary); }
  .billing-form-error { margin: 0; color: var(--ui-danger); font-size: var(--ui-font-size-sm); }
  .billing-submit { justify-self: start; }
  .optional { color: var(--ui-text-tertiary); font-size: var(--ui-font-size-xs); font-weight: var(--ui-weight-medium); }
  .license-flow-step {
    display: grid;
    min-width: 0;
    grid-template-columns: 34px minmax(0, 1fr);
    align-items: center;
    gap: var(--ui-gap-md);
    padding: 0;
    border: 0;
    background: transparent;
  }
  .license-flow-step > span {
    display: grid;
    width: 34px;
    height: 34px;
    place-items: center;
    border-radius: 12px;
    background: var(--ui-brand);
    color: var(--ui-on-brand);
    font-weight: var(--ui-weight-bold);
    box-shadow: 0 7px 18px color-mix(in srgb, var(--ui-brand) 24%, transparent);
  }
  .license-flow-step div,
  .license-auto-callback div { min-width: 0; }
  .license-flow-step strong,
  .license-flow-step small,
  .license-auto-callback strong,
  .license-auto-callback small { display: block; }
  .license-flow-step small,
  .license-auto-callback small {
    margin-top: 2px;
    color: var(--ui-text-secondary);
    font-size: var(--ui-font-size-xs);
    line-height: var(--ui-line-height-body);
  }

  .license-auto-callback {
    display: grid;
    min-width: 0;
    grid-template-columns: 18px minmax(0, 1fr);
    align-items: center;
    gap: var(--ui-gap-md);
    padding: 11px 12px;
    border: 1px solid color-mix(in srgb, var(--ui-info) 20%, var(--ui-border-subtle));
    border-radius: var(--ui-radius-sm);
    background: color-mix(in srgb, var(--ui-info-soft) 58%, var(--ui-surface-1));
  }
  .license-auto-callback > span {
    width: 18px;
    height: 18px;
    border: 2px solid color-mix(in srgb, var(--ui-info) 22%, transparent);
    border-top-color: var(--ui-info);
    border-radius: 50%;
    animation: license-spin .9s linear infinite;
  }

  .license-inline-actions { display: flex; flex-wrap: wrap; gap: var(--ui-gap-sm); }
  .license-inline-actions button { white-space: normal; }

  .license-devices { overflow: hidden; }
  .license-devices > header {
    display: flex;
    min-width: 0;
    align-items: center;
    justify-content: space-between;
    gap: var(--ui-gap-lg);
    padding: 14px 16px;
    border-bottom: 1px solid var(--ui-divider);
    background: color-mix(in srgb, var(--ui-surface-raised) 55%, transparent);
  }
  .license-devices > header > div { min-width: 0; }
  .license-devices h3 { margin: 0; font-size: var(--ui-font-size-lg); }
  .license-devices header p {
    margin: 3px 0 0;
    color: var(--ui-text-secondary);
    font-size: var(--ui-font-size-xs);
  }
  .license-devices > header button { flex: 0 1 auto; white-space: normal; }

  .license-device-list { display: grid; gap: var(--ui-gap-sm); padding: 10px; }
  .license-device-list article {
    display: grid;
    min-width: 0;
    grid-template-columns: 10px minmax(0, 1fr) auto;
    grid-template-areas: 'dot identity state';
    align-items: center;
    gap: var(--ui-gap-md);
    padding: 11px 12px;
    border: 1px solid var(--ui-border-subtle);
    border-radius: var(--ui-radius-sm);
    background: var(--ui-surface-1);
    transition:
      border-color var(--ui-duration-fast) var(--ui-ease-standard),
      background-color var(--ui-duration-fast) var(--ui-ease-standard),
      transform var(--ui-duration-fast) var(--ui-ease-standard);
  }
  .license-device-list article.actionable {
    grid-template-columns: 10px minmax(0, 1fr) auto auto;
    grid-template-areas: 'dot identity state action';
  }
  .license-device-list article:hover { border-color: var(--ui-border-default); transform: translateY(-1px); }
  .license-device-list article.current {
    border-color: color-mix(in srgb, var(--ui-brand) 30%, var(--ui-border-default));
    background: color-mix(in srgb, var(--ui-brand-soft) 58%, var(--ui-surface-1));
  }
  .license-device-list article > div { min-width: 0; grid-area: identity; }
  .license-device-list article strong,
  .license-device-list article small { display: block; }
  .license-device-list article small { margin-top: 3px; color: var(--ui-text-secondary); }
  .license-device-list article .device-state-pill { min-height: var(--ui-control-sm); grid-area: state; padding-inline: 10px; }
  .license-device-list article button { min-height: var(--ui-control-sm); grid-area: action; padding: 4px 10px; border-radius: var(--ui-radius-xs); white-space: normal; }

  .device-dot {
    --dot-tone: var(--ui-text-tertiary);
    width: 8px;
    height: 8px;
    grid-area: dot;
    border-radius: 50%;
    background: var(--dot-tone);
    box-shadow: 0 0 0 4px color-mix(in srgb, var(--dot-tone) 13%, transparent);
  }
  .device-dot.active { --dot-tone: var(--ui-success); }
  .device-dot.pending_activation { --dot-tone: var(--ui-warning); }
  .device-dot.removed,
  .device-dot.revoked,
  .device-dot.suspicious { --dot-tone: var(--ui-danger); }
  .device-dot.local { --dot-tone: var(--ui-info); }

  .license-device-pagination {
    display: flex;
    justify-content: center;
    padding: 0 12px 12px;
  }
  .license-empty {
    margin: 0;
    padding: 24px 16px;
    color: var(--ui-text-secondary);
    text-align: center;
  }

  .license-secondary-action { color: var(--ui-text-link); }
  .license-danger-action {
    border-color: color-mix(in srgb, var(--ui-danger) 25%, var(--ui-border-default));
    background: color-mix(in srgb, var(--ui-danger-soft) 60%, transparent);
    color: var(--ui-danger);
    box-shadow: none;
  }

  .license-switch {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    gap: var(--ui-gap-lg);
    padding: 14px 16px;
  }
  .license-switch > div { min-width: 0; }
  .license-switch strong,
  .license-switch small { display: block; }
  .license-switch small {
    margin-top: 3px;
    color: var(--ui-text-secondary);
    font-size: var(--ui-font-size-xs);
    line-height: var(--ui-line-height-body);
  }
  .license-switch button { max-width: 210px; white-space: normal; }

  @keyframes license-spin { to { transform: rotate(360deg); } }
  @keyframes license-pulse {
    0%, 100% { box-shadow: 0 0 0 4px color-mix(in srgb, currentColor 12%, transparent); }
    50% { box-shadow: 0 0 0 8px color-mix(in srgb, currentColor 4%, transparent); }
  }

  @media (max-width: 900px) {
    .license-hero { grid-template-columns: auto minmax(0, 1fr); align-items: start; }
    .license-actions {
      min-width: 0;
      grid-column: 1 / -1;
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
    .license-grid { grid-template-columns: minmax(0, 1fr); }
  }

  @media (max-width: 640px) {
    .license-panel { gap: var(--ui-gap-md); }
    .license-hero { grid-template-columns: minmax(0, 1fr); }
    .license-orb { width: 58px; height: 58px; border-radius: 19px; }
    .license-orb::before { inset: 6px; border-radius: 14px; }
    .license-orb svg { width: 43px; height: 43px; }
    .license-actions { grid-template-columns: minmax(0, 1fr); }
    .license-actions .primary { grid-column: auto; }
    .license-card > header,
    .license-billing > header,
    .license-devices > header { align-items: flex-start; flex-direction: column; }
    .license-card > header strong { text-align: left; }
    .license-card dl > div { grid-template-columns: minmax(0, 1fr); gap: 3px; }
    .license-card dd { text-align: left; }
    .license-card dd .license-standing-pill,
    .license-card dd .device-state-pill { margin-left: 0; }
    .license-devices > header button { width: 100%; }
    .license-billing > header button { width: 100%; }
    .billing-sync-actions { width: 100%; justify-items: stretch; text-align: left; }
    .billing-invoice { grid-template-columns: minmax(0, 1fr); }
    .billing-reference { text-align: left; }
    .billing-payment-form { grid-template-columns: minmax(0, 1fr); }
    .license-device-list article {
      grid-template-columns: 10px minmax(0, 1fr);
      grid-template-areas:
        'dot identity'
        '. state';
      align-items: start;
    }
    .license-device-list article.actionable {
      grid-template-columns: 10px minmax(0, 1fr);
      grid-template-areas:
        'dot identity'
        '. state'
        '. action';
    }
    .license-device-list article .device-state-pill { justify-self: start; }
    .license-device-list article button { justify-self: start; }
    .license-switch { grid-template-columns: minmax(0, 1fr); }
    .license-switch button { width: 100%; max-width: none; }
  }

  @media (max-width: 420px) {
    .license-flow,
    .billing-payment-form,
    .license-switch { padding: 12px; }
    .license-card dl { padding-inline: 12px; }
    .license-card > header,
    .license-billing > header,
    .license-devices > header { padding-inline: 12px; }
    .license-device-list article { padding-inline: 10px; }
    .license-device-list article button { width: 100%; }
    .license-inline-actions { display: grid; grid-template-columns: minmax(0, 1fr); }
  }

  @media (prefers-reduced-motion: reduce) {
    .license-sync-state.syncing > span,
    .license-auto-callback > span { animation: none; }
    .license-device-list article { transition: none; }
    .license-device-list article:hover { transform: none; }
  }
</style>

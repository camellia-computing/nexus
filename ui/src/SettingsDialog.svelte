<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { focusTrap } from './lib/actions/focusTrap';
  import Icon from './lib/components/Icon.svelte';
  import type { ColorMode, ThemeId, UiScale } from './lib/theme';
  import ErrorNotice from './ErrorNotice.svelte';
  import LicenseSettingsPanel from './LicenseSettingsPanel.svelte';
  import { t, uiLanguage, type UiLanguage } from './i18n';
  import type { ErrorInfo } from './api';
  import type {
    AppLogLevel,
    AppSettings,
    EntitlementState,
    CustomerPaymentSubmission,
    CreateTeamInvitation,
    LicenseAuthorizationRequest,
    LocalLicenseDevice,
    LicenseServiceSettings,
    LicenseBillingSummary,
    LeaveWorkspace,
    MemberDeviceEnrollment,
    TeamInvitation,
    TeamProfile,
    TransferWorkspaceOwnership,
    WorkspaceMember,
    UpdateWorkspaceMember,
    RegisteredLicenseDevice,
  } from './types';

  export let appearanceTheme: ThemeId;
  export let colorMode: ColorMode;
  export let uiScale: UiScale;
  export let appAutostart: boolean;
  export let appSettings: AppSettings;
  export let entitlementState: EntitlementState | null = null;
  export let licenseServiceSettings: LicenseServiceSettings | null = null;
  export let licenseAuthorizationRequest: LicenseAuthorizationRequest | null = null;
  export let localLicenseDevice: LocalLicenseDevice | null = null;
  export let licenseAuthorizationDisplayName = '';
  export let licenseDevices: RegisteredLicenseDevice[] = [];
  export let licenseDevicesNextCursor: string | null = null;
  export let licenseBillingSummary: LicenseBillingSummary | null = null;
  export let licenseBillingError: ErrorInfo | null = null;
  export let licenseBillingLoading = false;
  export let licenseBillingLastUpdatedAt = 0;
  export let licenseDataSyncing = false;
  export let licenseLastSyncedAt = 0;
  export let licenseTeamProfile: TeamProfile | null = null;
  export let licenseTeamMembers: WorkspaceMember[] = [];
  export let licenseTeamMembersHasMore = false;
  export let licenseTeamMembersLoadingMore = false;
  export let licenseTeamInvitation: TeamInvitation | null = null;
  export let licenseTeamDeviceEnrollment: MemberDeviceEnrollment | null = null;
  export let licenseTeamSecretGeneration = 0;
  export let licenseTeamError: ErrorInfo | null = null;
  export let appVersion = '';
  export let behaviorSaved = false;
  export let focusSection: 'license' | null = null;
  export let initialFocus = '';
  export let error: ErrorInfo | null = null;
  export let licenseError: ErrorInfo | null = null;
  export let busy = false;
  export let busyAction = '';
  export let onToggleAutostart: () => void;
  export let onOpenDataDirectory: () => void;
  export let onOpenAppLogDirectory: () => void;
  export let onOpenAbout: () => void;
  export let onUpdateAppSettings: (settings: AppSettings) => void;
  export let onChangeLanguage: (language: UiLanguage) => void;
  export let onBeginLicenseAuthorization: () => void;
  export let onRefreshLicense: () => void;
  export let onReconnectLicense: () => void;
  export let onLoadLicenseDevices: () => void;
  export let onLoadMoreLicenseDevices: () => void;
  export let onLoadLicenseBilling: () => void;
  export let onShowLicense: () => void;
  export let onSubmitLicensePayment: (submission: CustomerPaymentSubmission) => void;
  export let onLoadLicenseTeam: () => void;
  export let onLoadMoreLicenseTeamMembers: () => void;
  export let onCreateLicenseTeamInvitation: (request: CreateTeamInvitation) => Promise<void>;
  export let onDismissLicenseTeamInvitation: () => void;
  export let onAcceptLicenseTeamInvitation: (
    token: string,
    operationId: string,
  ) => Promise<void>;
  export let onUpdateLicenseTeamMember: (
    memberId: string,
    request: UpdateWorkspaceMember,
  ) => Promise<void>;
  export let onCreateLicenseTeamDeviceEnrollment: (operationId: string) => Promise<void>;
  export let onCreateLicenseTeamMemberDeviceEnrollment: (
    memberId: string,
    operationId: string,
  ) => Promise<void>;
  export let onDismissLicenseTeamDeviceEnrollment: () => void;
  export let onAcceptLicenseTeamDeviceEnrollment: (
    token: string,
    operationId: string,
  ) => Promise<void>;
  export let onLeaveLicenseTeamWorkspace: (request: LeaveWorkspace) => Promise<void>;
  export let onTransferLicenseTeamOwnership: (
    request: TransferWorkspaceOwnership,
  ) => Promise<void>;
  export let onConfirmTeamWorkspaceAction: (
    title: string,
    message: string,
    confirmLabel: string,
    danger?: boolean,
  ) => Promise<boolean>;
  export let onRemoveLicenseDevice: (deviceId: string) => void;
  export let onCancelLicenseAuthorization: () => void;
  export let onLogoutLicense: () => void;
  export let onUseAnotherLicense: () => void;
  export let onDismissLicenseError: () => void;
  export let onDismissLicenseBillingError: () => void;
  export let onDismissLicenseTeamError: () => void;
  export let onClose: () => void;

  type SettingsSection = 'appearance' | 'general' | 'license' | 'behavior';

  interface SettingsNavigationItem {
    id: SettingsSection;
    label: string;
    icon: 'palette' | 'general' | 'shield' | 'sliders';
  }

  const settingsNavigation: SettingsNavigationItem[] = [
    { id: 'appearance', label: 'Appearance', icon: 'palette' },
    { id: 'general', label: 'General', icon: 'general' },
    { id: 'license', label: 'License', icon: 'shield' },
    { id: 'behavior', label: 'Program behavior', icon: 'sliders' },
  ];

  const styles: Array<{ id: ThemeId; name: string; description: string }> = [
    {
      id: 'cupertino',
      name: 'Cupertino Glass',
      description: 'Translucent surfaces and restrained motion',
    },
    {
      id: 'material',
      name: 'Material You',
      description: 'Tonal surfaces and geometric depth',
    },
    {
      id: 'aurora',
      name: 'Aurora Flow',
      description: 'Luminous layers and fluid motion',
    },
  ];

  const appLogLevels: Array<{ id: AppLogLevel; label: string }> = [
    { id: 'error', label: 'Error' },
    { id: 'warn', label: 'Warn' },
    { id: 'info', label: 'Info' },
    { id: 'debug', label: 'Debug' },
    { id: 'trace', label: 'Trace' },
  ];

  export let activeSection: SettingsSection = focusSection === 'license' ? 'license' : 'appearance';
  let settingsNavigationElement: HTMLElement;
  let compactNavigation = false;

  $: selectedAppLogLevel = appSettings.logLevel ?? 'warn';

  onMount(() => {
    const query = matchMedia('(max-width: 899px)');
    const update = (event: MediaQueryListEvent | MediaQueryList) => (compactNavigation = event.matches);
    update(query);
    query.addEventListener('change', update);
    return () => query.removeEventListener('change', update);
  });

  function activateSection(section: SettingsSection, restoreNavigationFocus = false): void {
    activeSection = section;
    if (section === 'license') onShowLicense();
    if (!restoreNavigationFocus) return;
    void tick().then(() => {
      settingsNavigationElement
        ?.querySelector<HTMLElement>(`[data-settings-section="${section}"]`)
        ?.focus({ preventScroll: true });
    });
  }

  function handleNavigationKeydown(event: KeyboardEvent, section: SettingsSection): void {
    const currentIndex = settingsNavigation.findIndex((item) => item.id === section);
    let nextIndex = currentIndex;

    if (event.key === 'ArrowDown' || event.key === 'ArrowRight') {
      nextIndex = (currentIndex + 1) % settingsNavigation.length;
    } else if (event.key === 'ArrowUp' || event.key === 'ArrowLeft') {
      nextIndex = (currentIndex - 1 + settingsNavigation.length) % settingsNavigation.length;
    } else if (event.key === 'Home') {
      nextIndex = 0;
    } else if (event.key === 'End') {
      nextIndex = settingsNavigation.length - 1;
    } else {
      return;
    }

    event.preventDefault();
    activateSection(settingsNavigation[nextIndex].id, true);
  }
</script>

<div
  class="modal-backdrop settings-backdrop"
  role="presentation"
  on:click={(event) => event.currentTarget === event.target && onClose()}
>
  <div
    use:focusTrap={{
      onEscape: onClose,
      initialFocus: initialFocus || `[data-settings-section="${activeSection}"]`,
    }}
    class="settings-dialog"
    role="dialog"
    aria-modal="true"
    aria-labelledby="settings-title"
  >
    <header class="settings-header">
      <div class="settings-heading">
        <p class="eyebrow">Camellia Nexus</p>
        <h2 id="settings-title">{$t('Settings')}</h2>
      </div>
      <button class="icon-button" type="button" aria-label={$t('Close settings')} on:click={onClose}>
        <Icon name="close" />
      </button>
    </header>

    <div class="settings-workspace">
      <aside class="settings-sidebar">
        <nav class="settings-nav" aria-label={$t('Settings categories')}>
          <div
            bind:this={settingsNavigationElement}
            class="settings-nav-list"
            role="tablist"
            aria-orientation={compactNavigation || appearanceTheme === 'material' ? 'horizontal' : 'vertical'}
          >
            {#each settingsNavigation as item (item.id)}
              <button
                class:active={activeSection === item.id}
                class="settings-nav-item"
                type="button"
                role="tab"
                id={`settings-tab-${item.id}`}
                aria-selected={activeSection === item.id}
                aria-controls={activeSection === item.id ? `settings-panel-${item.id}` : undefined}
                tabindex={activeSection === item.id ? 0 : -1}
                data-settings-section={item.id}
                on:click={() => activateSection(item.id)}
                on:keydown={(event) => handleNavigationKeydown(event, item.id)}
              >
                <span class="settings-nav-icon"><Icon name={item.icon} /></span>
                <span class="settings-nav-label">{$t(item.label)}</span>
              </button>
            {/each}
          </div>
        </nav>
      </aside>

      <div class="settings-content">
        {#if error}
          <div class="settings-content-notice"><ErrorNotice {error} /></div>
        {/if}

        {#if activeSection === 'appearance'}
          <div
            class="settings-pane appearance-settings-pane"
            id="settings-panel-appearance"
            role="tabpanel"
            aria-labelledby="settings-tab-appearance"
          >
            <header class="settings-pane-header">
              <div>
                <h3>{$t('Appearance')}</h3>
                <p>{$t('Configure visual style, color mode and interface scale.')}</p>
              </div>
            </header>

            <div class="appearance-grid" aria-label={$t('Visual style')}>
              {#each styles as style (style.id)}
                <button
                  class:active={appearanceTheme === style.id}
                  class="appearance-option"
                  type="button"
                  aria-pressed={appearanceTheme === style.id}
                  on:click={() => (appearanceTheme = style.id)}
                >
                  <span class={`theme-preview ${style.id}`} aria-hidden="true"><i></i><b></b><em></em></span>
                  <span class="appearance-option-copy">
                    <strong>{style.name}</strong>
                    <small>{$t(style.description)}</small>
                  </span>
                  <span class="selection-mark" aria-hidden="true"><Icon name="check" size={16} /></span>
                </button>
              {/each}
            </div>

            <div class="settings-group">
              <div class="settings-control-row">
                <div class="settings-control-copy">
                  <strong>{$t('Brightness')}</strong>
                </div>
                <div class="mode-picker settings-segmented-control" role="group" aria-label={$t('Brightness')}>
                  <button class:active={colorMode === 'system'} type="button" aria-pressed={colorMode === 'system'} on:click={() => (colorMode = 'system')}>{$t('System')}</button>
                  <button class:active={colorMode === 'light'} type="button" aria-pressed={colorMode === 'light'} on:click={() => (colorMode = 'light')}>{$t('Light')}</button>
                  <button class:active={colorMode === 'dark'} type="button" aria-pressed={colorMode === 'dark'} on:click={() => (colorMode = 'dark')}>{$t('Dark')}</button>
                </div>
              </div>

              <div class="settings-control-row">
                <div class="settings-control-copy">
                  <strong>{$t('Interface size')}</strong>
                </div>
                <div class="scale-picker settings-segmented-control" role="group" aria-label={$t('Interface size')}>
                  <button class:active={uiScale === 0.95} type="button" aria-pressed={uiScale === 0.95} on:click={() => (uiScale = 0.95)}>{$t('Compact')}</button>
                  <button class:active={uiScale === 1.05} type="button" aria-pressed={uiScale === 1.05} on:click={() => (uiScale = 1.05)}>{$t('Default')}</button>
                  <button class:active={uiScale === 1.15} type="button" aria-pressed={uiScale === 1.15} on:click={() => (uiScale = 1.15)}>{$t('Large')}</button>
                  <button class:active={uiScale === 1.3} type="button" aria-pressed={uiScale === 1.3} on:click={() => (uiScale = 1.3)}>XL</button>
                </div>
              </div>
            </div>
          </div>
        {:else if activeSection === 'general'}
          <div
            class="settings-pane general-settings-pane"
            id="settings-panel-general"
            role="tabpanel"
            aria-labelledby="settings-tab-general"
          >
            <header class="settings-pane-header">
              <div>
                <h3>{$t('General')}</h3>
              </div>
            </header>

            <div class="settings-group">
              <div class="settings-control-row language-setting">
                <div class="settings-control-copy">
                  <strong>{$t('Language')}</strong>
                </div>
                <div class="mode-picker settings-segmented-control" role="group" aria-label={$t('Language')}>
                  <button class:active={$uiLanguage === 'en'} type="button" aria-pressed={$uiLanguage === 'en'} on:click={() => onChangeLanguage('en')}>{$t('English')}</button>
                  <button class:active={$uiLanguage === 'zh-CN'} type="button" aria-pressed={$uiLanguage === 'zh-CN'} on:click={() => onChangeLanguage('zh-CN')}>{$t('Chinese')}</button>
                </div>
              </div>

              <button class="settings-action-row" type="button" aria-pressed={appAutostart} on:click={onToggleAutostart} disabled={busy}>
                <span class="settings-control-copy">
                  <strong>{$t('Start at login')}</strong>
                  <small>{$t('Register Camellia Nexus for operating-system startup.')}</small>
                </span>
                <span class:enabled={appAutostart} class="switch" aria-hidden="true"><i></i></span>
              </button>
            </div>

            <div class="settings-group settings-link-group">
              <button class="settings-action-row" type="button" on:click={onOpenDataDirectory}>
                <span class="settings-control-copy"><strong>{$t('Application data')}</strong></span>
                <Icon name="external" size={17} />
              </button>
              <button class="settings-action-row" type="button" on:click={onOpenAppLogDirectory}>
                <span class="settings-control-copy"><strong>{$t('Application logs')}</strong><small>{$t('Open diagnostic log files.')}</small></span>
                <Icon name="external" size={17} />
              </button>
              <button class="settings-action-row" data-settings-action="about" type="button" on:click={onOpenAbout}>
                <span class="settings-control-copy"><strong>{$t('About Camellia Nexus')}</strong><small>{appVersion ? `${$t('Version')} ${appVersion}` : ''}</small></span>
                <Icon name="chevron" size={17} />
              </button>
            </div>
          </div>
        {:else if activeSection === 'license'}
          <div
            class="settings-pane license-settings-pane"
            id="settings-panel-license"
            role="tabpanel"
            aria-labelledby="settings-tab-license"
          >
            <header class="settings-pane-header">
              <div>
                <h3>{$t('License')}</h3>
                <p>{$t('Manage device activation, entitlement and registered devices.')}</p>
              </div>
            </header>

            <div class="license-settings-content">
              <LicenseSettingsPanel
                {entitlementState}
                {appVersion}
                serviceSettings={licenseServiceSettings}
                authorizationRequest={licenseAuthorizationRequest}
                localDevice={localLicenseDevice}
                bind:displayName={licenseAuthorizationDisplayName}
                devices={licenseDevices}
                hasMoreDevices={!!licenseDevicesNextCursor}
                billingSummary={licenseBillingSummary}
                billingError={licenseBillingError}
                billingLoading={licenseBillingLoading}
                billingLastUpdatedAt={licenseBillingLastUpdatedAt}
                dataSyncing={licenseDataSyncing}
                lastSyncedAt={licenseLastSyncedAt}
                teamProfile={licenseTeamProfile}
                teamMembers={licenseTeamMembers}
                teamMembersHasMore={licenseTeamMembersHasMore}
                teamMembersLoadingMore={licenseTeamMembersLoadingMore}
                teamInvitation={licenseTeamInvitation}
                teamDeviceEnrollment={licenseTeamDeviceEnrollment}
                teamSecretGeneration={licenseTeamSecretGeneration}
                teamError={licenseTeamError}
                error={licenseError}
                {busy}
                {busyAction}
                onBeginAuthorization={onBeginLicenseAuthorization}
                onRefresh={onRefreshLicense}
                onReconnect={onReconnectLicense}
                onLoadDevices={onLoadLicenseDevices}
                onLoadMoreDevices={onLoadMoreLicenseDevices}
                onLoadBilling={onLoadLicenseBilling}
                onSubmitPayment={onSubmitLicensePayment}
                onLoadTeam={onLoadLicenseTeam}
                onLoadMoreTeamMembers={onLoadMoreLicenseTeamMembers}
                onCreateTeamInvitation={onCreateLicenseTeamInvitation}
                onDismissTeamInvitation={onDismissLicenseTeamInvitation}
                onAcceptTeamInvitation={onAcceptLicenseTeamInvitation}
                onUpdateTeamMember={onUpdateLicenseTeamMember}
                onCreateTeamDeviceEnrollment={onCreateLicenseTeamDeviceEnrollment}
                onCreateTeamMemberDeviceEnrollment={onCreateLicenseTeamMemberDeviceEnrollment}
                onDismissTeamDeviceEnrollment={onDismissLicenseTeamDeviceEnrollment}
                onAcceptTeamDeviceEnrollment={onAcceptLicenseTeamDeviceEnrollment}
                onLeaveTeamWorkspace={onLeaveLicenseTeamWorkspace}
                onTransferTeamOwnership={onTransferLicenseTeamOwnership}
                onConfirmTeamAction={onConfirmTeamWorkspaceAction}
                onRemoveDevice={onRemoveLicenseDevice}
                onCancelAuthorization={onCancelLicenseAuthorization}
                onLogout={onLogoutLicense}
                onUseAnotherLicense={onUseAnotherLicense}
                onDismissError={onDismissLicenseError}
                onDismissBillingError={onDismissLicenseBillingError}
                onDismissTeamError={onDismissLicenseTeamError}
              />
            </div>
          </div>
        {:else}
          <div
            class="settings-pane behavior-settings-pane"
            id="settings-panel-behavior"
            role="tabpanel"
            aria-labelledby="settings-tab-behavior"
          >
            <header class="settings-pane-header">
              <div>
                <h3>{$t('Program behavior')}</h3>
              </div>
            </header>

            <div class="settings-group">
              <div class="settings-control-row behavior-setting">
                <div class="settings-control-copy">
                  <strong>{$t('Application log level')}</strong>
                  <small>{$t('Applies after restart.')}</small>
                </div>
                <div class="behavior-picker log-level-picker settings-segmented-control" role="group" aria-label={$t('Application log level')}>
                  {#each appLogLevels as level (level.id)}
                    <button
                      class:active={selectedAppLogLevel === level.id}
                      type="button"
                      aria-pressed={selectedAppLogLevel === level.id}
                      disabled={busy}
                      on:click={() => onUpdateAppSettings({ ...appSettings, logLevel: level.id })}
                    >
                      {$t(level.label)}
                    </button>
                  {/each}
                </div>
              </div>

              <div class="settings-control-row behavior-setting">
                <div class="settings-control-copy">
                  <strong>{$t('Log history')}</strong>
                </div>
                <div class="behavior-picker settings-segmented-control" role="group" aria-label={$t('Log history')}>
                  <button class:active={appSettings.logRetention === 'preserve'} type="button" aria-pressed={appSettings.logRetention === 'preserve'} disabled={busy} on:click={() => onUpdateAppSettings({ ...appSettings, logRetention: 'preserve' })}>{$t('Preserve')}</button>
                  <button class:active={appSettings.logRetention === 'clearOnStart'} type="button" aria-pressed={appSettings.logRetention === 'clearOnStart'} disabled={busy} on:click={() => onUpdateAppSettings({ ...appSettings, logRetention: 'clearOnStart' })}>{$t('Clear on start')}</button>
                </div>
              </div>

              <div class="settings-control-row behavior-setting">
                <div class="settings-control-copy">
                  <strong>{$t('Startup spacing')}</strong>
                </div>
                <div class="behavior-picker settings-segmented-control" role="group" aria-label={$t('Startup spacing')}>
                  <button class:active={appSettings.programStartupDelayMs === 0} type="button" aria-pressed={appSettings.programStartupDelayMs === 0} disabled={busy} on:click={() => onUpdateAppSettings({ ...appSettings, programStartupDelayMs: 0 })}>{$t('Off')}</button>
                  <button class:active={appSettings.programStartupDelayMs === 750} type="button" aria-pressed={appSettings.programStartupDelayMs === 750} disabled={busy} on:click={() => onUpdateAppSettings({ ...appSettings, programStartupDelayMs: 750 })}>{$t('Balanced')}</button>
                  <button class:active={appSettings.programStartupDelayMs === 2000} type="button" aria-pressed={appSettings.programStartupDelayMs === 2000} disabled={busy} on:click={() => onUpdateAppSettings({ ...appSettings, programStartupDelayMs: 2000 })}>{$t('Gentle')}</button>
                </div>
              </div>
            </div>

            <p class:visible={behaviorSaved} class="settings-saved" aria-live="polite">
              {behaviorSaved ? $t('Settings saved') : ''}
            </p>
          </div>
        {/if}
      </div>
    </div>
  </div>
</div>

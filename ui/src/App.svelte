<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { get } from 'svelte/store';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import ArgumentPreview from './ArgumentPreview.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import ConfigSourceEditor from './ConfigSourceEditor.svelte';
  import GuidedConfigurationEditor from './GuidedConfigurationEditor.svelte';
  import ManagedIntegrationStatus from './ManagedIntegrationStatus.svelte';
  import ConfigurationSurfaceIssues from './ConfigurationSurfaceIssues.svelte';
  import FinalConfigurationEditor from './FinalConfigurationEditor.svelte';
  import ShareImportPreviewDialog from './ShareImportPreview.svelte';
  import EnvironmentEditor from './EnvironmentEditor.svelte';
  import ErrorNotice from './ErrorNotice.svelte';
  import HomeDashboard from './features/home/HomeDashboard.svelte';
  import ProgramDetailHeader from './features/programs/ProgramDetailHeader.svelte';
  import ProgramDetailLoading from './features/programs/ProgramDetailLoading.svelte';
  import ProgramTabs from './features/programs/ProgramTabs.svelte';
  import Icon from './lib/components/Icon.svelte';
  import OptionSelect from './lib/components/OptionSelect.svelte';
  import ResizeSeparator from './lib/components/ResizeSeparator.svelte';
  import { createAsyncListenerScope } from './lib/asyncListenerScope';
  import ProgramSidebar from './features/navigation/ProgramSidebar.svelte';
  import {
    loadCatalog,
    moveCatalogItem,
    moveCatalogItemBy,
    reconcileCatalog,
    saveCatalog,
  } from './catalog';
  import {
    configurationDiagnosticMessage,
    setLanguage,
    t,
    translate,
    uiLanguage,
  } from './i18n';
  import ProgramContextMenu from './ProgramContextMenu.svelte';
  import { managedWorkingDirectory } from './paths';
  import {
    semanticPathSegments,
    type ConfigurationEditorMarker,
  } from './editor/configurationMarkerModel';
  import MihomoDashboardEditor from './programs/mihomo/MihomoDashboardEditor.svelte';
  import SingBoxDashboardEditor from './programs/sing-box/SingBoxDashboardEditor.svelte';
  import XrayDashboardEditor from './programs/xray/XrayDashboardEditor.svelte';
  import XrayDashboardView from './programs/xray/XrayDashboardView.svelte';
  import {
    applySingBoxDashboardChange,
    type SingBoxDashboardChange,
    type SingBoxDashboardOptions,
  } from './programs/sing-box';
  import {
    formatArgumentLine,
    parseArgumentLine,
  } from './arguments';
  import { programDefinition } from './programs/registry';
  import {
    effectiveConfigSourceLimit,
    hasConfigurationArgument,
  } from './programs/shared/configuration';
  import { isRuntimeActive } from './programState';
  import { api, errorInfoOf, type ErrorInfo } from './api';
  import {
    configurationErrorInfo,
    coreAdmissionMessage,
    sourceIssueMessage,
    type ConfigurationErrorContext,
  } from './errors';
  import {
    applyAppearancePreferences,
    loadAppearancePreferences,
    resolveColorScheme,
    saveAppearancePreferences,
    type ColorMode,
    type EffectiveColorScheme,
    type ThemeId,
    type UiScale,
  } from './lib/theme';
  import {
    clearCreateDraft,
    defaultDraft,
    loadCreateDraft,
    saveCreateDraft,
    type CreateDraft,
    type EnvironmentEntry,
  } from './drafts';
  import {
    hasRefreshableLicenseSession,
    isNewerEntitlementSnapshot,
    licenseNoticeKey,
    licenseRuntimeImpact,
    licenseRuntimeNotice,
    licenseNoticeRequiresPersistentAttention,
    licenseStateNotice,
    type LicenseNotice,
  } from './license';
  import {
    canUseProgramLifecycleAction,
    deriveLicenseAccess,
    type ProgramLifecycleAction,
  } from './licenseAccess';
  import type {
    ActionDescriptor,
    ApplicationInfo,
    AppSettings,
    AutomaticConfigUpdateEvent,
    ConfigDocument,
    ConfigurationStateView,
    ConfigurationWorkspaceSnapshot,
    FinalConflictResolution,
    FinalConflictProjection,
    ResolveConfigurationConflictRequest,
    ConfigurationSchemaDocument,
    ConfigSource,
    CoreTargetIdentity,
    CreateTeamInvitation,
    TeamInvitation,
    TeamProfile,
    WorkspaceMember,
    UpdateWorkspaceMember,
    EntitlementSnapshot,
    EntitlementState,
    CustomerPaymentSubmission,
    LicenseAuthorizationCallbackEvent,
    LicenseAuthorizationFailedEvent,
    LicenseAuthorizationRequest,
    LicenseStateChangedEvent,
    LicenseServiceSettings,
    LicenseBillingSummary,
    LeaveWorkspace,
    LocalLicenseDevice,
    MemberDeviceEnrollment,
    InvalidProgram,
    ManagedConfig,
    MihomoDashboard,
    ProgramDetail,
    ProgramKind,
    ProgramSpec,
    ProgramState,
    ProgramSummary,
    PrivilegeAssessment,
    PrivilegePolicy,
    RegisteredLicenseDevice,
    ShareImportPreview,
    FinalEditorSession,
    TransferWorkspaceOwnership,
    XrayBalancerInfo,
    XrayDashboard,
    XrayDashboardSnapshot,
  } from './types';

  type Tab = 'overview' | 'intent' | 'sources' | 'compatibility' | 'configuration' | 'logs' | 'dashboard';
  type LogView = 'both' | 'stdout' | 'stderr';
  type LogPaneKind = 'stdout' | 'stderr';
  type LogPaneScrollState = { followLatest: boolean; scrollTop: number };
  type XrayTrafficSort = 'scope' | 'tag' | 'uplink' | 'downlink';
  type ProgramFilter = 'all' | 'running' | 'inactive' | 'issues' | ProgramKind;
  type ConfigUpdateStatus = { message: string; sourceCount?: number };
  type ConfigurationWorkspaceScope = 'details' | 'intent' | 'sources' | 'compatibility' | 'configuration';
  type ConfirmationRequest = {
    title: string;
    message: string;
    confirmLabel: string;
    danger: boolean;
    resolve: (confirmed: boolean) => void;
  };
  type ProgramMenu = { program: ProgramSummary; x: number; y: number };
  type LicensePrompt = { title: string; message: string; action: string };
  type SettingsFocusSection = 'license' | null;
  type SettingsSection = 'appearance' | 'general' | 'license' | 'behavior';
  type Translator = (source: string) => string;
  type XrayResizeKind = 'pair' | 'traffic';
  type ResizeKind = `xray:${XrayResizeKind}` | 'detail:config';
  type XrayDashboardLayout = { pairHeight?: number; trafficHeight?: number };
  type ProgramDetailLayout = { configHeight?: number };
  type CodeEditorComponent = typeof import('./CodeEditor.svelte').default;
  type CreateProgramDialogComponent = typeof import('./features/programs/CreateProgramDialog.svelte').default;
  type SettingsDialogComponent = typeof import('./SettingsDialog.svelte').default;
  type AboutDialogComponent = typeof import('./AboutDialog.svelte').default;
  const initialAppearance = loadAppearancePreferences();
  const xrayDashboardLayoutKey = 'camellia-nexus.xray-dashboard.layout';
  const programDetailLayoutKey = 'camellia-nexus.program-detail.layout';
  const sidebarPreferenceKey = 'camellia-nexus.sidebar';
  const licenseAuthorizationTimeoutMs = 3 * 60_000;
  const licenseNotificationTimeoutMs = 7_000;
  const globalNotificationTimeoutMs = 12_000;
  const licenseAuxiliaryAutoRefreshMinIntervalMs = 60_000;
  const licenseAuxiliaryAttentionRefreshMinIntervalMs = 5_000;
  const xrayPairResizeDefaultHeight = 520;
  const xrayPairResizeMinHeight = 400;
  const xrayPairResizeMaxHeight = 1200;
  const xrayTrafficResizeDefaultHeight = 480;
  const xrayTrafficResizeMinHeight = 300;
  const xrayTrafficResizeMaxHeight = 1200;
  const detailPaneResizeMinHeight = 260;
  const detailPaneResizeMaxHeight = 2400;

  function loadSidebarCollapsed() {
    try {
      return localStorage.getItem(sidebarPreferenceKey) === 'collapsed';
    } catch {
      return false;
    }
  }

  function saveSidebarCollapsed(collapsed: boolean) {
    try {
      localStorage.setItem(sidebarPreferenceKey, collapsed ? 'collapsed' : 'expanded');
    } catch {
      // The in-memory preference remains usable for this session.
    }
  }

  function loadXrayDashboardLayout(): XrayDashboardLayout {
    try {
      const raw = localStorage.getItem(xrayDashboardLayoutKey);
      if (!raw) return {};
      const value = JSON.parse(raw) as XrayDashboardLayout;
      return {
        pairHeight: typeof value.pairHeight === 'number' && Number.isFinite(value.pairHeight)
          ? clampResizeHeight(
              value.pairHeight,
              xrayPairResizeMinHeight,
              xrayPairResizeMaxHeight,
            )
          : undefined,
        trafficHeight: typeof value.trafficHeight === 'number' && Number.isFinite(value.trafficHeight)
          ? clampResizeHeight(
              value.trafficHeight,
              xrayTrafficResizeMinHeight,
              xrayTrafficResizeMaxHeight,
            )
          : undefined,
      };
    } catch {
      return {};
    }
  }

  function loadProgramDetailLayout(): ProgramDetailLayout {
    try {
      const raw = localStorage.getItem(programDetailLayoutKey);
      if (!raw) return {};
      const value = JSON.parse(raw) as ProgramDetailLayout;
      return {
        configHeight: typeof value.configHeight === 'number' ? value.configHeight : undefined,
      };
    } catch {
      return {};
    }
  }

  function clampResizeHeight(height: number, min: number, max: number) {
    return Math.round(Math.min(max, Math.max(min, height)));
  }

  function clampDetailPaneResizeHeight(height: number) {
    return clampResizeHeight(height, detailPaneResizeMinHeight, detailPaneResizeMaxHeight);
  }

  function defaultLogFollowScrollState(): LogPaneScrollState {
    return { followLatest: true, scrollTop: 0 };
  }

  function defaultLogScrollState(): Record<LogPaneKind, LogPaneScrollState> {
    return {
      stdout: defaultLogFollowScrollState(),
      stderr: defaultLogFollowScrollState(),
    };
  }

  const initialXrayDashboardLayout = loadXrayDashboardLayout();
  const initialProgramDetailLayout = loadProgramDetailLayout();

  let programs: ProgramSummary[] = [];
  let invalidPrograms: InvalidProgram[] = [];
  let selectedId = '';
  let loadingProgramId = '';
  let selectionGeneration = 0;
  let programRefreshGeneration = 0;
  let detail: ProgramDetail | null = null;
  let activeTab: Tab = 'overview';
  let busy = '';
  const pendingExternalActions = new Set<string>();
  let notification: ErrorInfo | null = null;
  let notificationLicenseNotice: LicenseNotice | null = null;
  let notificationTimer: number | undefined;
  let licensePrompt: LicensePrompt | null = null;
  let licensePromptTimer: number | undefined;
  let configError: ErrorInfo | null = null;
  let workspaceErrors: Partial<Record<ConfigurationWorkspaceScope, ErrorInfo>> = {};
  let workspaceErrorActions: Partial<Record<ConfigurationWorkspaceScope, () => Promise<void>>> = {};
  let workspaceErrorBusyScope: ConfigurationWorkspaceScope | '' = '';
  let appSettingsError: ErrorInfo | null = null;
  let licenseError: ErrorInfo | null = null;
  let aboutError: ErrorInfo | null = null;
  let showCreate = false;
  let createReturnFocus: HTMLElement | null = null;
  let showSettings = false;
  let settingsReturnFocus: HTMLElement | null = null;
  let settingsFocusSection: SettingsFocusSection = null;
  let settingsActiveSection: SettingsSection = 'appearance';
  let settingsInitialFocus = '';
  let showAbout = false;
  let aboutReturnFocus: HTMLElement | null = null;
  let reopenSettingsAfterAbout = false;
  let confirmation: ConfirmationRequest | null = null;
  let confirmationReturnFocus: HTMLElement | null = null;
  let programMenu: ProgramMenu | null = null;
  let programMenuTrigger: HTMLElement | null = null;
  let createError: ErrorInfo | null = null;
  let createFieldErrors: Record<string, string> = {};
  let appAutostart = false;
  let entitlementState: EntitlementState | null = null;
  let licenseServiceSettings: LicenseServiceSettings | null = null;
  let localLicenseDevice: LocalLicenseDevice | null = null;
  let licenseAuthorizationRequest: LicenseAuthorizationRequest | null = null;
  let licenseAuthorizationDisplayName = '';
  let licenseAuthorizationGeneration = 0;
  let licenseAuthorizationCompletingState = '';
  let licenseAuthorizationCompletingStates = new Set<string>();
  let licenseAuthorizationCallbackTimer: number | undefined;
  let licenseAuthorizationTimeoutTimer: number | undefined;
  let licenseAuthorizationCallbackRequestActive = false;
  let licenseRefreshRequest: Promise<void> | null = null;
  let licenseDevicesRequestActive = false;
  let licenseDevicesLastLoadedAt = 0;
  let licenseDevices: RegisteredLicenseDevice[] = [];
  const licenseDeviceRemovalOperations = new Map<string, string>();
  let licenseIdentityResetOperationId = '';
  let licenseDevicesNextCursor: string | null = null;
  let licenseBillingSummary: LicenseBillingSummary | null = null;
  let licenseBillingError: ErrorInfo | null = null;
  let licenseBillingRequestActive = false;
  let licenseBillingLastLoadedAt = 0;
  let licenseTeamProfile: TeamProfile | null = null;
  let licenseTeamMembers: WorkspaceMember[] = [];
  let licenseTeamMembersNextCursor: string | null = null;
  let licenseTeamMembersHasMore = false;
  let licenseTeamMembersLoadingMore = false;
  let licenseTeamInvitation: TeamInvitation | null = null;
  let licenseTeamDeviceEnrollment: MemberDeviceEnrollment | null = null;
  let licenseTeamSecretGeneration = 0;
  let licenseTeamError: ErrorInfo | null = null;
  let licenseTeamRequest: Promise<boolean> | null = null;
  let licenseTeamLastLoadedAt = 0;
  let licenseAuxiliaryScope = '';
  let licenseAuxiliaryGeneration = 0;
  let licenseAuxiliaryLastAttentionRefreshAt = 0;
  let licenseStatusRequestActive = false;
  let licenseStateEffectGeneration = 0;
  let entitlementSnapshotGeneration = 0;
  let lastLicenseEventGeneration = 0;
  let lastLicenseNoticeKey = '';
  let repeatLicenseStopFailureNotice = false;
  let appSettings: AppSettings = {
    version: 1,
    logRetention: 'preserve',
    logLevel: 'warn',
    programStartupDelayMs: 750,
  };
  let behaviorSaved = false;
  let behaviorSavedTimer: number | undefined;
  let applicationInfo: ApplicationInfo | null = null;
  let appearanceTheme: ThemeId = initialAppearance.theme;
  let colorMode: ColorMode = initialAppearance.colorMode;
  let systemDark = matchMedia('(prefers-color-scheme: dark)').matches;
  let theme: EffectiveColorScheme = resolveColorScheme(colorMode, systemDark);
  let uiScale: UiScale = initialAppearance.scale;

  let configDocument: ConfigDocument | null = null;
  let configurationState: ConfigurationStateView | null = null;
  let configurationStateLoadingId = '';
  let configurationStateLoadError = false;
  let configurationWorkspaceRequestProgramId = '';
  let configurationWorkspaceRequest: Promise<ConfigurationWorkspaceSnapshot> | null = null;
  let configurationSchemaDocument: ConfigurationSchemaDocument | null = null;
  let configurationSchemaLoading = false;
  let configurationSchemaError = false;
  let configurationSchemaGeneration = 0;
  let configurationSchemaScope = '';
  let shareImportPreview: ShareImportPreview | null = null;
  let finalEditorSession: FinalEditorSession | null = null;
  let finalEditorAutosaveTimer: number | undefined;
  let finalEditorAutosavePaused = false;
  let finalEditorAutosaveContent = '';
  const finalEditorSavePromises = new Map<string, Promise<FinalEditorSession>>();
  let finalEditorAutosaveError: ErrorInfo | null = null;
  let finalEditorRedundantDraftDiscarding = false;
  let configContent = '';
  let configSyntaxStatus: { content: string; language: string; invalid: boolean } | null = null;
  let configDiagnosticStatus: { content: string; language: string; errorCount: number } | null = null;
  let finalEditorInputBaseContent: string | null = null;
  let focusSemanticPath = '';
  let selectedEditorPath = '';
  let selectedEditorConflictId = '';
  let conflictActionError: { conflictId: string; message: string } | null = null;
  let pendingConflictRequest: ResolveConfigurationConflictRequest | null = null;
  const conflictErrorsByProgram = new Map<string, { conflictId: string; message: string }>();
  const pendingConflictsByProgram = new Map<string, ResolveConfigurationConflictRequest>();
  const conflictHistoryRecords = new Map<string, FinalConflictProjection>();
  $: pendingConflictProjection = pendingConflictRequest
    ? conflictHistoryRecords.get(pendingConflictRequest.action.kind === 'resolve'
      ? pendingConflictRequest.operationId : pendingConflictRequest.action.resolutionOperationId) ?? null
    : null;
  let conflictStateProgramId = '';
  $: if (selectedId !== conflictStateProgramId) {
    conflictStateProgramId = selectedId;
    conflictActionError = conflictErrorsByProgram.get(selectedId) ?? null;
    pendingConflictRequest = pendingConflictsByProgram.get(selectedId) ?? null;
    selectedEditorConflictId = '';
  }
  let recentConflictChoice: { programId: string; operationId: string; undone: boolean } | null = null;
  let pathFocusMessage = '';
  let configEditorDirty = false;
  let configSaveRequiresRestart = false;
  let configOutputMessage = '';
  let configOutputTimer: number | undefined;
  let actions: ActionDescriptor[] = [];

  $: configurationEditorMarkers = buildConfigurationEditorMarkers(
    configurationState,
    finalEditorSession,
    $uiLanguage,
    conflictActionError,
    pendingConflictRequest,
    pendingConflictProjection,
  );
  $: managedIntegrationById = new Map(
    (configurationState?.managedIntegrations ?? []).map((projection) => [projection.integrationId, projection]),
  );
  $: finalEditorDraftConflicts = unresolvedDraftConflicts(finalEditorSession);
  $: if (selectedEditorPath && configurationState && ![
    ...configurationState.workspace.editor.changes,
    ...configurationState.workspace.editor.conflicts,
  ].some((path) => path.semanticPath === selectedEditorPath)
    && !finalEditorDraftConflicts.some((conflict) => (
      conflict.semanticPath === selectedEditorPath
    ))) {
    selectedEditorPath = '';
  }
  $: activeFinalMarkerId = pendingConflictProjection && conflictActionError
    ? pendingConflictProjection.conflictId
    : selectedEditorConflictId && configurationState?.workspace.editor.conflicts.some((conflict) => conflict.conflictId === selectedEditorConflictId)
    ? selectedEditorConflictId
    : selectedEditorPath
    ? configurationState?.workspace.editor.conflicts.find(
        (conflict) => conflict.semanticPath === selectedEditorPath,
      )?.conflictId
      ?? configurationState?.workspace.editor.changes.find(
        (change) => change.semanticPath === selectedEditorPath,
      )?.editId
      ?? ''
    : configurationState?.workspace.editor.conflicts[0]?.conflictId
      ?? '';

  $: finalEditorTextChanged = !!finalEditorSession && configContent !== finalEditorSession.workingContent;
  $: if (!finalEditorAutosavePaused && !pendingConflictRequest && busy !== 'save-candidate' && busy !== 'apply'
    && busy !== 'resolve-configuration-conflict' && !busy.startsWith('history-configuration-conflict') && finalEditorSession && selectedId
    && (configEditorDirty || (finalEditorSession.conflicts.length > 0 && finalEditorTextChanged))
    && finalEditorAutosaveContent !== configContent) {
    finalEditorAutosaveContent = configContent;
    if (finalEditorAutosaveTimer !== undefined) window.clearTimeout(finalEditorAutosaveTimer);
    const draftId = selectedId;
    const draft = structuredClone(finalEditorSession);
    draft.baseContent = finalEditorInputBaseContent ?? draft.baseContent;
    draft.workingContent = configContent;
    finalEditorAutosaveTimer = window.setTimeout(() => {
      finalEditorAutosaveTimer = undefined;
      void persistFinalEditorDraft(draftId, draft).catch((error) => {
        if (selectedId !== draftId || finalEditorSession?.sessionId !== draft.sessionId
          || configContent !== draft.workingContent) return;
        reportConfigError(error, 'final-editor-draft', async () => { await retryFinalEditorAutosave(draftId); });
        finalEditorAutosaveError = configError;
      });
    }, 750);
  }

  $: if (
    !configEditorDirty
    && !finalEditorSession?.conflicts.length
    && finalEditorAutosaveTimer !== undefined
    && configDocument
    && configContent === configDocument.content
  ) {
    window.clearTimeout(finalEditorAutosaveTimer);
    finalEditorAutosaveTimer = undefined;
    finalEditorAutosaveContent = configContent;
  }

  $: if (
    !busy
    && !finalEditorAutosavePaused
    && !finalEditorRedundantDraftDiscarding
    && !configEditorDirty
    && finalEditorSession?.draftRevision
    && finalEditorSession.conflicts.length === 0
    && !finalEditorSession.rebaseRequired
    && finalEditorSession.unresolvedConflictIds.length === 0
    && configDocument
    && configContent === configDocument.content
    && selectedId
  ) {
    void discardRedundantFinalEditorDraft(selectedId, finalEditorSession.draftRevision);
  }

  async function discardRedundantFinalEditorDraft(
    id: string,
    expectedRevision: number,
  ): Promise<void> {
    if (finalEditorRedundantDraftDiscarding) return;
    finalEditorRedundantDraftDiscarding = true;
    try {
      if (finalEditorAutosaveTimer !== undefined) {
        window.clearTimeout(finalEditorAutosaveTimer);
        finalEditorAutosaveTimer = undefined;
      }
      await finalEditorSavePromises.get(id);
      const snapshot = await api.discardConfigurationDraft(id, expectedRevision);
      if (
        selectedId !== id
        || configEditorDirty
        || configContent !== configDocument?.content
        || finalEditorSession?.draftRevision !== expectedRevision
      ) return;
      await adoptConfigurationWorkspace(id, snapshot);
    } catch (error) {
      if (isRetryableConfigurationConflict(error)) {
        const snapshot = await api.getConfigurationWorkspace(id);
        if (selectedId === id) {
          await adoptConfigurationWorkspace(
            id,
            snapshot,
            configEditorDirty ? 'preserveTyping' : 'authoritative',
          );
        }
        return;
      }
      reportConfigError(
        error,
        'final-editor-draft',
        async () => { await discardRedundantFinalEditorDraft(id, expectedRevision); },
      );
    } finally {
      finalEditorRedundantDraftDiscarding = false;
    }
  }

  async function persistFinalEditorDraft(id: string, draft: FinalEditorSession): Promise<FinalEditorSession> {
    const previous = finalEditorSavePromises.get(id);
    const request = (async () => {
      if (previous) {
        const saved = await previous;
        // Advance the CAS token without changing the baseline of the caller's text.
        draft = { ...draft, sessionId: saved.sessionId, draftRevision: saved.draftRevision };
      }
      const snapshot = await api.saveConfigurationDraft(id, draft, draft.draftRevision);
      const saved = snapshot.editorSession;
      if (!saved) throw new Error('Configuration editor snapshot is unavailable.');
      await adoptConfigurationWorkspace(id, snapshot,
        selectedId === id && configContent !== draft.workingContent ? 'preserveTyping' : 'authoritative');
      if (selectedId === id && configContent === saved.workingContent
        && finalEditorAutosaveTimer !== undefined) {
        window.clearTimeout(finalEditorAutosaveTimer);
        finalEditorAutosaveTimer = undefined;
      }
      if (selectedId === id && configError && configError === finalEditorAutosaveError) {
        clearConfigurationError();
      }
      return saved;
    })();
    finalEditorSavePromises.set(id, request);
    try {
      return await request;
    } finally {
      if (finalEditorSavePromises.get(id) === request) finalEditorSavePromises.delete(id);
    }
  }

  async function retryFinalEditorAutosave(id: string) {
    if (selectedId !== id) return;
    await mutateConfiguration(id, 'save-draft', async () => {
      clearConfigurationError();
      await flushFinalEditorDraftBeforeUpstreamMutation(id);
    }, async (error) => {
      if (selectedId !== id) return;
      if (isRetryableConfigurationConflict(error)) {
        try {
          await adoptConfigurationWorkspace(id, await api.getConfigurationWorkspace(id), 'preserveTyping');
        } catch {
          // Retain the original text and baseline when the workspace cannot be read.
        }
      }
      reportConfigError(error, 'final-editor-draft', async () => { await retryFinalEditorAutosave(id); });
      finalEditorAutosaveError = configError;
    });
  }

  async function flushFinalEditorDraftBeforeUpstreamMutation(
    id: string,
  ): Promise<FinalEditorSession | null> {
    const wasPaused = finalEditorAutosavePaused;
    finalEditorAutosavePaused = true;
    try {
      if (finalEditorAutosaveTimer !== undefined) {
        window.clearTimeout(finalEditorAutosaveTimer);
        finalEditorAutosaveTimer = undefined;
      }
      await finalEditorSavePromises.get(id);
      if (selectedId !== id) return null;
      let session = finalEditorSession;
      if (!session) {
        const snapshot = await api.getConfigurationWorkspace(id);
        if (selectedId !== id) return null;
        await adoptConfigurationWorkspace(id, snapshot, 'preserveTyping');
        session = snapshot.editorSession ?? null;
        if (!session) throw new Error('Configuration editor snapshot is unavailable.');
      }
      const recoveryIsParked = session.rebaseRequired
        && configContent === configurationState?.workspace.editor.document.content;
      if (recoveryIsParked || configContent === session.workingContent) return session;
      const next = structuredClone(session);
      next.baseContent = finalEditorInputBaseContent ?? next.baseContent;
      next.workingContent = configContent;
      const saved = await persistFinalEditorDraft(id, next);
      if (selectedId === id) {
        finalEditorSession = saved;
        finalEditorAutosaveContent = saved.workingContent;
      }
      return saved;
    } finally {
      finalEditorAutosavePaused = wasPaused;
    }
  }

  async function synchronizeFinalEditorBeforeUpstreamMutation(
    id: string,
  ): Promise<ConfigurationStateView> {
    await flushFinalEditorDraftBeforeUpstreamMutation(id);
    if (selectedId !== id) throw new Error('The selected program changed.');
    if (configurationState) return configurationState;
    const snapshot = await requestConfigurationWorkspace(id);
    await adoptConfigurationWorkspace(id, snapshot);
    return snapshot.state;
  }

  function resumeFinalEditorDraft() {
    if (!finalEditorSession?.rebaseRequired) return;
    configContent = finalEditorSession.workingContent;
    finalEditorAutosaveContent = finalEditorSession.workingContent;
  }

  function buildConfigurationEditorMarkers(
    state: ConfigurationStateView | null,
    _draft: FinalEditorSession | null,
    _language: string,
    actionError: { conflictId: string; message: string } | null,
    pendingRequest: ResolveConfigurationConflictRequest | null,
    pendingProjection: FinalConflictProjection | null,
  ): ConfigurationEditorMarker[] {
    const editorMarkers: ConfigurationEditorMarker[] = [];
    const conflicts = [...(state?.workspace.editor.conflicts ?? [])];
    if (pendingProjection && !conflicts.some((item) => item.conflictId === pendingProjection.conflictId)) conflicts.push(pendingProjection);
    for (const conflict of conflicts) {
      editorMarkers.push({
        id: conflict.conflictId,
        kind: 'conflict',
        severity: 'warning',
        messageKey: 'FINAL_EDIT_CONFLICT',
        message: translate('The upstream configuration and your edit changed the same path.'),
        semanticPath: conflict.semanticPath || '/',
        segments: conflict.segments,
        resolvable: true,
        canCombine: conflict.canMerge,
        conflict,
        actionError: actionError?.conflictId === conflict.conflictId ? translate(actionError.message) : undefined,
        retryable: !!pendingRequest && pendingProjection?.conflictId === conflict.conflictId,
        pending: !!pendingRequest,
      });
    }
    for (const change of (state?.workspace.editor.changes ?? [])) {
      editorMarkers.push({
        id: change.editId,
        kind: change.issues.some((issue) => issue.blocking) ? 'validation' : 'source',
        severity: change.issues.some((issue) => issue.blocking) ? 'error' : 'info',
        message: translate('This path differs from the current upstream configuration.'),
        semanticPath: change.semanticPath || '/',
        segments: change.segments,
        changeKind: change.kind,
        issueIds: change.issues.map((issue) => issue.id),
      });
    }
    for (const [index, diagnostic] of (state?.desired.diagnostics ?? []).entries()) {
      if (!diagnostic.location || diagnostic.scope?.surface !== 'configuration') continue;
      editorMarkers.push({
        id: `desired-diagnostic:${index}:${diagnostic.code}`,
        kind: state?.desired.validation === 'invalid' ? 'validation' : 'warning',
        severity: state?.desired.validation === 'invalid' ? 'error' : 'warning',
        message: translate(configurationDiagnosticMessage(diagnostic.messageKey ?? diagnostic.code)),
        messageKey: diagnostic.messageKey ?? diagnostic.code,
        semanticPath: diagnostic.location.semanticPath,
        segments: [],
        documentPath: diagnostic.location.documentPath,
      });
    }
    return editorMarkers;
  }

  function unresolvedDraftConflicts(draft: FinalEditorSession | null) {
    if (!draft) return [];
    const unresolved = new Set(draft.unresolvedConflictIds);
    return draft.conflicts.filter((conflict) => unresolved.has(conflict.conflictId));
  }

  let logView: LogView = 'both';
  let logFilter = '';
  let logContents = { stdout: '', stderr: '' };
  let logTruncated = { stdout: false, stderr: false };
  let logTimer: number | undefined;
  let logRequestKey = '';
  let manualLogRequestKey = '';
  let logGeneration = 0;
  let logScrollProgramId = '';
  let logScrollState = defaultLogScrollState();
  let logScrollRestoring = false;
  let logScrollRestoreGeneration = 0;
  let manualLogRefreshing = false;
  let xrayDashboardSnapshot: XrayDashboardSnapshot | null = null;
  let xrayDashboardError: ErrorInfo | null = null;
  let xrayDashboardRefreshing = false;
  let xrayDashboardManualRefreshing = false;
  let xrayDashboardTimer: number | undefined;
  let xrayTrafficSort: XrayTrafficSort = 'downlink';
  let xrayTrafficSortAscending = false;
  let xrayRoutingBusyTag = '';
  let xrayRoutingGeneration = 0;
  let xrayLoggerBusy = false;
  let xrayPairHeight = initialXrayDashboardLayout.pairHeight;
  let xrayTrafficHeight = initialXrayDashboardLayout.trafficHeight;
  let xrayLayoutSaveTimer: number | undefined;
  let detailConfigHeight = initialProgramDetailLayout.configHeight;
  let programDetailLayoutSaveTimer: number | undefined;
  let mainElement: HTMLElement | null = null;
  let stdoutLogElement: HTMLPreElement | null = null;
  let stderrLogElement: HTMLPreElement | null = null;
  let replacementPackageSource = '';
  let configUpdateStatus: ConfigUpdateStatus | null = null;
  let sourceSaveStatus = '';
  let runtimeArgumentLine = '';
  let environmentEntries: EnvironmentEntry[] = [];
  let createDraft: CreateDraft = defaultDraft('generic');
  let lifecycleBusyIds = new Set<string>();
  const lifecycleGenerations = new Map<string, number>();
  let bulkMode = false;
  let bulkSelectedIds = new Set<string>();
  let bulkBusy = '';
  let bulkGeneration = 0;
  let catalog = loadCatalog();
  let programQuery = '';
  let programFilter: ProgramFilter = 'all';
  let catalogToolsOpen = false;
  let programListElement: HTMLElement | null = null;
  let draggingProgramId = '';
  let suppressProgramClickId = '';
  let dropTargetProgramId = '';
  let dropAfterTarget = false;
  let programDragCleanup: (() => void) | null = null;
  let sidebarCollapsed = loadSidebarCollapsed();
  let sidebarDrawerOpen = false;
  let mobileViewport = false;
  let mobileNavToggleElement: HTMLButtonElement | null = null;
  let CodeEditorView: CodeEditorComponent | null = null;
  let codeEditorLoad: Promise<CodeEditorComponent> | null = null;
  let CreateProgramView: CreateProgramDialogComponent | null = null;
  let SettingsView: SettingsDialogComponent | null = null;
  let AboutView: AboutDialogComponent | null = null;
  let savedRuntimeFingerprint = '';
  let savedSettingsFingerprint = '';
  let privilegeAssessment: PrivilegeAssessment | null = null;
  let privilegeAssessmentLoadingId = '';
  let savedManagedConfigFingerprint = '';
  let savedRuntimeIntegrationFingerprint = '';
  let createDashboardOptionsValue: SingBoxDashboardOptions = {};
  let detailDashboardOptionsValue: SingBoxDashboardOptions = {};
  let savedDashboardOptionsValue: SingBoxDashboardOptions = {};
  let savedXrayDashboardValue: XrayDashboard | undefined;
  let savedMihomoDashboardValue: MihomoDashboard | undefined;

  $: runningCount = programs.filter((program) => program.state.status === 'running').length;
  $: runtimeActiveCount = programs.filter((program) => isRuntimeActive(program.state)).length;
  $: platform = navigator.userAgent.includes('Windows')
    ? 'Windows'
    : navigator.userAgent.includes('Mac')
      ? 'macOS'
      : 'Linux';
  $: issueCount = programs.filter((program) => isIssue(program.state)).length;
  $: autoStartCount = programs.filter((program) => program.autoStart).length;
  $: orderedPrograms = [...programs].sort((left, right) => {
    const leftIndex = catalog.order.indexOf(left.id);
    const rightIndex = catalog.order.indexOf(right.id);
    return (leftIndex < 0 ? Number.MAX_SAFE_INTEGER : leftIndex) -
      (rightIndex < 0 ? Number.MAX_SAFE_INTEGER : rightIndex) ||
      left.name.localeCompare(right.name);
  });
  $: loadingProgram = loadingProgramId
    ? programs.find((program) => program.id === loadingProgramId) ?? null
    : null;
  $: visiblePrograms = orderedPrograms.filter((program) => {
    const query = programQuery.trim().toLocaleLowerCase();
    return (!query || program.name.toLocaleLowerCase().includes(query)) &&
      matchesProgramFilter(program, programFilter);
  });
  $: canReorderPrograms = !programQuery.trim() && programFilter === 'all';
  $: allVisibleSelected = visiblePrograms.length > 0 &&
    visiblePrograms.every((program) => bulkSelectedIds.has(program.id));
  $: if (bulkMode) {
    const visibleIds = new Set(visiblePrograms.map((program) => program.id));
    const nextSelection = new Set(
      [...bulkSelectedIds].filter((id) => visibleIds.has(id)),
    );
    if (nextSelection.size !== bulkSelectedIds.size) bulkSelectedIds = nextSelection;
  }
  $: runtimeArgumentParse = parseArgumentLine(
    runtimeArgumentLine,
    '',
    platform,
  );
  $: createArgumentParse = parseArgumentLine(
    createDraft.argumentLine,
    createDraft.executable,
    platform,
  );
  $: runtimeArgumentView = enrichArgumentResult(
    runtimeArgumentParse,
    detail?.spec.type.kind ?? 'generic',
    !!detail?.spec.managedConfig,
    detail?.spec.type.kind !== 'generic',
  );
  $: createArgumentView = enrichArgumentResult(
    createArgumentParse,
    createDraft.kind,
    createDraft.managedConfiguration,
    createDraft.managedConfiguration || !!createDraft.initialConfig.trim(),
  );
  $: createUsesExplicitConfig = hasExplicitConfig(createDraft.kind, createArgumentParse.args);
  $: createHasStoredConfig =
    createDraft.kind !== 'generic' &&
    (createDraft.managedConfiguration || !!createDraft.initialConfig.trim());
  $: runtimeSettingsChanged =
    !!detail &&
    runtimeFingerprint(detail.spec, runtimeArgumentParse.args, environmentEntries) !==
      savedRuntimeFingerprint;
  $: settingsChanged =
    !!detail &&
    settingsFingerprint(detail.spec, runtimeArgumentParse.args, environmentEntries) !==
      savedSettingsFingerprint;
  $: managedConfigChanged =
    !!detail && managedSourcesFingerprint(detail.spec) !== savedManagedConfigFingerprint;
  $: runtimeIntegrationChanged =
    !!detail && runtimeIntegrationFingerprint(detail.spec) !== savedRuntimeIntegrationFingerprint;
  $: detailsChanged = settingsChanged || runtimeIntegrationChanged;
  $: configEditorDirty = configDocument !== null && configContent !== configDocument.content;
  $: coreAdmissionRejected = !!configurationState
    && configurationState.coreAdmission?.status !== 'admitted';
  $: compatibilityPrimaryLabel = configurationStateLoadError || !configurationState
    ? 'Retry detection'
    : coreAdmissionRejected
      ? 'Replace program'
      : !configEditorDirty && !finalEditorSession?.draftRevision
        && configurationState.workspace.editor.candidateStatus === 'applied'
        ? 'Review Final configuration' : 'Open Final configuration';
  $: compatibilityConclusion = configurationStateLoadError || !configurationState
    ? workspaceErrors.compatibility?.message ?? 'Program detection needs attention.'
    : coreAdmissionRejected
      ? coreAdmissionMessage(configurationState.coreAdmission?.messageKey) ?? 'Program detection needs attention.'
      : configEditorDirty || finalEditorSession?.draftRevision
        ? 'Your changes are not applied yet.'
        : configurationState.workspace.editor.editStatus === 'conflict'
          || configurationState.workspace.editor.candidateStatus === 'invalid'
          ? 'Fix the highlighted setting in Final configuration.'
          : configurationState.workspace.editor.candidateStatus === 'applied'
            ? 'Configuration is up to date.'
            : 'Check your changes in Final configuration.';
  $: compatibilityPrimaryDisabled = !!busy || configurationStateLoadingId === selectedId;
  $: configurationNeedsActivation = !!configurationState && (
    configEditorDirty
    || !!finalEditorSession?.draftRevision
    || configurationState.workspace.editor.candidateStatus !== 'applied'
  );
  $: configurationHasMergeBlocker = !!finalEditorSession?.rebaseRequired
    || finalEditorDraftConflicts.length > 0
    || configurationState?.workspace.editor.editStatus === 'conflict'
    || !!configurationState?.workspace.editor.blockers.some((blocker) => blocker.blocks.includes('save'));
  $: configurationSyntaxInvalid = configSyntaxStatus?.content === configContent
    && configSyntaxStatus?.language === configDocument?.language && configSyntaxStatus.invalid;
  $: configurationEditorHasErrors = configDiagnosticStatus?.content === configContent
    && configDiagnosticStatus?.language === configDocument?.language
    && configDiagnosticStatus.errorCount > 0;
  $: configurationActivationBlocked = !!busy
    || !!pendingConflictRequest
    || !canEditConfigurationByLicense
    || !canRunDiagnosticsByLicense
    || !configurationState
    || !configDocument
    || !configurationNeedsActivation
    || coreAdmissionRejected
    || configurationStateLoadError
    || configurationHasMergeBlocker
    || configurationSyntaxInvalid;
  $: configurationActivationReason = !canEditConfigurationByLicense || !canRunDiagnosticsByLicense
    ? licenseActionHint
    : pendingConflictRequest
      ? 'Retry the pending choice before continuing.'
    : busy
      ? 'Another configuration operation is running.'
      : coreAdmissionRejected || configurationStateLoadError
        ? 'Review program detection in Compatibility.'
      : configurationSyntaxInvalid
        ? 'Fix the highlighted configuration syntax.'
      : configurationHasMergeBlocker
        ? 'Resolve conflicts before continuing.'
        : !configurationNeedsActivation
          ? 'Configuration is up to date.'
          : configSaveRequiresRestart
            ? 'Apply and restart'
            : 'Apply changes';
  $: configSaveRequiresRestart = !!detail && isRuntimeActive(detail.state);
  $: createDashboardOptionsValue = dashboardOptionsFromDraft(createDraft);
  $: detailDashboardOptionsValue = dashboardOptionsFromManagedConfig(
    detail?.spec.managedConfig,
  );
  $: dashboardIsRunning = !!detail && detail.state.status === 'running';
  $: licenseAccess = deriveLicenseAccess(entitlementState, programs.length);
  $: activeLicense = licenseAccess.entitlement;
  $: activeLicenseCapabilities = activeLicense?.claims.capabilities ?? [];
  $: activeLicenseLimits = licenseAccess.limits;
  $: canUseManagedConfigCapability = licenseAccess.canUseManagedSources;
  $: canUseAdvancedDiagnosticsCapability = licenseAccess.canRunAdvancedDiagnostics;
  $: canUseRemoteDashboardCapability = licenseAccess.canOpenRemoteDashboard;
  $: canActivateProgramsByLicense = canUseProgramLifecycleAction(licenseAccess, 'start');
  $: canEditConfigurationByLicense = licenseAccess.canUseLocalPrograms;
  $: canRunDiagnosticsByLicense = licenseAccess.canRunAdvancedDiagnostics;
  $: maxProgramsLimit = activeLicenseLimits?.max_programs;
  $: maxConfigSourcesLimit = effectiveConfigSourceLimit(
    activeLicenseLimits?.max_config_sources_per_program,
  );
  $: programLimitReached = licenseAccess.programLimitReached;
  $: canCreateProgramByLicense = licenseAccess.canCreateProgram;
  $: createLicenseBlockReason = !activeLicense
    ? 'Activate this device with a valid license to create programs.'
    : !licenseAccess.configurationValid
      ? 'The signed license policy is incomplete. Refresh the license or contact support.'
      : programLimitReached
        ? 'The program limit for this license has been reached.'
        : '';
  $: licenseActionHint = !activeLicense ? 'Activate device to continue' : 'License plan needed';
  $: dashboardCanOpen = dashboardIsRunning && canUseRemoteDashboardCapability;
  $: detailXrayDashboardValue = detail?.spec.managedConfig?.xrayDashboard;
  $: detailMihomoDashboardValue = detail?.spec.managedConfig?.mihomoDashboard;
  $: xrayDashboardEnabled = !!detail && detail.spec.type.kind === 'xray' && !!savedXrayDashboardValue;
  $: xrayDashboardCanRefresh = xrayDashboardEnabled && dashboardIsRunning && canUseRemoteDashboardCapability && !busy && !xrayRoutingBusyTag;
  $: saveRequiresStop =
    runtimeSettingsChanged && !!detail && isRuntimeActive(detail.state);
  $: saveRequiresRestart = saveRequiresStop;
  $: filteredStdout = filterLog(logContents.stdout, logFilter);
  $: filteredStderr = filterLog(logContents.stderr, logFilter);
  $: if ($uiLanguage && notificationLicenseNotice) {
    notification = licenseNoticeInfo(notificationLicenseNotice);
  }
  $: if (showCreate) saveCreateDraft(createDraft);
  $: theme = resolveColorScheme(colorMode, systemDark);
  $: syncAppearance(appearanceTheme, colorMode, theme, uiScale);

  function saveXrayDashboardLayout() {
    try {
      localStorage.setItem(
        xrayDashboardLayoutKey,
        JSON.stringify({
          pairHeight: xrayPairHeight,
          trafficHeight: xrayTrafficHeight,
        } satisfies XrayDashboardLayout),
      );
    } catch {
      // Layout persistence is a UI convenience and should never block the app.
    }
  }

  function saveProgramDetailLayout() {
    try {
      localStorage.setItem(
        programDetailLayoutKey,
        JSON.stringify({
          configHeight: detailConfigHeight,
        } satisfies ProgramDetailLayout),
      );
    } catch {
      // Layout persistence is a UI convenience and should never block the app.
    }
  }

  function scheduleXrayDashboardLayoutSave() {
    if (xrayLayoutSaveTimer !== undefined) window.clearTimeout(xrayLayoutSaveTimer);
    xrayLayoutSaveTimer = window.setTimeout(() => {
      xrayLayoutSaveTimer = undefined;
      saveXrayDashboardLayout();
    }, 180);
  }

  function scheduleProgramDetailLayoutSave() {
    if (programDetailLayoutSaveTimer !== undefined) {
      window.clearTimeout(programDetailLayoutSaveTimer);
    }
    programDetailLayoutSaveTimer = window.setTimeout(() => {
      programDetailLayoutSaveTimer = undefined;
      saveProgramDetailLayout();
    }, 180);
  }

  function resizeStyle(height: number | undefined) {
    return height ? `--panel-resize-height: ${height}px;` : '';
  }

  function setResizeHeight(kind: ResizeKind, height: number) {
    if (kind === 'xray:pair') {
      xrayPairHeight = clampResizeHeight(
        height,
        xrayPairResizeMinHeight,
        xrayPairResizeMaxHeight,
      );
      return;
    }
    if (kind === 'xray:traffic') {
      xrayTrafficHeight = clampResizeHeight(
        height,
        xrayTrafficResizeMinHeight,
        xrayTrafficResizeMaxHeight,
      );
      return;
    }
    detailConfigHeight = clampDetailPaneResizeHeight(height);
  }

  function scheduleResizeLayoutSave(kind: ResizeKind) {
    if (kind.startsWith('xray:')) scheduleXrayDashboardLayoutSave();
    else scheduleProgramDetailLayoutSave();
  }

  function resizeBounds(kind: ResizeKind) {
    if (kind === 'xray:pair') {
      return { min: xrayPairResizeMinHeight, max: xrayPairResizeMaxHeight };
    }
    if (kind === 'xray:traffic') {
      return { min: xrayTrafficResizeMinHeight, max: xrayTrafficResizeMaxHeight };
    }
    return { min: detailPaneResizeMinHeight, max: detailPaneResizeMaxHeight };
  }

  function resizeValue(kind: ResizeKind) {
    if (kind === 'xray:pair') return xrayPairHeight ?? xrayPairResizeDefaultHeight;
    if (kind === 'xray:traffic') {
      return xrayTrafficHeight ?? xrayTrafficResizeDefaultHeight;
    }
    return detailConfigHeight ?? 720;
  }

  function beginResizeFromHandle(event: PointerEvent, kind: ResizeKind) {
    if (event.button !== 0) return;
    const handle = event.currentTarget as HTMLElement;
    const container = handle.parentElement;
    if (!container) return;
    event.preventDefault();
    const pointerId = event.pointerId;
    const startY = event.clientY;
    const startHeight = container.getBoundingClientRect().height;
    try {
      handle.setPointerCapture?.(pointerId);
    } catch {
      // Synthetic pointer events used by automation do not own an active pointer.
    }
    container.classList.add('panel-resizing');
    const move = (moveEvent: PointerEvent) => {
      if (moveEvent.pointerId !== pointerId) return;
      setResizeHeight(kind, startHeight + moveEvent.clientY - startY);
    };
    const finish = (finishEvent?: Event) => {
      if (finishEvent instanceof PointerEvent && finishEvent.pointerId !== pointerId) return;
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', finish);
      window.removeEventListener('pointercancel', finish);
      window.removeEventListener('blur', finish);
      if (handle.hasPointerCapture?.(pointerId)) handle.releasePointerCapture(pointerId);
      container.classList.remove('panel-resizing');
      scheduleResizeLayoutSave(kind);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', finish);
    window.addEventListener('pointercancel', finish);
    window.addEventListener('blur', finish);
  }

  function handleResizeKeydown(event: KeyboardEvent, kind: ResizeKind) {
    if (!['ArrowUp', 'ArrowDown', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    const bounds = resizeBounds(kind);
    const step = event.shiftKey ? 64 : 16;
    const next = event.key === 'Home'
      ? bounds.min
      : event.key === 'End'
        ? bounds.max
        : resizeValue(kind) + (event.key === 'ArrowDown' ? step : -step);
    setResizeHeight(kind, next);
    scheduleResizeLayoutSave(kind);
  }

  onMount(() => {
    const listenerScope = createAsyncListenerScope(reportGlobalError);
    const licenseAuxiliaryRefreshTimer = window.setInterval(
      refreshVisibleLicenseData,
      licenseAuxiliaryAutoRefreshMinIntervalMs,
    );
    const refreshLicenseDataAfterAttention = () => refreshVisibleLicenseDataAfterAttention();
    const refreshLicenseDataAfterVisibility = () => {
      if (document.visibilityState === 'visible') refreshVisibleLicenseDataAfterAttention();
    };
    const reportMountError = (error: unknown) => {
      if (listenerScope.active()) reportGlobalError(error);
    };
    const colorScheme = matchMedia('(prefers-color-scheme: dark)');
    const mobileLayout = matchMedia('(max-width: 899px)');
    const colorSchemeChanged = (event: MediaQueryListEvent) => (systemDark = event.matches);
    const mobileLayoutChanged = (event: MediaQueryListEvent) => {
      mobileViewport = event.matches;
      closeSidebarDrawer(false);
    };
    mobileViewport = mobileLayout.matches;
    colorScheme.addEventListener('change', colorSchemeChanged);
    mobileLayout.addEventListener('change', mobileLayoutChanged);
    const beforeUnload = (event: BeforeUnloadEvent) => {
      if (configEditorDirty || detailsChanged || managedConfigChanged) {
        event.preventDefault();
        event.returnValue = '';
      }
    };
    const suppressBrowserMenu = (event: MouseEvent) => {
      const target = event.target instanceof Element ? event.target : null;
      if (target?.closest('input, textarea, [contenteditable="true"], .cm-editor')) return;
      event.preventDefault();
      if (!target?.closest('.program-item')) closeProgramMenu(false);
    };
    const suppressBrowserShortcut = (event: KeyboardEvent) => {
      if (event.key === 'F5' || ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'r')) {
        event.preventDefault();
      }
    };
    window.addEventListener('beforeunload', beforeUnload);
    window.addEventListener('contextmenu', suppressBrowserMenu);
    window.addEventListener('keydown', suppressBrowserShortcut);
    window.addEventListener('online', refreshLicenseDataAfterAttention);
    document.addEventListener('visibilitychange', refreshLicenseDataAfterVisibility);
    listenerScope.track(getCurrentWindow().onThemeChanged(({ payload }) => {
      if (!listenerScope.active()) return;
      if (colorMode === 'system') systemDark = payload === 'dark';
    }));
    listenerScope.track(getCurrentWindow().onFocusChanged(({ payload }) => {
      if (listenerScope.active() && payload) refreshVisibleLicenseDataAfterAttention();
    }));
    listenerScope.track(api.onManagerEvent((event) => {
      if (!listenerScope.active()) return;
      if (event.type === 'programListChanged') {
        void refreshPrograms().catch(reportMountError);
        return;
      }
      if (event.type === 'programAutoStartPrivilegeRequired') {
        const names = event.ids.map((id) => programs.find((program) => program.id === id)?.name ?? id);
        const message = `${translate('Some programs were not started automatically')}: ${names.join(', ')}`;
        showNotification({
          code: 'PRIVILEGE_REQUIRED',
          title: translate('Background start needs attention'),
          message,
          fallbackMessage: message,
          details: '',
          suggestion: translate('Open each program and start it manually to review administrator authorization.'),
        }, null);
        return;
      }
      if (!programs.some((program) => program.id === event.id)) {
        void refreshPrograms().catch(reportMountError);
        return;
      }
      programs = programs.map((program) =>
        program.id === event.id ? { ...program, state: event.state } : program,
      );
      if (detail?.spec.id === event.id) detail = { ...detail, state: event.state };
    }));
    listenerScope.track(api.onAutomaticConfigUpdate((event) => {
      if (listenerScope.active()) void handleAutomaticConfigUpdate(event);
    }));
    listenerScope.track(api.onOpenCreateProgram(() => {
      if (listenerScope.active()) openCreate();
    }));
    listenerScope.track(api.onOpenAbout(() => {
      if (listenerScope.active()) void openAbout();
    }));
    listenerScope.track(api.onSelectProgram((programId) => {
      if (listenerScope.active()) void selectProgram(programId).catch(reportMountError);
    }));
    listenerScope.track(api.onLicenseAuthorizationCallback((event) => {
      if (listenerScope.active()) void handleLicenseAuthorizationCallback(event);
    }));
    listenerScope.track(api.onLicenseAuthorizationFailed((event) => {
      if (listenerScope.active()) handleLicenseAuthorizationFailed(event);
    }));
    listenerScope.track(api.onLicenseStateChanged((event) => {
      if (listenerScope.active()) void handleLicenseStateChanged(event).catch(reportMountError);
    }));
    void (async () => {
      try {
        const pendingIntent = await api.frontendReady();
        if (!listenerScope.active()) return;
        if (pendingIntent?.type === 'createProgram') openCreate();
        if (pendingIntent?.type === 'about') void openAbout();
        if (pendingIntent?.type === 'selectProgram') {
          void selectProgram(pendingIntent.programId).catch(reportMountError);
        }
      } catch (value) {
        reportMountError(value);
      }
      try {
        await refreshPrograms();
        if (!listenerScope.active()) return;
      } catch (value) {
        reportMountError(value);
      }
      const [
        settingsResult,
        applicationInfoResult,
        entitlementResult,
        licenseSettingsResult,
        localLicenseDeviceResult,
      ] = await Promise.allSettled([
        api.getAppSettings(),
        api.getApplicationInfo(),
        api.getEntitlementState(),
        api.getLicenseServiceSettings(),
        api.getLocalLicenseDevice(),
      ]);
      if (!listenerScope.active()) return;
      if (applicationInfoResult.status === 'fulfilled') {
        applicationInfo = applicationInfoResult.value;
      } else {
        aboutError = errorInfoOf(applicationInfoResult.reason);
      }
      if (settingsResult.status === 'fulfilled') {
        const nextAppSettings = normalizeAppSettings(settingsResult.value);
        if (nextAppSettings.language) {
          setLanguage(nextAppSettings.language);
          appSettings = nextAppSettings;
        } else {
          appSettings = { ...nextAppSettings, language: get(uiLanguage) };
          try {
            await api.setAppSettings(appSettings);
            if (!listenerScope.active()) return;
          } catch (value) {
            appSettingsError = errorInfoOf(value);
          }
        }
      } else {
        appSettingsError = errorInfoOf(settingsResult.reason);
        showNotification(appSettingsError, null, 12_000);
      }
      if (entitlementResult.status === 'fulfilled') {
        await applyEntitlementSnapshot(entitlementResult.value, 'startup');
        if (!listenerScope.active()) return;
      } else {
        licenseError = errorInfoOf(entitlementResult.reason);
      }
      if (licenseSettingsResult.status === 'fulfilled') {
        licenseServiceSettings = licenseSettingsResult.value;
      } else if (!licenseError) {
        licenseError = errorInfoOf(licenseSettingsResult.reason);
      }
      if (localLicenseDeviceResult.status === 'fulfilled') {
        localLicenseDevice = localLicenseDeviceResult.value;
      }
      try {
        appAutostart = await api.getAutostart();
        if (!listenerScope.active()) return;
      } catch (value) {
        reportMountError(value);
      }
      startLicenseAuthorizationCallbackPolling();
    })().catch(reportMountError);
    return () => {
      listenerScope.dispose();
      stopLicenseAuthorizationCallbackPolling();
      clearLicenseAuthorizationTimeout();
      stopLogPolling();
      stopXrayDashboardPolling();
      if (behaviorSavedTimer !== undefined) window.clearTimeout(behaviorSavedTimer);
      if (xrayLayoutSaveTimer !== undefined) window.clearTimeout(xrayLayoutSaveTimer);
      if (programDetailLayoutSaveTimer !== undefined) window.clearTimeout(programDetailLayoutSaveTimer);
      if (notificationTimer !== undefined) window.clearTimeout(notificationTimer);
      clearConfigOutput();
      if (licensePromptTimer !== undefined) window.clearTimeout(licensePromptTimer);
      window.clearInterval(licenseAuxiliaryRefreshTimer);
      colorScheme.removeEventListener('change', colorSchemeChanged);
      mobileLayout.removeEventListener('change', mobileLayoutChanged);
      window.removeEventListener('beforeunload', beforeUnload);
      window.removeEventListener('contextmenu', suppressBrowserMenu);
      window.removeEventListener('keydown', suppressBrowserShortcut);
      window.removeEventListener('online', refreshLicenseDataAfterAttention);
      document.removeEventListener('visibilitychange', refreshLicenseDataAfterVisibility);
    };
  });

  function syncAppearance(
    appearanceTheme: ThemeId,
    mode: ColorMode,
    effective: EffectiveColorScheme,
    scale: UiScale,
  ) {
    const preferences = { version: 3 as const, theme: appearanceTheme, colorMode: mode, scale };
    applyAppearancePreferences(preferences, effective === 'dark');
    saveAppearancePreferences(preferences);
    void getCurrentWindow().setTheme(mode === 'system' ? null : effective).catch(() => {});
    void getCurrentWebview().setZoom(scale).catch(() => {});
  }

  async function refreshPrograms() {
    const generation = ++programRefreshGeneration;
    const [nextPrograms, nextInvalidPrograms] = await Promise.all([
      api.listPrograms(),
      api.listInvalidPrograms(),
    ]);
    if (generation !== programRefreshGeneration) return;
    programs = nextPrograms;
    invalidPrograms = nextInvalidPrograms;
    const nextCatalog = reconcileCatalog(catalog, programs.map((program) => program.id));
    if (JSON.stringify(nextCatalog) !== JSON.stringify(catalog)) {
      catalog = nextCatalog;
      saveCatalog(catalog);
    }
    bulkSelectedIds = new Set(
      [...bulkSelectedIds].filter((id) => programs.some((program) => program.id === id)),
    );
    if (selectedId && !programs.some((program) => program.id === selectedId)) {
      selectedId = '';
      detail = null;
      privilegeAssessment = null;
      privilegeAssessmentLoadingId = '';
      resetLogScrollState('');
    } else if (selectedId && detail) {
      const selected = programs.find((program) => program.id === selectedId);
      if (selected) detail = { ...detail, state: selected.state };
    }
  }

  async function openCreate(kind: ProgramKind = 'generic') {
    if (!canCreateProgramByLicense) {
      showLicensePrompt(createLicenseBlockReason);
      return;
    }
    createReturnFocus = activeFocusTarget();
    try {
      CreateProgramView ??= (await import('./features/programs/CreateProgramDialog.svelte')).default;
    } catch (value) {
      reportGlobalError(value);
      return;
    }
    createDraft = loadCreateDraft(kind, platform);
    createError = null;
    createFieldErrors = {};
    closeSidebarDrawer();
    showCreate = true;
  }

  function closeCreateDialog() {
    showCreate = false;
    const returnFocus = createReturnFocus;
    createReturnFocus = null;
    void restoreFocusAfterModal(returnFocus);
  }

  function changeCreateKind(kind: ProgramKind) {
    saveCreateDraft(createDraft);
    createDraft = loadCreateDraft(kind, platform);
    createError = null;
    createFieldErrors = {};
  }

  function changeCreateMode(mode: 'managed' | 'external') {
    if (mode === createDraft.mode) return;
    createDraft.mode = mode;
    if (mode === 'external' && !isAbsoluteHostPath(createDraft.executable)) {
      createDraft.executable = '';
    } else if (mode === 'managed' && isAbsoluteHostPath(createDraft.executable)) {
      createDraft.executable = defaultDraft(createDraft.kind, platform).executable;
    }
    createFieldErrors = {};
  }

  function resetDraft() {
    createDraft = clearCreateDraft(createDraft.kind, platform);
    createError = null;
    createFieldErrors = {};
  }

  async function selectProgram(id: string) {
    closeSidebarDrawer();
    if (id === loadingProgramId) return;
    if (id === selectedId && detail) {
      cancelProgramSelection();
      return;
    }
    if (!(await confirmDetailDiscard())) return;
    if (id === loadingProgramId) return;
    if (id === selectedId && detail) return;
    captureVisibleLogScrollState();
    stopLogPolling();
    stopXrayDashboardPolling();
    invalidateLogRequests();
    const generation = ++selectionGeneration;
    loadingProgramId = id;
    let nextDetail: ProgramDetail;
    let nextActions: ActionDescriptor[];
    try {
      [nextDetail, nextActions] = await Promise.all([
        api.getProgram(id),
        api.listActions(id),
      ]);
    } catch (error) {
      if (generation === selectionGeneration) {
        loadingProgramId = '';
        resumeSelectedProgramPolling();
      }
      throw error;
    }
    if (generation !== selectionGeneration) return;
    selectedId = id;
    detail = nextDetail;
    privilegeAssessment = null;
    privilegeAssessmentLoadingId = id;
    actions = nextActions;
    if (hasEditableConfig(nextDetail.spec)) void ensureCodeEditor();
    activeTab = 'overview';
    workspaceErrors = {};
    workspaceErrorActions = {};
    workspaceErrorBusyScope = '';
    clearConfigurationError();
    configDocument = null;
    configurationState = null;
    configurationStateLoadingId = '';
    configurationStateLoadError = false;
    finalEditorSession = null;
    configDiagnosticStatus = null;
    selectedEditorPath = '';
    focusSemanticPath = '';
    pathFocusMessage = '';
    resetConfigurationSchemaState();
    configContent = '';
    clearConfigOutput();
    configUpdateStatus = null;
    sourceSaveStatus = '';
    xrayDashboardSnapshot = null;
    xrayDashboardError = null;
    xrayDashboardRefreshing = false;
    xrayDashboardManualRefreshing = false;
    xrayRoutingBusyTag = '';
    xrayLoggerBusy = false;
    xrayRoutingGeneration += 1;
    logContents = { stdout: '', stderr: '' };
    logTruncated = { stdout: false, stderr: false };
    logFilter = '';
    resetLogScrollState(id);
    replacementPackageSource = '';
    runtimeArgumentLine = formatArgumentLine(programArgs(detail.spec));
    environmentEntries = Object.entries(detail.spec.environment).map(([key, value]) => ({
      key,
      value,
    }));
    savedRuntimeFingerprint = runtimeFingerprint(
      detail.spec,
      programArgs(detail.spec),
      environmentEntries,
    );
    savedSettingsFingerprint = settingsFingerprint(
      detail.spec,
      programArgs(detail.spec),
      environmentEntries,
    );
    savedManagedConfigFingerprint = managedSourcesFingerprint(detail.spec);
    savedRuntimeIntegrationFingerprint = runtimeIntegrationFingerprint(detail.spec);
    savedDashboardOptionsValue = dashboardOptionsFromManagedConfig(detail.spec.managedConfig);
    savedXrayDashboardValue = detail.spec.managedConfig?.xrayDashboard;
    savedMihomoDashboardValue = detail.spec.managedConfig?.mihomoDashboard;
    loadingProgramId = '';
    void loadSelectedPrivilegeAssessment(id);
    if (nextDetail.spec.type.kind !== 'generic') void loadConfigurationStateSummary(id);
  }

  function requestConfigurationWorkspace(programId: string): Promise<ConfigurationWorkspaceSnapshot> {
    if (
      configurationWorkspaceRequest
      && configurationWorkspaceRequestProgramId === programId
    ) {
      return configurationWorkspaceRequest;
    }
    const request = api.getConfigurationWorkspace(programId);
    configurationWorkspaceRequestProgramId = programId;
    configurationWorkspaceRequest = request;
    void request.finally(() => {
      if (configurationWorkspaceRequest === request) {
        configurationWorkspaceRequest = null;
        configurationWorkspaceRequestProgramId = '';
      }
    }).catch(() => undefined);
    return request;
  }

  async function requestConfigurationState(programId: string): Promise<ConfigurationStateView> {
    return (await requestConfigurationWorkspace(programId)).state;
  }

  type WorkspaceAdoption = 'authoritative' | 'preserveTyping';

  /** Apply a complete state and editor-session snapshot without rebuilding it in the UI. */
  async function adoptConfigurationWorkspace(
    programId: string,
    snapshot: ConfigurationWorkspaceSnapshot,
    adoption: WorkspaceAdoption = 'authoritative',
  ): Promise<void> {
    if (selectedId !== programId) return;
    if (
      configurationState
      && snapshot.state.stateRevision < configurationState.stateRevision
    ) return;
    if (snapshot.editorSession && finalEditorSession
      && snapshot.editorSession.sessionId === finalEditorSession.sessionId
      && snapshot.editorSession.draftRevision < finalEditorSession.draftRevision) return;
    const localEditorDraft = adoption === 'preserveTyping' && (configEditorDirty || finalEditorTextChanged)
      ? configContent
      : null;
    const state = snapshot.state;
    const draft = snapshot.editorSession ?? null;
    configurationState = state;
    configurationStateLoadError = false;
    finalEditorSession = draft;
    const draftContent = draft
      && draft.draftRevision > 0
      && !draft.rebaseRequired
      && draft.basedOnCandidateGeneration === state.generation
      ? draft.workingContent
      : state.workspace.editor.document.content;
    if (localEditorDraft !== null) {
      configContent = localEditorDraft;
      finalEditorAutosaveContent = draft?.workingContent ?? state.workspace.editor.document.content;
    } else {
      configContent = draftContent;
      finalEditorAutosaveContent = draftContent;
      finalEditorInputBaseContent = draft?.baseContent ?? state.workspace.editor.document.content;
    }
    if (configDocument) {
      configDocument = {
        ...configDocument,
        content: state.workspace.editor.document.content,
        baseHash: state.desired.revision.contentHash,
        language: state.format,
      };
    }
    // Authoritative editor blockers own validation feedback; action notices are transient.
    clearConfigOutput();
  }


  async function loadConfigurationStateSummary(programId: string): Promise<void> {
    if (selectedId !== programId || detail?.spec.type.kind === 'generic') return;
    clearWorkspaceError('compatibility');
    configurationStateLoadingId = programId;
    configurationStateLoadError = false;
    try {
      const snapshot = await requestConfigurationWorkspace(programId);
      if (selectedId !== programId || detail?.spec.id !== programId) return;
      await adoptConfigurationWorkspace(programId, snapshot, 'preserveTyping');
    } catch (error) {
      if (selectedId === programId && detail?.spec.id === programId) {
        configurationStateLoadError = true;
        reportWorkspaceError('compatibility', error, 'configuration-load', async () => { await loadConfigurationStateSummary(programId); });
      }
    } finally {
      if (configurationStateLoadingId === programId) configurationStateLoadingId = '';
    }
  }

  async function loadSelectedPrivilegeAssessment(id: string) {
    await tick();
    if (selectedId !== id || detail?.spec.id !== id) return;
    const assessment = await api
      .getProgramPrivilegeAssessment(id)
      .catch(() => null);
    if (selectedId !== id || detail?.spec.id !== id) return;
    privilegeAssessment = assessment;
    privilegeAssessmentLoadingId = '';
  }

  function cancelProgramSelection() {
    if (!loadingProgramId) return;
    selectionGeneration += 1;
    loadingProgramId = '';
    resumeSelectedProgramPolling();
  }

  function resumeSelectedProgramPolling() {
    if (!selectedId) return;
    if (activeTab === 'logs') startLogPolling();
    if (activeTab === 'dashboard') startXrayDashboardPolling();
  }

  async function goHome() {
    if (!(await confirmDetailDiscard())) return;
    captureVisibleLogScrollState();
    stopLogPolling();
    stopXrayDashboardPolling();
    invalidateLogRequests();
    selectionGeneration += 1;
    loadingProgramId = '';
    selectedId = '';
    detail = null;
    privilegeAssessment = null;
    privilegeAssessmentLoadingId = '';
    resetLogScrollState('');
    activeTab = 'overview';
    workspaceErrors = {};
    workspaceErrorActions = {};
    workspaceErrorBusyScope = '';
    clearConfigurationError();
    configDocument = null;
    configurationState = null;
    configurationStateLoadingId = '';
    configurationStateLoadError = false;
    finalEditorSession = null;
    configDiagnosticStatus = null;
    selectedEditorPath = '';
    focusSemanticPath = '';
    pathFocusMessage = '';
    resetConfigurationSchemaState();
    configContent = '';
    clearConfigOutput();
    sourceSaveStatus = '';
    closeProgramMenu(false);
    closeSidebarDrawer();
    savedRuntimeFingerprint = '';
    savedSettingsFingerprint = '';
    savedManagedConfigFingerprint = '';
    savedRuntimeIntegrationFingerprint = '';
    savedDashboardOptionsValue = {};
    savedXrayDashboardValue = undefined;
    savedMihomoDashboardValue = undefined;
    xrayDashboardSnapshot = null;
    xrayDashboardError = null;
    xrayDashboardRefreshing = false;
    xrayDashboardManualRefreshing = false;
    xrayRoutingBusyTag = '';
    xrayLoggerBusy = false;
    xrayRoutingGeneration += 1;
  }

  async function confirmDetailDiscard() {
    if (!configEditorDirty && !detailsChanged && !managedConfigChanged) return true;
    const parts = [
      detailsChanged || managedConfigChanged ? translate('program settings') : '',
      configEditorDirty ? translate('configuration') : '',
    ].filter(Boolean);
    return askConfirmation(
      translate('Discard unsaved changes?'),
      `${translate('Unsaved')} ${parts.join(` ${translate('and')} `)} ${translate('changes will be lost.')}`,
      translate('Discard changes'),
      true,
    );
  }

  function askConfirmation(
    title: string,
    message: string,
    confirmLabel: string,
    danger = false,
  ) {
    if (confirmation) return Promise.resolve(false);
    return new Promise<boolean>((resolve) => {
      confirmationReturnFocus = activeFocusTarget();
      confirmation = { title, message, confirmLabel, danger, resolve };
    });
  }

  function resolveConfirmation(confirmed: boolean) {
    const request = confirmation;
    confirmation = null;
    const returnFocus = confirmationReturnFocus;
    confirmationReturnFocus = null;
    request?.resolve(confirmed);
    void restoreFocusAfterModal(returnFocus);
  }

  const configurationMutationQueues = new Map<string, Promise<void>>();

  async function mutate(
    label: string,
    operation: () => Promise<unknown>,
    onError: (value: unknown) => unknown | Promise<unknown> = reportGlobalError,
  ) {
    if (busy) return false;
    busy = label;
    try {
      await operation();
      return true;
    } catch (value) {
      await onError(value);
      return false;
    } finally {
      if (busy === label) busy = '';
    }
  }

  /** Serialize configuration writes per program while preserving the
   * operation's existing idempotent error/retry handling. */
  async function mutateConfiguration(
    programId: string,
    label: string,
    operation: () => Promise<unknown>,
    onError: (value: unknown) => unknown | Promise<unknown> = reportGlobalError,
  ): Promise<boolean> {
    const previous = configurationMutationQueues.get(programId) ?? Promise.resolve();
    const run = previous.catch(() => undefined).then(() => mutate(label, operation, onError));
    const settled = run.then(() => undefined, () => undefined);
    configurationMutationQueues.set(programId, settled);
    try {
      return await run;
    } finally {
      if (configurationMutationQueues.get(programId) === settled) {
        configurationMutationQueues.delete(programId);
      }
    }
  }

  async function mutateLicenseTeam(
    label: string,
    operation: () => Promise<void>,
    onError: (value: unknown) => unknown | Promise<unknown>,
  ) {
    if (busy) throw new Error('another mutation is already in progress');
    busy = label;
    try {
      await operation();
    } catch (value) {
      await onError(value);
      throw value;
    } finally {
      if (busy === label) busy = '';
    }
  }

  async function runExternalAction(
    key: string,
    operation: () => Promise<unknown>,
    onError: (value: unknown) => unknown | Promise<unknown> = reportGlobalError,
  ) {
    if (pendingExternalActions.has(key)) return false;
    pendingExternalActions.add(key);
    try {
      await operation();
      return true;
    } catch (value) {
      await onError(value);
      return false;
    } finally {
      pendingExternalActions.delete(key);
    }
  }

  function logLicenseFlow(
    level: 'warn' | 'info' | 'debug',
    message: string,
  ) {
    void api.logFrontendEvent(level, `license.${message}`).catch(() => null);
  }

  function licenseStateIsActive(state: EntitlementState | null) {
    return state?.status === 'active';
  }

  function licenseAuxiliaryScopeOf(state: EntitlementState | null) {
    if (!licenseStateIsActive(state)) return '';
    const claims = state.entitlement.claims;
    return JSON.stringify([
      claims.sub,
      claims.licenseId,
      claims.deviceId,
      claims.licenseEpoch,
      claims.plan,
      claims.planRevision,
      claims.policyHash,
      claims.licenseStatus,
      [...claims.workspacePermissions].sort(),
    ]);
  }

  function clearLicenseAuxiliaryState() {
    licenseDevices = [];
    licenseDevicesNextCursor = null;
    licenseDevicesLastLoadedAt = 0;
    licenseBillingSummary = null;
    licenseBillingError = null;
    licenseBillingLastLoadedAt = 0;
    licenseTeamProfile = null;
    licenseTeamMembers = [];
    licenseTeamMembersNextCursor = null;
    licenseTeamMembersHasMore = false;
    licenseTeamMembersLoadingMore = false;
    if (licenseTeamInvitation) licenseTeamInvitation.invitationToken = '';
    if (licenseTeamDeviceEnrollment) licenseTeamDeviceEnrollment.enrollmentToken = '';
    licenseTeamInvitation = null;
    licenseTeamDeviceEnrollment = null;
    licenseTeamSecretGeneration += 1;
    licenseTeamError = null;
    licenseTeamLastLoadedAt = 0;
  }

  function refreshLicenseAuxiliaryAfterStaleRequest(generation: number) {
    if (generation !== licenseAuxiliaryGeneration) {
      queueMicrotask(() => refreshVisibleLicenseData(true));
    }
  }

  async function applyEntitlementSnapshot(
    snapshot: EntitlementSnapshot,
    reason = 'state-sync',
  ) {
    if (!isNewerEntitlementSnapshot(snapshot, entitlementSnapshotGeneration)) return;
    entitlementSnapshotGeneration = snapshot.generation;
    await applyEntitlementState(snapshot.entitlementState, reason);
  }

  async function reconcileEntitlementState(reason: string) {
    const snapshot = await api.getEntitlementState();
    await applyEntitlementSnapshot(snapshot, reason);
  }

  async function reconcileEntitlementStateAfterFailure(reason: string) {
    try {
      await reconcileEntitlementState(reason);
    } catch {
      logLicenseFlow('warn', 'state-reconcile-failed');
    }
  }

  async function applyEntitlementState(
    nextState: EntitlementState,
    _reason = 'state-sync',
  ) {
    const generation = ++licenseStateEffectGeneration;
    const wasActive = licenseStateIsActive(entitlementState);
    const isActive = licenseStateIsActive(nextState);
    const runtimeImpact = licenseRuntimeImpact(nextState);
    const nextAuxiliaryScope = licenseAuxiliaryScopeOf(nextState);
    if (nextAuxiliaryScope !== licenseAuxiliaryScope) {
      licenseAuxiliaryScope = nextAuxiliaryScope;
      licenseAuxiliaryGeneration += 1;
      clearLicenseAuxiliaryState();
    }
    entitlementState = nextState;
    if (isActive && licenseAuthorizationRequest) resetLicenseAuthorizationProgress();
    const currentStateNotice = licenseStateNotice(nextState, applicationInfo?.version ?? '');
    if (notificationLicenseNotice && (
      !currentStateNotice
      || currentStateNotice.title !== notificationLicenseNotice.title
      || currentStateNotice.message !== notificationLicenseNotice.message
    )) dismissNotification();
    if (isActive && notification?.code === 'LICENSE_REQUIRED') dismissNotification();
    if (!isActive) {
      if (runtimeImpact === 'hardInactive' || wasActive || runtimeActiveCount > 0) {
        logLicenseFlow('info', 'runtime-sync');
        stopXrayDashboardPolling();
        xrayDashboardSnapshot = null;
        xrayDashboardError = null;
        xrayDashboardRefreshing = false;
        xrayDashboardManualRefreshing = false;
        xrayRoutingBusyTag = '';
        xrayRoutingGeneration += 1;
        await refreshPrograms().catch(reportGlobalError);
      }
      if (generation !== licenseStateEffectGeneration) return;
      return;
    }
    if (
      nextState.entitlement.claims.licenseStatus === 'active'
      && !licenseStateNotice(nextState, applicationInfo?.version ?? '')
    ) {
      lastLicenseNoticeKey = '';
    }
    refreshVisibleLicenseData();
  }

  async function syncLicenseState(reportError = false) {
    if (licenseStatusRequestActive) return;
    licenseStatusRequestActive = true;
    try {
      const [entitlementResult, settingsResult] = await Promise.allSettled([
        api.getEntitlementState(),
        api.getLicenseServiceSettings(),
      ]);
      if (entitlementResult.status === 'fulfilled') {
        await applyEntitlementSnapshot(entitlementResult.value, 'status-sync');
      } else if (reportError) {
        licenseError = errorInfoOf(entitlementResult.reason);
      }
      if (settingsResult.status === 'fulfilled') {
        licenseServiceSettings = settingsResult.value;
      } else if (reportError && !licenseError) {
        licenseError = errorInfoOf(settingsResult.reason);
      }
      if (entitlementResult.status === 'fulfilled' && settingsResult.status === 'fulfilled' && reportError) {
        licenseError = null;
      }
    } finally {
      licenseStatusRequestActive = false;
    }
  }

  function startLicenseAuthorizationCallbackPolling() {
    stopLicenseAuthorizationCallbackPolling();
    licenseAuthorizationCallbackTimer = window.setInterval(() => {
      void pollLicenseAuthorizationCallback().catch((value) => {
        if (showSettings) licenseError = errorInfoOf(value);
      });
    }, 1000);
  }

  function stopLicenseAuthorizationCallbackPolling() {
    if (licenseAuthorizationCallbackTimer !== undefined) {
      window.clearInterval(licenseAuthorizationCallbackTimer);
      licenseAuthorizationCallbackTimer = undefined;
    }
  }

  function clearLicenseAuthorizationTimeout() {
    if (licenseAuthorizationTimeoutTimer !== undefined) {
      window.clearTimeout(licenseAuthorizationTimeoutTimer);
      licenseAuthorizationTimeoutTimer = undefined;
    }
  }

  function resetLicenseAuthorizationProgress() {
    licenseAuthorizationRequest = null;
    licenseAuthorizationDisplayName = '';
    licenseAuthorizationGeneration += 1;
    licenseAuthorizationCompletingState = '';
    licenseAuthorizationCompletingStates = new Set<string>();
    clearLicenseAuthorizationTimeout();
    if (busy === 'license-authorize' || busy === 'license-complete') busy = '';
  }

  function failLicenseAuthorization(request: LicenseAuthorizationRequest, message: string, flowEvent: string) {
    logLicenseFlow('warn', flowEvent);
    licenseError = {
      title: 'Device activation',
      message,
      fallbackMessage: 'The operation could not be completed.',
      details: '',
      suggestion: 'Restart activation',
    };
    resetLicenseAuthorizationProgress();
    void api.cancelLicenseAuthorization(request.state).catch(() => null);
    void syncLicenseState(false);
  }

  function scheduleLicenseAuthorizationTimeout(request: LicenseAuthorizationRequest, generation: number) {
    clearLicenseAuthorizationTimeout();
    const timer = window.setTimeout(() => {
      if (licenseAuthorizationTimeoutTimer !== timer) return;
      licenseAuthorizationTimeoutTimer = undefined;
      if (generation !== licenseAuthorizationGeneration || licenseAuthorizationRequest?.state !== request.state) return;
      if (licenseAuthorizationCompletingState === request.state) return;
      failLicenseAuthorization(
        request,
        'Device activation timed out before the browser returned to Camellia Nexus.',
        'timeout',
      );
    }, licenseAuthorizationTimeoutMs);
    licenseAuthorizationTimeoutTimer = timer;
  }

  async function pollLicenseAuthorizationCallback() {
    const request = licenseAuthorizationRequest;
    if (!request || busy || licenseAuthorizationCompletingState || licenseAuthorizationCallbackRequestActive) return;
    licenseAuthorizationCallbackRequestActive = true;
    try {
      const event = await api.takeLicenseAuthorizationCallback(request.state);
      if (event && licenseAuthorizationRequest?.state === request.state) {
        logLicenseFlow('info', 'callback-polled');
        await handleLicenseAuthorizationCallback(event);
      }
    } finally {
      licenseAuthorizationCallbackRequestActive = false;
    }
  }

  function reportGlobalError(value: unknown) {
    if (repeatLicenseStopFailureNotice) lastLicenseNoticeKey = '';
    repeatLicenseStopFailureNotice = false;
    const error = errorInfoOf(value);
    showNotification(
      error,
      null,
      error.code === 'LICENSE_REQUIRED'
        ? licenseNotificationTimeoutMs
        : globalNotificationTimeoutMs,
    );
  }

  function clearNotificationTimer() {
    if (notificationTimer === undefined) return;
    window.clearTimeout(notificationTimer);
    notificationTimer = undefined;
  }

  function showNotification(
    nextNotification: ErrorInfo,
    licenseNotice: LicenseNotice | null,
    timeoutMs = 0,
  ) {
    clearNotificationTimer();
    notificationLicenseNotice = licenseNotice;
    notification = nextNotification;
    if (timeoutMs <= 0) return;
    notificationTimer = window.setTimeout(dismissNotification, timeoutMs);
  }

  function dismissNotification() {
    clearNotificationTimer();
    notificationLicenseNotice = null;
    notification = null;
    if (repeatLicenseStopFailureNotice) lastLicenseNoticeKey = '';
    repeatLicenseStopFailureNotice = false;
  }

  function licenseNoticeInfo(notice: LicenseNotice): ErrorInfo {
    const message = [notice.message, ...(notice.additionalMessages ?? [])]
      .map((part) => translate(part))
      .join(' ');
    return {
      title: translate(notice.title),
      message,
      fallbackMessage: message,
      details: '',
      suggestion: translate(notice.suggestion),
    };
  }

  async function handleLicenseStateChanged(event: LicenseStateChangedEvent) {
    if (!Number.isSafeInteger(event.generation) || event.generation <= 0) return;
    if (event.generation <= lastLicenseEventGeneration) return;
    lastLicenseEventGeneration = event.generation;
    if (event.generation < entitlementSnapshotGeneration) return;
    await applyEntitlementSnapshot(event, event.reason);
    if (event.generation !== entitlementSnapshotGeneration) return;
    if (
      (event.reason === 'license_logout' || event.reason === 'license_identity_reset')
      && event.failedPrograms === 0
    ) return;
    const notice = licenseRuntimeNotice(event, applicationInfo?.version ?? '');
    if (!notice) return;
    const key = licenseNoticeKey(event);
    if (key === lastLicenseNoticeKey) return;
    lastLicenseNoticeKey = key;
    repeatLicenseStopFailureNotice = event.failedPrograms > 0;
    showNotification(
      licenseNoticeInfo(notice),
      notice,
      licenseNoticeRequiresPersistentAttention(event) ? 0 : licenseNotificationTimeoutMs,
    );
  }

  function clearLicensePrompt() {
    if (licensePromptTimer !== undefined) {
      window.clearTimeout(licensePromptTimer);
      licensePromptTimer = undefined;
    }
    licensePrompt = null;
  }

  function showLicensePrompt(message: string) {
    if (licensePromptTimer !== undefined) window.clearTimeout(licensePromptTimer);
    const title = !activeLicense
      ? 'Activate device to continue'
      : programLimitReached
        ? 'Program limit reached'
        : 'License plan needed';
    licensePrompt = {
      title,
      message: message || 'This action is unavailable for the current license.',
      action: 'License settings',
    };
    licensePromptTimer = window.setTimeout(() => {
      licensePrompt = null;
      licensePromptTimer = undefined;
    }, 4_800);
  }

  function openLicenseSettingsFromPrompt() {
    clearLicensePrompt();
    void openSettingsDialog('license');
  }

  function reportPanelError(value: unknown) {
    reportWorkspaceError('details', value, 'details-save');
  }

  function clearWorkspaceError(scope: ConfigurationWorkspaceScope) {
    const nextErrors = { ...workspaceErrors };
    delete nextErrors[scope];
    workspaceErrors = nextErrors;
    const nextActions = { ...workspaceErrorActions };
    delete nextActions[scope];
    workspaceErrorActions = nextActions;
  }

  function reportWorkspaceError(
    scope: ConfigurationWorkspaceScope,
    value: unknown,
    context: ConfigurationErrorContext,
    retry?: () => Promise<void>,
  ) {
    workspaceErrors = {
      ...workspaceErrors,
      [scope]: configurationErrorInfo(value, context),
    };
    workspaceErrorActions = {
      ...workspaceErrorActions,
      [scope]: retry,
    };
  }

  async function retryWorkspaceError(scope: ConfigurationWorkspaceScope) {
    const retry = workspaceErrorActions[scope];
    if (!retry || workspaceErrorBusyScope) return;
    workspaceErrorBusyScope = scope;
    clearWorkspaceError(scope);
    try {
      await retry();
    } finally {
      if (workspaceErrorBusyScope === scope) workspaceErrorBusyScope = '';
    }
  }

  function clearConfigurationError() {
    configError = null;
    finalEditorAutosaveError = null;
    clearWorkspaceError('configuration');
  }

  function clearIntentError() {
    clearWorkspaceError('intent');
  }

  function reportIntentError(
    value: unknown,
    retry?: () => Promise<void>,
    context: ConfigurationErrorContext = 'guided-change',
  ) {
    reportWorkspaceError('intent', value, context, retry);
  }

  function reportConfigError(
    value: unknown,
    context: ConfigurationErrorContext = 'configuration-apply',
    retry?: () => Promise<void>,
  ) {
    const info = configurationErrorInfo(value, context);
    configError = info;
    reportWorkspaceError('configuration', value, context, retry);
  }

  function isRetryableConfigurationConflict(value: unknown): boolean {
    const info = errorInfoOf(value);
    return info.code === 'CONFIG_CONFLICT'
      && ['CONFIGURATION_STATE_STALE', 'CONFIGURATION_GENERATION_STALE', 'CONFIGURATION_DRAFT_STALE']
        .includes(info.messageKey ?? '');
  }

  function normalizeAppSettings(settings: AppSettings): AppSettings {
    return {
      ...settings,
      logLevel: settings.logLevel ?? 'warn',
    };
  }

  function setConfigOutputMessage(message: string) {
    clearConfigOutput();
    configOutputMessage = message;
    configOutputTimer = window.setTimeout(clearConfigOutput, 6_000);
  }

  function clearConfigOutput() {
    if (configOutputTimer !== undefined) window.clearTimeout(configOutputTimer);
    configOutputTimer = undefined;
    configOutputMessage = '';
  }

  function isRunning(state: ProgramState | undefined) {
    return state?.status === 'running' || state?.status === 'starting';
  }

  function isIssue(state: ProgramState | undefined) {
    return !!state && (
      state.status === 'error' ||
      state.status === 'stopFailed' ||
      state.status === 'backoff' ||
      (state.status === 'exited' && !state.success)
    );
  }

  function canStop(state: ProgramState | undefined) {
    return !!state && state.status !== 'stopped';
  }

  function canStart(state: ProgramState | undefined) {
    return !!state && ['stopped', 'exited', 'error'].includes(state.status);
  }

  function lifecycleBusy(id: string) {
    return lifecycleBusyIds.has(id);
  }

  function runtimeFingerprint(
    spec: ProgramSpec,
    args: string[],
    environment: EnvironmentEntry[],
  ) {
    return JSON.stringify({
      executable: {
        mode: spec.executable.mode,
        path: spec.executable.path,
      },
      args,
      environment: environment.filter((entry) => entry.key || entry.value),
      privilegePolicy: spec.privilegePolicy,
    });
  }

  function settingsFingerprint(
    spec: ProgramSpec,
    args: string[],
    environment: EnvironmentEntry[],
  ) {
    return JSON.stringify({
      name: spec.name,
      autoStart: spec.autoStart,
      restartPolicy: spec.restartPolicy,
      privilegePolicy: spec.privilegePolicy,
      runtime: {
        args,
        environment: environment.filter((entry) => entry.key || entry.value),
      },
    });
  }

  function managedSourcesFingerprint(spec: ProgramSpec) {
    return JSON.stringify(spec.managedConfig
      ? { sources: spec.managedConfig.sources, remoteUpdate: spec.managedConfig.remoteUpdate }
      : null);
  }

  function mergeManagedConfigDraft(
    persisted: ProgramSpec['managedConfig'],
    draft: ProgramSpec['managedConfig'],
    preserveSources: boolean,
    preserveIntegrations: boolean,
  ): ProgramSpec['managedConfig'] {
    if (!persisted && !draft) return undefined;
    const base = structuredClone(persisted ?? draft) as NonNullable<ProgramSpec['managedConfig']>;
    if (!preserveSources && !preserveIntegrations) return base;
    if (preserveSources) {
      base.sources = structuredClone(draft?.sources ?? base.sources);
      base.remoteUpdate = structuredClone(draft?.remoteUpdate);
    }
    if (preserveIntegrations) {
      base.singBoxDashboard = structuredClone(draft?.singBoxDashboard);
      base.singBoxClashDashboard = structuredClone(draft?.singBoxClashDashboard);
      base.xrayDashboard = structuredClone(draft?.xrayDashboard);
      base.mihomoDashboard = structuredClone(draft?.mihomoDashboard);
    }
    return base;
  }

  function runtimeIntegrationFingerprint(spec: ProgramSpec) {
    const integrations = spec.managedConfig
      ? {
          singBoxDashboard: spec.managedConfig.singBoxDashboard,
          singBoxClashDashboard: spec.managedConfig.singBoxClashDashboard,
          xrayDashboard: spec.managedConfig.xrayDashboard,
          mihomoDashboard: spec.managedConfig.mihomoDashboard,
        }
      : null;
    // Creating the managed source shell is not a runtime integration change.
    // Keep the fingerprint stable until a dashboard integration actually has
    // a value, so a Sources-only save does not leave a false Details draft.
    return JSON.stringify(
      integrations && Object.values(integrations).some((value) => value !== undefined)
        ? integrations
        : null,
    );
  }

  function validHttpsUrl(value: string) {
    try {
      const url = new URL(value);
      return url.protocol === 'https:' && !url.username && !url.password;
    } catch {
      return false;
    }
  }

  function validBasicAuthentication(
    authentication: NonNullable<Extract<ConfigSource, { mode: 'remote' }>['authentication']>,
  ) {
    const { username, credentialId, password = '' } = authentication;
    const passwordValid = password.length > 0
      ? new TextEncoder().encode(password).length <= 4096 && !/[\0\r\n]/.test(password)
      : !!credentialId && /^cfg-[0-9a-f]{64}$/.test(credentialId);
    return (
      !!username &&
      new TextEncoder().encode(username).length <= 256 &&
      !username.includes(':') &&
      !/[\u0000-\u001f\u007f-\u009f]/.test(username) &&
      passwordValid
    );
  }

  const DASHBOARD_DURATION_RE = /^\d+(?:ns|us|µs|μs|ms|s|m|h|d)(?:\d+(?:ns|us|µs|μs|ms|s|m|h|d))*$/;

  function validateManagedConfigSettings(spec: ProgramSpec) {
    const managed = spec.managedConfig;
    if (!managed) return;
    if (managed.sources.length > maxConfigSourcesLimit) {
      throw new Error('The configuration source limit for this license has been reached.');
    }
    if (
      managed.remoteUpdate &&
      (!Number.isInteger(managed.remoteUpdate.intervalMinutes) ||
        managed.remoteUpdate.intervalMinutes < 5 ||
        managed.remoteUpdate.intervalMinutes > 10_080)
    ) {
      throw new Error('Select an update interval between 5 minutes and 7 days.');
    }
    const sourceIds = new Set<string>();
    for (const source of managed.sources) {
      if (!source.name.trim() || new TextEncoder().encode(source.name).length > 128) {
        throw new Error('Configuration source names must be between 1 and 128 bytes.');
      }
      if (!/^[A-Za-z0-9_-]{1,64}$/.test(source.id) || sourceIds.has(source.id)) {
        throw new Error('Configuration source identifiers must be unique.');
      }
      sourceIds.add(source.id);
      if (!source.enabled) continue;
      if (
        source.mode === 'local' &&
        !isAbsoluteHostPath(source.path.trim()) &&
        !safeRelativePath(source.path, false)
      ) {
        throw new Error('Use an absolute path or a path relative to the working folder.');
      }
      if (source.mode === 'remote' && !validHttpsUrl(source.url.trim())) {
        throw new Error('Remote configuration sources must use HTTPS without embedded credentials');
      }
      if (source.mode === 'remote' && source.authentication) {
        if (!validBasicAuthentication(source.authentication)) {
          throw new Error('Enter valid Basic authentication credentials.');
        }
      }
    }
    const dashboard = managed.singBoxDashboard;
    if (dashboard) {
      if (!Number.isInteger(dashboard.listenPort) || dashboard.listenPort < 1024 || dashboard.listenPort > 65535) {
        throw new Error('Enter a Dashboard port between 1024 and 65535.');
      }
      if (!DASHBOARD_DURATION_RE.test(dashboard.updateInterval)) {
        throw new Error('Use a duration such as 12h or 1d.');
      }
    }
    const clashDashboard = managed.singBoxClashDashboard;
    if (clashDashboard) {
      if (!Number.isInteger(clashDashboard.listenPort) || clashDashboard.listenPort < 1024 || clashDashboard.listenPort > 65535) {
        throw new Error('Enter a Dashboard port between 1024 and 65535.');
      }
      if (clashDashboard.listenPort === dashboard?.listenPort) {
        throw new Error('sing-box API and Clash API require different ports.');
      }
      if (clashDashboard.downloadUrl && !validHttpsUrl(clashDashboard.downloadUrl)) {
        throw new Error('Clash Dashboard download URL must use HTTPS without credentials.');
      }
    }
    const xrayDashboard = managed.xrayDashboard;
    if (xrayDashboard) {
      if (
        !Number.isInteger(xrayDashboard.apiPort) ||
        xrayDashboard.apiPort < 1024 ||
        xrayDashboard.apiPort > 65535 ||
        !Number.isInteger(xrayDashboard.metricsPort) ||
        xrayDashboard.metricsPort < 1024 ||
        xrayDashboard.metricsPort > 65535
      ) {
        throw new Error('Enter Dashboard ports between 1024 and 65535.');
      }
      if (xrayDashboard.apiPort === xrayDashboard.metricsPort) {
        throw new Error('Xray API and Metrics ports must be different.');
      }
    }
    const mihomoDashboard = managed.mihomoDashboard;
    if (mihomoDashboard) {
      if (
        !Number.isInteger(mihomoDashboard.listenPort) ||
        mihomoDashboard.listenPort < 1024 ||
        mihomoDashboard.listenPort > 65535
      ) {
        throw new Error('Enter a Dashboard port between 1024 and 65535.');
      }
      if (mihomoDashboard.downloadUrl && !validHttpsUrl(mihomoDashboard.downloadUrl)) {
        throw new Error('Mihomo Dashboard download URL must use HTTPS without credentials.');
      }
    }
  }

  function stateLabel(state: ProgramState, localize: Translator) {
    switch (state.status) {
      case 'running':
        return `${localize('Running')} · PID ${state.pid}`;
      case 'backoff':
        return `${localize('Backoff')} · ${state.delaySeconds}s`;
      case 'error':
        return localize('Error');
      case 'stopFailed':
        return `${localize('Stop failed')} · PID ${state.pid}`;
      case 'exited':
        return state.success
          ? localize('Exited')
          : `${localize('Exited')} · ${state.code ?? 'signal'}`;
      default:
        return localize(state.status[0].toUpperCase() + state.status.slice(1));
    }
  }

  function coreTargetLabel(identity?: CoreTargetIdentity) {
    if (!identity) return null;
    if (identity.reportedVersion) return identity.reportedVersion;
    if (identity.coordinate.kind === 'release') return identity.coordinate.tag;
    return 'Not detected';
  }

  function stateNameKey(state: ProgramState) {
    const names: Record<ProgramState['status'], string> = {
      stopped: 'Stopped',
      starting: 'Starting',
      running: 'Running',
      stopping: 'Stopping',
      exited: 'Exited',
      backoff: 'Backoff',
      stopFailed: 'Stop failed',
      error: 'Error',
    };
    return names[state.status];
  }

  function matchesProgramFilter(program: ProgramSummary, filter: ProgramFilter) {
    if (filter === 'all') return true;
    if (filter === 'running') return isRunning(program.state);
    if (filter === 'inactive') {
      return program.state.status === 'stopped' || program.state.status === 'exited';
    }
    if (filter === 'issues') return isIssue(program.state);
    return program.kind === filter;
  }

  async function runLifecycle(action: 'start' | 'stop' | 'restart') {
    if (!selectedId) return;
    await runProgramLifecycle(selectedId, action);
  }

  async function runProgramLifecycle(id: string, action: ProgramLifecycleAction) {
    if (!canUseProgramLifecycleAction(licenseAccess, action)) {
      closeProgramMenu();
      showLicensePrompt(licenseActionHint);
      return;
    }
    if (lifecycleBusy(id) && action !== 'stop') return;
    if (!(await confirmLifecycleUsesSavedValues(id, action))) return;
    const generation = (lifecycleGenerations.get(id) ?? 0) + 1;
    lifecycleGenerations.set(id, generation);
    lifecycleBusyIds = new Set(lifecycleBusyIds).add(id);
    closeProgramMenu();
    try {
      if (action === 'start') await api.startProgram(id);
      if (action === 'stop') await api.stopProgram(id);
      if (action === 'restart') await api.restartProgram(id);
    } catch (value) {
      if (lifecycleGenerations.get(id) === generation) reportGlobalError(value);
    } finally {
      if (lifecycleGenerations.get(id) !== generation) return;
      try {
        await refreshPrograms();
        if (lifecycleGenerations.get(id) !== generation) return;
        if (selectedId === id && detail) {
          const [nextDetail, nextActions] = await Promise.all([
            api.getProgram(id),
            api.listActions(id),
          ]);
          if (lifecycleGenerations.get(id) !== generation) return;
          if (selectedId === id && detail) {
            const currentState = nextDetail.state;
            detail = {
              ...detail,
              state: currentState,
              workingDirectory: nextDetail.workingDirectory,
            };
            actions = nextActions;
            if (activeTab === 'dashboard') {
              if (currentState.status === 'running') {
                await refreshXrayDashboard(true);
                startXrayDashboardPolling();
              } else {
                stopXrayDashboardPolling();
                xrayDashboardSnapshot = null;
                xrayDashboardError = null;
                xrayDashboardRefreshing = false;
                xrayDashboardManualRefreshing = false;
                xrayRoutingBusyTag = '';
                xrayRoutingGeneration += 1;
              }
            }
          }
        }
      } catch (value) {
        reportGlobalError(value);
      }
      const next = new Set(lifecycleBusyIds);
      next.delete(id);
      lifecycleBusyIds = next;
      lifecycleGenerations.delete(id);
    }
  }

  async function confirmLifecycleUsesSavedValues(
    id: string,
    action: 'start' | 'stop' | 'restart',
  ) {
    if (
      action === 'stop' || selectedId !== id ||
      (!settingsChanged && !configEditorDirty)
    ) return true;
    const pending = [
      settingsChanged ? translate('program settings') : '',
      configEditorDirty ? translate('configuration') : '',
    ].filter(Boolean).join(` ${translate('and')} `);
    return askConfirmation(
      translate('Changes are not active yet'),
      `${translate('Save or apply the edited')} ${pending} ${translate('first, or continue using the last saved values.')}`,
      translate('Use saved values'),
    );
  }

  function toggleBulkMode() {
    bulkMode = !bulkMode;
    bulkSelectedIds = new Set();
    closeProgramMenu(false);
    draggingProgramId = '';
    suppressProgramClickId = '';
    dropTargetProgramId = '';
    dropAfterTarget = false;
    programDragCleanup?.();
  }

  function toggleBulkSelection(id: string) {
    const next = new Set(bulkSelectedIds);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    bulkSelectedIds = next;
  }

  function activateProgramFromList(event: MouseEvent, id: string) {
    if (suppressProgramClickId === id) {
      suppressProgramClickId = '';
      return;
    }
    const target = event.target instanceof Element ? event.target : null;
    if (target?.closest('.program-grip')) return;
    if (bulkMode) toggleBulkSelection(id);
    else void selectProgram(id).catch(reportGlobalError);
  }

  function selectAllPrograms() {
    const visibleIds = visiblePrograms.map((program) => program.id);
    const everyVisibleProgramSelected = visibleIds.every((id) => bulkSelectedIds.has(id));
    const next = new Set(bulkSelectedIds);
    for (const id of visibleIds) {
      if (everyVisibleProgramSelected) next.delete(id);
      else next.add(id);
    }
    bulkSelectedIds = next;
  }

  async function runBulkLifecycle(action: ProgramLifecycleAction) {
    if (!canUseProgramLifecycleAction(licenseAccess, action)) {
      showLicensePrompt(licenseActionHint);
      return;
    }
    if ((bulkBusy && action !== 'stop') || bulkBusy === 'stop' || bulkSelectedIds.size === 0) return;
    const targets = programs.filter(
      (program) =>
        bulkSelectedIds.has(program.id) &&
        (action === 'start'
          ? canStart(program.state)
          : action === 'restart'
            ? program.state.status === 'running' || program.state.status === 'backoff'
            : canStop(program.state)),
    );
    if (targets.length === 0) return;
    if (
      action === 'start' && selectedId &&
      targets.some((program) => program.id === selectedId) &&
      !(await confirmLifecycleUsesSavedValues(selectedId, 'start'))
    ) return;
    const operationGeneration = ++bulkGeneration;
    bulkBusy = action;
    const generations = new Map(
      targets.map((program) => {
        const generation = (lifecycleGenerations.get(program.id) ?? 0) + 1;
        lifecycleGenerations.set(program.id, generation);
        return [program.id, generation] as const;
      }),
    );
    lifecycleBusyIds = new Set([
      ...lifecycleBusyIds,
      ...targets.map((program) => program.id),
    ]);
    try {
      const results = await Promise.allSettled(
        targets.map((program) =>
          action === 'start'
            ? api.startProgram(program.id)
            : action === 'restart'
              ? api.restartProgram(program.id)
              : api.stopProgram(program.id),
        ),
      );
      const failure = results.find(
        (result): result is PromiseRejectedResult => result.status === 'rejected',
      );
      if (bulkGeneration !== operationGeneration) return;
      if (failure) reportGlobalError(failure.reason);
      await refreshPrograms();
    } finally {
      const next = new Set(lifecycleBusyIds);
      for (const program of targets) {
        if (lifecycleGenerations.get(program.id) !== generations.get(program.id)) continue;
        lifecycleGenerations.delete(program.id);
        next.delete(program.id);
      }
      lifecycleBusyIds = next;
      if (bulkGeneration === operationGeneration) bulkBusy = '';
    }
  }

  function persistCatalog(next = catalog) {
    catalog = next;
    saveCatalog(catalog);
  }

  function commitProgramDrop(sourceId: string, targetId = '', afterTarget = false) {
    if (sourceId && sourceId === targetId) {
      finishProgramDrag();
      return;
    }
    let beforeId: string | undefined = targetId || undefined;
    if (sourceId && targetId && afterTarget) {
      const orderWithoutSource = catalog.order.filter((id) => id !== sourceId);
      const targetIndex = orderWithoutSource.indexOf(targetId);
      beforeId = targetIndex < 0 ? undefined : orderWithoutSource[targetIndex + 1];
    }
    if (canReorderPrograms && sourceId && sourceId !== beforeId) {
      persistCatalog(moveCatalogItem(catalog, sourceId, beforeId));
    }
    finishProgramDrag();
  }

  function updateProgramDropTarget(clientY: number) {
    if (!programListElement) return;
    const rows = Array.from(
      programListElement.querySelectorAll<HTMLElement>('.program-item[data-program-id]'),
    ).filter((row) => row.dataset.programId !== draggingProgramId);
    if (!rows.length) {
      dropTargetProgramId = '';
      dropAfterTarget = false;
      return;
    }
    const row = rows.find((candidate) => clientY < candidate.getBoundingClientRect().bottom)
      ?? rows[rows.length - 1];
    const bounds = row.getBoundingClientRect();
    dropTargetProgramId = row.dataset.programId ?? '';
    dropAfterTarget = clientY >= bounds.top + bounds.height / 2;
  }

  function startProgramPointerDrag(event: PointerEvent, id: string) {
    if (!canReorderPrograms || bulkMode || event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    programDragCleanup?.();
    const grip = event.currentTarget as HTMLElement;
    const pointerId = event.pointerId;
    const startX = event.clientX;
    const startY = event.clientY;
    let moved = false;
    grip.setPointerCapture?.(pointerId);

    const move = (moveEvent: PointerEvent) => {
      if (moveEvent.pointerId !== pointerId) return;
      if (!moved && Math.hypot(moveEvent.clientX - startX, moveEvent.clientY - startY) < 4) return;
      moveEvent.preventDefault();
      if (!moved) {
        moved = true;
        draggingProgramId = id;
        suppressProgramClickId = id;
      }
      if (programListElement) {
        const bounds = programListElement.getBoundingClientRect();
        const edge = 30;
        if (moveEvent.clientY < bounds.top + edge) programListElement.scrollBy({ top: -14 });
        else if (moveEvent.clientY > bounds.bottom - edge) programListElement.scrollBy({ top: 14 });
      }
      updateProgramDropTarget(moveEvent.clientY);
    };
    const stop = (stopEvent: PointerEvent) => {
      if (stopEvent.pointerId !== pointerId) return;
      const targetId = dropTargetProgramId;
      const afterTarget = dropAfterTarget;
      cleanup();
      if (moved) commitProgramDrop(id, targetId, afterTarget);
    };
    const cleanup = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', stop);
      window.removeEventListener('pointercancel', stop);
      if (grip.hasPointerCapture?.(pointerId)) grip.releasePointerCapture(pointerId);
      programDragCleanup = null;
      if (!moved) finishProgramDrag();
    };
    programDragCleanup = cleanup;
    window.addEventListener('pointermove', move, { passive: false });
    window.addEventListener('pointerup', stop);
    window.addEventListener('pointercancel', stop);
  }

  function finishProgramDrag() {
    const draggedId = draggingProgramId;
    programDragCleanup?.();
    draggingProgramId = '';
    dropTargetProgramId = '';
    dropAfterTarget = false;
    if (draggedId) {
      window.setTimeout(() => {
        if (suppressProgramClickId === draggedId) suppressProgramClickId = '';
      }, 0);
    }
  }

  function moveProgramBy(id: string, offset: -1 | 1) {
    closeProgramMenu();
    if (!canReorderPrograms) return;
    persistCatalog(moveCatalogItemBy(catalog, id, offset));
  }

  async function saveSettings(
    confirmRestart = true,
    applyManagedConfiguration = true,
    scope: 'all' | 'details' | 'sources' = 'all',
  ) {
    if (!detail) return false;
    const workspaceScope: ConfigurationWorkspaceScope = scope === 'sources'
        ? 'sources'
        : 'details';
    const workspaceContext: ConfigurationErrorContext = scope === 'sources'
        ? 'sources-save'
        : 'details-save';
    clearWorkspaceError(workspaceScope);
    if (workspaceScope === 'sources') sourceSaveStatus = '';
    const id = detail.spec.id;
    const draftBeforeSave = structuredClone(detail.spec) as ProgramSpec;
    const preserveDetailsDraft = scope !== 'all' && detailsChanged;
    const preserveSourcesDraft = scope !== 'all' && scope !== 'sources' && managedConfigChanged;
    const preserveIntegrationsDraft = scope !== 'all' && scope !== 'details' && runtimeIntegrationChanged;
    const spec = structuredClone(detail.spec) as ProgramSpec;
    let persistedSpecForScope: ProgramSpec | null = null;
    if (scope !== 'all') {
      const persisted = await api.getProgram(id);
      persistedSpecForScope = persisted.spec;
      if (scope === 'details') {
        spec.managedConfig = mergeManagedConfigDraft(
          persisted.spec.managedConfig,
          spec.managedConfig,
          false,
          true,
        );
      } else if (scope === 'sources') {
        Object.assign(spec, structuredClone(persisted.spec));
        spec.managedConfig = mergeManagedConfigDraft(
          persisted.spec.managedConfig,
          detail.spec.managedConfig,
          true,
          false,
        );
      }
    }
    try {
      if (scope === 'all' || scope === 'details') {
        if (runtimeArgumentView.error) throw new Error(runtimeArgumentView.error);
        if (spec.type.kind === 'generic') {
          spec.type = { ...spec.type, args: [...runtimeArgumentParse.args] };
        } else {
          spec.type = { ...spec.type, extraArgs: [...runtimeArgumentParse.args] };
        }
        spec.environment = environmentToRecord(environmentEntries);
      }
      if (spec.managedConfig) {
        spec.managedConfig.sources = spec.managedConfig.sources.map(normalizeConfigSource);
        if (spec.managedConfig.singBoxDashboard) {
          const dashboard = spec.managedConfig.singBoxDashboard;
          dashboard.updateInterval = dashboard.updateInterval.trim();
        }
        if (spec.managedConfig.singBoxClashDashboard) {
          const dashboard = spec.managedConfig.singBoxClashDashboard;
          dashboard.downloadUrl = dashboard.downloadUrl?.trim() || undefined;
        }
        if (spec.managedConfig.mihomoDashboard) {
          const dashboard = spec.managedConfig.mihomoDashboard;
          dashboard.downloadUrl = dashboard.downloadUrl?.trim() || undefined;
        }
      }
      validateManagedConfigSettings(spec);
      if (spec.executable.mode === 'managed') {
        spec.executable.path = normalizeRelativePath(spec.executable.path);
        spec.workingDirectory = managedWorkingDirectory(spec.executable.path);
      } else {
        spec.executable.path = normalizeHostPath(spec.executable.path.trim());
        spec.workingDirectory = parentHostPath(spec.executable.path);
      }
    } catch (value) {
      reportWorkspaceError(
        workspaceScope,
        value,
        workspaceContext,
        async () => { await saveSettings(confirmRestart, applyManagedConfiguration, scope); },
      );
      return false;
    }
    const comparisonSpec = persistedSpecForScope ?? detail.spec;
    const managedConfigChangedForCommit =
      (scope === 'all' || scope === 'sources') &&
      managedSourcesFingerprint(spec) !== managedSourcesFingerprint(comparisonSpec);
    const runtimeSettingsChangedForCommit = scope === 'all' || scope === 'details'
      ? runtimeSettingsChanged
      : false;
    const restartAfterSave = runtimeSettingsChangedForCommit && !!detail && isRuntimeActive(detail.state);
    const updateManagedConfiguration =
      applyManagedConfiguration && managedConfigChangedForCommit && !!spec.managedConfig;
    let expectedConfigurationGeneration = configurationState?.generation;
    if (spec.type.kind !== 'generic') {
      try {
        const state = await synchronizeFinalEditorBeforeUpstreamMutation(id);
        expectedConfigurationGeneration = state.generation;
      } catch (value) {
        reportWorkspaceError(
          workspaceScope,
          value,
          workspaceContext,
          async () => { await saveSettings(confirmRestart, applyManagedConfiguration, scope); },
        );
        return false;
      }
    }
    if (
      saveRequiresRestart && confirmRestart &&
      !(await askConfirmation(
        translate('Save and restart the program?'),
        translate('The active program will restart after the changes are applied.'),
        translate('Save and restart'),
      ))
    ) return false;
    const saved = await mutateConfiguration(
      id,
      'save',
      async () => {
        if (restartAfterSave && !updateManagedConfiguration) {
          const snapshot = await api.updateProgramAndRestart(spec, expectedConfigurationGeneration);
          if (snapshot) await adoptConfigurationWorkspace(id, snapshot);
        } else {
          let stoppedForSave = false;
          try {
            if (restartAfterSave) {
              await api.stopProgram(id);
              stoppedForSave = true;
            }
            if (updateManagedConfiguration) {
              const requestedSources = spec.managedConfig?.sources ?? [];
              const requestedRemoteUpdate = spec.managedConfig?.remoteUpdate;
              const nonSourceSpec = structuredClone(spec) as ProgramSpec;
              if (nonSourceSpec.managedConfig) {
                nonSourceSpec.managedConfig.sources = structuredClone(
                  persistedSpecForScope?.managedConfig?.sources
                    ?? (scope === 'sources' && !persistedSpecForScope?.managedConfig ? [] : detail?.spec.managedConfig?.sources ?? []),
                );
                nonSourceSpec.managedConfig.remoteUpdate = structuredClone(
                  persistedSpecForScope?.managedConfig?.remoteUpdate
                    ?? (scope === 'sources' && !persistedSpecForScope?.managedConfig ? undefined : detail?.spec.managedConfig?.remoteUpdate),
                );
              }
              // A Sources-only save is committed by the dedicated Rust
              // source transaction below.  Sending the full ProgramSpec
              // first duplicates that write and can race the controller's
              // mutation lease, surfacing a misleading PROGRAM_BUSY even
              // though the program is stopped.  The one exception is the
              // first Sources save: persist the empty managed-config shell
              // before the source transaction so the backend has an enabled
              // source domain to update.  Details/all saves still persist
              // their non-source fields first, then the source transaction
              // rebases the candidate against that authoritative generation.
              const enablingManagedSources = scope === 'sources'
                && !persistedSpecForScope?.managedConfig
                && !!nonSourceSpec.managedConfig;
              let preliminarySnapshot: ConfigurationWorkspaceSnapshot | null = null;
              if (scope !== 'sources' || enablingManagedSources) {
                preliminarySnapshot = await api.updateProgram(
                  nonSourceSpec,
                  expectedConfigurationGeneration,
                );
              }
              const currentGeneration = preliminarySnapshot?.state.generation
                ?? expectedConfigurationGeneration;
              if (currentGeneration === undefined) {
                throw new Error('Configuration state is unavailable.');
              }
              const snapshot = await api.updateConfigurationSources(
                id,
                requestedSources,
                requestedRemoteUpdate,
                currentGeneration,
              );
              await adoptConfigurationWorkspace(id, snapshot);
              if (selectedId === id) sourceSaveStatus = 'Sources saved.';
            } else {
              const snapshot = await api.updateProgram(spec, expectedConfigurationGeneration);
              if (snapshot) await adoptConfigurationWorkspace(id, snapshot);
            }
            if (stoppedForSave) {
              stoppedForSave = false;
              await api.startProgram(id);
            }
          } catch (value) {
            if (stoppedForSave) await api.startProgram(id).catch(() => undefined);
            throw value;
          }
        }
        if (selectedId !== id) return;
        const persistedDetail = await api.getProgram(id);
        if (selectedId !== id) return;
        const mergedSpec = structuredClone(persistedDetail.spec) as ProgramSpec;
        if (preserveDetailsDraft) {
          mergedSpec.name = draftBeforeSave.name;
          mergedSpec.autoStart = draftBeforeSave.autoStart;
          mergedSpec.restartPolicy = draftBeforeSave.restartPolicy;
          mergedSpec.privilegePolicy = draftBeforeSave.privilegePolicy;
          mergedSpec.type = structuredClone(draftBeforeSave.type);
          mergedSpec.environment = structuredClone(draftBeforeSave.environment);
          mergedSpec.workingDirectory = draftBeforeSave.workingDirectory;
          mergedSpec.executable = structuredClone(draftBeforeSave.executable);
        }
        if (preserveSourcesDraft || preserveIntegrationsDraft) {
          mergedSpec.managedConfig = mergeManagedConfigDraft(
            mergedSpec.managedConfig,
            draftBeforeSave.managedConfig,
            preserveSourcesDraft,
            preserveIntegrationsDraft,
          );
        }
        detail = { ...persistedDetail, spec: mergedSpec };
        if (configDocument) {
          void refreshConfigurationSchemaCapability(id);
        }
        runtimeArgumentLine = formatArgumentLine(programArgs(detail.spec));
        environmentEntries = Object.entries(detail.spec.environment).map(([key, value]) => ({
          key,
          value,
        }));
        if (scope === 'all' || scope === 'details') {
          savedRuntimeFingerprint = runtimeFingerprint(detail.spec, programArgs(detail.spec), environmentEntries);
          savedSettingsFingerprint = settingsFingerprint(detail.spec, programArgs(detail.spec), environmentEntries);
          savedRuntimeIntegrationFingerprint = runtimeIntegrationFingerprint(detail.spec);
          savedDashboardOptionsValue = dashboardOptionsFromManagedConfig(detail.spec.managedConfig);
          savedXrayDashboardValue = detail.spec.managedConfig?.xrayDashboard;
          savedMihomoDashboardValue = detail.spec.managedConfig?.mihomoDashboard;
        }
        if (scope === 'all' || scope === 'sources') {
          savedManagedConfigFingerprint = managedSourcesFingerprint(persistedDetail.spec);
        }
        await refreshPrograms();
      },
      async (value) => {
        reportWorkspaceError(
          workspaceScope,
          value,
          workspaceContext,
          async () => { await saveSettings(confirmRestart, applyManagedConfiguration, scope); },
        );
      },
    );
    return saved;
  }

  async function revertSettings(
    scope: 'all' | 'details' | 'sources' = 'all',
  ) {
    if (!selectedId) return;
    const workspaceScope: ConfigurationWorkspaceScope = scope === 'sources'
        ? 'sources'
        : 'details';
    clearWorkspaceError(workspaceScope);
    const id = selectedId;
    await mutate(
      'revert-settings',
      async () => {
        const nextDetail = await api.getProgram(id);
        if (selectedId !== id) return;
        const currentSpec = structuredClone(detail?.spec) as ProgramSpec | undefined;
        const nextSpec = structuredClone(nextDetail.spec) as ProgramSpec;
        if (currentSpec && scope !== 'all') {
          if (scope !== 'details') {
            nextSpec.name = currentSpec.name;
            nextSpec.autoStart = currentSpec.autoStart;
            nextSpec.restartPolicy = currentSpec.restartPolicy;
            nextSpec.privilegePolicy = currentSpec.privilegePolicy;
            nextSpec.type = structuredClone(currentSpec.type);
            nextSpec.environment = structuredClone(currentSpec.environment);
            nextSpec.workingDirectory = currentSpec.workingDirectory;
            nextSpec.executable = structuredClone(currentSpec.executable);
          }
          if (scope === 'details') {
            nextSpec.managedConfig = mergeManagedConfigDraft(
              nextDetail.spec.managedConfig,
              currentSpec.managedConfig,
              true,
              false,
            );
          } else if (scope === 'sources') {
            nextSpec.managedConfig = mergeManagedConfigDraft(
              nextDetail.spec.managedConfig,
              currentSpec.managedConfig,
              false,
              true,
            );
          }
        }
        detail = { ...nextDetail, spec: nextSpec };
        runtimeArgumentLine = formatArgumentLine(programArgs(nextSpec));
        environmentEntries = Object.entries(nextSpec.environment).map(([key, value]) => ({
          key,
          value,
        }));
        if (scope === 'all' || scope === 'details') {
          savedRuntimeFingerprint = runtimeFingerprint(nextDetail.spec, programArgs(nextDetail.spec), environmentEntries);
          savedSettingsFingerprint = settingsFingerprint(nextDetail.spec, programArgs(nextDetail.spec), environmentEntries);
          savedRuntimeIntegrationFingerprint = runtimeIntegrationFingerprint(nextDetail.spec);
          savedDashboardOptionsValue = dashboardOptionsFromManagedConfig(nextDetail.spec.managedConfig);
          savedXrayDashboardValue = nextDetail.spec.managedConfig?.xrayDashboard;
          savedMihomoDashboardValue = nextDetail.spec.managedConfig?.mihomoDashboard;
        }
        if (scope === 'all' || scope === 'sources') {
          savedManagedConfigFingerprint = managedSourcesFingerprint(nextDetail.spec);
        }
      },
      async (value) => {
        reportWorkspaceError(
          workspaceScope,
          value,
          workspaceScope === 'sources' ? 'sources-save' : 'details-save',
          async () => { await revertSettings(scope); },
        );
      },
    );
  }

  function programArgs(spec: ProgramSpec) {
    return spec.type.kind === 'generic' ? spec.type.args : spec.type.extraArgs;
  }

  function environmentToRecord(entries: EnvironmentEntry[]) {
    const result: Record<string, string> = {};
    const portableKeys = new Set<string>();
    for (const entry of entries) {
      if (!entry.key && !entry.value) continue;
      if (!entry.key || entry.key.includes('=') || entry.key.includes('\0')) {
        throw new Error('Every environment variable needs a valid key without “=”.');
      }
      const portableKey = entry.key.toUpperCase();
      if (portableKeys.has(portableKey)) throw new Error(`Duplicate environment key: ${entry.key}`);
      if (entry.value.includes('\0')) throw new Error(`Environment value for ${entry.key} contains a null character.`);
      portableKeys.add(portableKey);
      result[entry.key] = entry.value;
    }
    return result;
  }

  function hasExplicitConfig(kind: ProgramKind, args: string[]) {
    const configuration = programDefinition(kind).configuration;
    return !!configuration && hasConfigurationArgument(configuration.flags, args);
  }

  function enrichArgumentResult(
    result: ReturnType<typeof parseArgumentLine>,
    kind: ProgramKind,
    managedConfiguration = false,
    storedConfiguration = false,
  ) {
    return programDefinition(kind).configuration?.enrichArguments(result, {
      managedConfiguration,
      storedConfiguration,
    }) ?? result;
  }

  function hasEditableConfig(spec: ProgramSpec) {
    return spec.type.kind !== 'generic';
  }

  function privilegePolicyValue(policy: PrivilegePolicy) {
    return policy.mode;
  }

  function setDetailPrivilegePolicy(value: string) {
    if (!detail) return;
    const mode = value;
    const privilegePolicy: PrivilegePolicy = mode === 'standard'
      ? { mode: 'standard' }
      : mode === 'elevated'
        ? { mode: 'elevated' }
        : { mode: 'automatic' };
    detail = { ...detail, spec: { ...detail.spec, privilegePolicy } };
  }

  function privilegeAssessmentText(
    assessment: PrivilegeAssessment | null,
    policy: PrivilegePolicy,
    loading: boolean,
  ) {
    if (loading) return 'Checking administrator access requirements';
    if (!assessment) return 'Privilege assessment unavailable';
    if (assessment.detected === 'elevated') {
      return assessment.reasons.some((reason) => reason.code === 'tunInterface')
        ? 'Administrator access is required because TUN is enabled'
        : 'Administrator access is required by this program configuration';
    }
    if (policy.mode === 'elevated') return 'Administrator access is enabled by the program policy';
    if (assessment.detected === 'unknown') {
      return 'No reliable requirement was detected; standard access is used unless overridden';
    }
    return 'This configuration can use standard user access';
  }

  function enableManagedConfiguration() {
    if (!detail || detail.spec.type.kind === 'generic') return;
    detail.spec.managedConfig ??= { sources: [] };
    detail = { ...detail, spec: { ...detail.spec } };
  }

  function dashboardOptionsFromManagedConfig(
    managedConfig: ProgramSpec['managedConfig'],
  ): SingBoxDashboardOptions {
    return {
      native: managedConfig?.singBoxDashboard,
      clash: managedConfig?.singBoxClashDashboard,
    };
  }

  function updateDetailDashboard(change: SingBoxDashboardChange) {
    if (!detail?.spec.managedConfig || detail.spec.type.kind !== 'singBox') return;
    const options = applySingBoxDashboardChange(
      dashboardOptionsFromManagedConfig(detail.spec.managedConfig),
      change,
    );
    const managedConfig = {
      ...detail.spec.managedConfig,
      singBoxDashboard: options.native,
      singBoxClashDashboard: options.clash,
    };
    detail = {
      ...detail,
      spec: { ...detail.spec, managedConfig },
    };
  }

  async function useSavedManagedSetting(settingId: string) {
    if (!selectedId || detailsChanged || busy) return;
    const programId = selectedId;
    await mutateConfiguration(programId, 'save', async () => {
      const state = await synchronizeFinalEditorBeforeUpstreamMutation(programId);
      const persisted = await api.getProgram(programId);
      const snapshot = await api.updateProgram(persisted.spec, state.generation, [settingId]);
      if (snapshot) await adoptConfigurationWorkspace(programId, snapshot);
    }, async (error) => {
      reportWorkspaceError('details', error, 'details-save', async () => { await useSavedManagedSetting(settingId); });
    });
  }

  function updateDetailXrayDashboard(value: XrayDashboard | undefined) {
    if (!detail?.spec.managedConfig || detail.spec.type.kind !== 'xray') return;
    detail = {
      ...detail,
      spec: {
        ...detail.spec,
        managedConfig: {
          ...detail.spec.managedConfig,
          xrayDashboard: value,
        },
      },
    };
  }

  function updateDetailMihomoDashboard(value: MihomoDashboard | undefined) {
    if (!detail?.spec.managedConfig || detail.spec.type.kind !== 'mihomo') return;
    detail = {
      ...detail,
      spec: {
        ...detail.spec,
        managedConfig: {
          ...detail.spec.managedConfig,
          mihomoDashboard: value,
        },
      },
    };
  }

  function dashboardOptionsFromDraft(draft: CreateDraft): SingBoxDashboardOptions {
    return {
      native: draft.dashboardEnabled
        ? {
            listenPort: draft.dashboardPort,
            updateInterval: draft.dashboardUpdateInterval,
          }
        : undefined,
      clash: draft.clashDashboardEnabled
        ? {
            listenPort: draft.clashDashboardPort,
            downloadUrl: draft.clashDashboardDownloadUrl || undefined,
          }
        : undefined,
    };
  }

  function updateCreateDashboard(change: SingBoxDashboardChange) {
    const options = applySingBoxDashboardChange(
      dashboardOptionsFromDraft(createDraft),
      change,
    );
    createDraft = {
      ...createDraft,
      dashboardEnabled: !!options.native,
      dashboardPort: options.native?.listenPort ?? createDraft.dashboardPort,
      dashboardUpdateInterval:
        options.native?.updateInterval ?? createDraft.dashboardUpdateInterval,
      clashDashboardEnabled: !!options.clash,
      clashDashboardPort: options.clash?.listenPort ?? createDraft.clashDashboardPort,
      clashDashboardDownloadUrl:
        options.clash ? options.clash.downloadUrl ?? '' : createDraft.clashDashboardDownloadUrl,
    };
  }

  async function refreshManagedConfiguration() {
    if (!detail || !selectedId || !detail.spec.managedConfig) return;
    clearWorkspaceError('sources');
    configUpdateStatus = null;
    const id = selectedId;
    if (settingsChanged && !(await saveSettings(true, false, 'details'))) return;
    await mutateConfiguration(
      id,
      'refresh-config-sources',
      async () => {
        await synchronizeFinalEditorBeforeUpstreamMutation(id);
        const snapshot = await api.refreshConfigurationSources(id);
        if (selectedId !== id) return;
        await adoptConfigurationWorkspace(id, snapshot);
        clearConfigOutput();
        const sourceCount = snapshot.state.sourceStatuses.filter((source) => source.freshness === 'fresh').length;
        configUpdateStatus = sourceCount
          ? { message: 'sources updated', sourceCount }
          : { message: 'sources updated' };
        const nextDetail = await api.getProgram(id);
        if (selectedId === id) detail = nextDetail;
        await refreshPrograms();
      },
      async (value) => {
        reportWorkspaceError(
          'sources',
          value,
          'sources-refresh',
          async () => { await refreshManagedConfiguration(); },
        );
      },
    );
  }

  async function previewShareImport(content: string) {
    if (!selectedId || !content.trim()) return;
    busy = 'preview-share-import';
    clearConfigurationError();
    try {
      shareImportPreview = await api.previewConfigurationImport(selectedId, content);
    } catch (error) {
      reportConfigError(error, 'sources-refresh');
    } finally {
      busy = '';
    }
  }

  async function rebaseFinalEditorDraft() {
    if (!selectedId) return;
    const id = selectedId;
    clearConfigurationError();
    await mutateConfiguration(
      id,
      'rebase-final-editor-draft',
      async () => {
        await finalEditorSavePromises.get(id);
        const snapshot = await api.rebaseConfigurationDraft(id, finalEditorSession?.draftRevision ?? 0);
        await adoptConfigurationWorkspace(id, snapshot);
      },
      async (error) => {
        reportConfigError(error, 'configuration-rebase', async () => { await rebaseFinalEditorDraft(); });
      },
    );
  }

  const resolvingConflictIds = new Set<string>();

  async function resolveConfigurationConflict(
    event: CustomEvent<{ conflictId: string; resolution: FinalConflictResolution }>,
  ) {
    if (!selectedId || !configurationState || resolvingConflictIds.has(event.detail.conflictId)) return;
    const id = selectedId;
    const conflictId = event.detail.conflictId;
    const observed = configurationState.workspace.editor.conflicts.find((item) => item.conflictId === conflictId)?.reference
      ?? (pendingConflictProjection?.conflictId === conflictId ? pendingConflictProjection.reference : undefined);
    resolvingConflictIds.add(conflictId);
    try {
      await mutateConfiguration(id, 'resolve-configuration-conflict', async () => {
        const pending = pendingConflictsByProgram.get(id);
        if (pending && (pending.action.kind !== 'resolve'
          || pending.action.reference.conflictId !== observed?.conflictId
          || pending.action.reference.origin !== observed?.origin
          || JSON.stringify(pending.action.resolution) !== JSON.stringify(event.detail.resolution)
        )) throw { code: 'CONFIG_CONFLICT', messageKey: 'CONFIGURATION_OPERATION_PENDING' };
        if (!pending) await flushFinalEditorDraftBeforeUpstreamMutation(id);
        if (selectedId !== id) return;
        const conflict = configurationState?.workspace.editor.conflicts.find((item) => item.conflictId === conflictId);
        if (!pending && (!conflict || !configurationState || conflict.reference.fingerprint !== observed?.fingerprint)) {
          throw { code: 'CONFIG_CONFLICT', messageKey: 'CONFIGURATION_CONFLICT_STALE' };
        }
        const request: ResolveConfigurationConflictRequest = pending ?? {
              operationId: crypto.randomUUID(),
              expectedStateRevision: configurationState!.stateRevision,
              ...(finalEditorSession && finalEditorSession.draftRevision > 0 ? {
                editorSessionId: finalEditorSession.sessionId,
                expectedDraftRevision: finalEditorSession.draftRevision,
              } : {}),
              action: { kind: 'resolve', reference: conflict!.reference, resolution: event.detail.resolution },
            };
        pendingConflictRequest = request;
        if (conflict) conflictHistoryRecords.set(request.operationId, conflict);
        pendingConflictsByProgram.set(id, request);
        const submittedContent = configContent;
        const snapshot = await api.resolveConfigurationConflict(id, request);
        pendingConflictsByProgram.delete(id);
        conflictErrorsByProgram.delete(id);
        if (selectedId !== id) return;
        pendingConflictRequest = null;
        conflictActionError = null;
        await adoptConfigurationWorkspace(id, snapshot, configContent === submittedContent ? 'authoritative' : 'preserveTyping');
        if (selectedId === id) {
          recentConflictChoice = { programId: id, operationId: request.operationId, undone: false };
          selectedEditorConflictId = snapshot.state.workspace.editor.conflicts[0]?.conflictId ?? '';
          selectedEditorPath = snapshot.state.workspace.editor.conflicts[0]?.semanticPath ?? '';
        }
      }, async (error) => {
        if (selectedId !== id) return;
        const pending = pendingConflictsByProgram.get(id);
        if (pending?.action.kind === 'resolve'
          && ['CONFIGURATION_STATE_STALE', 'CONFIGURATION_DRAFT_STALE'].includes(errorInfoOf(error).messageKey ?? '')) {
          try {
            const reference = pending.action.reference;
            const latest = await api.getConfigurationWorkspace(id);
            const same = latest.state.workspace.editor.conflicts.find((item) =>
              item.reference.origin === reference.origin
              && item.reference.conflictId === reference.conflictId
              && item.reference.fingerprint === reference.fingerprint);
            await adoptConfigurationWorkspace(id, latest, 'preserveTyping');
            if (same) {
              const retry: ResolveConfigurationConflictRequest = {
                ...pending,
                operationId: crypto.randomUUID(),
                expectedStateRevision: latest.state.stateRevision,
                ...(latest.editorSession && latest.editorSession.draftRevision > 0 ? {
                  editorSessionId: latest.editorSession.sessionId,
                  expectedDraftRevision: latest.editorSession.draftRevision,
                } : { editorSessionId: undefined, expectedDraftRevision: undefined }),
              };
              pendingConflictRequest = retry;
              conflictHistoryRecords.set(retry.operationId, same);
              pendingConflictsByProgram.set(id, retry);
              const submittedContent = configContent;
              const resolved = await api.resolveConfigurationConflict(id, retry);
              pendingConflictsByProgram.delete(id);
              conflictErrorsByProgram.delete(id);
              if (selectedId !== id) return;
              pendingConflictRequest = null;
              conflictActionError = null;
              await adoptConfigurationWorkspace(id, resolved, configContent === submittedContent ? 'authoritative' : 'preserveTyping');
              recentConflictChoice = { programId: id, operationId: retry.operationId, undone: false };
              selectedEditorConflictId = resolved.state.workspace.editor.conflicts[0]?.conflictId ?? '';
              selectedEditorPath = resolved.state.workspace.editor.conflicts[0]?.semanticPath ?? '';
              return;
            }
            pendingConflictRequest = null;
            pendingConflictsByProgram.delete(id);
          } catch (retryError) {
            error = retryError;
          }
        }
        if (errorInfoOf(error).messageKey === 'CONFIGURATION_CONFLICT_STALE') {
          pendingConflictsByProgram.delete(id);
          pendingConflictRequest = null;
          const latest = await api.getConfigurationWorkspace(id);
          await adoptConfigurationWorkspace(id, latest, 'preserveTyping');
        }
        conflictActionError = { conflictId, message: configurationErrorInfo(error, 'configuration-rebase').message };
        conflictErrorsByProgram.set(id, conflictActionError);
      });
    } finally {
      resolvingConflictIds.delete(conflictId);
    }
  }

  async function retryConfigurationConflict(conflictId: string) {
    const request = pendingConflictRequest;
    if (request && request.action.kind !== 'resolve') {
      await historyConfigurationConflict(request.action.kind, request.action.resolutionOperationId);
      return;
    }
    if (!request || request.action.kind !== 'resolve'
      || `${request.action.reference.origin}:${request.action.reference.conflictId}` !== conflictId) return;
    await resolveConfigurationConflict(new CustomEvent('resolve', {
      detail: { conflictId, resolution: request.action.resolution },
    }));
  }

  async function historyConfigurationConflict(kind: 'undo' | 'redo', operationId: string) {
    if (!selectedId || !configurationState) return;
    const id = selectedId;
    await mutateConfiguration(id, `history-configuration-conflict-${kind}`, async () => {
      if (!pendingConflictsByProgram.has(id)) await flushFinalEditorDraftBeforeUpstreamMutation(id);
      if (selectedId !== id || !configurationState) return;
      const pending = pendingConflictsByProgram.get(id);
      if (pending && (pending.action.kind !== kind || !('resolutionOperationId' in pending.action)
        || pending.action.resolutionOperationId !== operationId)) {
        throw { code: 'CONFIG_CONFLICT', messageKey: 'CONFIGURATION_OPERATION_PENDING' };
      }
      const request: ResolveConfigurationConflictRequest = pending ?? {
      operationId: crypto.randomUUID(),
      expectedStateRevision: configurationState.stateRevision,
      ...(finalEditorSession && finalEditorSession.draftRevision > 0 ? {
        editorSessionId: finalEditorSession.sessionId,
        expectedDraftRevision: finalEditorSession.draftRevision,
      } : {}),
      action: { kind, resolutionOperationId: operationId },
      };
      pendingConflictsByProgram.set(id, request);
      pendingConflictRequest = request;
      const submittedContent = configContent;
      const snapshot = await api.resolveConfigurationConflict(id, request);
      pendingConflictsByProgram.delete(id);
      conflictErrorsByProgram.delete(id);
      if (selectedId !== id) return;
      pendingConflictRequest = null;
      conflictActionError = null;
      await adoptConfigurationWorkspace(id, snapshot, configContent === submittedContent ? 'authoritative' : 'preserveTyping');
      if (selectedId === id) {
        selectedEditorConflictId = snapshot.state.workspace.editor.conflicts[0]?.conflictId ?? '';
        selectedEditorPath = snapshot.state.workspace.editor.conflicts[0]?.semanticPath ?? '';
      }
    }, async (error) => {
      const conflict = conflictHistoryRecords.get(operationId);
      if (!conflict) return;
      const failure = { conflictId: conflict.conflictId, message: configurationErrorInfo(error, 'configuration-rebase').message };
      conflictErrorsByProgram.set(id, failure);
      if (selectedId === id) {
        conflictActionError = failure;
        selectedEditorConflictId = conflict.conflictId;
      }
    });
  }

  async function discardFinalEditorDraft() {
    if (!selectedId) return;
    const id = selectedId;
    clearConfigurationError();
    await mutate(
      'discard-draft',
      async () => {
        if (finalEditorAutosaveTimer !== undefined) {
          window.clearTimeout(finalEditorAutosaveTimer);
          finalEditorAutosaveTimer = undefined;
        }
        await finalEditorSavePromises.get(id);
        const snapshot = await api.discardConfigurationDraft(
          id,
          finalEditorSession?.draftRevision ?? 0,
        );
        await adoptConfigurationWorkspace(id, snapshot);
        clearConfigOutput();
      },
      async (error) => {
        reportConfigError(error, 'final-editor-draft', async () => { await discardFinalEditorDraft(); });
      },
    );
  }

  async function handleAutomaticConfigUpdate(event: AutomaticConfigUpdateEvent) {
    if (selectedId !== event.programId) return;
    configUpdateStatus = {
      message: event.succeeded ? 'Automatically updated' : 'Automatic update failed',
    };
    if (!event.succeeded) return;
    try {
      if (configDocument) {
        await flushFinalEditorDraftBeforeUpstreamMutation(event.programId);
      }
      const snapshot = await api.getConfigurationWorkspace(event.programId);
      if (selectedId !== event.programId) return;
      await adoptConfigurationWorkspace(event.programId, snapshot);
    } catch {
      // The next automatic or manual refresh can update the editor.
    }
  }

  async function openSingBoxDashboard(dashboardKind: 'native' | 'clash') {
    if (!selectedId) return;
    const id = selectedId;
    await runExternalAction(
      `sing-box-dashboard:${id}:${dashboardKind}`,
      () => api.openSingBoxDashboard(id, dashboardKind),
    );
  }

  async function openMihomoDashboard() {
    if (!selectedId) return;
    const id = selectedId;
    await runExternalAction(
      `mihomo-dashboard:${id}`,
      () => api.openMihomoDashboard(id),
    );
  }

  function configModeKey(spec: ProgramSpec, args = programArgs(spec)) {
    if (hasExplicitConfig(spec.type.kind, args)) return 'Explicit argument';
    if (spec.managedConfig) return 'Managed configuration';
    return 'Manual configuration';
  }

  function normalizeRelativePath(value: string) {
    return value.replaceAll('\\', '/');
  }

  function normalizeConfigSource(source: ConfigSource): ConfigSource {
    if (source.mode === 'inline') {
      return { ...source, name: source.name.trim() };
    }
    return source.mode === 'local'
      ? { ...source, name: source.name.trim(), path: normalizeHostPath(source.path.trim()) }
      : {
          ...source,
          name: source.name.trim(),
          url: source.url.trim(),
          authentication: source.authentication
            ? { ...source.authentication, username: source.authentication.username.trim() }
            : undefined,
        };
  }

  function updateDetailRemoteUpdate(remoteUpdate: ManagedConfig['remoteUpdate']) {
    if (!detail?.spec.managedConfig) return;
    detail = {
      ...detail,
      spec: {
        ...detail.spec,
        managedConfig: { ...detail.spec.managedConfig, remoteUpdate },
      },
    };
  }

  /**
   * ConfigSourceEditor owns its local editing array and emits a semantic
   * `change` event for every edit.  A Svelte `bind:sources` binding listens
   * for a `sources` event, not this domain event, so relying on the binding
   * leaves the textarea visually changed while the ProgramSpec (and the Save
   * button fingerprint) still contains the old source.  Treat the event as
   * the authoritative source draft and replace the immutable detail object
   * immediately; this keeps Sources, status fingerprints and every sibling
   * workspace projection in sync before a save is attempted.
   */
  function updateDetailSources(sources: ConfigSource[]) {
    if (!detail?.spec.managedConfig) return;
    detail = {
      ...detail,
      spec: {
        ...detail.spec,
        managedConfig: {
          ...detail.spec.managedConfig,
          sources: structuredClone(sources),
        },
      },
    };
  }

  function updateCreateRemoteUpdate(remoteUpdate: ManagedConfig['remoteUpdate']) {
    createDraft = {
      ...createDraft,
      remoteAutoUpdate: remoteUpdate?.enabled ?? false,
      remoteUpdateIntervalMinutes:
        remoteUpdate?.intervalMinutes ?? createDraft.remoteUpdateIntervalMinutes,
    };
  }

  function normalizeHostPath(value: string) {
    return platform === 'Windows' ? value.replaceAll('\\', '/') : value;
  }

  function parentHostPath(value: string) {
    const path = normalizeHostPath(value).replace(/\/+$/, '');
    const separator = path.lastIndexOf('/');
    if (separator < 0) return '.';
    if (separator === 0) return '/';
    if (/^[A-Za-z]:$/.test(path.slice(0, separator))) return `${path.slice(0, separator)}/`;
    return path.slice(0, separator);
  }

  function isAbsoluteHostPath(value: string) {
    if (platform === 'Windows') {
      return /^[A-Za-z]:[\\/]/.test(value) || /^\\\\/.test(value) || /^\/\//.test(value);
    }
    return value.startsWith('/');
  }

  function safeRelativePath(value: string, allowCurrent: boolean) {
    const path = normalizeRelativePath(value.trim());
    if (allowCurrent && path === '.') return true;
    return (
      path.length > 0 &&
      !path.startsWith('/') &&
      !/^[A-Za-z]:/.test(path) &&
      !path.split('/').some((part) => part === '..')
    );
  }

  function validateCreateDraft() {
    const errors: Record<string, string> = {};
    const draft = createDraft;
    if (!/^[a-z0-9][a-z0-9-]{0,62}$/.test(draft.id)) {
      errors.id = 'Use lowercase letters, numbers and hyphens; start with a letter or number.';
    }
    if (!draft.name.trim() || new TextEncoder().encode(draft.name).length > 128) {
      errors.name = 'Enter a name between 1 and 128 bytes.';
    }
    if (draft.mode === 'managed') {
      const executable = normalizeRelativePath(draft.executable.trim());
      if (!safeRelativePath(executable, false)) {
        errors.executable = 'Enter an executable contained in the selected Program folder.';
      }
      if (!isAbsoluteHostPath(draft.packageSource.trim())) {
        errors.packageSource = 'Enter an absolute directory path for this operating system.';
      }
    } else {
      if (!isAbsoluteHostPath(draft.executable.trim())) {
        errors.executable = 'Enter an absolute executable path for this operating system.';
      }
    }
    if (createArgumentView.error) errors.args = createArgumentView.error;
    if (draft.managedConfiguration && createUsesExplicitConfig) {
      errors.args = 'Configuration path arguments are unavailable in managed mode.';
    }
    if (draft.managedConfiguration) {
      if (
        typeof maxConfigSourcesLimit === 'number' &&
        draft.configSources.length > maxConfigSourcesLimit
      ) {
        errors.configSources = 'The configuration source limit for this license has been reached.';
      }
      if (
        draft.remoteAutoUpdate &&
        (!Number.isInteger(draft.remoteUpdateIntervalMinutes) ||
          draft.remoteUpdateIntervalMinutes < 5 ||
          draft.remoteUpdateIntervalMinutes > 10_080)
      ) {
        errors.configSources = 'Select an update interval between 5 minutes and 7 days.';
      }
      if (!draft.configSources.some((source) => source.enabled)) {
        errors.configSources = 'Enable at least one configuration source.';
      }
      const sourceIds = new Set<string>();
      for (const source of draft.configSources) {
        if (!source.name.trim() || new TextEncoder().encode(source.name).length > 128) {
          errors.configSources = 'Configuration source names must be between 1 and 128 bytes.';
          break;
        }
        if (!/^[A-Za-z0-9_-]{1,64}$/.test(source.id) || sourceIds.has(source.id)) {
          errors.configSources = 'Configuration source identifiers must be unique.';
          break;
        }
      sourceIds.add(source.id);
      if (!source.enabled) continue;
      if (source.mode === 'inline') {
        const byteLength = new TextEncoder().encode(source.content).length;
        if (!source.content.trim() || byteLength > 4 * 1024 * 1024) {
          throw new Error('Inline configuration content must be non-empty and no larger than 4 MiB.');
        }
      } else if (
        source.mode === 'local' &&
          !isAbsoluteHostPath(source.path.trim()) &&
          !safeRelativePath(source.path, false)
        ) {
          errors.configSources = 'Use an absolute path or a path relative to the working folder.';
          break;
        }
      if (source.mode === 'remote' && !validHttpsUrl(source.url.trim())) {
          errors.configSources = 'Remote configuration sources must use HTTPS without embedded credentials';
          break;
        }
        if (source.mode === 'remote' && source.authentication) {
          if (!validBasicAuthentication(source.authentication)) {
            errors.configSources = 'Enter valid Basic authentication credentials.';
            break;
          }
        }
      }
      if (draft.kind === 'singBox' && draft.dashboardEnabled) {
        if (!Number.isInteger(draft.dashboardPort) || draft.dashboardPort < 1024 || draft.dashboardPort > 65535) {
          errors.dashboard = 'Enter a Dashboard port between 1024 and 65535.';
        } else if (!DASHBOARD_DURATION_RE.test(draft.dashboardUpdateInterval)) {
          errors.dashboard = 'Use a duration such as 12h or 1d.';
        }
      }
      if (draft.kind === 'singBox' && draft.clashDashboardEnabled) {
        if (!Number.isInteger(draft.clashDashboardPort) || draft.clashDashboardPort < 1024 || draft.clashDashboardPort > 65535) {
          errors.dashboard = 'Enter a Dashboard port between 1024 and 65535.';
        } else if (draft.dashboardEnabled && draft.clashDashboardPort === draft.dashboardPort) {
          errors.dashboard = 'sing-box API and Clash API require different ports.';
        } else if (draft.clashDashboardDownloadUrl && !validHttpsUrl(draft.clashDashboardDownloadUrl)) {
          errors.dashboard = 'Clash Dashboard download URL must use HTTPS without credentials.';
        }
      }
      if (draft.kind === 'xray' && draft.xrayDashboardEnabled) {
        if (
          !Number.isInteger(draft.xrayApiPort) ||
          draft.xrayApiPort < 1024 ||
          draft.xrayApiPort > 65535 ||
          !Number.isInteger(draft.xrayMetricsPort) ||
          draft.xrayMetricsPort < 1024 ||
          draft.xrayMetricsPort > 65535
        ) {
          errors.dashboard = 'Enter Dashboard ports between 1024 and 65535.';
        } else if (draft.xrayApiPort === draft.xrayMetricsPort) {
          errors.dashboard = 'Xray API and Metrics ports must be different.';
        }
      }
      if (draft.kind === 'mihomo' && draft.mihomoDashboardEnabled) {
        if (
          !Number.isInteger(draft.mihomoDashboardPort) ||
          draft.mihomoDashboardPort < 1024 ||
          draft.mihomoDashboardPort > 65535
        ) {
          errors.dashboard = 'Enter a Dashboard port between 1024 and 65535.';
        } else if (
          draft.mihomoDashboardDownloadUrl &&
          !validHttpsUrl(draft.mihomoDashboardDownloadUrl)
        ) {
          errors.dashboard = 'Mihomo Dashboard download URL must use HTTPS without credentials.';
        }
      }
    }
    try {
      environmentToRecord(draft.environment);
    } catch (value) {
      errors.environment = value instanceof Error ? value.message : String(value);
    }
    if (
      !draft.managedConfiguration &&
      new TextEncoder().encode(draft.initialConfig).length > 4 * 1024 * 1024
    ) {
      errors.initialConfig = 'Initial configuration cannot exceed 4 MiB.';
    }
    createFieldErrors = errors;
    return Object.keys(errors).length === 0;
  }

  async function createProgram() {
    createError = null;
    if (!validateCreateDraft() || busy) return;
    busy = 'create';
    try {
      const draft = structuredClone(createDraft) as CreateDraft;
      if (createArgumentView.error) throw new Error(createArgumentView.error);
      const args = [...createArgumentParse.args];
      const initialConfig = createHasStoredConfig
        ? draft.managedConfiguration
          ? undefined
          : draft.initialConfig
        : undefined;
      const type =
        draft.kind === 'generic'
          ? { kind: 'generic' as const, args }
          : {
              kind: draft.kind,
              extraArgs: args,
            };
      const executablePath =
        draft.mode === 'managed'
          ? `bin/${normalizeRelativePath(draft.executable.trim())}`
          : normalizeHostPath(draft.executable.trim());
      const spec: ProgramSpec = {
        id: draft.id,
        name: draft.name.trim(),
        executable: {
          mode: draft.mode,
          path: executablePath,
        },
        type,
        managedConfig: draft.managedConfiguration
          ? {
              sources: draft.configSources.map(normalizeConfigSource),
              remoteUpdate: {
                enabled: draft.remoteAutoUpdate,
                intervalMinutes: draft.remoteUpdateIntervalMinutes,
              },
              singBoxDashboard:
                draft.kind === 'singBox' && draft.dashboardEnabled
                  ? {
                      listenPort: draft.dashboardPort,
                      updateInterval: draft.dashboardUpdateInterval,
                    }
                  : undefined,
              singBoxClashDashboard:
                draft.kind === 'singBox' && draft.clashDashboardEnabled
                  ? {
                      listenPort: draft.clashDashboardPort,
                      downloadUrl: draft.clashDashboardDownloadUrl.trim() || undefined,
                    }
                  : undefined,
              xrayDashboard:
                draft.kind === 'xray' && draft.xrayDashboardEnabled
                  ? {
                      apiPort: draft.xrayApiPort,
                      metricsPort: draft.xrayMetricsPort,
                    }
                  : undefined,
              mihomoDashboard:
                draft.kind === 'mihomo' && draft.mihomoDashboardEnabled
                  ? {
                      listenPort: draft.mihomoDashboardPort,
                      downloadUrl: draft.mihomoDashboardDownloadUrl.trim() || undefined,
                    }
                  : undefined,
            }
          : undefined,
        workingDirectory:
          draft.mode === 'managed'
            ? managedWorkingDirectory(executablePath)
            : parentHostPath(executablePath),
        environment: environmentToRecord(draft.environment),
        autoStart: draft.autoStart,
        restartPolicy: draft.restartPolicy,
        privilegePolicy: draft.privilegeMode === 'standard'
          ? { mode: 'standard' }
          : { mode: draft.privilegeMode },
      };
      await api.createProgram({
        spec,
        packageSource: draft.mode === 'managed' ? draft.packageSource.trim() : undefined,
        initialConfig,
      });
      saveCreateDraft(draft);
      closeCreateDialog();
      await refreshPrograms();
      await selectProgram(spec.id);
    } catch (value) {
      createError = errorInfoOf(value);
    } finally {
      busy = '';
    }
  }

  async function removeSelected() {
    if (!detail) return;
    await removeProgram(detail.spec.id, detail.spec.name, reportPanelError);
  }

  async function removeProgram(
    id: string,
    name: string,
    onError: (value: unknown) => void = reportGlobalError,
  ) {
    closeProgramMenu();
    const confirmed = await askConfirmation(
      `${translate('Delete')} ${name}?`,
      translate('Its configuration, generated files and logs will also be deleted.'),
      translate('Delete program'),
      true,
    );
    if (!confirmed) return;
    await mutate(
      'remove',
      async () => {
        await api.removeProgram(id);
        if (selectedId === id) {
          selectedId = '';
          detail = null;
          privilegeAssessment = null;
          privilegeAssessmentLoadingId = '';
        }
        await refreshPrograms();
      },
      onError,
    );
  }

  async function replacePackage() {
    if (busy || !detail || detail.spec.executable.mode !== 'managed') return;
    if (!isAbsoluteHostPath(replacementPackageSource.trim())) {
      reportPanelError(new Error(translate('Enter an absolute directory path for this operating system.')));
      return;
    }
    const id = detail.spec.id;
    const source = replacementPackageSource.trim();
    const hasWorkspace = detail.spec.type.kind !== 'generic';
    const confirmed = await askConfirmation(
      translate('Replace the program folder?'),
      translate('The current managed files will be replaced.'),
      translate('Replace folder'),
    );
    if (!confirmed || selectedId !== id) return;
    await mutateConfiguration(
      id,
      'replace-package',
      async () => {
        const state = hasWorkspace
          ? await synchronizeFinalEditorBeforeUpstreamMutation(id)
          : null;
        const snapshot = await api.replacePackage(id, source, state?.stateRevision);
        if (snapshot) await adoptConfigurationWorkspace(id, snapshot);
        if (selectedId !== id) return;
        const updated = await api.getProgram(id);
        if (selectedId !== id) return;
        // Replacing files changes executable identity, not unsaved settings in other fields.
        detail = {
          ...updated,
          spec: { ...detail!.spec, executable: updated.spec.executable },
        };
        if (configDocument) {
          void refreshConfigurationSchemaCapability(id);
        }
        replacementPackageSource = '';
        await refreshPrograms();
      },
      reportPanelError,
    );
  }

  async function showConfiguration() {
    const programId = selectedId;
    const previousMainScrollTop = mainElement?.scrollTop ?? null;
    captureVisibleLogScrollState();
    stopLogPolling();
    stopXrayDashboardPolling();
    activeTab = 'configuration';
    clearConfigurationError();
    try {
      const editor = ensureCodeEditor();
      if (!selectedId || configDocument) {
        await editor;
        return;
      }
      const id = selectedId;
      let state = configurationState;
      if (!state || configurationStateLoadError) {
        const snapshot = await requestConfigurationWorkspace(id);
        if (selectedId !== id || activeTab !== 'configuration') return;
        await adoptConfigurationWorkspace(id, snapshot);
        state = snapshot.state;
      }
      if (selectedId !== id || activeTab !== 'configuration') return;
      configDocument = {
        content: state.workspace.editor.document.content,
        baseHash: state.desired.revision.contentHash,
        language: state.format,
        documentationUrl: '',
      };
      void loadConfigurationEditorMetadata(id, selectionGeneration);
      await editor;
    } catch (value) {
      reportConfigError(value, 'configuration-load', async () => { await showConfiguration(); });
    } finally {
      await restoreMainScrollAfterTabRender(previousMainScrollTop, 'configuration', programId);
    }
  }

  async function loadConfigurationEditorMetadata(programId: string, selection: number) {
    try {
      const document = await api.loadConfig(programId);
      if (selectedId !== programId || selectionGeneration !== selection || !configDocument) return;
      configDocument = {
        ...configDocument,
        documentationUrl: document.documentationUrl,
        configurationSchema: document.configurationSchema,
      };
      void loadConfigurationSchemaForEditor(programId, document.configurationSchema);
    } catch {
      if (selectedId === programId && selectionGeneration === selection
        && detail?.spec.type.kind === 'singBox') {
        configurationSchemaError = true;
      }
    }
  }

  async function ensureCodeEditor(): Promise<CodeEditorComponent> {
    if (CodeEditorView) return CodeEditorView;
    codeEditorLoad ??= import('./CodeEditor.svelte').then((module) => module.default);
    try {
      CodeEditorView = await codeEditorLoad;
      return CodeEditorView;
    } catch (error) {
      codeEditorLoad = null;
      throw error;
    }
  }

  function resetConfigurationSchemaState() {
    configurationSchemaGeneration += 1;
    configurationSchemaScope = '';
    configurationSchemaDocument = null;
    configurationSchemaLoading = false;
    configurationSchemaError = false;
  }

  async function refreshConfigurationSchemaCapability(programId: string): Promise<void> {
    if (!configDocument || selectedId !== programId) return;
    const previouslySupported = configDocument.configurationSchema !== undefined;
    resetConfigurationSchemaState();
    try {
      const latest = await api.loadConfig(programId);
      if (!configDocument || selectedId !== programId) return;
      configDocument = {
        ...configDocument,
        configurationSchema: latest.configurationSchema,
      };
      await loadConfigurationSchemaForEditor(
        programId,
        latest.configurationSchema,
        true,
      );
    } catch {
      if (selectedId === programId && previouslySupported) {
        configurationSchemaError = true;
      }
    }
  }

  async function loadConfigurationSchemaForEditor(
    programId: string,
    descriptor: ConfigDocument['configurationSchema'],
    force = false,
  ): Promise<void> {
    const scope = descriptor
      ? `${programId}:${descriptor.source}:${descriptor.dialect}`
      : `${programId}:none`;
    if (!force && configurationSchemaScope === scope) return;

    const generation = ++configurationSchemaGeneration;
    configurationSchemaScope = scope;
    configurationSchemaDocument = null;
    configurationSchemaError = false;
    configurationSchemaLoading = descriptor !== undefined;
    if (!descriptor) return;

    try {
      const document = await api.loadConfigurationSchema(programId);
      if (
        generation !== configurationSchemaGeneration
        || selectedId !== programId
        || configurationSchemaScope !== scope
      ) {
        return;
      }
      if (
        !document
        || document.source !== descriptor.source
        || document.dialect !== descriptor.dialect
      ) {
        configurationSchemaError = true;
        return;
      }
      configurationSchemaDocument = document;
    } catch {
      if (
        generation === configurationSchemaGeneration
        && selectedId === programId
        && configurationSchemaScope === scope
      ) {
        configurationSchemaError = true;
      }
    } finally {
      if (generation === configurationSchemaGeneration) {
        configurationSchemaLoading = false;
      }
    }
  }

  function retryConfigurationSchema() {
    if (!selectedId || !configDocument) return;
    if (!configDocument.configurationSchema) {
      void loadConfigurationEditorMetadata(selectedId, selectionGeneration);
      return;
    }
    void loadConfigurationSchemaForEditor(
      selectedId,
      configDocument.configurationSchema,
      true,
    );
  }

  async function restoreMainScrollAfterTabRender(
    scrollTop: number | null,
    expectedTab: Tab,
    expectedProgramId: string,
  ) {
    if (scrollTop === null) return;
    await tick();
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
    if (!mainElement || activeTab !== expectedTab || selectedId !== expectedProgramId) return;
    const maxScrollTop = Math.max(0, mainElement.scrollHeight - mainElement.clientHeight);
    mainElement.scrollTop = Math.min(scrollTop, maxScrollTop);
  }

  async function changeGuidedSetting(
    event: CustomEvent<{
      settingId: string;
      value?: unknown;
    }>,
  ) {
    if (!selectedId || !canEditConfigurationByLicense) return;
    const id = selectedId;
    clearIntentError();
    await mutateConfiguration(
      id,
      'guided-setting',
      async () => {
        let current = await synchronizeFinalEditorBeforeUpstreamMutation(id);
        let snapshot: ConfigurationWorkspaceSnapshot | null = null;
        for (let attempt = 0; attempt < 2; attempt += 1) {
          try {
            snapshot = await api.setGuidedIntent(id, {
              settingId: event.detail.settingId,
              value: event.detail.value,
              expectedGeneration: current.generation,
            });
            break;
          } catch (error) {
            if (attempt !== 0 || !isRetryableConfigurationConflict(error)) throw error;
            const refreshed = await api.getConfigurationWorkspace(id);
            if (selectedId !== id) throw error;
            await adoptConfigurationWorkspace(id, refreshed);
            current = refreshed.state;
          }
        }
        if (snapshot) await adoptConfigurationWorkspace(id, snapshot);
      },
      async (value) => {
        reportIntentError(value, async () => { await changeGuidedSetting(event); });
      },
    );
  }

  function runCompatibilityPrimaryAction() {
    if (configurationStateLoadError || !configurationState) void loadConfigurationStateSummary(selectedId);
    else if (coreAdmissionRejected) activateTab('overview');
    else openFinalConfiguration();
  }

  function openFinalConfiguration() {
    if (selectedId) void showConfiguration();
  }

  async function focusFinalConfigurationPath(path: string) {
    pathFocusMessage = '';
    selectedEditorPath = path;
    focusSemanticPath = '';
    await tick();
    focusSemanticPath = path;
  }

  function saveConfigurationFromEditor() {
    const candidateNeedsSave = configurationState?.workspace.editor.candidateStatus === 'unsaved';
    if (busy || (!configEditorDirty && !candidateNeedsSave) || !configurationState?.workspace.editor.canSave || !canEditConfigurationByLicense) return;
    void saveConfigurationCandidate();
  }

  async function adoptConfigurationSubmission(
    id: string,
    snapshot: ConfigurationWorkspaceSnapshot,
    submittedContent: string | undefined,
  ) {
    if (selectedId !== id) return;
    const continuedTyping = submittedContent !== undefined && configContent !== submittedContent;
    const basisGeneration = finalEditorSession?.basedOnCandidateGeneration ?? configurationState?.generation;
    const saved = snapshot.operationResult?.savedCandidate;
    await adoptConfigurationWorkspace(id, snapshot,
      continuedTyping || submittedContent === undefined || !saved ? 'preserveTyping' : 'authoritative');
    if (configurationState === snapshot.state && saved && submittedContent !== undefined
      && basisGeneration !== undefined && basisGeneration <= saved.generation) {
      // Only this request's durable save can advance the basis of continued typing.
      finalEditorInputBaseContent = saved.contentHash === snapshot.state.desired.revision.contentHash
        ? snapshot.state.desired.content : submittedContent;
    }
  }

  async function saveConfigurationCandidate(retryRequest?: import('./types').ConfigurationMutationContext, submittedContent?: string) {
    if (!configDocument || !selectedId || !configurationState
      || (!retryRequest && !configurationState.workspace.editor.canSave)) return false;
    const candidateNeedsSave = configurationState.workspace.editor.candidateStatus === 'unsaved';
    if (!retryRequest && !configEditorDirty && !candidateNeedsSave) return false;
    clearConfigurationError();
    const id = selectedId;
    let request = retryRequest;
    return mutateConfiguration(
      id,
      'save-candidate',
      async () => {
        let snapshot: ConfigurationWorkspaceSnapshot;
        if (request) {
          snapshot = await api.getConfigurationOperation(id, request);
          if (!snapshot.operationResult) snapshot = await api.commitConfigurationDraft(id, request);
        } else {
          await flushFinalEditorDraftBeforeUpstreamMutation(id);
          if (selectedId !== id || !configurationState) return;
          request = {
            operationId: crypto.randomUUID(),
            kind: 'save',
            expectedStateRevision: configurationState.stateRevision,
            editorSessionId: finalEditorSession?.sessionId,
            expectedDraftRevision: finalEditorSession?.draftRevision,
          };
          submittedContent = finalEditorSession?.workingContent ?? configurationState.workspace.editor.document.content;
          snapshot = await api.commitConfigurationDraft(id, request);
        }
        const continuedTyping = configContent !== submittedContent;
        await adoptConfigurationSubmission(id, snapshot, submittedContent);
        if (selectedId === id && snapshot.state.desired.validation !== 'invalid') {
          setConfigOutputMessage(continuedTyping ? 'Submitted changes saved.' : 'Changes saved.');
        }
      },
      async (value) => {
        if (selectedId !== id) return;
        if (isRetryableConfigurationConflict(value)) {
          request = undefined;
          submittedContent = undefined;
          try {
            await adoptConfigurationWorkspace(id, await api.getConfigurationWorkspace(id), 'preserveTyping');
          } catch {
            // Keep the text available when refreshing the workspace fails.
          }
        } else if (request) {
          try {
            await adoptConfigurationSubmission(id, await api.getConfigurationOperation(id, request), submittedContent);
          } catch {
            // Keep the original request available when its result cannot be read.
          }
        }
        if (selectedId !== id) return;
        reportConfigError(value, 'configuration-save', async () => {
          if (selectedId === id) await saveConfigurationCandidate(request, submittedContent);
        });
      },
    );
  }

  async function applyConfiguration(retryRequest?: import('./types').ConfigurationMutationContext, submittedContent?: string) {
    if (!configDocument || !selectedId || !configurationState || (!retryRequest && configurationActivationBlocked)) return false;
    clearConfigurationError();
    clearConfigOutput();
    const id = selectedId;
    const restartsProgram = !!detail && isRuntimeActive(detail.state);
    if (
      !retryRequest && restartsProgram
      && !(await askConfirmation(
        translate('Apply changes and restart?'),
        translate('The program will restart once with the updated configuration.'),
        translate('Apply and restart'),
      ))
    ) return false;
    let request = retryRequest;
    return mutateConfiguration(
      id,
      'apply',
      async () => {
        let snapshot: import('./types').ConfigurationWorkspaceSnapshot;
        if (request) {
          snapshot = await api.getConfigurationOperation(id, request);
          if (!snapshot.operationResult) snapshot = await api.activateConfigurationCandidate(id, request);
        } else {
          await flushFinalEditorDraftBeforeUpstreamMutation(id);
          if (selectedId !== id || !configurationState) return;
          request = {
            operationId: crypto.randomUUID(),
            kind: 'apply',
            expectedStateRevision: configurationState.stateRevision,
            editorSessionId: finalEditorSession?.sessionId,
            expectedDraftRevision: finalEditorSession?.draftRevision,
          };
          submittedContent = finalEditorSession?.workingContent ?? configurationState.workspace.editor.document.content;
          snapshot = await api.activateConfigurationCandidate(id, request);
        }
        await adoptConfigurationSubmission(id, snapshot, submittedContent);
        if (selectedId !== id) return;
        if (snapshot.operationResult?.status === 'interrupted'
          || (snapshot.operationResult?.status === 'rejected' && snapshot.state.desired.validation !== 'invalid')) {
          reportConfigError({
            code: 'CONFIG_CONFLICT',
            messageKey: snapshot.operationResult.messageKey ?? 'CONFIGURATION_OPERATION_REJECTED',
            message: 'Review the current configuration before applying again.',
          });
        }
        if (selectedId !== id || activeTab !== 'configuration' || !configDocument) return;
        const [nextDetail, nextActions] = await Promise.all([
          api.getProgram(id),
          api.listActions(id),
        ]);
        if (selectedId === id && detail) {
          detail = { ...detail, state: nextDetail.state };
          actions = nextActions;
        }
        await refreshPrograms();
      },
      async (value) => {
        if (selectedId !== id) return;
        if (isRetryableConfigurationConflict(value)) {
          request = undefined;
          submittedContent = undefined;
          try {
            const snapshot = await api.getConfigurationWorkspace(id);
            await adoptConfigurationWorkspace(id, snapshot, 'preserveTyping');
          } catch {
            // The retry action reloads the workspace again.
          }
        } else if (request) {
          try {
            await adoptConfigurationSubmission(id, await api.getConfigurationOperation(id, request), submittedContent);
          } catch {
            // Keep the original request available when its result cannot be read.
          }
        }
        if (selectedId !== id) return;
        reportConfigError(value, 'configuration-apply', async () => {
          if (selectedId === id) await applyConfiguration(request, submittedContent);
        });
      },
    );
  }

  function revertConfiguration() {
    if (!configDocument) return;
    if (finalEditorSession?.draftRevision) {
      void discardFinalEditorDraft();
      return;
    }
    configContent = configDocument.content;
    clearConfigOutput();
    clearConfigurationError();
  }

  async function runProgramAction(action: ActionDescriptor) {
    if (!configDocument || !selectedId) return;
    if (
      action.confirmation &&
      !(await askConfirmation(translate('Run this action?'), translate(action.label), translate('Run action')))
    ) return;
    clearConfigurationError();
    const id = selectedId;
    await mutate(
      action.id,
      async () => {
        const result = await api.runAction(
          id,
          action.id,
          configContent,
          configDocument!.baseHash,
        );
        if (selectedId !== id || activeTab !== 'configuration') return;
        if (result.previewContent !== undefined) {
          const changed = result.previewContent !== configContent;
          configContent = result.previewContent;
          setConfigOutputMessage(changed
            ? 'Formatting complete. Save to keep these changes.'
            : 'Configuration is already formatted.');
        } else {
          setConfigOutputMessage(configurationDiagnosticMessage(result.report.messageKey));
        }
      },
      reportConfigError,
    );
  }

  function actionAllowed(action: ActionDescriptor) {
    return detail !== null &&
      canRunDiagnosticsByLicense &&
      action.allowedStates.includes(detail.state.status);
  }

  async function showLogs() {
    stopXrayDashboardPolling();
    ensureLogScrollStateForSelectedProgram();
    beginLogScrollRestore();
    activeTab = 'logs';
    await refreshLogs(false);
    await restoreVisibleLogScrollAfterRender();
    if (activeTab === 'logs' && selectedId) startLogPolling();
  }

  async function refreshLogs(manual = false) {
    if (!selectedId) return;
    const id = selectedId;
    const generation = logGeneration;
    const requestKey = `${generation}:${id}`;
    if (logRequestKey === requestKey) return;
    logRequestKey = requestKey;
    if (manual) {
      manualLogRequestKey = requestKey;
      manualLogRefreshing = true;
    }
    try {
      const [stdout, stderr] = await Promise.all([
        api.readLogs(id, 'stdout', 131072),
        api.readLogs(id, 'stderr', 131072),
      ]);
      if (selectedId !== id || activeTab !== 'logs' || generation !== logGeneration) return;
      ensureLogScrollStateForSelectedProgram();
      const followStdout = logScrollState.stdout.followLatest && logIsNearBottom(stdoutLogElement);
      const followStderr = logScrollState.stderr.followLatest && logIsNearBottom(stderrLogElement);
      let contentChanged = false;
      if (
        logContents.stdout !== stdout.content ||
        logContents.stderr !== stderr.content
      ) {
        logContents = { stdout: stdout.content, stderr: stderr.content };
        contentChanged = true;
      }
      if (
        logTruncated.stdout !== stdout.truncated ||
        logTruncated.stderr !== stderr.truncated
      ) {
        logTruncated = { stdout: stdout.truncated, stderr: stderr.truncated };
      }
      if (contentChanged) {
        await tick();
        if (selectedId !== id || activeTab !== 'logs' || generation !== logGeneration) return;
        if (followStdout) scrollLogPaneToBottom(stdoutLogElement);
        else if (stdoutLogElement) applyLogScroll('stdout', stdoutLogElement);
        if (followStderr) scrollLogPaneToBottom(stderrLogElement);
        else if (stderrLogElement) applyLogScroll('stderr', stderrLogElement);
        if (stdoutLogElement) updateLogScrollState('stdout', stdoutLogElement);
        if (stderrLogElement) updateLogScrollState('stderr', stderrLogElement);
      }
    } catch (value) {
      if (selectedId === id && activeTab === 'logs' && generation === logGeneration) {
        reportGlobalError(value);
      }
    } finally {
      if (logRequestKey === requestKey) logRequestKey = '';
      if (manualLogRequestKey === requestKey) {
        manualLogRequestKey = '';
        manualLogRefreshing = false;
      }
    }
  }

  function invalidateLogRequests() {
    logGeneration += 1;
    logRequestKey = '';
    manualLogRequestKey = '';
    manualLogRefreshing = false;
  }

  async function clearLogHistory() {
    if (!selectedId) return;
    const confirmed = await askConfirmation(
      translate('Clear all logs?'),
      translate('Current and rotated output for this program will be removed.'),
      translate('Clear logs'),
      true,
    );
    if (!confirmed) return;
    const id = selectedId;
    invalidateLogRequests();
    stopLogPolling();
    const cleared = await mutate('clear-logs', () => api.clearLogs(id));
    if (!cleared || selectedId !== id) {
      if (selectedId === id && activeTab === 'logs') startLogPolling();
      return;
    }
    logContents = { stdout: '', stderr: '' };
    logTruncated = { stdout: false, stderr: false };
    resetLogScrollState(id);
    startLogPolling();
  }

  function startLogPolling() {
    stopLogPolling();
    const poll = async () => {
      await refreshLogs(false);
      if (activeTab === 'logs' && selectedId) {
        logTimer = window.setTimeout(() => void poll(), 2_000);
      }
    };
    logTimer = window.setTimeout(() => void poll(), 2_000);
  }

  function stopLogPolling() {
    if (logTimer !== undefined) window.clearTimeout(logTimer);
    logTimer = undefined;
  }

  function resetLogScrollState(programId = selectedId) {
    logScrollProgramId = programId;
    logScrollState = defaultLogScrollState();
  }

  function ensureLogScrollStateForSelectedProgram() {
    if (selectedId && logScrollProgramId !== selectedId) resetLogScrollState(selectedId);
  }

  function beginLogScrollRestore() {
    logScrollRestoring = true;
    logScrollRestoreGeneration += 1;
    return logScrollRestoreGeneration;
  }

  function finishLogScrollRestore(generation: number) {
    if (generation === logScrollRestoreGeneration) logScrollRestoring = false;
  }

  function visibleLogPaneKinds(): LogPaneKind[] {
    if (logView === 'stdout') return ['stdout'];
    if (logView === 'stderr') return ['stderr'];
    return ['stdout', 'stderr'];
  }

  function setLogScrollState(kind: LogPaneKind, nextState: LogPaneScrollState) {
    logScrollState = { ...logScrollState, [kind]: nextState };
  }

  function updateLogScrollState(kind: LogPaneKind, element: HTMLPreElement) {
    ensureLogScrollStateForSelectedProgram();
    setLogScrollState(kind, {
      followLatest: logIsNearBottom(element),
      scrollTop: element.scrollTop,
    });
  }

  function captureVisibleLogScrollState() {
    if (stdoutLogElement) updateLogScrollState('stdout', stdoutLogElement);
    if (stderrLogElement) updateLogScrollState('stderr', stderrLogElement);
  }

  function scrollElementToBottom(element: HTMLElement | null) {
    if (!element) return;
    element.scrollTop = element.scrollHeight;
  }

  function scrollLogPaneToBottom(element: HTMLPreElement | null) {
    scrollElementToBottom(element);
  }

  function applyLogScroll(kind: LogPaneKind, element: HTMLPreElement) {
    const state = logScrollState[kind];
    if (state.followLatest) {
      element.scrollTop = element.scrollHeight;
      return;
    }
    const maxScrollTop = Math.max(0, element.scrollHeight - element.clientHeight);
    element.scrollTop = Math.min(state.scrollTop, maxScrollTop);
  }

  async function restoreVisibleLogScrollAfterRender() {
    ensureLogScrollStateForSelectedProgram();
    const expectedProgramId = logScrollProgramId;
    const restoreGeneration = beginLogScrollRestore();
    try {
      await tick();
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      if (selectedId !== expectedProgramId || activeTab !== 'logs') return;
      if (stdoutLogElement) {
        applyLogScroll('stdout', stdoutLogElement);
        updateLogScrollState('stdout', stdoutLogElement);
      }
      if (stderrLogElement) {
        applyLogScroll('stderr', stderrLogElement);
        updateLogScrollState('stderr', stderrLogElement);
      }
    } finally {
      finishLogScrollRestore(restoreGeneration);
    }
  }

  function trackLogScroll(node: HTMLPreElement, kind: LogPaneKind) {
    const programId = selectedId;
    const onScroll = () => {
      if (logScrollRestoring) return;
      if (selectedId === programId && logScrollProgramId === programId) {
        updateLogScrollState(kind, node);
      }
    };
    void restoreVisibleLogScrollAfterRender();
    node.addEventListener('scroll', onScroll, { passive: true });
    return {
      destroy() {
        node.removeEventListener('scroll', onScroll);
      },
    };
  }

  async function showXrayDashboard() {
    if (!dashboardCanOpen) return;
    captureVisibleLogScrollState();
    stopLogPolling();
    activeTab = 'dashboard';
    await refreshXrayDashboard(true);
    startXrayDashboardPolling();
  }

  async function refreshXrayDashboard(_manual = false) {
    if (!selectedId || !detail || detail.spec.type.kind !== 'xray') return;
    if (!detail.spec.managedConfig?.xrayDashboard) return;
    if (detail.state.status !== 'running') {
      stopXrayDashboardPolling();
      xrayDashboardRefreshing = false;
      xrayDashboardManualRefreshing = false;
      xrayDashboardError = null;
      return;
    }
    if (xrayDashboardRefreshing) return;
    const id = selectedId;
    const routingGeneration = xrayRoutingGeneration;
    xrayDashboardRefreshing = true;
    if (_manual) xrayDashboardManualRefreshing = true;
    xrayDashboardError = null;
    try {
      const snapshot = await api.getXrayDashboardSnapshot(
        id,
        true,
        _manual || !xrayDashboardSnapshot?.topology,
      );
      if (selectedId !== id || activeTab !== 'dashboard') return;
      if (!snapshot.topology && xrayDashboardSnapshot?.topology) {
        snapshot.topology = xrayDashboardSnapshot.topology;
        snapshot.topologyError = xrayDashboardSnapshot.topologyError;
      }
      if (routingGeneration !== xrayRoutingGeneration && xrayDashboardSnapshot?.balancers) {
        snapshot.balancers = xrayDashboardSnapshot.balancers;
        snapshot.routingError = xrayDashboardSnapshot.routingError;
      }
      xrayDashboardSnapshot = snapshot;
    } catch (value) {
      if (selectedId === id && activeTab === 'dashboard') {
        xrayDashboardError = errorInfoOf(value);
      }
    } finally {
      if (selectedId === id) xrayDashboardRefreshing = false;
      if (selectedId === id && _manual) xrayDashboardManualRefreshing = false;
    }
  }

  function startXrayDashboardPolling() {
    stopXrayDashboardPolling();
    if (!dashboardCanOpen) return;
    const poll = async () => {
      await refreshXrayDashboard(false);
      if (activeTab === 'dashboard' && selectedId && dashboardCanOpen) {
        xrayDashboardTimer = window.setTimeout(() => void poll(), 3_000);
      }
    };
    xrayDashboardTimer = window.setTimeout(() => void poll(), 3_000);
  }

  function stopXrayDashboardPolling() {
    if (xrayDashboardTimer !== undefined) window.clearTimeout(xrayDashboardTimer);
    xrayDashboardTimer = undefined;
  }

  function logIsNearBottom(element: HTMLElement | null) {
    return !element || element.scrollHeight - element.clientHeight - element.scrollTop < 48;
  }

  async function showOverview() {
    captureVisibleLogScrollState();
    stopLogPolling();
    stopXrayDashboardPolling();
    activeTab = 'overview';
  }

  function activateTab(tab: Tab) {
    if (tab === 'overview') void showOverview();
    else if (tab === 'dashboard') void showXrayDashboard();
    else if (tab === 'configuration') void showConfiguration();
    else if (tab === 'intent' || tab === 'sources' || tab === 'compatibility') void showConfigurationWorkspace(tab);
    else void showLogs();
  }

  async function showConfigurationWorkspace(tab: Extract<Tab, 'intent' | 'sources' | 'compatibility'>) {
    captureVisibleLogScrollState();
    stopLogPolling();
    stopXrayDashboardPolling();
    activeTab = tab;
    if (!selectedId || detail?.spec.type.kind === 'generic') return;
    if (configurationState && !configurationStateLoadError) return;
    const id = selectedId;
    await loadConfigurationStateSummary(id);
  }

  function filterLog(content: string, query: string) {
    const needle = query.trim().toLocaleLowerCase();
    if (!needle) return content;
    return content
      .split(/\r?\n/)
      .filter((line) => line.toLocaleLowerCase().includes(needle))
      .join('\n');
  }

  function logLineCount(content: string) {
    return content ? content.split(/\r?\n/).length : 0;
  }

  function objectOf(value: unknown): Record<string, unknown> | null {
    return value && typeof value === 'object' && !Array.isArray(value)
      ? value as Record<string, unknown>
      : null;
  }

  function changeXrayTrafficSort(key: XrayTrafficSort) {
    if (xrayTrafficSort === key) {
      xrayTrafficSortAscending = !xrayTrafficSortAscending;
      return;
    }
    xrayTrafficSort = key;
    xrayTrafficSortAscending = key === 'scope' || key === 'tag';
  }

  async function setXrayBalancerTarget(balancer: XrayBalancerInfo, target: string) {
    if (!selectedId || xrayRoutingBusyTag || !dashboardIsRunning) return;
    const id = selectedId;
    xrayRoutingGeneration += 1;
    xrayRoutingBusyTag = balancer.tag;
    xrayDashboardError = null;
    try {
      const updated = await api.setXrayBalancerTarget(id, balancer.tag, target || undefined);
      if (selectedId !== id || !xrayDashboardSnapshot?.balancers) return;
      xrayDashboardSnapshot = {
        ...xrayDashboardSnapshot,
        balancers: xrayDashboardSnapshot.balancers.map((entry) =>
          entry.tag === updated.tag ? updated : entry
        ),
        routingError: undefined,
      };
    } catch (value) {
      if (selectedId === id) xrayDashboardError = errorInfoOf(value);
    } finally {
      if (selectedId === id) xrayRoutingBusyTag = '';
    }
  }

  async function restartXrayLogger() {
    if (!selectedId || xrayLoggerBusy || !dashboardIsRunning) return;
    const id = selectedId;
    xrayLoggerBusy = true;
    xrayDashboardError = null;
    try {
      await api.restartXrayLogger(id);
      if (selectedId === id && activeTab === 'dashboard') {
        await refreshXrayDashboard(true);
      }
    } catch (value) {
      if (selectedId === id) xrayDashboardError = errorInfoOf(value);
    } finally {
      if (selectedId === id) xrayLoggerBusy = false;
    }
  }

  async function openWorkingDirectory(id = selectedId) {
    if (!id) return;
    closeProgramMenu();
    await runExternalAction(`working-directory:${id}`, () => api.openWorkingDirectory(id));
  }

  function openProgramMenu(event: MouseEvent | KeyboardEvent, program: ProgramSummary) {
    event.preventDefault();
    event.stopPropagation();
    if (bulkMode) return;
    const width = 224;
    const target = event.currentTarget instanceof HTMLElement ? event.currentTarget : null;
    programMenuTrigger = target;
    const bounds = target?.getBoundingClientRect();
    const requestedX = event instanceof MouseEvent ? event.clientX : (bounds?.right ?? 8) + 4;
    const requestedY = event instanceof MouseEvent ? event.clientY : (bounds?.top ?? 8);
    programMenu = {
      program,
      x: Math.max(8, Math.min(requestedX, window.innerWidth - width - 8)),
      y: Math.max(8, Math.min(requestedY, window.innerHeight - 8)),
    };
  }

  function closeProgramMenu(restoreFocus = true) {
    programMenu = null;
    if (restoreFocus && programMenuTrigger?.isConnected) programMenuTrigger.focus({ preventScroll: true });
    programMenuTrigger = null;
  }

  async function openDataDirectory() {
    await runExternalAction('data-directory', () => api.openDataDirectory());
  }

  async function updateAppAutostart() {
    const next = !appAutostart;
    appSettingsError = null;
    await mutate(
      'autostart',
      async () => {
        await api.setAutostart(next);
        appAutostart = next;
      },
      (value) => (appSettingsError = errorInfoOf(value)),
    );
  }

  async function openSettingsDialog(focusSection: SettingsFocusSection = null) {
    settingsReturnFocus = activeFocusTarget();
    appSettingsError = null;
    licenseError = null;
    if (!SettingsView) {
      try {
        SettingsView = (await import('./SettingsDialog.svelte')).default;
      } catch (value) {
        reportGlobalError(value);
        return;
      }
    }
    behaviorSaved = false;
    settingsFocusSection = focusSection;
    settingsActiveSection = focusSection ?? 'appearance';
    settingsInitialFocus = `[data-settings-section="${settingsActiveSection}"]`;
    closeSidebarDrawer();
    showSettings = true;
    try {
      const [autostartResult, settingsResult, entitlementResult, licenseSettingsResult, localLicenseDeviceResult] = await Promise.allSettled([
        api.getAutostart(),
        api.getAppSettings(),
        api.getEntitlementState(),
        api.getLicenseServiceSettings(),
        api.getLocalLicenseDevice(),
      ]);
      if (autostartResult.status === 'fulfilled') {
        appAutostart = autostartResult.value;
      } else {
        appSettingsError = errorInfoOf(autostartResult.reason);
      }
      if (settingsResult.status === 'fulfilled') {
        appSettings = normalizeAppSettings(settingsResult.value);
      } else if (!appSettingsError) {
        appSettingsError = errorInfoOf(settingsResult.reason);
      }
      if (entitlementResult.status === 'fulfilled') {
        await applyEntitlementSnapshot(entitlementResult.value, 'settings-open');
      } else {
        licenseError = errorInfoOf(entitlementResult.reason);
      }
      if (licenseSettingsResult.status === 'fulfilled') {
        licenseServiceSettings = licenseSettingsResult.value;
      } else if (!licenseError) {
        licenseError = errorInfoOf(licenseSettingsResult.reason);
      }
      if (localLicenseDeviceResult.status === 'fulfilled') {
        localLicenseDevice = localLicenseDeviceResult.value;
      } else if (!licenseError) {
        licenseError = errorInfoOf(localLicenseDeviceResult.reason);
      }
      if (licenseStateIsActive(entitlementState) && visibleActiveLicenseState()) {
        showLicenseSettingsData();
      }
    } catch (value) {
      appSettingsError = errorInfoOf(value);
    }
  }

  function closeSettingsDialog() {
    showSettings = false;
    licenseTeamInvitation = null;
    licenseTeamDeviceEnrollment = null;
    licenseTeamSecretGeneration += 1;
    settingsFocusSection = null;
    settingsInitialFocus = '';
    const returnFocus = settingsReturnFocus;
    settingsReturnFocus = null;
    void restoreFocusAfterModal(returnFocus);
  }

  async function beginLicenseAuthorization() {
    const previousState = licenseAuthorizationRequest?.state;
    if (previousState) {
      await api.cancelLicenseAuthorization(previousState).catch(() => null);
    }
    licenseError = null;
    licenseAuthorizationRequest = null;
    licenseAuthorizationCompletingState = '';
    licenseAuthorizationCompletingStates = new Set<string>();
    clearLicenseAuthorizationTimeout();
    const generation = ++licenseAuthorizationGeneration;
    logLicenseFlow('info', 'begin');
    const ok = await mutate(
      'license-authorize',
      async () => {
        const request = await api.beginLicenseAuthorization(import.meta.env.MODE !== 'e2e');
        if (generation !== licenseAuthorizationGeneration) return;
        licenseAuthorizationRequest = request;
        logLicenseFlow('info', 'request-ready');
        scheduleLicenseAuthorizationTimeout(request, generation);
        if (!licenseAuthorizationDisplayName.trim()) {
          licenseAuthorizationDisplayName = request.suggestedDeviceName;
        }
      },
      (value) => {
        if (generation === licenseAuthorizationGeneration) {
          logLicenseFlow('warn', 'begin-failed');
          licenseError = errorInfoOf(value);
        }
      },
    );
    if (ok) {
      licenseServiceSettings = await api.getLicenseServiceSettings().catch(() => licenseServiceSettings);
    }
  }

  async function handleLicenseAuthorizationCallback(event: LicenseAuthorizationCallbackEvent) {
    const request = licenseAuthorizationRequest;
    logLicenseFlow('info', 'callback-received-by-ui');
    if (!request || event.state !== request.state || licenseAuthorizationCompletingState === request.state) return;
    if (busy && busy !== 'license-complete') {
      logLicenseFlow('debug', 'callback-waiting-for-idle');
      return;
    }
    licenseAuthorizationCompletingState = request.state;
    const generation = licenseAuthorizationGeneration;
    try {
      await completeLicenseAuthorization(
        {
          expectedState: request.state,
          displayName: licenseAuthorizationDisplayName.trim() || request.suggestedDeviceName,
        },
        generation,
      );
    } finally {
      licenseAuthorizationCompletingState = '';
    }
  }

  function handleLicenseAuthorizationFailed(event: LicenseAuthorizationFailedEvent) {
    const request = licenseAuthorizationRequest;
    if (!request || event.state !== request.state) return;
    failLicenseAuthorization(request, event.message, 'callback-failed');
  }

  async function cancelLicenseAuthorization() {
    const state = licenseAuthorizationRequest?.state;
    if (state) {
      logLicenseFlow('info', 'cancel');
      await api.cancelLicenseAuthorization(state).catch(() => null);
    }
    resetLicenseAuthorizationProgress();
    licenseError = null;
    void syncLicenseState(false);
  }

  async function completeLicenseAuthorization(request: {
    expectedState: string;
    displayName?: string;
  }, generation = licenseAuthorizationGeneration) {
    licenseError = null;
    if (licenseAuthorizationCompletingStates.has(request.expectedState)) {
      logLicenseFlow('debug', 'complete-duplicate-ignored');
      return;
    }
    licenseAuthorizationCompletingStates = new Set([
      ...licenseAuthorizationCompletingStates,
      request.expectedState,
    ]);
    logLicenseFlow('info', 'complete-start');
    await mutate(
      'license-complete',
      async () => {
        logLicenseFlow('debug', 'complete-invoke');
        const snapshot = await api.completeLicenseAuthorization(request);
        logLicenseFlow('info', 'complete-success');
        if (generation !== licenseAuthorizationGeneration) return;
        await applyEntitlementSnapshot(snapshot, 'authorization-complete');
        resetLicenseAuthorizationProgress();
        localLicenseDevice = await api.getLocalLicenseDevice().catch(() => localLicenseDevice);
        void refreshLicenseDevicesQuietly(false, true);
      },
      (value) => {
        if (generation === licenseAuthorizationGeneration) {
          logLicenseFlow('warn', 'complete-failed');
          const error = errorInfoOf(value);
          licenseError = error;
          const state = licenseAuthorizationRequest?.state;
          resetLicenseAuthorizationProgress();
          if (state) void api.cancelLicenseAuthorization(state).catch(() => null);
          return reconcileEntitlementStateAfterFailure('authorization-complete-failed');
        }
      },
    );
    licenseAuthorizationCompletingStates = new Set(
      [...licenseAuthorizationCompletingStates].filter((state) => state !== request.expectedState),
    );
  }

  async function refreshLicenseEntitlement() {
    licenseError = null;
    await mutate(
      'license-refresh',
      async () => {
        if (
          licenseServiceSettings?.configured
          && (
            hasRefreshableLicenseSession(entitlementState)
            || (
              entitlementState?.status === 'deviceDenied'
              && entitlementState.state !== 'removed'
            )
          )
        ) {
          try {
            await refreshLicenseOnline();
          } catch (value) {
            await reconcileEntitlementStateAfterFailure('online-refresh-failed');
            throw value;
          }
        } else {
          await syncLicenseState(true);
        }
      },
      (value) => (licenseError = errorInfoOf(value)),
    );
  }

  async function refreshLicenseOnline() {
    const activeRequest = licenseRefreshRequest;
    if (activeRequest) return activeRequest;
    const request = (async () => {
      const snapshot = await api.refreshLicenseEntitlement();
      await applyEntitlementSnapshot(snapshot, 'online-refresh');
    })();
    licenseRefreshRequest = request;
    try {
      await request;
    } finally {
      if (licenseRefreshRequest === request) licenseRefreshRequest = null;
    }
  }

  async function reconnectLicenseDevice() {
    licenseError = null;
    await mutate(
      'license-reconnect-device',
      async () => {
        try {
          const snapshot = await api.reconnectLicenseDevice();
          await applyEntitlementSnapshot(snapshot, 'device-reconnected');
          void refreshLicenseDevicesQuietly(false, true);
        } catch (value) {
          await reconcileEntitlementStateAfterFailure('device-reconnect-failed');
          throw value;
        }
      },
      (value) => (licenseError = errorInfoOf(value)),
    );
  }

  async function loadLicenseDevices() {
    licenseError = null;
    await mutate(
      'license-devices',
      async () => {
        await refreshLicenseDevicesQuietly(true, true);
      },
      (value) => (licenseError = errorInfoOf(value)),
    );
  }

  async function loadMoreLicenseDevices() {
    if (!licenseDevicesNextCursor) return;
    licenseError = null;
    await mutate(
      'license-devices-more',
      async () => {
        await refreshLicenseDevicesQuietly(true, true, true);
      },
      (value) => (licenseError = errorInfoOf(value)),
    );
  }

  async function refreshLicenseDevicesQuietly(
    reportError = false,
    force = false,
    append = false,
  ) {
    if (licenseDevicesRequestActive || !licenseStateIsActive(entitlementState)) return;
    const now = Date.now();
    if (!force && now - licenseDevicesLastLoadedAt < licenseAuxiliaryAutoRefreshMinIntervalMs) return;
    const auxiliaryGeneration = licenseAuxiliaryGeneration;
    licenseDevicesRequestActive = true;
    try {
      const cursor = append ? licenseDevicesNextCursor ?? undefined : undefined;
      const page = await api.getLicenseDevices(cursor);
      if (
        auxiliaryGeneration !== licenseAuxiliaryGeneration
        || !licenseStateIsActive(entitlementState)
      ) return;
      if (append) {
        const merged = new Map(licenseDevices.map((device) => [device.deviceId, device]));
        for (const device of page.devices) merged.set(device.deviceId, device);
        licenseDevices = [...merged.values()];
      } else {
        licenseDevices = page.devices;
      }
      licenseDevicesNextCursor = page.nextCursor ?? null;
      licenseDevicesLastLoadedAt = Date.now();
      if (reportError) licenseError = null;
    } catch (value) {
      await reconcileEntitlementStateAfterFailure('device-list-failed');
      if (reportError && auxiliaryGeneration === licenseAuxiliaryGeneration) {
        licenseError = errorInfoOf(value);
      }
    } finally {
      licenseDevicesRequestActive = false;
      refreshLicenseAuxiliaryAfterStaleRequest(auxiliaryGeneration);
    }
  }

  async function refreshLicenseBillingQuietly(reportError = false, force = false) {
    if (licenseBillingRequestActive || !licenseStateIsActive(entitlementState)) return;
    if (!force && Date.now() - licenseBillingLastLoadedAt < licenseAuxiliaryAutoRefreshMinIntervalMs) return;
    const auxiliaryGeneration = licenseAuxiliaryGeneration;
    licenseBillingRequestActive = true;
    try {
      const summary = await api.getLicenseBillingSummary();
      if (
        auxiliaryGeneration !== licenseAuxiliaryGeneration
        || !licenseStateIsActive(entitlementState)
      ) return;
      licenseBillingSummary = summary;
      licenseBillingLastLoadedAt = Date.now();
      if (reportError) licenseBillingError = null;
    } catch (value) {
      if (reportError && auxiliaryGeneration === licenseAuxiliaryGeneration) {
        licenseBillingError = errorInfoOf(value);
      }
    } finally {
      licenseBillingRequestActive = false;
      refreshLicenseAuxiliaryAfterStaleRequest(auxiliaryGeneration);
    }
  }

  async function submitLicensePaymentClaim(submission: CustomerPaymentSubmission) {
    licenseBillingError = null;
    await mutate(
      'license-payment-claim',
      async () => {
        await api.submitLicensePaymentClaim(submission);
        await refreshLicenseBillingQuietly(true, true);
      },
      (value) => (licenseBillingError = errorInfoOf(value)),
    );
  }

  function visibleActiveLicenseState(): Extract<EntitlementState, { status: 'active' }> | null {
    const state = entitlementState;
    return showSettings
      && settingsActiveSection === 'license'
      && licenseStateIsActive(state)
      ? state
      : null;
  }

  function refreshVisibleLicenseData(force = false) {
    const state = visibleActiveLicenseState();
    if (!state) return;
    void refreshLicenseDevicesQuietly(false, force);
    if (state.entitlement.claims.plan === 'team') {
      void (async () => {
        const profileLoaded = await refreshLicenseTeamQuietly(false, false, force);
        if (!profileLoaded) return;
        if (licenseTeamProfile?.permissions.includes('billing.read')) {
          await refreshLicenseBillingQuietly(false, force);
        } else {
          licenseBillingSummary = null;
          licenseBillingError = null;
          licenseBillingLastLoadedAt = 0;
        }
      })();
    } else {
      void refreshLicenseBillingQuietly(false, force);
    }
  }

  function showLicenseSettingsData() {
    // Opening an already-fresh surface counts as the current attention refresh.
    // Focus, visibility and online events arriving in the same short window must
    // merge with it instead of immediately scheduling another full Team read.
    licenseAuxiliaryLastAttentionRefreshAt = Date.now();
    refreshVisibleLicenseData();
  }

  function refreshVisibleLicenseDataAfterAttention() {
    if (!visibleActiveLicenseState()) return;
    const now = Date.now();
    if (
      now - licenseAuxiliaryLastAttentionRefreshAt
      < licenseAuxiliaryAttentionRefreshMinIntervalMs
    ) return;
    licenseAuxiliaryLastAttentionRefreshAt = now;
    refreshVisibleLicenseData(true);
  }

  async function refreshLicenseTeamQuietly(
    reportError = false,
    forceAfterPending = false,
    force = false,
  ) {
    if (
      !licenseStateIsActive(entitlementState)
      || entitlementState.entitlement.claims.plan !== 'team'
    ) return false;
    if (!force && Date.now() - licenseTeamLastLoadedAt < licenseAuxiliaryAutoRefreshMinIntervalMs) return true;
    const pendingRequest = licenseTeamRequest;
    if (pendingRequest) {
      let pendingSucceeded = false;
      try {
        pendingSucceeded = await pendingRequest;
        if (reportError) licenseTeamError = null;
      } catch (value) {
        pendingSucceeded = false;
        if (reportError) licenseTeamError = errorInfoOf(value);
      }
      if (
        !forceAfterPending
        || !licenseStateIsActive(entitlementState)
        || entitlementState.entitlement.claims.plan !== 'team'
      ) return pendingSucceeded;
      const successorRequest = licenseTeamRequest;
      if (successorRequest && successorRequest !== pendingRequest) {
        try {
          const successorSucceeded = await successorRequest;
          if (reportError) licenseTeamError = null;
          return successorSucceeded;
        } catch (value) {
          if (reportError) licenseTeamError = errorInfoOf(value);
          return false;
        }
      }
    }
    const auxiliaryGeneration = licenseAuxiliaryGeneration;
    const request = (async () => {
      const profile = await api.getLicenseTeamProfile();
      let members: WorkspaceMember[] = [];
      let nextCursor: string | null = null;
      let hasMore = false;
      if (profile.enabled && profile.permissions.includes('team.read')) {
        const page = await api.getLicenseTeamMembers(null, 100);
        members = page.members;
        nextCursor = page.nextCursor ?? null;
        hasMore = page.hasMore;
      }
      if (
        auxiliaryGeneration !== licenseAuxiliaryGeneration
        || !licenseStateIsActive(entitlementState)
        || entitlementState.entitlement.claims.plan !== 'team'
      ) return false;
      if (licenseTeamSecretScope(profile) !== licenseTeamSecretScope(licenseTeamProfile)) {
        dismissLicenseTeamInvitation();
        dismissLicenseTeamDeviceEnrollment();
      }
      licenseTeamProfile = profile;
      if (!profile.member || profile.member.status !== 'active') {
        dismissLicenseTeamDeviceEnrollment();
      }
      licenseTeamMembers = members;
      licenseTeamMembersNextCursor = nextCursor;
      licenseTeamMembersHasMore = hasMore;
      if (
        licenseTeamInvitation
        && members.some((member) =>
          member.id === licenseTeamInvitation?.member.id && member.status !== 'invited'
        )
      ) dismissLicenseTeamInvitation();
      return true;
    })();
    licenseTeamRequest = request;
    try {
      if (!await request) return false;
      licenseTeamLastLoadedAt = Date.now();
      if (reportError) licenseTeamError = null;
      return true;
    } catch (value) {
      if (reportError) licenseTeamError = errorInfoOf(value);
      return false;
    } finally {
      if (licenseTeamRequest === request) licenseTeamRequest = null;
      refreshLicenseAuxiliaryAfterStaleRequest(auxiliaryGeneration);
    }
  }

  function licenseTeamSecretScope(profile: TeamProfile | null) {
    const member = profile?.member;
    return member
      ? [
          member.id,
          member.status,
          member.role,
          member.rowVersion,
          ...[...profile.permissions].sort(),
        ].join('\u0000')
      : 'unlinked';
  }

  async function refreshLicenseAfterWorkspaceBinding() {
    try {
      await refreshLicenseOnline();
      licenseError = null;
    } catch (value) {
      await reconcileEntitlementStateAfterFailure('team-binding-refresh-failed');
      licenseError = errorInfoOf(value);
    }
    await refreshLicenseTeamQuietly(true, true, true);
  }

  async function loadMoreLicenseTeamMembers() {
    const cursor = licenseTeamMembersNextCursor;
    if (
      !cursor
      || !licenseTeamMembersHasMore
      || licenseTeamRequest
      || !licenseStateIsActive(entitlementState)
      || !licenseTeamProfile?.permissions.includes('team.read')
    ) return;
    const auxiliaryGeneration = licenseAuxiliaryGeneration;
    const request = (async () => {
      const page = await api.getLicenseTeamMembers(cursor, 100);
      if (auxiliaryGeneration !== licenseAuxiliaryGeneration) return false;
      const merged = new Map(licenseTeamMembers.map((member) => [member.id, member]));
      for (const member of page.members) merged.set(member.id, member);
      licenseTeamMembers = [...merged.values()];
      licenseTeamMembersNextCursor = page.nextCursor ?? null;
      licenseTeamMembersHasMore = page.hasMore;
      return true;
    })();
    licenseTeamMembersLoadingMore = true;
    licenseTeamRequest = request;
    try {
      if (await request) licenseTeamError = null;
    } catch (value) {
      licenseTeamError = errorInfoOf(value);
    } finally {
      licenseTeamMembersLoadingMore = false;
      if (licenseTeamRequest === request) licenseTeamRequest = null;
      refreshLicenseAuxiliaryAfterStaleRequest(auxiliaryGeneration);
    }
  }

  async function createLicenseTeamInvitation(request: CreateTeamInvitation) {
    licenseTeamError = null;
    licenseTeamDeviceEnrollment = null;
    const secretGeneration = licenseTeamSecretGeneration;
    await mutateLicenseTeam(
      'license-team-invitation',
      async () => {
        const invitation = await api.createLicenseTeamInvitation(request);
        if (secretGeneration === licenseTeamSecretGeneration && showSettings) {
          licenseTeamInvitation = invitation;
        } else {
          invitation.invitationToken = '';
        }
        await refreshLicenseTeamQuietly(true, false, true);
      },
      (value) => (licenseTeamError = errorInfoOf(value)),
    );
  }

  async function acceptLicenseTeamInvitation(invitationToken: string, operationId: string) {
    licenseTeamError = null;
    await mutateLicenseTeam(
      'license-team-invitation-accept',
      async () => {
        licenseTeamProfile = await api.acceptLicenseTeamInvitation(
          invitationToken.trim(),
          operationId,
        );
        licenseTeamInvitation = null;
        licenseTeamDeviceEnrollment = null;
      },
      (value) => (licenseTeamError = errorInfoOf(value)),
    );
    await refreshLicenseAfterWorkspaceBinding();
  }

  async function updateLicenseTeamMember(memberId: string, request: UpdateWorkspaceMember) {
    licenseTeamError = null;
    await mutateLicenseTeam(
      'license-team-member-update',
      async () => {
        const updated = await api.updateLicenseTeamMember(memberId, request);
        if (request.status === 'removed' && licenseTeamInvitation?.member.id === memberId) {
          licenseTeamInvitation.invitationToken = '';
          licenseTeamInvitation = null;
        }
        licenseTeamMembers = licenseTeamMembers.map((member) => member.id === updated.id ? updated : member);
        await refreshLicenseTeamQuietly(true, false, true);
      },
      reportLicenseTeamMutationError,
    );
  }

  async function reportLicenseTeamMutationError(value: unknown) {
    const error = errorInfoOf(value);
    if (
      error.code === 'LICENSE_WORKSPACE_CONFLICT'
      || error.code === 'LICENSE_OPERATION_CONFLICT'
    ) {
      await refreshLicenseTeamQuietly(false, true, true);
    }
    licenseTeamError = error;
  }

  async function createLicenseTeamDeviceEnrollment(operationId: string) {
    licenseTeamError = null;
    licenseTeamInvitation = null;
    const secretGeneration = licenseTeamSecretGeneration;
    await mutateLicenseTeam(
      'license-team-device-enrollment-create',
      async () => {
        const enrollment = await api.createLicenseTeamDeviceEnrollment(operationId);
        if (secretGeneration === licenseTeamSecretGeneration && showSettings) {
          licenseTeamDeviceEnrollment = enrollment;
        } else {
          enrollment.enrollmentToken = '';
        }
      },
      (value) => (licenseTeamError = errorInfoOf(value)),
    );
  }

  async function createLicenseTeamMemberDeviceEnrollment(
    memberId: string,
    operationId: string,
  ) {
    licenseTeamError = null;
    licenseTeamInvitation = null;
    const secretGeneration = licenseTeamSecretGeneration;
    await mutateLicenseTeam(
      'license-team-member-device-enrollment-create',
      async () => {
        const enrollment = await api.createLicenseTeamMemberDeviceEnrollment(
          memberId,
          operationId,
        );
        if (secretGeneration === licenseTeamSecretGeneration && showSettings) {
          licenseTeamDeviceEnrollment = enrollment;
        } else {
          enrollment.enrollmentToken = '';
        }
      },
      (value) => (licenseTeamError = errorInfoOf(value)),
    );
  }

  function dismissLicenseTeamInvitation() {
    if (licenseTeamInvitation) licenseTeamInvitation.invitationToken = '';
    licenseTeamInvitation = null;
  }

  function dismissLicenseTeamDeviceEnrollment() {
    if (licenseTeamDeviceEnrollment) licenseTeamDeviceEnrollment.enrollmentToken = '';
    licenseTeamDeviceEnrollment = null;
  }

  async function acceptLicenseTeamDeviceEnrollment(
    enrollmentToken: string,
    operationId: string,
  ) {
    licenseTeamError = null;
    await mutateLicenseTeam(
      'license-team-device-enrollment-accept',
      async () => {
        licenseTeamProfile = await api.acceptLicenseTeamDeviceEnrollment(
          enrollmentToken.trim(),
          operationId,
        );
        licenseTeamInvitation = null;
        licenseTeamDeviceEnrollment = null;
      },
      (value) => (licenseTeamError = errorInfoOf(value)),
    );
    await refreshLicenseAfterWorkspaceBinding();
  }

  async function leaveLicenseTeamWorkspace(request: LeaveWorkspace) {
    licenseTeamError = null;
    licenseTeamInvitation = null;
    licenseTeamDeviceEnrollment = null;
    await mutateLicenseTeam(
      'license-team-leave',
      async () => {
        await api.leaveLicenseTeamWorkspace(request);
        licenseTeamProfile = null;
        licenseTeamMembers = [];
        licenseTeamMembersNextCursor = null;
        licenseTeamMembersHasMore = false;
        await reconcileEntitlementStateAfterFailure('workspace-left');
      },
      reportLicenseTeamMutationError,
    );
  }

  async function transferLicenseTeamOwnership(request: TransferWorkspaceOwnership) {
    licenseTeamError = null;
    licenseTeamInvitation = null;
    licenseTeamDeviceEnrollment = null;
    await mutateLicenseTeam(
      'license-team-ownership-transfer',
      async () => {
        const result = await api.transferLicenseTeamOwnership(request);
        if (licenseTeamProfile) {
          licenseTeamProfile = { ...licenseTeamProfile, member: result.previousOwner };
        }
        licenseTeamMembers = licenseTeamMembers.map((member) =>
          member.id === result.newOwner.id ? result.newOwner : member
        );
        await refreshLicenseTeamQuietly(true, false, true);
      },
      reportLicenseTeamMutationError,
    );
  }

  async function removeLicenseDevice(deviceId: string) {
    const confirmed = await askConfirmation(
      translate('Remove device'),
      translate('This device will lose access until it is activated again.'),
      translate('Remove'),
      true,
    );
    if (!confirmed) return;
    licenseError = null;
    const operationId = licenseDeviceRemovalOperations.get(deviceId) ?? crypto.randomUUID();
    licenseDeviceRemovalOperations.set(deviceId, operationId);
    await mutate(
      'license-remove-device',
      async () => {
        try {
          await api.removeLicenseDevice(deviceId, operationId);
        } catch (value) {
          await reconcileEntitlementStateAfterFailure('device-remove-failed');
          throw value;
        }
        licenseDeviceRemovalOperations.delete(deviceId);
        licenseDevices = licenseDevices.filter((device) => device.deviceId !== deviceId);
        await reconcileEntitlementState('device-removed');
        void refreshLicenseDevicesQuietly(false, true);
      },
      (value) => (licenseError = errorInfoOf(value)),
    );
  }

  async function logoutLicenseSession() {
    const confirmed = await askConfirmation(
      translate('Sign out'),
      translate('This removes the local license session from this device.'),
      translate('Sign out'),
      true,
    );
    if (!confirmed) return;
    licenseError = null;
    licenseTeamInvitation = null;
    licenseTeamDeviceEnrollment = null;
    licenseTeamSecretGeneration += 1;
    await mutate(
      'license-logout',
      async () => {
        let failure: unknown;
        let failed = false;
        try {
          await api.logoutLicenseSession();
        } catch (value) {
          failure = value;
          failed = true;
        }
        if (failed) {
          await reconcileEntitlementStateAfterFailure('logout-failed');
        } else {
          await reconcileEntitlementState('logout');
        }
        licenseDevices = [];
        licenseDevicesNextCursor = null;
        licenseDevicesLastLoadedAt = 0;
        resetLicenseAuthorizationProgress();
        if (failed) throw failure;
      },
      (value) => (licenseError = errorInfoOf(value)),
    );
  }

  async function useAnotherLicense() {
    const confirmed = await askConfirmation(
      translate('Use another license'),
      translate('This retires the current device on its license before replacing the local identity. The switch stops if the server cannot confirm retirement.'),
      translate('Use another license'),
      true,
    );
    if (!confirmed) return;
    licenseError = null;
    licenseTeamInvitation = null;
    licenseTeamDeviceEnrollment = null;
    licenseTeamSecretGeneration += 1;
    licenseIdentityResetOperationId ||= crypto.randomUUID();
    const switched = await mutate(
      'license-switch',
      async () => {
        let snapshot: EntitlementSnapshot;
        try {
          snapshot = await api.resetLicenseDeviceIdentity(licenseIdentityResetOperationId);
        } catch (value) {
          await reconcileEntitlementStateAfterFailure('identity-reset-failed');
          throw value;
        }
        resetLicenseAuthorizationProgress();
        licenseIdentityResetOperationId = '';
        localLicenseDevice = null;
        licenseDevices = [];
        licenseDevicesNextCursor = null;
        licenseDevicesLastLoadedAt = 0;
        await applyEntitlementSnapshot(snapshot, 'identity-reset');
      },
      (value) => (licenseError = errorInfoOf(value)),
    );
    if (switched) await beginLicenseAuthorization();
  }

  async function openAppLogDirectory() {
    appSettingsError = null;
    await runExternalAction(
      'app-log-directory',
      () => api.openAppLogDirectory(),
      (value) => (appSettingsError = errorInfoOf(value)),
    );
  }

  async function openAbout() {
    aboutReturnFocus = activeFocusTarget();
    aboutError = null;
    if (!AboutView) {
      try {
        AboutView = (await import('./AboutDialog.svelte')).default;
      } catch (value) {
        reportGlobalError(value);
        return;
      }
    }
    reopenSettingsAfterAbout = showSettings;
    if (showSettings) {
      settingsInitialFocus = '[data-settings-action="about"]';
      licenseTeamInvitation = null;
      licenseTeamDeviceEnrollment = null;
      licenseTeamSecretGeneration += 1;
      showSettings = false;
    }
    showAbout = true;
    if (applicationInfo) return;
    try {
      applicationInfo = await api.getApplicationInfo();
    } catch (value) {
      aboutError = errorInfoOf(value);
    }
  }

  function closeAbout() {
    showAbout = false;
    if (reopenSettingsAfterAbout) {
      reopenSettingsAfterAbout = false;
      showSettings = true;
      aboutReturnFocus = null;
      return;
    }
    const returnFocus = aboutReturnFocus;
    aboutReturnFocus = null;
    void restoreFocusAfterModal(returnFocus);
  }

  function activeFocusTarget(): HTMLElement | null {
    const activeElement = document.activeElement;
    return activeElement instanceof HTMLElement && activeElement !== document.body
      ? activeElement
      : null;
  }

  async function restoreFocusAfterModal(target: HTMLElement | null) {
    await tick();
    if (target?.isConnected && !target.closest('[inert]')) {
      target.focus({ preventScroll: true });
    }
  }

  async function updateAppSettings(next: AppSettings) {
    const normalized = normalizeAppSettings(next);
    if (JSON.stringify(normalized) === JSON.stringify(appSettings) && !appSettingsError) return;
    appSettingsError = null;
    behaviorSaved = false;
    const saved = await mutate(
      'app-settings',
      () => api.setAppSettings(normalized),
      (value) => (appSettingsError = errorInfoOf(value)),
    );
    if (!saved) return;
    appSettings = normalized;
    if (normalized.language) setLanguage(normalized.language);
    behaviorSaved = true;
    if (behaviorSavedTimer !== undefined) window.clearTimeout(behaviorSavedTimer);
    behaviorSavedTimer = window.setTimeout(() => (behaviorSaved = false), 1_800);
  }

  async function openDocumentation() {
    if (!selectedId) return;
    const id = selectedId;
    await runExternalAction(`documentation:${id}`, () => api.openDocumentation(id));
  }

  function dismissTransientUi(event: MouseEvent) {
    programMenu = null;
    programMenuTrigger = null;
    const target = event.target instanceof Element ? event.target : null;
    if (!target?.closest('.catalog-tools')) catalogToolsOpen = false;
  }

  function sidebarDrawerFocusableElements() {
    const sidebar = document.getElementById('primary-sidebar');
    if (!sidebar) return [];
    return Array.from(sidebar.querySelectorAll<HTMLElement>('button:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])'))
      .filter((element) => {
        const style = getComputedStyle(element);
        return style.display !== 'none' && style.visibility !== 'hidden' && element.getClientRects().length > 0;
      });
  }

  async function openSidebarDrawer() {
    sidebarDrawerOpen = true;
    await tick();
    sidebarDrawerFocusableElements()[0]?.focus({ preventScroll: true });
  }

  function closeSidebarDrawer(restoreFocus = true) {
    if (!sidebarDrawerOpen) return;
    sidebarDrawerOpen = false;
    if (restoreFocus) {
      void tick().then(() => mobileNavToggleElement?.focus({ preventScroll: true }));
    }
  }

  function handleGlobalKeydown(event: KeyboardEvent) {
    if (sidebarDrawerOpen && event.key === 'Tab') {
      const elements = sidebarDrawerFocusableElements();
      if (!elements.length) return;
      const first = elements[0];
      const last = elements[elements.length - 1];
      const active = document.activeElement;
      if (event.shiftKey && (active === first || !active || !document.getElementById('primary-sidebar')?.contains(active))) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && (active === last || !active || !document.getElementById('primary-sidebar')?.contains(active))) {
        event.preventDefault();
        first.focus();
      }
      return;
    }
    if (event.key !== 'Escape') return;
    closeProgramMenu();
    catalogToolsOpen = false;
    closeSidebarDrawer();
  }

  function toggleSidebar() {
    if (mobileViewport) {
      closeSidebarDrawer();
      return;
    }
    sidebarCollapsed = !sidebarCollapsed;
    if (sidebarCollapsed) {
      catalogToolsOpen = false;
      if (bulkMode) toggleBulkMode();
    }
    saveSidebarCollapsed(sidebarCollapsed);
  }
</script>

<svelte:head><title>Camellia Nexus</title></svelte:head>
<svelte:window on:click={dismissTransientUi} on:keydown={handleGlobalKeydown} />

<div
  class:sidebar-collapsed={sidebarCollapsed}
  class:sidebar-open={sidebarDrawerOpen}
  class="shell"
  inert={showCreate || showSettings || showAbout || !!confirmation}
  aria-hidden={showCreate || showSettings || showAbout || confirmation ? 'true' : undefined}
>
  <button class="sidebar-scrim" type="button" aria-label={$t('Close navigation')} on:click={() => closeSidebarDrawer()}></button>
  <ProgramSidebar
    collapsed={sidebarCollapsed}
    drawerMode={mobileViewport}
    drawerOpen={sidebarDrawerOpen}
    {programs}
    {visiblePrograms}
    {selectedId}
    loadingId={loadingProgramId}
    bind:query={programQuery}
    bind:filter={programFilter}
    bind:toolsOpen={catalogToolsOpen}
    {bulkMode}
    {bulkSelectedIds}
    {allVisibleSelected}
    {bulkBusy}
    canReorder={canReorderPrograms}
    canActivate={canActivateProgramsByLicense}
    licenseHint={licenseActionHint}
    createBlockReason={createLicenseBlockReason}
    draggingId={draggingProgramId}
    dropTargetId={dropTargetProgramId}
    dropAfter={dropAfterTarget}
    bind:listElement={programListElement}
    {stateNameKey}
    {isRunning}
    {isIssue}
    onToggleCollapsed={toggleSidebar}
    onHome={() => void goHome()}
    onCreate={() => openCreate()}
    onToggleBulk={toggleBulkMode}
    onSelectAll={selectAllPrograms}
    onBulkLifecycle={(action) => void runBulkLifecycle(action)}
    onActivate={activateProgramFromList}
    onContextMenu={openProgramMenu}
    onStartDrag={startProgramPointerDrag}
    onSettings={() => void openSettingsDialog()}
  />

  <main
    bind:this={mainElement}
    inert={sidebarDrawerOpen}
    aria-hidden={sidebarDrawerOpen ? 'true' : undefined}
    aria-busy={!!loadingProgramId}
  >
    <div class="mobile-nav-bar">
      <button bind:this={mobileNavToggleElement} class="mobile-nav-toggle" type="button" aria-label={$t('Open navigation')} aria-controls="primary-sidebar" aria-expanded={sidebarDrawerOpen} on:click={() => void openSidebarDrawer()}><Icon name="menu" /></button>
    </div>
    {#if notification || licensePrompt}
      <div class="notification-stack" aria-label={$t('Notifications')}>
        {#if notification}
          <div class="notification"><ErrorNotice error={notification} dismissible onDismiss={dismissNotification} /></div>
        {/if}
        {#if licensePrompt}
          <div class="license-prompt" role="status" aria-live="polite">
            <span class="license-prompt-mark" aria-hidden="true">
              <Icon name="lock" size={19} />
            </span>
            <div>
              <strong>{$t(licensePrompt.title)}</strong>
              <small>{$t(licensePrompt.message)}</small>
            </div>
            <button type="button" on:click={openLicenseSettingsFromPrompt}>{$t(licensePrompt.action)}</button>
            <button class="icon-button" type="button" aria-label={$t('Dismiss')} on:click={clearLicensePrompt}><Icon name="close" /></button>
          </div>
        {/if}
      </div>
    {/if}

    {#if loadingProgramId}
      <ProgramDetailLoading
        program={loadingProgram}
        onCancel={cancelProgramSelection}
      />
    {:else if detail}
      <ProgramDetailHeader
        {detail}
        stateLabel={stateLabel(detail.state, $t)}
        running={isRunning(detail.state)}
        issue={isIssue(detail.state)}
        lifecycleBusy={lifecycleBusy(detail.spec.id)}
        startAvailable={canStart(detail.state)}
        stopAvailable={canStop(detail.state)}
        canActivate={canActivateProgramsByLicense}
        licenseHint={licenseActionHint}
        dashboardRunning={dashboardIsRunning}
        dashboardAvailable={dashboardCanOpen}
        nativeDashboard={detail.spec.type.kind === 'singBox' && !!savedDashboardOptionsValue.native}
        clashDashboard={detail.spec.type.kind === 'singBox' && !!savedDashboardOptionsValue.clash}
        xrayDashboard={detail.spec.type.kind === 'xray' && !!savedXrayDashboardValue}
        mihomoDashboard={detail.spec.type.kind === 'mihomo' && !!savedMihomoDashboardValue}
        configurationMode={$t(configModeKey(detail.spec, runtimeArgumentParse.args))}
        onHome={() => void goHome()}
        onDashboard={(kind) => kind === 'xray'
          ? void showXrayDashboard()
          : kind === 'mihomo'
            ? void openMihomoDashboard()
            : void openSingBoxDashboard(kind)}
        onLifecycle={(action) => void runLifecycle(action)}
        onOpenWorkingDirectory={() => void openWorkingDirectory()}
      />

      <ProgramTabs
        active={activeTab}
        label={`${detail.spec.name} ${$t('Details')}`}
        dashboardVisible={xrayDashboardEnabled}
        dashboardDisabled={!dashboardCanOpen}
        dashboardTitle={$t(dashboardCanOpen ? 'Open Xray Dashboard' : 'Start the program to refresh live metrics')}
        configurationVisible={hasEditableConfig(detail.spec)}
        settingsVisible={hasEditableConfig(detail.spec)}
        onSelect={activateTab}
      />

      {#if activeTab === 'overview'}
        <div id="program-panel-overview" role="tabpanel" tabindex="0" aria-labelledby="program-tab-overview" class="panel settings-panel">
          {#if workspaceErrors.details}<ErrorNotice error={workspaceErrors.details} dismissible onDismiss={() => clearWorkspaceError('details')} actionLabel="Retry" onAction={() => retryWorkspaceError('details')} actionBusy={workspaceErrorBusyScope === 'details'} />{/if}
          <ConfigurationSurfaceIssues state={configurationState} surface="details" />
          {#if detailsChanged}<div class="change-notice"><span><i></i><span><strong>{$t('Unsaved program changes')}</strong></span></span><div><button type="button" on:click={() => void revertSettings('details')} disabled={!!busy}>{$t('Revert')}</button><button class="primary" type="button" on:click={() => void saveSettings(true, true, 'details')} disabled={!!busy}>{busy === 'save' ? `${$t('Saving')}…` : $t(saveRequiresRestart ? 'Save and restart' : 'Save')}</button></div></div>{/if}
          <section class="detail-section general-detail-section">
            <div class="section-heading"><div><h2>{$t('General')}</h2></div></div>
            <div class="form-grid">
              <label>{$t('Name')}<input bind:value={detail.spec.name} /></label>
              <label>{$t('Restart policy')}<OptionSelect value={detail.spec.restartPolicy} options={[{ value: 'never', label: $t('Never') }, { value: 'onFailure', label: $t('On failure') }, { value: 'always', label: $t('Always') }]} ariaLabel={$t('Restart policy')} align="center" width="content" on:change={(event) => { if (detail) detail.spec.restartPolicy = event.detail.value as ProgramSpec['restartPolicy']; }} /></label>
              <label class="wide privilege-setting">{$t('Administrator access')}<OptionSelect value={privilegePolicyValue(detail.spec.privilegePolicy)} options={[{ value: 'automatic', label: $t('Automatic detection — ask when starting') }, { value: 'standard', label: $t('Always standard') }, { value: 'elevated', label: $t('Always elevated — ask when starting') }]} ariaLabel={$t('Administrator access')} align="start" width="fill" on:change={(event) => setDetailPrivilegePolicy(String(event.detail.value))} /><small class="field-hint" role="status" aria-live="polite">{$t(privilegeAssessmentText(privilegeAssessment, detail.spec.privilegePolicy, privilegeAssessmentLoadingId === detail.spec.id))}</small></label>
              <label class="check"><input type="checkbox" bind:checked={detail.spec.autoStart} /> {$t('Start with Camellia Nexus')}</label>
              {#if detail.spec.executable.mode === 'external'}<label class="wide">{$t('Executable')}<input bind:value={detail.spec.executable.path} /></label>{/if}
            </div>
          </section>

          <section class="detail-section runtime-detail-section">
            <div class="section-heading"><div><h2>{$t('Runtime')}</h2></div></div>
            <div class="runtime-subsection">
              <div class="subsection-label">{$t('Arguments')}</div>
              <input class="command-line-input" bind:value={runtimeArgumentLine} aria-invalid={!!runtimeArgumentView.error} placeholder="--arg1 value --arg2 --name 'value with spaces'" />
              <ArgumentPreview result={runtimeArgumentView} />
              {#if detail.spec.type.kind !== 'generic'}
                <div class="resolution-note"><strong>{$t('Configuration')}</strong><span>{$t(configModeKey(detail.spec, runtimeArgumentParse.args))}</span></div>
              {/if}
            </div>

            <details class="advanced-section">
              <summary>{$t('Environment variables')}</summary>
              <EnvironmentEditor bind:entries={environmentEntries} />
            </details>

            {#if detail.spec.type.kind === 'singBox'}
              <section class="detail-section runtime-integration-section">
                <div class="section-heading"><div><h2>{$t('Runtime integrations')}</h2></div></div>
                <SingBoxDashboardEditor value={detailDashboardOptionsValue} disabled={!!busy} on:change={(event) => updateDetailDashboard(event.detail)} />
                <div class="managed-integration-projections">
                  <ManagedIntegrationStatus label="sing-box API" projection={managedIntegrationById.get('dashboard.singBoxApi')} disabled={!!busy || detailsChanged} on:useValue={(event) => void useSavedManagedSetting(event.detail)} />
                  <ManagedIntegrationStatus label="Clash API" projection={managedIntegrationById.get('dashboard.singBoxClash')} disabled={!!busy || detailsChanged} on:useValue={(event) => void useSavedManagedSetting(event.detail)} />
                </div>
              </section>
            {:else if detail.spec.type.kind === 'xray'}
              <section class="detail-section runtime-integration-section">
                <div class="section-heading"><div><h2>{$t('Runtime integrations')}</h2></div></div>
                <XrayDashboardEditor value={detailXrayDashboardValue} disabled={!!busy} on:change={(event) => updateDetailXrayDashboard(event.detail)} />
                <ManagedIntegrationStatus label="Xray Dashboard" projection={managedIntegrationById.get('dashboard.xray')} disabled={!!busy || detailsChanged} on:useValue={(event) => void useSavedManagedSetting(event.detail)} />
              </section>
            {:else if detail.spec.type.kind === 'mihomo'}
              <section class="detail-section runtime-integration-section">
                <div class="section-heading"><div><h2>{$t('Runtime integrations')}</h2></div></div>
                <MihomoDashboardEditor value={detailMihomoDashboardValue} disabled={!!busy} on:change={(event) => updateDetailMihomoDashboard(event.detail)} />
                <ManagedIntegrationStatus label="Mihomo Dashboard" projection={managedIntegrationById.get('dashboard.mihomo')} disabled={!!busy || detailsChanged} on:useValue={(event) => void useSavedManagedSetting(event.detail)} />
              </section>
            {/if}

            <div class="metadata">
              <span>{$t('Executable')}</span><code>{detail.spec.executable.mode === 'managed' ? detail.spec.executable.path.replace(/^bin[\\/]/, '') : detail.spec.executable.path}</code>
              <span>{$t('Working folder')}</span><code>{detail.workingDirectory}</code>
              <span>{$t('Version')}</span><code>{coreTargetLabel(detail.spec.executable.metadata?.coreTarget) ?? $t('Not reported')}</code>
            </div>

            {#if detail.spec.executable.mode === 'managed'}
              <div class="package-update"><label>{$t('Updated program folder')}<input bind:value={replacementPackageSource} placeholder={platform === 'Windows' ? 'C:/Downloads/sing-box' : '/home/user/Downloads/sing-box'} /></label><button on:click={() => void replacePackage()} disabled={!!busy || isRuntimeActive(detail.state)}>{$t('Replace')}</button></div>
            {/if}
          </section>
          <div class="panel-actions"><button class="danger" on:click={() => void removeSelected()} disabled={!!busy}>{$t('Delete program')}</button></div>
        </div>
      {:else if activeTab === 'intent'}
        <div id="program-panel-intent" role="tabpanel" tabindex="0" aria-labelledby="program-tab-intent" class="panel configuration-workspace">
          {#if workspaceErrors.intent}<ErrorNotice error={workspaceErrors.intent} dismissible onDismiss={() => clearWorkspaceError('intent')} actionLabel="Retry" onAction={() => retryWorkspaceError('intent')} actionBusy={workspaceErrorBusyScope === 'intent'} />{/if}
          <header class="workspace-header">
            <div><p class="eyebrow">{$t('Intent')}</p><h2>{$t('Common settings')}</h2><p>{$t('Changes here update Final configuration.')}</p></div>
          </header>
          {#if configurationState && configurationState.guidedDescriptors.length > 0}
            <GuidedConfigurationEditor
              state={configurationState}
              disabled={!!busy || !canEditConfigurationByLicense}
              on:change={changeGuidedSetting}
            />
          {:else if configurationStateLoadingId === detail.spec.id}
            <div class="loading configuration-loading">{$t('Loading compatibility profile')}…</div>
          {:else if configurationStateLoadError}
            <div class="workspace-error" role="alert"><span>{$t('Compatibility state could not be loaded.')}</span><button type="button" on:click={() => void loadConfigurationStateSummary(selectedId)}>{$t('Retry')}</button></div>
          {:else}
            <div class="workspace-empty"><strong>{$t('No Guided settings available')}</strong><span>{$t('Use Configuration for the full native editor.')}</span></div>
          {/if}
        </div>
      {:else if activeTab === 'sources'}
        <div id="program-panel-sources" role="tabpanel" tabindex="0" aria-labelledby="program-tab-sources" class="panel configuration-workspace">
          <header class="workspace-header">
            <div><p class="eyebrow">{$t('Sources')}</p><h2>{$t('Managed configuration')}</h2><p>{$t(programDefinition(detail.spec.type.kind).configuration?.language === 'yaml' ? 'Combine ordered native YAML sources into the active configuration' : 'Combine ordered native JSON sources into the active configuration')}</p></div>
            <div class="workspace-actions"><button type="button" on:click={() => void revertSettings('sources')} disabled={!!busy || !managedConfigChanged}>{$t('Revert')}</button><button class="primary" type="button" on:click={() => void saveSettings(false, true, 'sources')} disabled={!!busy || !managedConfigChanged}>{busy === 'save' ? `${$t('Saving')}…` : $t('Save sources')}</button></div>
          </header>
          {#if sourceSaveStatus}<p class="workspace-success" role="status" aria-live="polite">{$t(sourceSaveStatus)}</p>{/if}
          {#if workspaceErrors.sources}<ErrorNotice error={workspaceErrors.sources} dismissible onDismiss={() => clearWorkspaceError('sources')} actionLabel="Retry" onAction={() => retryWorkspaceError('sources')} actionBusy={workspaceErrorBusyScope === 'sources'} />{/if}
          <ConfigurationSurfaceIssues state={configurationState} surface="sources" />
          {#if !detail.spec.managedConfig}
            <section class="workspace-empty"><strong>{$t('Managed configuration is disabled')}</strong><span>{$t('Enable it to combine ordered local, inline or HTTPS sources.')}</span><button type="button" on:click={enableManagedConfiguration} disabled={!!busy}>{$t('Enable')}</button></section>
          {:else}
            <ConfigSourceEditor
              sources={detail.spec.managedConfig.sources}
              {platform}
              disabled={!!busy}
              maxSources={maxConfigSourcesLimit}
              remoteUpdate={detail.spec.managedConfig.remoteUpdate}
              on:change={(event) => updateDetailSources(event.detail)}
              on:remoteUpdate={(event) => updateDetailRemoteUpdate(event.detail)}
              on:preview={(event) => void previewShareImport(event.detail)}
            />
            <div class="source-status-summary" aria-live="polite">
              {#if configurationState}
                {#each configurationState.sourceStatuses as source (source.sourceId)}
                  <div class="source-status-item" class:warning={source.freshness === 'stale'} class:problem={source.freshness === 'invalid' || source.freshness === 'unavailable'}>
                    <span>{source.sourceName}: {$t(source.freshness)}</span>
                    {#if source.messageKey}<small class="source-status-reason">{$t(sourceIssueMessage(source.messageKey))}</small>{/if}
                    {#if source.freshness !== 'fresh'}<button type="button" class="source-status-retry" on:click={() => void refreshManagedConfiguration()} disabled={!!busy || managedConfigChanged}>{$t('Retry source update')}</button>{/if}
                  </div>
                {/each}
              {:else if configurationStateLoadingId === detail.spec.id}{$t('Loading')}…{:else}{$t('Unavailable')}{/if}
            </div>
            <div class="managed-config-actions"><span role="status" aria-live="polite">{#if configUpdateStatus}{#if configUpdateStatus.sourceCount !== undefined}{configUpdateStatus.sourceCount} {/if}{$t(configUpdateStatus.message)}{/if}</span><button type="button" on:click={() => void refreshManagedConfiguration()} disabled={!!busy || managedConfigChanged}>{$t('Update configuration')}</button></div>
          {/if}
        </div>
      {:else if activeTab === 'compatibility'}
        <div id="program-panel-compatibility" role="tabpanel" tabindex="0" aria-labelledby="program-tab-compatibility" class="panel configuration-workspace">
          <header class="workspace-header">
            <div><h2 id="core-compatibility-heading">{$t('Program compatibility')}</h2><p>{$t('Checks whether this program can use your configuration.')}</p></div>
          </header>
          <div class="compatibility-overview">
            <section class="compatibility-card" aria-label={$t('Detected program')}>
              <span>{$t('Detected program')}</span>
              <div><strong>{programDefinition(detail.spec.type.kind).displayName}</strong><strong>{!configurationStateLoadError && configurationState?.coreAdmission?.baseline?.version || $t('Not detected')}</strong></div>
            </section>
            <section class:warning={configurationStateLoadError || coreAdmissionRejected || configurationState?.workspace.editor.editStatus === 'conflict' || configurationState?.workspace.editor.candidateStatus === 'invalid'} class="compatibility-status-card" aria-live="polite">
              <strong>{$t(compatibilityConclusion)}</strong>
              <button class="primary" type="button" on:click={runCompatibilityPrimaryAction} disabled={compatibilityPrimaryDisabled} title={$t(compatibilityPrimaryLabel)}>{$t(compatibilityPrimaryLabel)}</button>
            </section>
          </div>
          <details class="compatibility-advanced">
            <summary>{$t('About compatibility')}</summary>
            <ConfigurationSurfaceIssues state={configurationState} surface="compatibility" />
            <div class="compatibility-facts">
              {#if configurationState?.coreAdmission}
                <div><strong>{$t('Supported versions')}</strong><span>{configurationState.coreAdmission.maintainedFamilies.join(' · ')}</span></div>
              {/if}
              {#if configurationState && !coreAdmissionRejected && !configurationStateLoadError}
                <div><strong>{$t('Configuration check')}</strong><span>{$t(configurationState.desired.validationEvidence?.nativeAccepted ? 'This program accepted the saved configuration.' : 'Changes are checked with this program when you apply them.')}</span></div>
              {/if}
            </div>
          </details>
        </div>
      {:else if activeTab === 'dashboard'}
        <XrayDashboardView
          snapshot={xrayDashboardSnapshot}
          dashboard={savedXrayDashboardValue}
          error={xrayDashboardError}
          runtimeStateLabel={$t(stateNameKey(detail.state))}
          running={dashboardIsRunning}
          canRefresh={xrayDashboardCanRefresh}
          manualRefreshing={xrayDashboardManualRefreshing}
          routingBusyTag={xrayRoutingBusyTag}
          loggerBusy={xrayLoggerBusy}
          trafficSort={xrayTrafficSort}
          trafficSortAscending={xrayTrafficSortAscending}
          pairHeight={xrayPairHeight}
          trafficHeight={xrayTrafficHeight}
          pairMinHeight={xrayPairResizeMinHeight}
          pairMaxHeight={xrayPairResizeMaxHeight}
          trafficMinHeight={xrayTrafficResizeMinHeight}
          trafficMaxHeight={xrayTrafficResizeMaxHeight}
          onRefresh={() => void refreshXrayDashboard(true)}
          onSetBalancerTarget={(balancer, target) => void setXrayBalancerTarget(balancer, target)}
          onRestartLogger={() => void restartXrayLogger()}
          onTrafficSortChange={changeXrayTrafficSort}
          onPairPointerDown={(event) => beginResizeFromHandle(event, 'xray:pair')}
          onPairKeyDown={(event) => handleResizeKeydown(event, 'xray:pair')}
          onTrafficPointerDown={(event) => beginResizeFromHandle(event, 'xray:traffic')}
          onTrafficKeyDown={(event) => handleResizeKeydown(event, 'xray:traffic')}
        />
      {:else if activeTab === 'configuration'}
        <div id="program-panel-configuration" role="tabpanel" tabindex="0" aria-labelledby="program-tab-configuration" class="panel configuration">
          {#if configError}<ErrorNotice error={configError} dismissible onDismiss={() => { configError = null; clearWorkspaceError('configuration'); }} actionLabel="Retry" onAction={() => retryWorkspaceError('configuration')} actionBusy={workspaceErrorBusyScope === 'configuration'} />{/if}
          {#if configDocument}
            {#if configurationState?.workspace}
              <FinalConfigurationEditor
                workspace={configurationState.workspace}
                draft={finalEditorSession}
                selectedPath={selectedEditorPath}
                selectedConflictId={activeFinalMarkerId}
                disabled={!!busy || !canEditConfigurationByLicense}
                draftDirty={configEditorDirty}
                syntaxInvalid={configurationSyntaxInvalid}
                editorHasErrors={configurationEditorHasErrors}
                on:focusPath={(event) => { void focusFinalConfigurationPath(event.detail.path); }}
                on:selectConflict={(event) => { selectedEditorConflictId = event.detail.conflictId; void focusFinalConfigurationPath(event.detail.path); }}
                on:rebaseDraft={() => void rebaseFinalEditorDraft()}
                on:resumeDraft={resumeFinalEditorDraft}
                on:discardDraft={() => void discardFinalEditorDraft()}
                on:navigateSurface={(event) => { activateTab(event.detail.surface === 'details' ? 'overview' : event.detail.surface); }}
              >
                <svelte:fragment slot="actions">
                <div class="config-toolbar">
                  <details class="config-more-actions">
                    <summary>{$t('More')}</summary>
                    <div>
                      <button class="link-button documentation-link" type="button" on:click={() => void openDocumentation()}><span>{$t('Documentation')}</span><Icon name="external" size={16} /></button>
                      {#each actions as action (action.id)}<button type="button" on:click={() => void runProgramAction(action)} disabled={!!busy || !actionAllowed(action)} title={$t(canRunDiagnosticsByLicense ? action.label : licenseActionHint)}>{$t(action.label)}</button>{/each}
                    </div>
                  </details>
                  <div class="config-toolbar-commit">
                    <button type="button" on:click={revertConfiguration} disabled={!!busy || (!configEditorDirty && !finalEditorSession?.draftRevision)} title={$t(busy ? 'Another configuration operation is running.' : configEditorDirty || finalEditorSession?.draftRevision ? 'Revert' : 'There are no unsaved editor changes to revert.')}>{$t('Revert')}</button>
                    <button class="primary config-save" type="button" on:click={() => void applyConfiguration()} disabled={configurationActivationBlocked} title={$t(configurationActivationReason)}>{busy === 'apply' ? `${$t('Applying')}…` : $t(configSaveRequiresRestart ? 'Apply and restart' : 'Apply changes')}</button>
                  </div>
                </div>
                </svelte:fragment>
                <svelte:fragment slot="editor">
                <div style={resizeStyle(detailConfigHeight)} class="config-editor-resize">
                  {#if CodeEditorView}
                    <CodeEditorView
                      bind:value={configContent}
                      {theme}
                      language={configDocument.language}
                      revision={configDocument.baseHash}
                      configurationSchema={configurationSchemaDocument}
                      configurationSchemaLoading={configurationSchemaLoading}
                      configurationSchemaError={configurationSchemaError}
                      jsonSchemaSemantics={programDefinition(detail.spec.type.kind).configuration?.jsonSchemaSemantics}
                      markers={configurationEditorMarkers}
                      activeMarkerId={activeFinalMarkerId}
                      focusSemanticPath={focusSemanticPath}
                      markerActionsDisabled={!!busy || !canEditConfigurationByLicense}
                      conflictResolutionId={recentConflictChoice?.programId === selectedId ? recentConflictChoice.operationId : ''}
                      on:retrySchema={retryConfigurationSchema}
                      on:save={saveConfigurationFromEditor}
                      on:syntaxStatus={(event) => { configSyntaxStatus = event.detail; }}
                      on:diagnosticStatus={(event) => { configDiagnosticStatus = event.detail; }}
                      on:resolveMarker={resolveConfigurationConflict}
                      on:retryMarker={(event) => void retryConfigurationConflict(event.detail.conflictId)}
                      on:historyConflict={(event) => void historyConfigurationConflict(event.detail.kind, event.detail.operationId)}
                      on:pathFocus={(event) => { pathFocusMessage = event.detail.found ? '' : 'Fix the document format to locate this setting.'; }}
                      on:markerPathSelected={(event) => { selectedEditorConflictId = event.detail.conflictId; selectedEditorPath = event.detail.path; void focusFinalConfigurationPath(event.detail.path); }}
                    />
                  {/if}
                  <ResizeSeparator label={$t('Resize panel')} value={detailConfigHeight ?? 720} min={detailPaneResizeMinHeight} max={detailPaneResizeMaxHeight} onPointerDown={(event) => beginResizeFromHandle(event, 'detail:config')} onKeyDown={(event) => handleResizeKeydown(event, 'detail:config')} />
                </div>
                  {#if pathFocusMessage}<p class="config-path-focus-notice" role="status">{$t(pathFocusMessage)}</p>{/if}
                  {#if configOutputMessage}<p class="result" role="status">{$t(configOutputMessage)}</p>{/if}
                </svelte:fragment>
              </FinalConfigurationEditor>
            {/if}
          {:else if !configError}<div style={resizeStyle(detailConfigHeight)} class="loading configuration-loading">{$t('Loading configuration')}…</div>{/if}
        </div>
      {:else}
        <div id="program-panel-logs" role="tabpanel" tabindex="0" aria-labelledby="program-tab-logs" class="panel logs">
          <div class="log-toolbar"><div class="log-view-picker" role="group" aria-label={$t('Logs')}><button class:active={logView === 'both'} type="button" aria-pressed={logView === 'both'} on:click={() => (logView = 'both')}>{$t('Both')}</button><button class:active={logView === 'stdout'} type="button" aria-pressed={logView === 'stdout'} on:click={() => (logView = 'stdout')}>{$t('Output')}</button><button class:active={logView === 'stderr'} type="button" aria-pressed={logView === 'stderr'} on:click={() => (logView = 'stderr')}>{$t('Errors')}</button></div><input bind:value={logFilter} aria-label={$t('Filter logs')} placeholder={$t('Filter visible lines…')} /><span class="live-indicator" role="status" aria-label={$t('Live')}><i></i>{$t('Live')}</span><button class="log-clear" type="button" on:click={() => void clearLogHistory()} disabled={!!busy}><Icon name="trash" size={16} /><span>{$t('Clear logs')}</span></button><button class="log-refresh" type="button" on:click={() => void refreshLogs(true)} disabled={manualLogRefreshing}><Icon name="restart" size={16} /><span>{$t(manualLogRefreshing ? 'Refreshing' : 'Refresh')}</span></button></div>
          <div class="log-grid-frame">
            <div class:single={logView !== 'both'} class="log-grid">
              {#if logView !== 'stderr'}<article class="log-pane stdout"><header><span><i></i><strong>{$t('Standard output')}</strong></span><small>{logLineCount(filteredStdout)} {$t('lines')}{logTruncated.stdout ? ' · 128 KiB' : ''}</small></header><pre use:trackLogScroll={'stdout'} bind:this={stdoutLogElement}>{filteredStdout || $t(logFilter ? 'No matching output.' : 'No standard output.')}</pre></article>{/if}
              {#if logView !== 'stdout'}<article class="log-pane stderr"><header><span><i></i><strong>{$t('Error output')}</strong></span><small>{logLineCount(filteredStderr)} {$t('lines')}{logTruncated.stderr ? ' · 128 KiB' : ''}</small></header><pre use:trackLogScroll={'stderr'} bind:this={stderrLogElement}>{filteredStderr || $t(logFilter ? 'No matching errors.' : 'No error output.')}</pre></article>{/if}
            </div>
          </div>
        </div>
      {/if}
    {:else}
      <HomeDashboard
        programs={orderedPrograms}
        {invalidPrograms}
        {runningCount}
        {issueCount}
        {autoStartCount}
        createBlockReason={createLicenseBlockReason}
        {stateNameKey}
        onCreate={openCreate}
        onSelect={(id) => void selectProgram(id)}
      />
    {/if}
  </main>
</div>

{#if showCreate && CreateProgramView}
  <CreateProgramView
    bind:draft={createDraft}
    {platform}
    argumentView={createArgumentView}
    fieldErrors={createFieldErrors}
    error={createError}
    busy={!!busy}
    creating={busy === 'create'}
    dashboardOptions={createDashboardOptionsValue}
    usesExplicitConfig={createUsesExplicitConfig}
    maxConfigSources={maxConfigSourcesLimit}
    onChangeKind={changeCreateKind}
    onChangeMode={changeCreateMode}
    onRemoteUpdate={updateCreateRemoteUpdate}
    onDashboardChange={updateCreateDashboard}
    onReset={resetDraft}
    onSubmit={() => void createProgram()}
    onClose={closeCreateDialog}
  />
{/if}

{#if showSettings && SettingsView}
  <div class="settings-modal-layer" inert={!!confirmation} aria-hidden={confirmation ? 'true' : undefined}>
    <SettingsView
      bind:appearanceTheme
      bind:colorMode
      bind:uiScale
      bind:activeSection={settingsActiveSection}
      initialFocus={settingsInitialFocus}
      {appAutostart}
      {appSettings}
      {entitlementState}
      {licenseServiceSettings}
      {licenseAuthorizationRequest}
      {localLicenseDevice}
      bind:licenseAuthorizationDisplayName
      {licenseDevices}
      {licenseDevicesNextCursor}
      {licenseBillingSummary}
      {licenseBillingError}
      licenseBillingLoading={licenseBillingRequestActive}
      licenseBillingLastUpdatedAt={licenseBillingLastLoadedAt}
      licenseDataSyncing={licenseDevicesRequestActive || licenseBillingRequestActive || !!licenseTeamRequest}
      licenseLastSyncedAt={Math.max(
        licenseDevicesLastLoadedAt,
        licenseBillingLastLoadedAt,
        licenseTeamLastLoadedAt,
      )}
      {licenseTeamProfile}
      {licenseTeamMembers}
      {licenseTeamMembersHasMore}
      {licenseTeamMembersLoadingMore}
      {licenseTeamInvitation}
      {licenseTeamDeviceEnrollment}
      {licenseTeamSecretGeneration}
      {licenseTeamError}
      {behaviorSaved}
      appVersion={applicationInfo?.version ?? ''}
      focusSection={settingsFocusSection}
      error={appSettingsError}
      licenseError={licenseError}
      busy={!!busy}
      busyAction={busy}
      onToggleAutostart={() => void updateAppAutostart()}
      onUpdateAppSettings={(settings) => void updateAppSettings(settings)}
      onChangeLanguage={(language) => void updateAppSettings({ ...appSettings, language })}
      onBeginLicenseAuthorization={() => void beginLicenseAuthorization()}
      onRefreshLicense={() => void refreshLicenseEntitlement()}
      onReconnectLicense={() => void reconnectLicenseDevice()}
      onLoadLicenseDevices={() => void loadLicenseDevices()}
      onLoadMoreLicenseDevices={() => void loadMoreLicenseDevices()}
      onLoadLicenseBilling={() => void refreshLicenseBillingQuietly(true, true)}
      onShowLicense={showLicenseSettingsData}
      onSubmitLicensePayment={(submission) => void submitLicensePaymentClaim(submission)}
      onLoadLicenseTeam={() => void refreshLicenseTeamQuietly(true, true, true)}
      onLoadMoreLicenseTeamMembers={() => void loadMoreLicenseTeamMembers()}
      onCreateLicenseTeamInvitation={createLicenseTeamInvitation}
      onDismissLicenseTeamInvitation={dismissLicenseTeamInvitation}
      onAcceptLicenseTeamInvitation={acceptLicenseTeamInvitation}
      onUpdateLicenseTeamMember={updateLicenseTeamMember}
      onCreateLicenseTeamDeviceEnrollment={createLicenseTeamDeviceEnrollment}
      onCreateLicenseTeamMemberDeviceEnrollment={createLicenseTeamMemberDeviceEnrollment}
      onDismissLicenseTeamDeviceEnrollment={dismissLicenseTeamDeviceEnrollment}
      onAcceptLicenseTeamDeviceEnrollment={acceptLicenseTeamDeviceEnrollment}
      onLeaveLicenseTeamWorkspace={leaveLicenseTeamWorkspace}
      onTransferLicenseTeamOwnership={transferLicenseTeamOwnership}
      onConfirmTeamWorkspaceAction={askConfirmation}
      onRemoveLicenseDevice={(deviceId) => void removeLicenseDevice(deviceId)}
      onCancelLicenseAuthorization={cancelLicenseAuthorization}
      onLogoutLicense={() => void logoutLicenseSession()}
      onUseAnotherLicense={() => void useAnotherLicense()}
      onDismissLicenseError={() => (licenseError = null)}
      onDismissLicenseBillingError={() => (licenseBillingError = null)}
      onDismissLicenseTeamError={() => (licenseTeamError = null)}
      onOpenDataDirectory={() => void openDataDirectory()}
      onOpenAppLogDirectory={() => void openAppLogDirectory()}
      onOpenAbout={() => void openAbout()}
      onClose={closeSettingsDialog}
    />
  </div>
{/if}

{#if showAbout && AboutView}
  <AboutView info={applicationInfo} error={aboutError} onClose={closeAbout} />
{/if}

{#if programMenu}
  <ProgramContextMenu
    program={programMenu.program}
    x={programMenu.x}
    y={programMenu.y}
    busy={lifecycleBusy(programMenu.program.id) || !!busy}
    stopBusy={programMenu.program.state.status === 'stopping'}
    canActivate={canActivateProgramsByLicense}
    {licenseActionHint}
    canMoveUp={canReorderPrograms && catalog.order.indexOf(programMenu.program.id) > 0}
    canMoveDown={canReorderPrograms && catalog.order.indexOf(programMenu.program.id) < catalog.order.length - 1}
    onOpenFolder={() => void openWorkingDirectory(programMenu!.program.id)}
    onStart={() => void runProgramLifecycle(programMenu!.program.id, 'start')}
    onStop={() => void runProgramLifecycle(programMenu!.program.id, 'stop')}
    onRestart={() => void runProgramLifecycle(programMenu!.program.id, 'restart')}
    onMoveUp={() => moveProgramBy(programMenu!.program.id, -1)}
    onMoveDown={() => moveProgramBy(programMenu!.program.id, 1)}
    onDelete={() => void removeProgram(programMenu!.program.id, programMenu!.program.name)}
    onClose={closeProgramMenu}
  />
{/if}

{#if confirmation}
  <ConfirmDialog
    title={confirmation.title}
    message={confirmation.message}
    confirmLabel={confirmation.confirmLabel}
    danger={confirmation.danger}
    onResolve={resolveConfirmation}
  />
{/if}

{#if shareImportPreview}
  <ShareImportPreviewDialog
    preview={shareImportPreview}
    busy={busy === 'preview-share-import'}
    on:close={() => { shareImportPreview = null; }}
    on:confirm={() => { shareImportPreview = null; }}
  />
{/if}

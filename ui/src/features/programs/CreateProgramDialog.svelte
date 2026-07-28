<script lang="ts">
  import ArgumentPreview from '../../ArgumentPreview.svelte';
  import ConfigSourceEditor from '../../ConfigSourceEditor.svelte';
  import EnvironmentEditor from '../../EnvironmentEditor.svelte';
  import ErrorNotice from '../../ErrorNotice.svelte';
  import type { ArgumentParseResult } from '../../arguments';
  import type { ErrorInfo } from '../../api';
  import type { CreateDraft } from '../../drafts';
  import { t } from '../../i18n';
  import { focusTrap } from '../../lib/actions/focusTrap';
  import Icon from '../../lib/components/Icon.svelte';
  import OptionSelect from '../../lib/components/OptionSelect.svelte';
  import MihomoDashboardEditor from '../../programs/mihomo/MihomoDashboardEditor.svelte';
  import type { SingBoxDashboardChange, SingBoxDashboardOptions } from '../../programs/sing-box';
  import SingBoxDashboardEditor from '../../programs/sing-box/SingBoxDashboardEditor.svelte';
  import { programDefinition, programDefinitions } from '../../programs/registry';
  import XrayDashboardEditor from '../../programs/xray/XrayDashboardEditor.svelte';
  import type { ManagedConfig, ProgramKind, RestartPolicy } from '../../types';

  interface Props {
    draft: CreateDraft;
    platform: string;
    argumentView: ArgumentParseResult;
    fieldErrors: Record<string, string>;
    error: ErrorInfo | null;
    busy: boolean;
    creating: boolean;
    dashboardOptions: SingBoxDashboardOptions;
    usesExplicitConfig: boolean;
    maxConfigSources: number;
    onChangeKind: (kind: ProgramKind) => void;
    onChangeMode: (mode: 'managed' | 'external') => void;
    onRemoteUpdate: (remoteUpdate: ManagedConfig['remoteUpdate']) => void;
    onDashboardChange: (change: SingBoxDashboardChange) => void;
    onReset: () => void;
    onSubmit: () => void;
    onClose: () => void;
  }

  let {
    draft = $bindable(),
    platform,
    argumentView,
    fieldErrors,
    error,
    busy,
    creating,
    dashboardOptions,
    usesExplicitConfig,
    maxConfigSources,
    onChangeKind,
    onChangeMode,
    onRemoteUpdate,
    onDashboardChange,
    onReset,
    onSubmit,
    onClose,
  }: Props = $props();
  let modalElement: HTMLDivElement;
  let focusedErrorSignature = '';
  let advancedOpen = $state(false);

  $effect(() => {
    const signature = Object.entries(fieldErrors).map(([key, value]) => `${key}:${value}`).join('|');
    if (!signature || signature === focusedErrorSignature) return;
    focusedErrorSignature = signature;
    queueMicrotask(() => {
      const firstKey = Object.keys(fieldErrors)[0];
      const target = modalElement?.querySelector<HTMLElement>(`[data-create-field="${firstKey}"]`)
        ?? modalElement?.querySelector<HTMLElement>('.create-validation-summary');
      target?.focus({ preventScroll: true });
      target?.scrollIntoView({ block: 'nearest' });
    });
  });

  $effect(() => {
    if (fieldErrors.environment) advancedOpen = true;
  });

  function close() {
    if (!busy) onClose();
  }

  function updateDraft<Key extends keyof CreateDraft>(key: Key, value: CreateDraft[Key]) {
    draft = { ...draft, [key]: value };
  }

  function usesExclusiveStoredConfiguration(kind: ProgramKind) {
    return programDefinition(kind).configuration?.storedConfigurationMode === 'exclusive';
  }
</script>

<div class="modal-backdrop" role="presentation" onclick={(event) => event.currentTarget === event.target && close()}>
  <div
    bind:this={modalElement}
    use:focusTrap={{ onEscape: busy ? undefined : onClose, initialFocus: 'input' }}
    class="modal"
    role="dialog"
    aria-modal="true"
    aria-labelledby="create-title"
  >
    <form class="modal-form" onsubmit={(event) => { event.preventDefault(); onSubmit(); }}>
      <div class="modal-title"><div><p class="eyebrow">{$t('New program')}</p><h2 id="create-title">{$t('Add to Camellia Nexus')}</h2></div><button class="icon-button" type="button" aria-label={$t('Close')} onclick={close} disabled={busy}><Icon name="close" /></button></div>

      <div class="modal-body">
      <div class="profile-picker" role="group" aria-label={$t('Program type')}>
        {#each programDefinitions as definition (definition.kind)}
          <button class:active={draft.kind === definition.kind} type="button" aria-pressed={draft.kind === definition.kind} onclick={() => onChangeKind(definition.kind)}>{$t(definition.displayName)}</button>
        {/each}
      </div>

      {#if error}<ErrorNotice {error} />{/if}
      {#if Object.keys(fieldErrors).length}
        <div class="create-validation-summary" role="alert" tabindex="-1"><Icon name="alert" size={17} /><span>{$t('Review the highlighted values and try again.')}</span></div>
      {/if}

      <section class="create-section identity-create-section">
        <div class="form-grid create-grid">
          <label>{$t('Program ID')}<input data-create-field="id" value={draft.id} oninput={(event) => updateDraft('id', event.currentTarget.value)} aria-invalid={!!fieldErrors.id} aria-describedby={fieldErrors.id ? 'create-error-id' : undefined} placeholder="edge-proxy" />{#if fieldErrors.id}<small id="create-error-id" class="field-error">{$t(fieldErrors.id)}</small>{/if}</label>
          <label>{$t('Display name')}<input data-create-field="name" value={draft.name} oninput={(event) => updateDraft('name', event.currentTarget.value)} aria-invalid={!!fieldErrors.name} aria-describedby={fieldErrors.name ? 'create-error-name' : undefined} placeholder="Edge Proxy" />{#if fieldErrors.name}<small id="create-error-name" class="field-error">{$t(fieldErrors.name)}</small>{/if}</label>
        </div>
      </section>

      <section class="create-section program-create-section">
        <div class="section-heading"><div><h3>{$t('Program')}</h3></div><div class="segmented" role="group" aria-label={$t('Program source')}><button class:active={draft.mode === 'managed'} type="button" aria-pressed={draft.mode === 'managed'} onclick={() => onChangeMode('managed')}>{$t('Managed copy')}</button><button class:active={draft.mode === 'external'} type="button" aria-pressed={draft.mode === 'external'} onclick={() => onChangeMode('external')}>{$t('Use in place')}</button></div></div>
        <div class="form-grid create-grid">
          {#if draft.mode === 'managed'}<label>{$t('Program folder')}<input data-create-field="packageSource" value={draft.packageSource} oninput={(event) => updateDraft('packageSource', event.currentTarget.value)} aria-invalid={!!fieldErrors.packageSource} aria-describedby={fieldErrors.packageSource ? 'create-error-package-source' : undefined} placeholder={platform === 'Windows' ? 'C:/Tools/my-program' : '/home/user/tools/my-program'} />{#if fieldErrors.packageSource}<small id="create-error-package-source" class="field-error">{$t(fieldErrors.packageSource)}</small>{/if}</label>{/if}
          <label>{$t('Executable')}<input data-create-field="executable" value={draft.executable} oninput={(event) => updateDraft('executable', event.currentTarget.value)} aria-invalid={!!fieldErrors.executable} aria-describedby={fieldErrors.executable ? 'create-error-executable' : undefined} placeholder={draft.mode === 'managed' ? 'program.exe' : platform === 'Windows' ? 'C:/Tools/program.exe' : '/usr/local/bin/program'} />{#if fieldErrors.executable}<small id="create-error-executable" class="field-error">{$t(fieldErrors.executable)}</small>{/if}</label>
        </div>
      </section>

      <section class="create-section arguments-create-section">
        <div class="section-heading"><div><h3>{$t('Arguments')}</h3></div></div>
        <input data-create-field="args" class="command-line-input" value={draft.argumentLine} oninput={(event) => updateDraft('argumentLine', event.currentTarget.value)} aria-invalid={!!argumentView.error || !!fieldErrors.args} aria-describedby={fieldErrors.args ? 'create-error-args' : undefined} placeholder="--arg1 value --arg2 --name 'value with spaces'" />
        <ArgumentPreview result={argumentView} />
        {#if fieldErrors.args}<small id="create-error-args" class="field-error block-error">{$t(fieldErrors.args)}</small>{/if}
      </section>

      {#if draft.kind !== 'generic'}
        <section class="create-section configuration-create-section">
          <div class="section-heading"><div><h3>{$t('Configuration')}</h3></div></div>
          <div class="configuration-mode-picker" role="group" aria-label={$t('Configuration mode')}>
            <button class:active={!draft.managedConfiguration} type="button" aria-pressed={!draft.managedConfiguration} onclick={() => updateDraft('managedConfiguration', false)}><span class="mode-icon"><svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 7h14M5 12h9M5 17h11"></path><circle cx="17" cy="12" r="2"></circle></svg></span><span class="mode-copy"><strong>{$t('Manual configuration')}</strong><small>{$t(usesExclusiveStoredConfiguration(draft.kind) ? 'Use arguments or an optional stored configuration' : 'Use arguments with an optional final override')}</small></span></button>
            <button class:active={draft.managedConfiguration} type="button" aria-pressed={draft.managedConfiguration} onclick={() => updateDraft('managedConfiguration', true)}><span class="mode-icon"><svg viewBox="0 0 24 24" aria-hidden="true"><path d="m5 8 7-4 7 4-7 4Z"></path><path d="m5 12 7 4 7-4M5 16l7 4 7-4"></path></svg></span><span class="mode-copy"><strong>{$t('Managed configuration')}</strong><small>{$t(programDefinition(draft.kind).configuration?.language === 'yaml' ? 'Merge local and remote YAML configuration sources' : 'Merge local and remote configuration sources')}</small></span></button>
          </div>
          {#if draft.managedConfiguration}
            <div class="resolution-note managed-mode-notice"><strong>{$t('Managed configuration')}</strong><span>{$t('Configuration path arguments are disabled')}</span></div>
            <div class="configuration-mode-content managed-mode-content"><ConfigSourceEditor sources={draft.configSources} {platform} disabled={busy} maxSources={maxConfigSources} remoteUpdate={{ enabled: draft.remoteAutoUpdate, intervalMinutes: draft.remoteUpdateIntervalMinutes }} on:change={(event) => updateDraft('configSources', event.detail)} on:remoteUpdate={(event) => onRemoteUpdate(event.detail)} /></div>
            {#if fieldErrors.configSources}<small data-create-field="configSources" tabindex="-1" class="field-error block-error">{$t(fieldErrors.configSources)}</small>{/if}
            {#if draft.kind === 'singBox'}
              <div class="create-dashboard">
                <SingBoxDashboardEditor value={dashboardOptions} disabled={busy} on:change={(event) => onDashboardChange(event.detail)} />
                {#if fieldErrors.dashboard}<small data-create-field="dashboard" tabindex="-1" class="field-error block-error">{$t(fieldErrors.dashboard)}</small>{/if}
              </div>
            {:else if draft.kind === 'xray'}
              <div class="create-dashboard">
                <XrayDashboardEditor
                  value={draft.xrayDashboardEnabled ? { apiPort: draft.xrayApiPort, metricsPort: draft.xrayMetricsPort } : undefined}
                  disabled={busy}
                  on:change={(event) => {
                    draft = {
                      ...draft,
                      xrayDashboardEnabled: !!event.detail,
                      xrayApiPort: event.detail?.apiPort ?? draft.xrayApiPort,
                      xrayMetricsPort: event.detail?.metricsPort ?? draft.xrayMetricsPort,
                    };
                  }}
                />
                {#if fieldErrors.dashboard}<small data-create-field="dashboard" tabindex="-1" class="field-error block-error">{$t(fieldErrors.dashboard)}</small>{/if}
              </div>
            {:else if draft.kind === 'mihomo'}
              <div class="create-dashboard">
                <MihomoDashboardEditor
                  value={draft.mihomoDashboardEnabled ? { listenPort: draft.mihomoDashboardPort, downloadUrl: draft.mihomoDashboardDownloadUrl || undefined } : undefined}
                  disabled={busy}
                  on:change={(event) => {
                    draft = {
                      ...draft,
                      mihomoDashboardEnabled: !!event.detail,
                      mihomoDashboardPort: event.detail?.listenPort ?? draft.mihomoDashboardPort,
                      mihomoDashboardDownloadUrl: event.detail?.downloadUrl ?? '',
                    };
                  }}
                />
                {#if fieldErrors.dashboard}<small data-create-field="dashboard" tabindex="-1" class="field-error block-error">{$t(fieldErrors.dashboard)}</small>{/if}
              </div>
            {/if}
          {:else}
            <div class="resolution-note manual-mode-notice">
              <strong>{$t(usesExplicitConfig ? 'Using configuration from Arguments' : draft.initialConfig.trim() ? usesExclusiveStoredConfiguration(draft.kind) ? 'Using the stored configuration' : 'Using the initial override' : 'Program selects its configuration')}</strong>
              <span>{$t(usesExclusiveStoredConfiguration(draft.kind) && draft.initialConfig.trim() ? 'Configuration path arguments are unavailable while stored configuration is present' : usesExplicitConfig && draft.initialConfig.trim() ? 'The initial override is merged last' : 'Configuration paths remain available in Arguments')}</span>
            </div>
            <div class="configuration-mode-content manual-mode-content">
              <div class="section-heading compact"><div><h3>{$t(usesExclusiveStoredConfiguration(draft.kind) ? 'Stored configuration' : 'Initial override')}<em>{$t('(Optional)')}</em></h3><p>{$t(usesExclusiveStoredConfiguration(draft.kind) ? 'Used as the program configuration file' : 'Applied after command-line configuration')}</p></div></div>
              <textarea data-create-field="initialConfig" class:empty={!draft.initialConfig.trim()} class="config-input" value={draft.initialConfig} oninput={(event) => updateDraft('initialConfig', event.currentTarget.value)} rows="4" placeholder={programDefinition(draft.kind).configuration?.initialConfigPlaceholder ?? ''} aria-invalid={!!fieldErrors.initialConfig} aria-describedby={fieldErrors.initialConfig ? 'create-error-initial-config' : undefined}></textarea>
              {#if fieldErrors.initialConfig}<small id="create-error-initial-config" class="field-error block-error">{$t(fieldErrors.initialConfig)}</small>{/if}
            </div>
          {/if}
        </section>
      {/if}

      <details class="advanced-section" bind:open={advancedOpen}>
        <summary>{$t('Advanced options')}</summary>
        <div class="advanced-content">
          <div class="create-advanced-options">
            <label class="create-advanced-card">
              <span class="create-advanced-copy"><strong>{$t('Restart policy')}</strong><small>{$t('Choose how the program restarts after it exits')}</small></span>
              <OptionSelect ariaLabel={$t('Restart policy')} value={draft.restartPolicy} options={[{ value: 'never', label: $t('Never') }, { value: 'onFailure', label: $t('On failure') }, { value: 'always', label: $t('Always') }]} align="center" size="md" width="content" on:change={(event) => updateDraft('restartPolicy', event.detail.value as RestartPolicy)} />
            </label>
            <label class="create-advanced-card create-autostart">
              <span class="create-advanced-copy"><strong>{$t('Start with Camellia Nexus')}</strong><small>{$t('Launch this program when Camellia Nexus opens')}</small></span>
              <span class="compact-switch"><input type="checkbox" checked={draft.autoStart} onchange={(event) => updateDraft('autoStart', event.currentTarget.checked)} /><span></span></span>
            </label>
            <label class="create-advanced-card create-privilege">
              <span class="create-advanced-copy"><strong>{$t('Administrator access')}</strong><small>{$t('Automatically detect when the program needs elevated operating-system access')}</small></span>
              <OptionSelect ariaLabel={$t('Administrator access')} value={draft.privilegeMode} options={[{ value: 'automatic', label: $t('Automatic detection') }, { value: 'standard', label: $t('Always standard') }, { value: 'elevated', label: $t('Always elevated') }]} align="start" size="md" width="fill" on:change={(event) => updateDraft('privilegeMode', event.detail.value as CreateDraft['privilegeMode'])} />
            </label>
          </div>
          <div class="section-heading compact"><div><h3>{$t('Environment variables')}</h3></div></div><EnvironmentEditor entries={draft.environment} on:change={(event) => updateDraft('environment', event.detail)} />{#if fieldErrors.environment}<small data-create-field="environment" tabindex="-1" class="field-error block-error">{$t(fieldErrors.environment)}</small>{/if}
        </div>
      </details>
      </div>

      <div class="modal-actions"><button type="button" class="text-button" onclick={onReset} disabled={busy}>{$t('Reset draft')}</button><span class="spacer"></span><button type="button" onclick={close} disabled={busy}>{$t('Cancel')}</button><button class="primary" type="submit" disabled={busy}>{creating ? `${$t('Creating')}…` : $t('Create program')}</button></div>
    </form>
  </div>
</div>

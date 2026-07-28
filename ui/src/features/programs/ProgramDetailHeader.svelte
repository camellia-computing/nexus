<script lang="ts">
  import ProgramGlyph from '../../ProgramGlyph.svelte';
  import { t } from '../../i18n';
  import Icon from '../../lib/components/Icon.svelte';
  import type { ProgramDetail } from '../../types';

  interface Props {
    detail: ProgramDetail;
    stateLabel: string;
    running: boolean;
    issue: boolean;
    lifecycleBusy: boolean;
    startAvailable: boolean;
    stopAvailable: boolean;
    canActivate: boolean;
    licenseHint: string;
    dashboardRunning: boolean;
    dashboardAvailable: boolean;
    nativeDashboard: boolean;
    clashDashboard: boolean;
    xrayDashboard: boolean;
    mihomoDashboard: boolean;
    configurationMode: string;
    onHome: () => void;
    onDashboard: (kind: 'native' | 'clash' | 'xray' | 'mihomo') => void;
    onLifecycle: (action: 'start' | 'stop' | 'restart') => void;
    onOpenWorkingDirectory: () => void;
  }

  let {
    detail,
    stateLabel,
    running,
    issue,
    lifecycleBusy,
    startAvailable,
    stopAvailable,
    canActivate,
    licenseHint,
    dashboardRunning,
    dashboardAvailable,
    nativeDashboard,
    clashDashboard,
    xrayDashboard,
    mihomoDashboard,
    configurationMode,
    onHome,
    onDashboard,
    onLifecycle,
    onOpenWorkingDirectory,
  }: Props = $props();
</script>

<header class="page-header program-hero">
  <div class="page-title program-hero-identity">
    <button class="back-button" type="button" onclick={onHome}>
      <Icon name="back" size={16} />
      <span>{$t('Dashboard')}</span>
    </button>

    <div class="detail-heading">
      <span class={`detail-program-icon ${detail.spec.type.kind}`} aria-hidden="true">
        <ProgramGlyph kind={detail.spec.type.kind} />
      </span>
      <div class="detail-heading-copy">
        <p class="eyebrow">{detail.spec.type.kind === 'singBox' ? 'sing-box' : detail.spec.type.kind}</p>
        <h1>{detail.spec.name}</h1>
        <p class="state-line" class:running class:error-state={issue}>
          <span class:running class:error-state={issue} class="dot" aria-hidden="true"></span>
          <span>{stateLabel}</span>
        </p>
      </div>
    </div>
  </div>

  <div class="page-actions">
    {#if nativeDashboard || clashDashboard || xrayDashboard || mihomoDashboard}
      <div class="dashboard-launcher" aria-label={$t('Dashboard access')}>
        {#if nativeDashboard}
          <button class="dashboard-launch native" type="button" title={$t(dashboardRunning ? 'Open sing-box UI' : 'Start the program to open this interface')} onclick={() => onDashboard('native')} disabled={!dashboardAvailable}>
            <Icon name="dashboard" size={17} />
            <span>{$t('sing-box UI')}</span>
            <Icon name="external" size={14} />
          </button>
        {/if}
        {#if clashDashboard}
          <button class="dashboard-launch clash" type="button" title={$t(dashboardRunning ? 'Open Clash UI' : 'Start the program to open this interface')} onclick={() => onDashboard('clash')} disabled={!dashboardAvailable}>
            <Icon name="grid" size={17} />
            <span>{$t('Clash UI')}</span>
            <Icon name="external" size={14} />
          </button>
        {/if}
        {#if xrayDashboard}
          <button class="dashboard-launch xray" type="button" title={$t(dashboardRunning ? 'Open Xray Dashboard' : 'Start the program to refresh live metrics')} onclick={() => onDashboard('xray')} disabled={!dashboardAvailable}>
            <Icon name="activity" size={17} />
            <span>{$t('Xray Dashboard')}</span>
            <Icon name="external" size={14} />
          </button>
        {/if}
        {#if mihomoDashboard}
          <button class="dashboard-launch mihomo" type="button" title={$t(dashboardRunning ? 'Open Mihomo Dashboard' : 'Start the program to open this interface')} onclick={() => onDashboard('mihomo')} disabled={!dashboardAvailable}>
            <Icon name="grid" size={17} />
            <span>{$t('Mihomo Dashboard')}</span>
            <Icon name="external" size={14} />
          </button>
        {/if}
      </div>
    {/if}

    <div class="toolbar program-hero-toolbar">
      <div class="lifecycle-actions">
        {#if startAvailable}
          <button class="primary" type="button" onclick={() => onLifecycle('start')} disabled={lifecycleBusy || !canActivate} title={$t(canActivate ? 'Start' : licenseHint)}>
            <Icon name="start" size={16} />
            <span>{$t('Start')}</span>
          </button>
        {/if}
        {#if detail.state.status === 'backoff'}
          <button class="primary" type="button" onclick={() => onLifecycle('restart')} disabled={lifecycleBusy || !canActivate} title={$t(canActivate ? 'Retry now' : licenseHint)}>
            <Icon name="restart" size={16} />
            <span>{$t('Retry now')}</span>
          </button>
        {/if}
        {#if stopAvailable}
          <button class="stop-button" type="button" onclick={() => onLifecycle('stop')} disabled={detail.state.status === 'stopping'}>
            <Icon name="stop" size={16} />
            <span>{$t(detail.state.status === 'backoff' ? 'Stop retries' : 'Stop')}</span>
          </button>
        {/if}
        {#if detail.state.status === 'running'}
          <button type="button" onclick={() => onLifecycle('restart')} disabled={lifecycleBusy || !canActivate} title={$t(canActivate ? 'Restart' : licenseHint)}>
            <Icon name="restart" size={16} />
            <span>{$t('Restart')}</span>
          </button>
        {/if}
      </div>
      <button class="working-folder-action" type="button" onclick={onOpenWorkingDirectory}>
        <Icon name="folder" size={16} />
        <span>{$t('Open working folder')}</span>
      </button>
    </div>
  </div>
</header>

<div class="summary-strip">
  <div class="summary-item">
    <Icon name="config" size={18} />
    <div><span>{$t('Configuration')}</span><strong>{configurationMode}</strong></div>
  </div>
  <div class="summary-item">
    <Icon name="clock" size={18} />
    <div><span>{$t('Start policy')}</span><strong>{$t(detail.spec.autoStart ? 'Automatic' : 'Manual')}</strong></div>
  </div>
  <div class="summary-item">
    <Icon name="folder" size={18} />
    <div><span>{$t('Program source')}</span><strong>{$t(detail.spec.executable.mode === 'managed' ? 'Managed copy' : 'Use in place')}</strong></div>
  </div>
</div>

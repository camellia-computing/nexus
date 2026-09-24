<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { t } from './i18n';
  import type { ManagedIntegrationProjection } from './types';

  export let projection: ManagedIntegrationProjection | undefined;
  export let label: string;
  export let disabled = false;
  const dispatch = createEventDispatcher<{ useValue: string }>();
  const settingLabels: Record<string, string> = {
    'dashboard.singBoxApi.listenPort': 'API port',
    'dashboard.singBoxApi.updateInterval': 'Update interval',
    'dashboard.singBoxClash.listenPort': 'API port',
    'dashboard.singBoxClash.downloadUrl': 'Dashboard download URL',
    'dashboard.xray.apiPort': 'API port',
    'dashboard.xray.metricsPort': 'Metrics port',
    'dashboard.mihomo.listenPort': 'API port',
    'dashboard.mihomo.downloadUrl': 'Dashboard download URL',
  };
  function display(value: import('./types').SemanticValue): string {
    return value.state === 'missing' ? $t('Not set')
      : typeof value.value === 'string' ? value.value : JSON.stringify(value.value);
  }

  const statusLabels: Record<ManagedIntegrationProjection['status'], string> = {
    inactive: 'Inactive',
    explicit: 'Managed by Details',
    overridden: 'Edited in Final configuration',
    finalOnly: 'Enabled in Final configuration',
    needsAttention: 'Needs attention',
    latestSettings: 'Using latest settings',
  };
</script>

{#if projection}
  <div class:problem={projection.status === 'overridden' || projection.status === 'needsAttention'} class:final-only={projection.status === 'finalOnly'} class="managed-integration-status" role="status">
    <div>
      <strong>{$t(label)}</strong>
      <span>{$t(statusLabels[projection.status])}</span>
    </div>
    {#if projection.status === 'finalOnly'}
      <small>{$t('This integration is enabled in Final configuration.')}</small>
    {:else if projection.status === 'overridden'}
      <small>{$t('Some settings are edited in Final configuration.')}</small>
    {:else if projection.status === 'needsAttention'}
      <small>{$t('Review this integration in Final configuration.')}</small>
    {/if}
    {#each projection.settings.filter((setting) => setting.canUseSavedValue) as setting (setting.settingId)}
      <div class="setting-choice">
        <span>{$t(settingLabels[setting.settingId] ?? 'Setting')}: {$t('Latest setting')} <code>{display(setting.effectiveValue)}</code></span>
        <button type="button" {disabled} on:click={() => dispatch('useValue', setting.settingId)} aria-label={`${$t('Use this value')}: ${$t(settingLabels[setting.settingId] ?? 'Setting')} ${display(setting.savedValue)}`}>
          {$t('Use this value')}: {display(setting.savedValue)}
        </button>
      </div>
    {/each}
    {#if projection.finalPaths.length > 0}
      <details class="managed-paths">
        <summary>{$t('Edited paths')}</summary>
        <div>{#each projection.finalPaths as path (path)}<code>{path}</code>{/each}</div>
      </details>
    {/if}
  </div>
{/if}

<style>
  .managed-integration-status { display: grid; gap: 6px; padding: 9px 11px; border: 1px solid var(--border-color, rgba(127,127,127,.24)); border-radius: 10px; background: rgba(80,130,210,.06); }
  .managed-integration-status > div:first-child { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 8px; }
  .managed-integration-status span { width: fit-content; padding: 2px 8px; border-radius: 999px; font-size: .76rem; background: rgba(80,130,210,.14); }
  .managed-integration-status.problem { border-color: rgba(210,75,75,.42); background: rgba(210,75,75,.07); }
  .managed-integration-status.final-only { border-color: rgba(210,155,45,.42); background: rgba(210,155,45,.07); }
  small { line-height: 1.4; opacity: .78; }
  .managed-paths { min-width: 0; font-size: .75rem; }
  .managed-paths summary { cursor: pointer; overflow-wrap: anywhere; }
  .managed-paths > div { display: flex; min-width: 0; flex-wrap: wrap; justify-content: flex-start !important; gap: 5px !important; margin-top: 5px; }
  code { max-width: 100%; overflow-wrap: anywhere; padding: 2px 6px; border-radius: 5px; font-size: .75rem; background: rgba(127,127,127,.1); }
  .setting-choice { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; min-width: 0; }
  .setting-choice span { flex: 1 1 180px; width: auto; background: none; padding: 0; overflow-wrap: anywhere; }
  .setting-choice button { min-width: 0; max-width: 100%; white-space: normal; overflow-wrap: anywhere; }
</style>

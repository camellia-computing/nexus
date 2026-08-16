<script lang="ts">
  import { t } from './i18n';
  import type { ManagedIntegrationProjection } from './types';

  export let projection: ManagedIntegrationProjection | undefined;
  export let label: string;

  const statusLabels: Record<ManagedIntegrationProjection['status'], string> = {
    inactive: 'Inactive',
    explicit: 'Managed by Details',
    overridden: 'Overridden by Raw',
    rawOnly: 'Provided by Raw',
    needsAttention: 'Needs attention',
  };
</script>

{#if projection}
  <div class:problem={projection.status === 'overridden' || projection.status === 'needsAttention'} class:raw-only={projection.status === 'rawOnly'} class="managed-integration-status" role="status">
    <div>
      <strong>{$t(label)}</strong>
      <span>{$t(statusLabels[projection.status])}</span>
    </div>
    {#if projection.status === 'rawOnly'}
      <small>{$t('This feature is currently provided by Raw advanced configuration. Details is not claiming ownership.')}</small>
    {:else if projection.status === 'overridden'}
      <small>{$t('Raw configuration overlaps fields owned by this integration. Confirm takeover before Details can replace them.')}</small>
    {:else if projection.status === 'needsAttention'}
      <small>{$t('The current candidate could not generate this integration safely. Applied and Last Known Good were retained.')}</small>
    {/if}
    {#if projection.rawPaths.length > 0}
      <div class="managed-paths" aria-label={$t('Raw semantic paths')}>
        {#each projection.rawPaths as path (path)}<code>{path}</code>{/each}
      </div>
    {/if}
  </div>
{/if}

<style>
  .managed-integration-status { display: grid; gap: 6px; padding: 9px 11px; border: 1px solid var(--border-color, rgba(127,127,127,.24)); border-radius: 10px; background: rgba(80,130,210,.06); }
  .managed-integration-status > div:first-child { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 8px; }
  .managed-integration-status span { width: fit-content; padding: 2px 8px; border-radius: 999px; font-size: .76rem; background: rgba(80,130,210,.14); }
  .managed-integration-status.problem { border-color: rgba(210,75,75,.42); background: rgba(210,75,75,.07); }
  .managed-integration-status.raw-only { border-color: rgba(210,155,45,.42); background: rgba(210,155,45,.07); }
  small { line-height: 1.4; opacity: .78; }
  .managed-paths { display: flex; flex-wrap: wrap; justify-content: flex-start !important; gap: 5px !important; }
  code { max-width: 100%; overflow-wrap: anywhere; padding: 2px 6px; border-radius: 5px; font-size: .75rem; background: rgba(127,127,127,.1); }
</style>

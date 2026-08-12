<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { t } from './i18n';
  import type {
    ConfigurationStateView,
    GuidedProjection,
    GuidedSettingDescriptor,
  } from './types';

  export let state: ConfigurationStateView;
  export let disabled = false;

  const dispatch = createEventDispatcher<{
    change: {
      settingId: string;
      value?: unknown;
      replaceRawOverride: boolean;
    };
  }>();

  $: projectionById = new Map(
    state.guidedProjection.map((projection) => [projection.settingId, projection]),
  );
  $: categories = [...new Set(state.guidedDescriptors.map((descriptor) => descriptor.category))];

  function projectionFor(descriptor: GuidedSettingDescriptor): GuidedProjection {
    return projectionById.get(descriptor.id) ?? {
      settingId: descriptor.id,
      status: 'inherited',
    };
  }

  function effectiveValue(descriptor: GuidedSettingDescriptor): unknown {
    return projectionFor(descriptor).value;
  }

  function dependencyEnabled(descriptor: GuidedSettingDescriptor): boolean {
    if (!descriptor.enabledWhen) return true;
    return projectionById.get(descriptor.enabledWhen)?.value === true;
  }

  function change(descriptor: GuidedSettingDescriptor, value: unknown) {
    const projection = projectionFor(descriptor);
    dispatch('change', {
      settingId: descriptor.id,
      value,
      replaceRawOverride: projection.status === 'overridden',
    });
  }

  function reset(descriptor: GuidedSettingDescriptor) {
    const projection = projectionFor(descriptor);
    dispatch('change', {
      settingId: descriptor.id,
      value: undefined,
      replaceRawOverride: projection.status === 'overridden',
    });
  }

  function statusLabel(projection: GuidedProjection): string {
    switch (projection.status) {
      case 'explicit': return 'Explicit';
      case 'custom': return 'Custom / Advanced';
      case 'overridden': return 'Overridden by Raw';
      default: return 'Following source';
    }
  }

  function categoryLabel(category: string): string {
    return `Guided category: ${category}`;
  }
</script>

<section class="guided-workspace" aria-label={$t('Common settings')}>
  <header>
    <div>
      <strong>{$t('Common settings')}</strong>
      <small>{$t('These controls update only the semantic field they own.')}</small>
    </div>
    <span
      class:invalid={state.desired.validation === 'invalid'}
      class:pending={state.desired.validation === 'pending'}
      class="candidate-state"
    >
      {$t(state.desired.validation === 'valid' ? 'Validated' : state.desired.validation === 'invalid' ? 'Needs attention' : 'Pending validation')}
    </span>
  </header>

  {#if state.sourceStatuses.length > 0}
    <div class="source-statuses" aria-label={$t('Configuration source status')}>
      {#each state.sourceStatuses as source (source.sourceId)}
        {@const summary = state.sourceParseSummaries?.[source.sourceId]}
        <span
          class:warning={source.freshness === 'stale'}
          class:problem={source.freshness === 'invalid' || source.freshness === 'unavailable'}
          title={source.message ?? source.sourceName}
        >
          <i></i>{source.sourceName} · {$t(source.freshness)}{#if summary} · {summary.acceptedItems}/{summary.totalItems} {$t('accepted')}{/if}
        </span>
      {/each}
    </div>
  {/if}

  {#each categories as category (category)}
    <div class="guided-category">
      <h3>{$t(categoryLabel(category))}</h3>
      <div class="guided-grid">
        {#each state.guidedDescriptors.filter((descriptor) => descriptor.category === category) as descriptor (descriptor.id)}
          {@const projection = projectionFor(descriptor)}
          {@const controlDisabled = disabled || !dependencyEnabled(descriptor)}
          <article class:custom={projection.status === 'custom'} class:overridden={projection.status === 'overridden'}>
            <div class="setting-copy">
              <strong>{$t(descriptor.label)}</strong>
              <small>{$t(descriptor.description)}</small>
              <span class="projection-status">{$t(statusLabel(projection))}</span>
            </div>
            <div class="setting-control">
              {#if descriptor.control === 'toggle'}
                <label class="guided-toggle">
                  <input
                    type="checkbox"
                    checked={effectiveValue(descriptor) === true}
                    disabled={controlDisabled}
                    aria-label={$t(descriptor.label)}
                    on:change={(event) => change(descriptor, event.currentTarget.checked)}
                  />
                  <span></span>
                </label>
              {:else if descriptor.control === 'select'}
                <select
                  value={typeof effectiveValue(descriptor) === 'string' && descriptor.allowedValues.includes(String(effectiveValue(descriptor))) ? String(effectiveValue(descriptor)) : ''}
                  disabled={controlDisabled}
                  aria-label={$t(descriptor.label)}
                  on:change={(event) => change(descriptor, event.currentTarget.value)}
                >
                  <option value="" disabled>{$t(projection.status === 'custom' ? 'Custom / Advanced' : 'Select a value')}</option>
                  {#each descriptor.allowedValues as value (value)}
                    <option {value}>{$t(value)}</option>
                  {/each}
                </select>
              {:else if descriptor.control === 'number'}
                <input type="number" value={typeof effectiveValue(descriptor) === 'number' ? Number(effectiveValue(descriptor)) : undefined} disabled={controlDisabled} aria-label={$t(descriptor.label)} on:change={(event) => change(descriptor, event.currentTarget.valueAsNumber)} />
              {:else}
                <input type="text" value={typeof effectiveValue(descriptor) === 'string' ? String(effectiveValue(descriptor)) : ''} disabled={controlDisabled} aria-label={$t(descriptor.label)} on:change={(event) => change(descriptor, event.currentTarget.value)} />
              {/if}
              <button type="button" on:click={() => reset(descriptor)} disabled={controlDisabled || projection.status === 'inherited'}>
                {$t(projection.status === 'overridden' ? 'Remove Raw override and follow source' : 'Follow source')}
              </button>
            </div>
            {#if projection.status === 'overridden'}
              <p>{$t('Changing this setting explicitly removes the overlapping Raw override.')}</p>
            {:else if projection.status === 'custom'}
              <p>{$t('The effective configuration cannot be represented safely by this simple control.')}</p>
            {/if}
          </article>
        {/each}
      </div>
    </div>
  {/each}

  {#if state.desired.diagnostics.length > 0 || state.desired.conflicts.length > 0}
    <div class="guided-diagnostics" role="status">
      {#each state.desired.diagnostics as diagnostic (`diagnostic-${diagnostic.code}`)}
        <span><strong>{diagnostic.code}</strong>{diagnostic.message}</span>
      {/each}
      {#each state.desired.conflicts as conflict (`conflict-${conflict.semanticPath}`)}
        <span><strong>{conflict.semanticPath}</strong>{conflict.reason}</span>
      {/each}
    </div>
  {/if}
</section>

<style>
  .guided-workspace { display: grid; gap: 14px; margin-bottom: var(--ui-gap-md, 16px); padding: 16px; border: 1px solid var(--border-color, rgba(127,127,127,.28)); border-radius: 14px; background: var(--panel-background, rgba(127,127,127,.045)); }
  header { display: flex; align-items: flex-start; justify-content: space-between; gap: 16px; }
  header > div, .setting-copy { display: grid; gap: 3px; }
  header small, .setting-copy small { opacity: .72; line-height: 1.35; }
  .candidate-state, .source-statuses span, .projection-status { width: fit-content; border-radius: 999px; padding: 3px 8px; font-size: .78rem; background: rgba(60, 150, 95, .12); }
  .candidate-state.pending, .source-statuses span.warning { background: rgba(220, 160, 40, .14); }
  .candidate-state.invalid, .source-statuses span.problem { background: rgba(210, 70, 70, .14); }
  .source-statuses { display: flex; flex-wrap: wrap; gap: 7px; }
  .source-statuses span { display: inline-flex; align-items: center; gap: 6px; }
  .source-statuses i { width: 7px; height: 7px; border-radius: 50%; background: currentColor; opacity: .65; }
  .guided-category { display: grid; gap: 8px; }
  .guided-category h3 { margin: 0; font-size: .86rem; text-transform: capitalize; opacity: .7; }
  .guided-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: 9px; }
  article { display: grid; gap: 10px; align-content: start; padding: 12px; border: 1px solid var(--border-color, rgba(127,127,127,.22)); border-radius: 11px; background: var(--surface-background, rgba(255,255,255,.025)); }
  article.custom, article.overridden { border-style: dashed; }
  .projection-status { margin-top: 4px; background: rgba(127,127,127,.12); }
  .setting-control { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
  .setting-control select, .setting-control input[type='number'], .setting-control input[type='text'] { min-width: 150px; min-height: 34px; }
  .setting-control button { min-height: 32px; }
  .guided-toggle { display: inline-flex; align-items: center; }
  .guided-toggle input { width: 18px; height: 18px; }
  article p { margin: 0; font-size: .8rem; line-height: 1.4; opacity: .78; }
  .guided-diagnostics { display: grid; gap: 5px; padding: 10px; border-radius: 9px; background: rgba(210,70,70,.09); }
  .guided-diagnostics span { display: flex; gap: 8px; font-size: .82rem; }
  @media (max-width: 720px) { .guided-grid { grid-template-columns: 1fr; } header { align-items: stretch; flex-direction: column; } }
</style>

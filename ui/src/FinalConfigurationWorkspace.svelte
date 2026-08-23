<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { t } from './i18n';
  import RawDecisionPanel from './RawDecisionPanel.svelte';
  import type {
    ConfigurationLayerTrace,
    ConfigurationWorkspaceView,
    RawDecisionResolution,
  } from './types';

  export let workspace: ConfigurationWorkspaceView;
  export let disabled = false;

  const dispatch = createEventDispatcher<{
    resolve: { decisionId: string; resolution: RawDecisionResolution };
    focusPath: { path: string };
    navigateSurface: { surface: 'intent' | 'details' | 'sources' | 'configuration' };
  }>();

  type TraceFilter = 'all' | 'conflicts' | 'raw' | 'superseded' | 'nonSource';
  let filter: TraceFilter = 'all';
  let expandedPath = '';

  $: decisions = workspace.rawDecisions;
  $: counts = {
    active: decisions.filter((item) => item.status === 'active').length,
    resolved: decisions.filter((item) => item.status === 'resolved').length,
    superseded: decisions.filter((item) => item.status === 'superseded').length,
    dormant: decisions.filter((item) => item.status === 'dormant').length,
  };
  $: workflowBlocked = workspace.gateBlockers.some((blocker) => blocker.blocks.includes('save') || blocker.blocks.includes('validate'));
  $: traces = workspace.layerTrace.filter((trace) => matchesFilter(trace));

  function matchesFilter(trace: ConfigurationLayerTrace): boolean {
    if (filter === 'conflicts') return trace.issueIds.length > 0 || trace.rawDecisionStatus === 'superseded';
    if (filter === 'raw') return trace.rawDecisionId !== undefined;
    if (filter === 'superseded') return trace.rawDecisionStatus === 'superseded';
    if (filter === 'nonSource') return trace.winnerLayer !== 'source';
    return true;
  }

  function value(value: unknown): string {
    if (value === undefined) return $t('Not present');
    if (value === null) return 'null';
    if (typeof value === 'string') return value;
    try { return JSON.stringify(value); } catch { return String(value); }
  }

  function layerLabel(layer: ConfigurationLayerTrace['winnerLayer']): string {
    return layer === 'rawDecision'
      ? $t('Raw decision')
      : layer === 'details'
        ? $t('Details value')
        : layer === 'intent'
          ? $t('Intent value')
          : $t('Source value');
  }

  function layerSurface(layer: ConfigurationLayerTrace['winnerLayer']): 'intent' | 'details' | 'sources' | 'configuration' {
    return layer === 'intent' ? 'intent' : layer === 'details' ? 'details' : layer === 'source' ? 'sources' : 'configuration';
  }

  function statusLabel(status: ConfigurationLayerTrace['rawDecisionStatus']): string {
    return status === 'superseded'
      ? $t('Superseded')
      : status === 'resolved'
        ? $t('Resolved')
        : status === 'dormant'
          ? $t('Dormant')
          : $t('Active');
  }

  function toggle(trace: ConfigurationLayerTrace): void {
    expandedPath = expandedPath === trace.semanticPath ? '' : trace.semanticPath;
  }

  function focus(trace: ConfigurationLayerTrace): void {
    expandedPath = trace.semanticPath;
    dispatch('focusPath', { path: trace.semanticPath });
  }

  function blockerLabelKey(code: string): string {
    const labels: Record<string, string> = {
      SOURCE_VALUE_CONFLICT: 'Source values conflict',
      LAYER_OWNERSHIP_CONFLICT: 'Layer ownership conflict',
      RAW_DECISION_SUPERSEDED: 'Raw decision needs rebase',
      CONFIGURATION_INVALID: 'Candidate configuration is invalid',
      CONFIG_INVALID: 'Candidate configuration is invalid',
      CORE_VALIDATION_EVIDENCE_STALE: 'Validation evidence is stale',
      CORE_PROFILE_MISMATCH: 'Compatibility profile changed',
      CORE_TARGET_CHANGED: 'Compatibility target changed',
      CORE_INVALID: 'The exact binary rejected the candidate',
      CORE_VALIDATION_REQUIRED: 'Validate the saved candidate with the exact binary',
    };
    return labels[code] ?? 'Configuration needs attention';
  }
</script>

<section class="final-workspace" aria-labelledby="final-configuration-title">
  <header class="workspace-heading">
    <div>
      <p class="eyebrow">{$t('Final configuration')}</p>
      <h2 id="final-configuration-title">{$t('Effective candidate')}</h2>
      <p class="pipeline">{$t('Sources')} <span>→</span> {$t('Intent')} <span>→</span> {$t('Details')} <span>→</span> {$t('Final decision')}</p>
    </div>
    <div class="candidate-state" role="status" aria-live="polite">
      <strong>{$t(workspace.validationStatus === 'valid' ? 'Validated' : workspace.validationStatus === 'invalid' ? 'Needs attention' : 'Pending validation')}</strong>
      <span>{workflowBlocked ? $t('Blocked') : workspace.validationStatus === 'valid' ? $t('Ready to apply') : $t('Saved candidate')}</span>
    </div>
  </header>

  <div class="decision-counts" aria-label={$t('Raw decision counts')}>
    <span>{decisions.length} {$t('Raw decisions')}</span>
    <span class="active">{counts.active} {$t('Active')}</span>
    <span class="resolved">{counts.resolved} {$t('Resolved')}</span>
    <span class="superseded">{counts.superseded} {$t('Superseded')}</span>
    <span class="dormant">{counts.dormant} {$t('Dormant')}</span>
  </div>

  {#if workspace.gateBlockers.length > 0}
    <section class="gate-blockers" aria-labelledby="gate-blockers-title">
      <h3 id="gate-blockers-title">{$t(workflowBlocked ? 'Resolve blocking issues before continuing' : 'Validate the candidate before applying')}</h3>
      {#each workspace.gateBlockers as blocker (blocker.code + (blocker.semanticPath ?? ''))}
        <article class="gate-blocker">
          <strong>{$t(blockerLabelKey(blocker.code))}</strong>
          {#if blocker.semanticPath}<button type="button" class="path-link" on:click={() => dispatch('focusPath', { path: blocker.semanticPath ?? '' })}>{blocker.semanticPath}</button>{/if}
          <span>{$t(blocker.blocks.includes('save') || blocker.blocks.includes('validate') ? 'Save, validate, and apply are blocked until this is resolved.' : 'Apply is blocked until this is resolved.')}</span>
          <small>{$t('Applied and Last Known Good are retained.')}</small>
        </article>
      {/each}
    </section>
  {/if}

  <RawDecisionPanel
    {workspace}
    {disabled}
    on:resolve={(event) => dispatch('resolve', event.detail)}
    on:focusPath={(event) => dispatch('focusPath', event.detail)}
  />

  <section class="trace-section" aria-labelledby="layer-trace-title">
    <div class="trace-heading">
      <div><p class="eyebrow">{$t('Layer trace')}</p><h3 id="layer-trace-title">{$t('Effective candidate paths')}</h3></div>
      <label class="trace-filter"><span>{$t('Filter paths')}</span><select bind:value={filter}><option value="all">{$t('All paths')}</option><option value="conflicts">{$t('Conflicts')}</option><option value="raw">{$t('Raw decisions')}</option><option value="superseded">{$t('Superseded')}</option><option value="nonSource">{$t('Non-source values')}</option></select></label>
    </div>
    {#if traces.length === 0}
      <p class="empty-trace">{$t('No paths match this filter.')}</p>
    {:else}
      <div class="trace-table" role="list" aria-label={$t('Layer trace')}>
        <div class="trace-row trace-header" aria-hidden="true"><span>{$t('Semantic path')}</span><span>{$t('Winner')}</span><span>{$t('Effective value')}</span><span>{$t('Status')}</span></div>
        {#each traces as trace (trace.semanticPath)}
          <div class:expanded={expandedPath === trace.semanticPath} class="trace-row" role="listitem">
            <button type="button" class="trace-path" on:click={() => focus(trace)}><code>{trace.semanticPath}</code></button>
            <button type="button" class="trace-layer" on:click={() => dispatch('navigateSurface', { surface: layerSurface(trace.winnerLayer) })}>{$t(layerLabel(trace.winnerLayer))}</button>
            <span class="trace-effective">{value(trace.effectiveValue)}</span>
            <button type="button" class="trace-status" on:click={() => toggle(trace)}>{statusLabel(trace.rawDecisionStatus)}</button>
            {#if expandedPath === trace.semanticPath}
              <div class="trace-details">
                <div><small>{$t('Source IDs')}</small><code>{trace.sourceIds.length ? trace.sourceIds.join(', ') : $t('Not present')}</code></div>
                <div><small>{$t('Source value')}</small><code>{value(trace.sourceValue)}</code></div>
                <div><small>{$t('Intent value')}</small><code>{value(trace.guidedValue)}</code></div>
                <div><small>{$t('Details value')}</small><code>{value(trace.detailsValue)}</code></div>
                <div><small>{$t('Raw value')}</small><code>{value(trace.rawValue)}</code></div>
                <div><small>{$t('Effective value')}</small><code>{value(trace.effectiveValue)}</code></div>
                {#if trace.issueIds.length}<div class="trace-issues"><small>{$t('Issues')}</small><code>{trace.issueIds.join(', ')}</code></div>{/if}
              </div>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  </section>
</section>

<style>
  .final-workspace { display: grid; gap: 14px; min-width: 0; }
  .workspace-heading, .trace-heading { display: flex; align-items: flex-start; justify-content: space-between; gap: 14px; flex-wrap: wrap; }
  .eyebrow { margin: 0; font-size: .72rem; letter-spacing: .04em; text-transform: uppercase; opacity: .64; }
  h2, h3 { margin: 3px 0 0; }
  h2 { font-size: 1.18rem; }
  h3 { font-size: .98rem; }
  .pipeline { margin: 7px 0 0; color: var(--text-muted, #6b7280); font-size: .8rem; }
  .pipeline span { padding: 0 4px; }
  .candidate-state { display: grid; gap: 3px; min-width: 155px; padding: 9px 11px; border: 1px solid var(--border-subtle, rgba(90,110,150,.25)); border-radius: 8px; background: var(--surface-raised, rgba(90,110,150,.06)); text-align: right; }
  .candidate-state span, .candidate-state strong { overflow-wrap: anywhere; }
  .candidate-state strong { font-size: .83rem; }
  .candidate-state span { font-size: .75rem; opacity: .72; }
  .decision-counts { display: flex; gap: 7px; flex-wrap: wrap; align-items: center; font-size: .75rem; }
  .decision-counts span { padding: 4px 7px; border: 1px solid var(--border-subtle, rgba(90,110,150,.2)); border-radius: 999px; overflow-wrap: anywhere; }
  .decision-counts .active { color: #256d50; }.decision-counts .superseded { color: #9b3030; }.decision-counts .dormant { opacity: .65; }
  .gate-blockers, .trace-section { display: grid; gap: 9px; min-width: 0; padding: 12px; border: 1px solid var(--border-subtle, rgba(90,110,150,.2)); border-radius: 9px; }
  .gate-blockers { border-color: rgba(180,70,70,.3); background: rgba(180,70,70,.06); }
  .gate-blocker { display: grid; gap: 3px; min-width: 0; padding: 8px 9px; border-left: 3px solid #b44646; background: rgba(255,255,255,.25); }
  .gate-blocker strong, .gate-blocker span, .gate-blocker small { overflow-wrap: anywhere; }
  .gate-blocker strong { font-size: .82rem; }.gate-blocker span, .gate-blocker small { font-size: .75rem; }.gate-blocker small { opacity: .74; }
  .path-link, .trace-path, .trace-layer, .trace-status { border: 0; background: transparent; color: inherit; cursor: pointer; text-align: left; }
  .path-link { justify-self: start; padding: 0; color: var(--accent, #2268a5); font: .75rem ui-monospace, SFMono-Regular, Consolas, monospace; overflow-wrap: anywhere; }
  .trace-filter { display: flex; align-items: center; gap: 7px; font-size: .78rem; }.trace-filter select { min-width: 150px; }
  .trace-table { display: grid; gap: 1px; min-width: 0; border: 1px solid var(--border-subtle, rgba(90,110,150,.18)); border-radius: 7px; overflow: hidden; }
  .trace-row { display: grid; grid-template-columns: minmax(150px, 1.4fr) minmax(90px, .75fr) minmax(100px, 1fr) minmax(78px, .65fr); gap: 8px; align-items: start; min-width: 0; padding: 8px 9px; background: var(--surface-raised, rgba(90,110,150,.04)); }
  .trace-row + .trace-row { border-top: 1px solid var(--border-subtle, rgba(90,110,150,.14)); }.trace-header { color: var(--text-muted, #6b7280); font-size: .7rem; text-transform: uppercase; }.trace-row code, .trace-effective { min-width: 0; overflow-wrap: anywhere; white-space: pre-wrap; font: .74rem/1.35 ui-monospace, SFMono-Regular, Consolas, monospace; }.trace-path code { color: var(--accent, #2268a5); }.trace-layer, .trace-status { padding: 0; font-size: .76rem; overflow-wrap: anywhere; }.trace-status { color: var(--text-muted, #6b7280); }.trace-details { grid-column: 1 / -1; display: grid; grid-template-columns: repeat(5, minmax(0, 1fr)); gap: 7px; padding: 8px 0 0; border-top: 1px dashed var(--border-subtle, rgba(90,110,150,.2)); }.trace-details div { display: grid; gap: 3px; min-width: 0; }.trace-details small { color: var(--text-muted, #6b7280); font-size: .69rem; }.trace-issues { grid-column: 1 / -1; }.empty-trace { margin: 0; color: var(--text-muted, #6b7280); font-size: .82rem; }
  @media (max-width: 760px) { .trace-header { display: none; }.trace-row { grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); }.trace-effective { grid-column: 1 / -1; }.trace-details { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
  @media (max-width: 520px) { .candidate-state { width: 100%; text-align: left; }.trace-row { grid-template-columns: 1fr; }.trace-effective, .trace-details { grid-column: 1; }.trace-details { grid-template-columns: 1fr; }.trace-filter { align-items: stretch; flex-direction: column; }.trace-filter select { width: 100%; } }
</style>

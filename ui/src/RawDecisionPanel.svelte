<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { t } from './i18n';
  import type {
    ConfigurationWorkspaceView,
    RawDecisionProjection,
    RawDecisionResolution,
  } from './types';

  export let workspace: ConfigurationWorkspaceView;
  export let disabled = false;

  const dispatch = createEventDispatcher<{
    resolve: { decisionId: string; resolution: RawDecisionResolution };
  }>();
  let expanded = '';
  let manualValues: Record<string, string> = {};
  let manualErrors: Record<string, string> = {};

  $: visible = workspace.rawDecisions.filter((decision) => decision.status !== 'dormant');
  $: superseded = visible.filter((decision) => decision.status === 'superseded');
  $: active = visible.filter((decision) => decision.status === 'active' || decision.status === 'resolved');

  function toggle(decision: RawDecisionProjection) {
    expanded = expanded === decision.decisionId ? '' : decision.decisionId;
    if (!(decision.decisionId in manualValues)) {
      manualValues[decision.decisionId] = JSON.stringify(decision.rawValue, null, 2) ?? 'null';
      manualValues = { ...manualValues };
    }
  }

  function resolve(decision: RawDecisionProjection, resolution: RawDecisionResolution) {
    dispatch('resolve', { decisionId: decision.decisionId, resolution });
  }

  function resolveManual(decision: RawDecisionProjection) {
    try {
      const value = JSON.parse(manualValues[decision.decisionId] ?? 'null');
      delete manualErrors[decision.decisionId];
      manualErrors = { ...manualErrors };
      resolve(decision, { manualEdit: { value } });
    } catch {
      manualErrors[decision.decisionId] = $t('Enter a valid JSON value for this conflict.');
      manualErrors = { ...manualErrors };
    }
  }
</script>

{#if visible.length > 0}
  <section class:blocked={superseded.length > 0} class="raw-decision-panel" aria-labelledby="raw-decisions-title">
    <header>
      <div>
        <p class="eyebrow">{$t('Final configuration')}</p>
        <h3 id="raw-decisions-title">{$t('Raw decisions')}</h3>
      </div>
      <div class="counts" aria-live="polite">
        {#if active.length}<span class="status active">{active.length} {$t('Active')}</span>{/if}
        {#if superseded.length}<span class="status superseded">{superseded.length} {$t('Superseded')}</span>{/if}
      </div>
    </header>
    <p class="help">
      {$t(superseded.length
        ? 'Upstream settings changed on paths previously decided in Raw. Resolve each item before saving or applying.'
        : 'Raw decisions are final for their current upstream basis. A later Source, Intent, or Details change will reopen only the affected paths.')}
    </p>
    <ol>
      {#each visible as decision (decision.decisionId)}
        <li class:superseded={decision.status === 'superseded'}>
          <button
            type="button"
            class="decision-heading"
            aria-expanded={expanded === decision.decisionId}
            on:click={() => toggle(decision)}
          >
            <span>{decision.semanticPath}</span>
            <small>{$t(decision.status === 'superseded' ? 'Superseded' : decision.status === 'resolved' ? 'Resolved' : 'Active')}</small>
          </button>
          {#if expanded === decision.decisionId}
            <div class="decision-values">
              <div><small>{$t('Current upstream')}</small><pre>{JSON.stringify(decision.upstreamValue, null, 2) ?? $t('Field absent')}</pre></div>
              <div><small>{$t('Raw decision')}</small><pre>{JSON.stringify(decision.rawValue, null, 2) ?? $t('Delete field')}</pre></div>
            </div>
            {#if decision.status === 'superseded'}
              <div class="resolution-actions">
                <button type="button" on:click={() => resolve(decision, 'acceptUpstream')} disabled={disabled}>{$t('Accept upstream')}</button>
                <button type="button" on:click={() => resolve(decision, 'keepRaw')} disabled={disabled}>{$t('Keep Raw')}</button>
              </div>
              <div class="manual-resolution">
                <label for={`raw-decision-${decision.decisionId}`}>{$t('Manual merge')}</label>
                <textarea
                  id={`raw-decision-${decision.decisionId}`}
                  bind:value={manualValues[decision.decisionId]}
                  rows="4"
                  spellcheck="false"
                  disabled={disabled}
                  aria-invalid={!!manualErrors[decision.decisionId]}
                ></textarea>
                {#if manualErrors[decision.decisionId]}<p role="alert">{manualErrors[decision.decisionId]}</p>{/if}
                <button type="button" on:click={() => resolveManual(decision)} disabled={disabled}>{$t('Use merged value')}</button>
              </div>
            {/if}
          {/if}
        </li>
      {/each}
    </ol>
  </section>
{/if}

<style>
  .raw-decision-panel { display: grid; gap: 10px; margin: 14px 0; padding: 14px; border: 1px solid rgba(82, 110, 160, .25); border-radius: 14px; background: rgba(82, 110, 160, .06); }
  .raw-decision-panel.blocked { border-color: rgba(205, 80, 80, .34); background: rgba(205, 80, 80, .07); }
  header, .counts, .resolution-actions, .decision-heading { display: flex; align-items: center; gap: 8px; }
  header { justify-content: space-between; flex-wrap: wrap; }
  .eyebrow { margin: 0; font-size: .7rem; text-transform: uppercase; opacity: .64; }
  h3 { margin: 2px 0 0; font-size: 1rem; }
  .counts { flex-wrap: wrap; }
  .status { padding: 3px 7px; border-radius: 999px; font-size: .72rem; background: rgba(80, 105, 145, .12); }
  .status.superseded { color: #9b3030; background: rgba(205, 80, 80, .12); }
  .help { margin: 0; font-size: .82rem; opacity: .76; }
  ol { display: grid; gap: 7px; margin: 0; padding: 0; list-style: none; }
  li { min-width: 0; border: 1px solid rgba(82, 110, 160, .16); border-radius: 9px; background: rgba(255,255,255,.38); }
  li.superseded { border-color: rgba(205,80,80,.3); }
  .decision-heading { width: 100%; justify-content: space-between; min-width: 0; padding: 9px 10px; border: 0; background: transparent; color: inherit; text-align: left; cursor: pointer; }
  .decision-heading span { min-width: 0; overflow-wrap: anywhere; font: .78rem/1.35 ui-monospace, SFMono-Regular, Consolas, monospace; }
  .decision-heading small { flex: 0 0 auto; opacity: .7; }
  .decision-values { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 8px; padding: 0 10px 9px; }
  .decision-values small { opacity: .7; }
  pre { max-height: 180px; margin: 4px 0 0; overflow: auto; padding: 7px; border-radius: 6px; background: rgba(20,30,45,.08); font-size: .72rem; white-space: pre-wrap; overflow-wrap: anywhere; }
  .resolution-actions { flex-wrap: wrap; padding: 0 10px 10px; }
  .manual-resolution { display: grid; gap: 6px; padding: 0 10px 10px; }
  .manual-resolution label { font-size: .78rem; font-weight: 650; }
  .manual-resolution textarea { width: 100%; min-width: 0; resize: vertical; padding: 8px; border: 1px solid rgba(90,110,150,.3); border-radius: 7px; background: rgba(255,255,255,.55); color: inherit; font: .76rem/1.4 ui-monospace, SFMono-Regular, Consolas, monospace; }
  .manual-resolution p { margin: 0; color: #a52b2b; font-size: .75rem; }
  .manual-resolution button { justify-self: start; }
  button { border: 1px solid rgba(90,110,150,.28); border-radius: 7px; padding: 6px 9px; background: transparent; color: inherit; cursor: pointer; }
  button:disabled { opacity: .45; cursor: not-allowed; }
  @media (max-width: 680px) { .decision-values { grid-template-columns: 1fr; } header { align-items: flex-start; flex-direction: column; } }
</style>

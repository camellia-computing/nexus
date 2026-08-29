<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { t } from './i18n';
  import type {
    ConfigurationEditorPath,
    ConfigurationWorkspaceView,
    RawConflictResolution,
    RawDecisionResolution,
    RawDraftSession,
  } from './types';

  export let workspace: ConfigurationWorkspaceView;
  export let draft: RawDraftSession | null = null;
  export let selectedPath = '';
  export let disabled = false;
  export let appliedMatchesCandidate = false;
  export let lastKnownGoodMatchesCandidate = false;

  const dispatch = createEventDispatcher<{
    focusPath: { path: string };
    resolveDecision: { decisionId: string; resolution: RawDecisionResolution };
    resolveDraftConflict: { conflictId: string; resolution: RawConflictResolution };
    rebaseDraft: void;
    discardDraft: void;
    navigateSurface: { surface: 'intent' | 'details' | 'sources' | 'configuration' };
  }>();

  let manualValue = '';
  let manualError = '';

  $: paths = workspace.editor?.paths ?? [];
  $: selected = paths.find((path) => path.semanticPath === selectedPath)
    ?? paths.find((path) => path.issues.length > 0)
    ?? null;
  $: rawDecision = selected?.rawDecision;
  $: draftConflicts = draft
    ? draft.conflicts.filter((conflict) => draft?.unresolvedConflictIds.includes(conflict.conflictId))
    : [];
  $: draftConflict = draftConflicts.find((conflict) => conflict.semanticPath === selectedPath)
    ?? draftConflicts[0]
    ?? null;
  $: candidateMatchesEffectiveState = appliedMatchesCandidate && lastKnownGoodMatchesCandidate;
  $: hasBlockingIssue = workspace.editor.validationStatus === 'invalid'
    || workspace.editor.blockers.some((blocker) => blocker.blocks.includes('save') || blocker.blocks.includes('validate'));
  $: stateDetail = hasBlockingIssue
    ? 'Resolve blocking issues before continuing.'
    : workspace.editor.validationStatus === 'pending'
      ? 'Validate the saved candidate before applying.'
      : candidateMatchesEffectiveState
        ? 'Applied and Last Known Good match this candidate.'
        : 'Applied and Last Known Good are retained.';
  $: if (rawDecision && !manualValue) manualValue = JSON.stringify(rawDecision.rawValue ?? null, null, 2);

  function display(value: unknown): string {
    if (value === undefined) return $t('Not present');
    if (value === null) return 'null';
    if (typeof value === 'string') return value;
    try { return JSON.stringify(value, null, 2); } catch { return String(value); }
  }

  function layerLabel(layer: ConfigurationEditorPath['winnerLayer']): string {
    return layer === 'rawDecision'
      ? 'Final decision'
      : layer === 'details'
        ? 'Details'
        : layer === 'intent'
          ? 'Intent'
          : 'Source';
  }

  function messageKey(code: string): string {
    const labels: Record<string, string> = {
      SOURCE_VALUE_CONFLICT: 'Source values conflict',
      LAYER_OWNERSHIP_CONFLICT: 'Layer ownership conflict',
      RAW_DECISION_SUPERSEDED: 'Final decision needs review',
      CONFIGURATION_INVALID: 'Candidate configuration is invalid',
      CONFIG_INVALID: 'Candidate configuration is invalid',
      CORE_VALIDATION_EVIDENCE_STALE: 'Validation evidence is stale',
      CORE_PROFILE_MISMATCH: 'Compatibility profile changed',
      CORE_TARGET_CHANGED: 'Compatibility target changed',
      CORE_INVALID: 'The exact binary rejected the candidate',
      CORE_VALIDATION_REQUIRED: 'Validate the saved candidate with the exact binary',
    };
    return labels[code] ?? code;
  }

  function statusLabel(path: ConfigurationEditorPath): string {
    const status = path.rawDecision?.status;
    if (status === 'superseded') return 'Superseded';
    if (status === 'resolved') return 'Resolved';
    if (status === 'dormant') return 'Dormant';
    if (status === 'active') return 'Active';
    return 'Effective';
  }

  function focus(path: string): void {
    dispatch('focusPath', { path });
  }

  function resolveManualDecision(): void {
    if (!rawDecision) return;
    try {
      const value = JSON.parse(manualValue || 'null');
      manualError = '';
      dispatch('resolveDecision', { decisionId: rawDecision.decisionId, resolution: { manualEdit: { value } } });
    } catch {
      manualError = $t('Enter a valid JSON value for this path.');
    }
  }

  function resolveDraft(conflict: typeof draftConflict, resolution: RawConflictResolution): void {
    if (!conflict) return;
    dispatch('resolveDraftConflict', { conflictId: conflict.conflictId, resolution });
  }
</script>

<section class="final-editor-workspace" aria-labelledby="final-editor-title">
  <header class="editor-workspace-heading">
    <div class="heading-copy">
      <p class="eyebrow">{$t('Configuration')}</p>
      <h2 id="final-editor-title">{$t('Final configuration')}</h2>
      <p class="pipeline"><span>{$t('Sources')}</span><b aria-hidden="true">→</b><span>{$t('Intent')}</span><b aria-hidden="true">→</b><span>{$t('Details')}</span><b aria-hidden="true">→</b><strong>{$t('Final decision')}</strong></p>
    </div>
    <div class="editor-state" role="status" aria-live="polite">
      <strong>{$t(workspace.editor.validationStatus === 'valid' ? 'Validated' : workspace.editor.validationStatus === 'invalid' ? 'Needs attention' : 'Pending validation')}</strong>
      <span>{$t(stateDetail)}</span>
      <code>g{workspace.editor.revision.generation}</code>
    </div>
  </header>

  <div class="decision-summary" aria-label={$t('Final decision status')}>
    <span>{$t('Final decisions')}</span>
    <span class="active">{workspace.editor.decisionCounts.active} {$t('Active')}</span>
    <span class="resolved">{workspace.editor.decisionCounts.resolved} {$t('Resolved')}</span>
    <span class:warning={workspace.editor.decisionCounts.superseded > 0} class="superseded">{workspace.editor.decisionCounts.superseded} {$t('Superseded')}</span>
    <span class="dormant">{workspace.editor.decisionCounts.dormant} {$t('Dormant')}</span>
  </div>

  {#if workspace.editor.blockers.length > 0}
    <section class:guidance={!hasBlockingIssue} class="editor-blockers" aria-labelledby="editor-blockers-title">
      <div class="blocker-heading"><strong id="editor-blockers-title">{$t(hasBlockingIssue ? 'Resolve blocking issues before continuing' : 'Next step')}</strong><small>{$t(candidateMatchesEffectiveState ? 'Applied and Last Known Good match this candidate.' : 'Applied and Last Known Good are retained.')}</small></div>
      <div class="blocker-list">
        {#each workspace.editor.blockers as blocker (blocker.code + (blocker.semanticPath ?? ''))}
          <button type="button" class="blocker" on:click={() => blocker.semanticPath && focus(blocker.semanticPath)}>
            <span class="blocker-icon" aria-hidden="true">{hasBlockingIssue ? '!' : '→'}</span>
            <span><strong>{$t(messageKey(blocker.messageKey || blocker.code))}</strong>{#if blocker.semanticPath}<code>{blocker.semanticPath}</code>{/if}</span>
          </button>
        {/each}
      </div>
    </section>
  {/if}

  <div class="editor-slot"><slot name="editor" /></div>

  {#if selected || draftConflict}
    <aside class="path-inspector" aria-labelledby="path-inspector-title">
      <header>
        <div>
          <p class="eyebrow">{$t('Selected path')}</p>
          <h3 id="path-inspector-title"><code>{selected?.semanticPath ?? draftConflict?.displayPath}</code></h3>
        </div>
        <button type="button" class="quiet-button" on:click={() => dispatch('focusPath', { path: selected?.semanticPath ?? draftConflict?.semanticPath ?? '/' })}>{$t('Locate in editor')}</button>
      </header>

      {#if selected}
        <div class="path-meta">
          <span class="layer-badge">{$t('Winner')}: {$t(layerLabel(selected.winnerLayer))}</span>
          <span class:danger={selected.rawDecision?.status === 'superseded'} class="layer-badge">{statusLabel(selected)}</span>
          {#if selected.sourceIds.length}<span class="source-note">{$t('Source IDs')}: {selected.sourceIds.join(', ')}</span>{/if}
        </div>
        <div class="value-grid">
          <div><small>{$t('Source value')}</small><pre>{display(selected.sourceValue)}</pre></div>
          <div><small>{$t('Intent value')}</small><pre>{display(selected.intentValue)}</pre></div>
          <div><small>{$t('Details value')}</small><pre>{display(selected.detailsValue)}</pre></div>
          <div><small>{$t('Final decision value')}</small><pre>{display(selected.rawValue)}</pre></div>
          <div class="effective-value"><small>{$t('Effective value')}</small><pre>{display(selected.effectiveValue)}</pre></div>
        </div>
        {#if selected.rawDecision}
          <div class="basis"><span>{$t('Basis generation')}: {selected.rawDecision.basis.upstreamGeneration}</span><code>{selected.rawDecision.basis.upstreamContentHash.slice(0, 12)}</code><code>{selected.rawDecision.basis.upstreamPathHash.slice(0, 12)}</code></div>
          {#if selected.rawDecision.status === 'superseded'}
            <div class="action-row">
              <button type="button" on:click={() => dispatch('resolveDecision', { decisionId: selected.rawDecision?.decisionId ?? '', resolution: 'acceptUpstream' })} disabled={disabled}>{$t('Accept upstream')}</button>
              <button type="button" on:click={() => dispatch('resolveDecision', { decisionId: selected.rawDecision?.decisionId ?? '', resolution: 'keepRaw' })} disabled={disabled}>{$t('Keep final decision')}</button>
            </div>
            <label class="manual-merge"><span>{$t('Manual merge')}</span><textarea bind:value={manualValue} rows="4" spellcheck="false" disabled={disabled} aria-invalid={!!manualError}></textarea></label>
            {#if manualError}<p class="inline-error" role="alert">{manualError}</p>{/if}
            <button type="button" class="manual-button" on:click={resolveManualDecision} disabled={disabled}>{$t('Use merged value')}</button>
          {/if}
        {/if}
        {#if selected.issues.length > 0}
          <div class="issue-list">
            {#each selected.issues as issue (issue.id)}<p><strong>{$t(messageKey(issue.messageKey || issue.code))}</strong><span>{$t(issue.blocking ? 'Save, validate, and apply are blocked until this is resolved.' : 'Review this path before applying.')}</span></p>{/each}
          </div>
        {/if}
      {/if}

      {#if draftConflict}
        <div class="draft-conflict">
          <strong>{$t('Draft conflict')}</strong>
          <p>{$t('The source changed while you were editing. Resolve this highlighted path before saving.')}</p>
          <div class="value-grid two-columns"><div><small>{$t('Updated source')}</small><pre>{display(draftConflict.updatedBase)}</pre></div><div><small>{$t('Your version')}</small><pre>{display(draftConflict.userValue)}</pre></div></div>
          <div class="action-row"><button type="button" on:click={() => resolveDraft(draftConflict, 'useUpdated')} disabled={disabled}>{$t('Use updated')}</button><button type="button" on:click={() => resolveDraft(draftConflict, 'keepMine')} disabled={disabled}>{$t('Keep mine')}</button>{#if draftConflict.canCombine}<button type="button" on:click={() => resolveDraft(draftConflict, 'combine')} disabled={disabled}>{$t('Combine')}</button>{/if}</div>
          <div class="draft-tools"><button type="button" on:click={() => dispatch('rebaseDraft')} disabled={disabled}>{$t('Rebase draft')}</button><button type="button" on:click={() => dispatch('discardDraft')} disabled={disabled}>{$t('Discard draft')}</button></div>
        </div>
      {/if}
    </aside>
  {/if}

  <div class="editor-actions"><slot name="actions" /></div>
</section>

<style>
  .final-editor-workspace { display: grid; gap: 10px; min-width: 0; }
  .editor-workspace-heading, .path-inspector > header, .blocker-heading { display: flex; align-items: flex-start; justify-content: space-between; gap: 12px; flex-wrap: wrap; }
  .heading-copy { min-width: 0; }.eyebrow { margin: 0; color: var(--ui-text-tertiary, #6b7280); font-size: .7rem; letter-spacing: .04em; text-transform: uppercase; }.heading-copy h2, .path-inspector h3 { margin: 2px 0 0; }.heading-copy h2 { font-size: 1.12rem; }.pipeline { display: flex; flex-wrap: wrap; align-items: center; gap: 5px; margin: 6px 0 0; color: var(--ui-text-secondary, #6b7280); font-size: .78rem; }.pipeline b { color: var(--ui-text-tertiary, #8b93a2); }.editor-state { display: grid; min-width: 190px; gap: 3px; padding: 8px 10px; border: 1px solid var(--ui-border-default, rgba(90,110,150,.2)); border-radius: 8px; background: var(--ui-surface-2, rgba(90,110,150,.06)); }.editor-state strong, .editor-state span, .editor-state code { overflow-wrap: anywhere; }.editor-state strong { font-size: .82rem; }.editor-state span { color: var(--ui-text-secondary, #6b7280); font-size: .72rem; }.editor-state code { color: var(--ui-text-tertiary, #6b7280); font-size: .7rem; }
  .decision-summary { display: flex; min-width: 0; flex-wrap: wrap; align-items: center; gap: 6px; color: var(--ui-text-secondary, #6b7280); font-size: .73rem; }.decision-summary span { padding: 3px 6px; border: 1px solid var(--ui-border-default, rgba(90,110,150,.18)); border-radius: 999px; }.decision-summary .active { color: var(--ui-success, #267052); }.decision-summary .resolved { color: var(--ui-brand, #2469a3); }.decision-summary .superseded.warning { color: var(--ui-danger, #a13b3b); border-color: color-mix(in srgb, var(--ui-danger, #a13b3b) 35%, var(--ui-border-default)); }.decision-summary .dormant { opacity: .65; }
  .editor-blockers { display: grid; gap: 7px; padding: 9px 10px; border: 1px solid color-mix(in srgb, var(--ui-danger, #a13b3b) 30%, var(--ui-border-default)); border-radius: 8px; background: color-mix(in srgb, var(--ui-danger-soft, #f5dcdc) 35%, transparent); }.editor-blockers.guidance { border-color: color-mix(in srgb, var(--ui-brand, #2469a3) 26%, var(--ui-border-default)); background: color-mix(in srgb, var(--ui-brand-soft, #d9ebfa) 42%, transparent); }.blocker-heading small { color: var(--ui-text-secondary, #6b7280); font-size: .72rem; }.blocker-list { display: grid; gap: 5px; }.blocker { display: flex; min-width: 0; align-items: center; gap: 7px; border: 0; background: transparent; padding: 3px; color: inherit; text-align: left; }.blocker:hover, .blocker:focus-visible { background: var(--ui-state-hover, rgba(90,110,150,.08)); }.blocker-icon { display: grid; width: 17px; height: 17px; flex: 0 0 17px; place-items: center; border-radius: 50%; background: var(--ui-danger, #a13b3b); color: white; font-size: .68rem; font-weight: 700; }.editor-blockers.guidance .blocker-icon { background: var(--ui-brand, #2469a3); }.blocker span:last-child { display: flex; min-width: 0; flex-wrap: wrap; gap: 4px 8px; align-items: baseline; }.blocker strong, .blocker code { overflow-wrap: anywhere; }.blocker strong { font-size: .77rem; }.blocker code { color: var(--ui-text-secondary, #6b7280); font-size: .7rem; }
  .editor-slot { min-width: 0; }.path-inspector { display: grid; gap: 9px; min-width: 0; padding: 11px; border: 1px solid var(--ui-border-default, rgba(90,110,150,.2)); border-radius: 8px; background: var(--ui-surface-2, rgba(90,110,150,.045)); }.path-inspector h3 { min-width: 0; font-size: .92rem; }.path-inspector h3 code { overflow-wrap: anywhere; white-space: pre-wrap; }.quiet-button, .action-row button, .manual-button, .draft-tools button { min-height: 30px; border: 1px solid var(--ui-border-default, rgba(90,110,150,.24)); border-radius: 6px; background: transparent; padding: 5px 8px; color: inherit; font-size: .74rem; }.quiet-button:hover, .action-row button:hover, .manual-button:hover, .draft-tools button:hover, button:focus-visible { border-color: var(--ui-focus-ring, #3d80bd); }.path-meta, .basis, .action-row, .draft-tools { display: flex; min-width: 0; flex-wrap: wrap; align-items: center; gap: 6px; }.layer-badge { padding: 3px 6px; border-radius: 999px; background: var(--ui-state-hover, rgba(90,110,150,.1)); color: var(--ui-text-secondary, #5f6877); font-size: .7rem; }.layer-badge.danger { color: var(--ui-danger, #a13b3b); }.source-note, .basis { color: var(--ui-text-secondary, #697386); font-size: .7rem; overflow-wrap: anywhere; }.basis code { overflow-wrap: anywhere; }.value-grid { display: grid; grid-template-columns: repeat(5, minmax(0, 1fr)); gap: 7px; min-width: 0; }.value-grid > div { min-width: 0; }.value-grid small { color: var(--ui-text-tertiary, #6b7280); font-size: .68rem; }.value-grid pre { max-height: 130px; min-height: 32px; margin: 3px 0 0; overflow: auto; border: 1px solid var(--ui-border-default, rgba(90,110,150,.15)); border-radius: 5px; background: var(--ui-input, rgba(255,255,255,.42)); padding: 5px; color: inherit; font: .7rem/1.35 ui-monospace, SFMono-Regular, Consolas, monospace; white-space: pre-wrap; overflow-wrap: anywhere; }.effective-value pre { border-color: color-mix(in srgb, var(--ui-brand, #2469a3) 32%, var(--ui-border-default)); }.issue-list, .draft-conflict { display: grid; gap: 6px; }.issue-list p, .draft-conflict p { margin: 0; font-size: .74rem; }.issue-list p { display: flex; min-width: 0; flex-wrap: wrap; gap: 4px 8px; }.issue-list span { color: var(--ui-text-secondary, #697386); }.draft-conflict { border-top: 1px dashed var(--ui-border-default, rgba(90,110,150,.2)); padding-top: 9px; }.draft-conflict > strong { font-size: .8rem; }.two-columns { grid-template-columns: repeat(2, minmax(0, 1fr)); }.manual-merge { display: grid; gap: 4px; max-width: 100%; color: var(--ui-text-secondary, #697386); font-size: .72rem; }.manual-merge textarea { width: 100%; min-width: 0; resize: vertical; border: 1px solid var(--ui-border-default, rgba(90,110,150,.25)); border-radius: 6px; background: var(--ui-input, rgba(255,255,255,.42)); color: inherit; padding: 7px; font: .72rem/1.35 ui-monospace, SFMono-Regular, Consolas, monospace; }.inline-error { color: var(--ui-danger, #a13b3b); }.editor-actions { min-width: 0; }
  @media (max-width: 900px) { .value-grid { grid-template-columns: repeat(3, minmax(0, 1fr)); }.effective-value { grid-column: span 3; } }
  @media (max-width: 680px) { .editor-state { width: 100%; }.value-grid, .two-columns { grid-template-columns: repeat(2, minmax(0, 1fr)); }.effective-value { grid-column: span 2; } }
  @media (max-width: 520px) { .value-grid, .two-columns { grid-template-columns: 1fr; }.effective-value { grid-column: auto; }.quiet-button { width: 100%; }.path-inspector > header { align-items: stretch; flex-direction: column; } }
</style>

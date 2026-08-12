<script lang="ts">
  import { createEventDispatcher, tick } from 'svelte';
  import { t } from './i18n';
  import type { RawConflict, RawConflictResolution, RawDraftSession } from './types';

  export let draft: RawDraftSession;
  export let disabled = false;
  const dispatch = createEventDispatcher<{
    resolve: { conflictId: string; resolution: RawConflictResolution };
    navigate: { conflictId: string };
    rebase: void;
    discard: void;
  }>();
  let expanded = '';
  let currentIndex = 0;
  let conflictButtons: HTMLButtonElement[] = [];
  let manualValues: Record<string, string> = {};
  let manualErrors: Record<string, string> = {};
  $: unresolvedIds = new Set(draft.unresolvedConflictIds);
  $: conflicts = draft.conflicts.filter((conflict) => unresolvedIds.has(conflict.conflictId));
  $: blocking = conflicts.filter((conflict) => conflict.severity === 'error');
  $: if (currentIndex >= conflicts.length) currentIndex = Math.max(0, conflicts.length - 1);

  function action(conflict: RawConflict, resolution: RawConflictResolution) {
    dispatch('resolve', { conflictId: conflict.conflictId, resolution });
  }

  function toggle(conflict: RawConflict, index: number) {
    currentIndex = index;
    dispatch('navigate', { conflictId: conflict.conflictId });
    expanded = expanded === conflict.conflictId ? '' : conflict.conflictId;
    if (!(conflict.conflictId in manualValues)) {
      manualValues[conflict.conflictId] = JSON.stringify(conflict.userValue, null, 2) ?? 'null';
      manualValues = { ...manualValues };
    }
  }

  async function navigate(delta: number) {
    if (conflicts.length === 0) return;
    currentIndex = (currentIndex + delta + conflicts.length) % conflicts.length;
    expanded = conflicts[currentIndex].conflictId;
    dispatch('navigate', { conflictId: expanded });
    await tick();
    conflictButtons[currentIndex]?.focus();
    conflictButtons[currentIndex]?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
  }

  function applyManual(conflict: RawConflict) {
    try {
      const value = JSON.parse(manualValues[conflict.conflictId] ?? 'null');
      delete manualErrors[conflict.conflictId];
      manualErrors = { ...manualErrors };
      action(conflict, { manualEdit: { value } });
    } catch {
      manualErrors[conflict.conflictId] = $t('Enter a valid JSON value for this conflict.');
      manualErrors = { ...manualErrors };
    }
  }
</script>

{#if conflicts.length > 0}
  <section class="raw-conflict-panel" aria-labelledby="raw-conflicts-title">
    <header>
      <div><p class="eyebrow">{$t('Raw configuration')}</p><h3 id="raw-conflicts-title">{blocking.length} {$t('blocking conflicts')}</h3></div>
      <div class="raw-conflict-actions"><button type="button" on:click={() => navigate(-1)} disabled={disabled || conflicts.length < 2} aria-label={$t('Previous conflict')}>← {$t('Previous')}</button><span aria-live="polite">{currentIndex + 1}/{conflicts.length}</span><button type="button" on:click={() => navigate(1)} disabled={disabled || conflicts.length < 2} aria-label={$t('Next conflict')}>{$t('Next')} →</button><button type="button" on:click={() => dispatch('rebase')} disabled={disabled}>{$t('Rebase')}</button><button type="button" on:click={() => dispatch('discard')} disabled={disabled}>{$t('Discard draft')}</button></div>
    </header>
    <p class="raw-conflict-help">{$t('The source changed while you were editing. Resolve each highlighted semantic path before applying.')}</p>
    <ol>
      {#each conflicts as conflict, index (conflict.conflictId)}
        <li class:current={index === currentIndex}>
          <button bind:this={conflictButtons[index]} type="button" class="conflict-heading" aria-expanded={expanded === conflict.conflictId} on:click={() => toggle(conflict, index)}>
            <span>{index + 1}. {conflict.displayPath}</span><small>{conflict.conflictType}</small>
          </button>
          {#if expanded === conflict.conflictId}
            <div class="conflict-values">
              <div><small>{$t('Updated source')}</small><pre>{JSON.stringify(conflict.updatedBase, null, 2)}</pre></div>
              <div><small>{$t('Your version')}</small><pre>{JSON.stringify(conflict.userValue, null, 2)}</pre></div>
            </div>
            <div class="resolution-actions">
              <button type="button" on:click={() => action(conflict, 'keepMine')} disabled={disabled}>{$t('Keep Mine')}</button>
              <button type="button" on:click={() => action(conflict, 'useUpdated')} disabled={disabled}>{$t('Use Updated')}</button>
              {#if conflict.canCombine}<button type="button" on:click={() => action(conflict, 'combine')} disabled={disabled}>{$t('Combine')}</button>{/if}
            </div>
            <div class="manual-resolution">
              <label for={`manual-conflict-${index}`}>{$t('Manual Edit')}</label>
              <textarea id={`manual-conflict-${index}`} bind:value={manualValues[conflict.conflictId]} rows="4" spellcheck="false" disabled={disabled} aria-invalid={!!manualErrors[conflict.conflictId]} aria-describedby={manualErrors[conflict.conflictId] ? `manual-conflict-error-${index}` : undefined}></textarea>
              {#if manualErrors[conflict.conflictId]}<p id={`manual-conflict-error-${index}`} class="manual-error">{manualErrors[conflict.conflictId]}</p>{/if}
              <button type="button" on:click={() => applyManual(conflict)} disabled={disabled}>{$t('Apply manual value')}</button>
            </div>
          {/if}
        </li>
      {/each}
    </ol>
  </section>
{/if}

<style>
  .raw-conflict-panel { display: grid; gap: 10px; margin: 14px 0; padding: 15px; border: 1px solid rgba(205, 80, 80, .28); border-radius: 14px; background: rgba(205, 80, 80, .07); }
  header, .raw-conflict-actions, .resolution-actions, .conflict-heading { display: flex; align-items: center; gap: 8px; }
  header { justify-content: space-between; flex-wrap: wrap; }
  .eyebrow { margin: 0; font-size: .7rem; text-transform: uppercase; opacity: .62; }
  h3 { margin: 2px 0 0; font-size: 1rem; }
  .raw-conflict-help { margin: 0; font-size: .82rem; opacity: .75; }
  ol { display: grid; gap: 7px; margin: 0; padding-left: 22px; }
  li { min-width: 0; border: 1px solid transparent; border-radius: 9px; background: rgba(255,255,255,.38); }
  li.current { border-color: rgba(205,80,80,.3); }
  .conflict-heading { width: 100%; justify-content: space-between; padding: 9px 10px; border: 0; background: transparent; text-align: left; cursor: pointer; }
  .conflict-heading small { opacity: .65; }
  .conflict-values { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 8px; padding: 0 10px 8px; }
  .conflict-values small { opacity: .68; }
  pre { max-height: 180px; margin: 4px 0 0; overflow: auto; padding: 7px; border-radius: 6px; background: rgba(20,30,45,.08); font-size: .72rem; white-space: pre-wrap; overflow-wrap: anywhere; }
  .resolution-actions { flex-wrap: wrap; padding: 0 10px 10px; }
  .manual-resolution { display: grid; gap: 6px; padding: 0 10px 10px; }
  .manual-resolution label { font-size: .78rem; font-weight: 650; }
  .manual-resolution textarea { width: 100%; min-width: 0; resize: vertical; padding: 8px; border: 1px solid rgba(90,110,150,.3); border-radius: 7px; background: rgba(255,255,255,.55); color: inherit; font: .76rem/1.4 ui-monospace, SFMono-Regular, Consolas, monospace; }
  .manual-resolution button { justify-self: start; }
  .manual-error { margin: 0; color: #a52b2b; font-size: .75rem; }
  button { border: 1px solid rgba(90,110,150,.28); border-radius: 7px; padding: 6px 9px; background: transparent; cursor: pointer; }
  button:disabled { opacity: .45; cursor: not-allowed; }
  @media (max-width: 900px) { .raw-conflict-actions { width: 100%; flex-wrap: wrap; } }
  @media (max-width: 720px) { .conflict-values { grid-template-columns: 1fr; } header { align-items: flex-start; flex-direction: column; } .raw-conflict-actions button { flex: 1 1 auto; } }
</style>

<script lang="ts">
  import { onDestroy } from 'svelte';
  import { t } from './i18n';
  import { configurationErrorInfo } from './errors';
  import Icon from './lib/components/Icon.svelte';
  import { intentEffectLabel } from './intentPresentation';
  import type { ConfigurationIntentAction, IntentChangePlan, IntentHistorySummary, IntentSubmission } from './types';
  export let records: IntentHistorySummary[];
  export let disabled = false;
  export let revision: number;
  export let submit: IntentSubmission;
  export let preview: (change: ConfigurationIntentAction) => Promise<IntentChangePlan>;
  export let onEditingChange: (editing: boolean) => void;
  let pending = false;
  let error = '';
  let review: { action: 'undo' | 'redo'; recordId: string; plan: IntentChangePlan } | null = null;
  let choices: Record<string, 'keepCurrent' | 'restore'> = {};
  let sequence = 0;
  $: latest = records.find((record) => record.canUndo);
  $: redo = records.find((record) => record.canRedo);
  $: onEditingChange(review !== null);
  onDestroy(() => { sequence += 1; onEditingChange(false); });
  function summary(values: IntentChangePlan['historyConflicts'][number]['current'], protectedValue: boolean) {
    if (protectedValue) return 'Protected values will stay hidden.';
    return values.map((value) => value?.state === 'present' ? JSON.stringify(value.value) : '—').join(' · ').slice(0, 1200);
  }
  async function restore(action: 'undo' | 'redo', recordId: string, chosen: Record<string, 'keepCurrent' | 'restore'> = {}) {
    if (disabled || pending) return;
    const request = ++sequence;
    pending = true; error = '';
    try {
      const comparison = await preview({ action, recordId, choices: {} });
      if (request !== sequence) return;
      const confirmed = Object.fromEntries(comparison.historyConflicts.flatMap((conflict) =>
        chosen[conflict.conflictId] ? [[conflict.conflictId, chosen[conflict.conflictId]]] : []));
      if (comparison.historyConflicts.some((conflict) => !confirmed[conflict.conflictId])) { review = { action, recordId, plan: comparison }; choices = confirmed; return; }
      const change: ConfigurationIntentAction = { action, recordId, choices: confirmed };
      const plan = comparison.historyConflicts.length ? await preview(change) : comparison;
      if (request !== sequence) return;
      if (!plan.canCommit) { error = configurationErrorInfo({ code: 'CONFIG_INVALID', messageKey: plan.blockers[0]?.messageKey }, 'guided-change').message; return; }
      await submit(change, plan, cancel);
    } catch (failure) { if (request === sequence) error = configurationErrorInfo(failure, 'guided-change').message; }
    finally { if (request === sequence) pending = false; }
  }
  function cancel() { review = null; choices = {}; error = ''; }
</script>

{#if records.length}<div class="intent-history" aria-busy={pending}>
  <div class="history-actions">{#if latest}<button type="button" disabled={disabled || pending || !!review} on:click={() => latest && void restore('undo', latest.recordId)}><Icon name="undo" size={15} />{$t('Undo last change')}</button>{/if}
    <details><summary>{$t('Recent changes')}</summary><div class="history-list">{#if redo}<button type="button" disabled={disabled || pending || !!review} on:click={() => redo && void restore('redo', redo.recordId)}><Icon name="redo" size={15} />{$t('Redo')}</button>{/if}{#each records as record (record.recordId)}<div><span><strong>{$t(intentEffectLabel('/' + record.subjectKey))}</strong><small>{$t(record.labelKey)}</small></span>{#if record.canUndo}<button type="button" disabled={disabled || pending || !!review} on:click={() => void restore('undo', record.recordId)}>{$t('Undo')}</button>{/if}</div>{/each}</div></details>
  </div>
  {#if review}<section class="history-review" aria-label={$t('Choose what to restore')}>
    <h4>{$t('Some settings changed later. Choose what to restore.')}</h4>
    {#if review.plan.stateRevision !== revision}<p role="status">{$t('The configuration changed. Your choices will be checked again.')}</p>{/if}
    {#each review.plan.historyConflicts as conflict (conflict.conflictId)}<fieldset><legend>{conflict.protected ? $t('Access protection') : conflict.semanticPaths.join(' · ')}</legend><div class="restore-options">
      <label><input type="radio" name={'restore-' + conflict.conflictId} checked={choices[conflict.conflictId] === 'keepCurrent'} on:change={() => choices = { ...choices, [conflict.conflictId]: 'keepCurrent' }} disabled={pending} /><span><strong>{$t('Keep current settings')}</strong><small>{$t(summary(conflict.current, conflict.protected))}</small></span></label>
      <label><input type="radio" name={'restore-' + conflict.conflictId} checked={choices[conflict.conflictId] === 'restore'} on:change={() => choices = { ...choices, [conflict.conflictId]: 'restore' }} disabled={pending} /><span><strong>{$t('Restore this change')}</strong><small>{$t(summary(conflict.restoring, conflict.protected))}</small></span></label>
    </div></fieldset>{/each}
    <div class="review-actions"><button type="button" disabled={pending} on:click={cancel}>{$t('Cancel')}</button><button type="button" class="primary" disabled={disabled || pending || review.plan.historyConflicts.some((conflict) => !choices[conflict.conflictId])} on:click={() => review && void restore(review.action, review.recordId, choices)}>{$t('Done')}</button></div>
  </section>{/if}
  {#if error}<p class="history-error" role="alert">{$t(error)}</p>{/if}
</div>{/if}

<style>
  .intent-history { min-width: 0; padding-bottom: 14px; }
  .history-actions { display: flex; flex-wrap: wrap; gap: 10px; align-items: flex-start; }
  button, summary { font-size: .8rem; box-shadow: none; white-space: normal; }
  summary { padding: 8px 4px; cursor: pointer; color: var(--ui-text-secondary); }
  .history-list { display: grid; gap: 6px; max-height: 260px; overflow-y: auto; padding: 8px 0; }
  .history-list > div { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; justify-content: space-between; }
  .history-list span { display: grid; gap: 3px; font-size: .8rem; }
  .history-review { display: grid; gap: 12px; margin-top: 12px; padding: 14px; border: 1px solid var(--ui-border-default); border-radius: var(--ui-radius-md); background: var(--ui-surface-2); }
  h4, p { margin: 0; font-size: .83rem; line-height: 1.5; }
  fieldset { min-width: 0; border: 1px solid var(--ui-border-default); border-radius: var(--ui-radius-sm); }
  legend { font-size: .8rem; }
  .restore-options { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 12px; }
  label { display: flex; align-items: flex-start; gap: 8px; min-width: 0; cursor: pointer; }
  label span { display: grid; gap: 4px; }
  strong { font-size: .82rem; }
  small { font-size: .76rem; color: var(--ui-text-secondary); }
  .review-actions { display: flex; flex-wrap: wrap; gap: 8px; justify-content: flex-end; }
  .history-error { color: var(--ui-danger); margin-top: 8px; }
  button, summary, span, small, legend, h4 { min-width: 0; max-width: 100%; overflow-wrap: anywhere; }
  @container (max-width: 600px) { .restore-options { grid-template-columns: 1fr; } }
  @container (max-width: 440px) { .review-actions button { flex: 1 1 100%; } }
</style>

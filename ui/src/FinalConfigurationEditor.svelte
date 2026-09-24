<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { t } from './i18n';
  import { coreAdmissionMessage, coreAssessmentMessage } from './errors';
  import type {
    ConfigurationWorkspaceView,
    FinalConflictResolution,
    FinalEditorSession,
    SemanticValue,
  } from './types';

  export let workspace: ConfigurationWorkspaceView;
  export let draft: FinalEditorSession | null = null;
  export let selectedPath = '';
  export let disabled = false;
  export let draftDirty = false;
  export let syntaxInvalid = false;
  export let editorHasErrors = false;

  const dispatch = createEventDispatcher<{
    focusPath: { path: string };
    resolveConflict: { conflictId: string; resolution: FinalConflictResolution };
    resolveDraftConflict: { conflictId: string; resolution: FinalConflictResolution };
    rebaseDraft: void;
    resumeDraft: void;
    discardDraft: void;
    navigateSurface: { surface: 'intent' | 'details' | 'sources' | 'configuration' | 'compatibility' };
  }>();

  let manualValue = '';
  let manualError = '';
  let manualConflictId = '';
  let manualOpen = false;

  const blockerMessages: Record<string, string> = {
    CONFIGURATION_INVALID: 'Fix the highlighted issue and save again.',
    CONFIG_INVALID: 'Fix the highlighted issue and save again.',
    CORE_INVALID: 'The current program rejected this candidate.',
    CORE_VALIDATION_EVIDENCE_STALE: 'This candidate will be checked again when you apply it.',
    CORE_PROFILE_MISMATCH: 'Review this candidate, then apply it again.',
    CORE_TARGET_CHANGED: 'Review this candidate for the current program.',
    SOURCE_VALUE_CONFLICT: 'Resolve the source conflict before continuing.',
    SOURCE_INVALID: 'Fix the invalid source before continuing.',
    SOURCE_UNAVAILABLE: 'Refresh or disable the unavailable source before continuing.',
    LAYER_OWNERSHIP_CONFLICT: 'Resolve the configuration ownership conflict before continuing.',
    FINAL_EDIT_CONFLICT: 'Resolve this edit conflict before continuing.',
    CONFIGURATION_IDENTITY_DUPLICATED: 'Remove the duplicate identity before continuing.',
  };

  $: changes = workspace.editor.changes ?? [];
  $: conflicts = workspace.editor.conflicts ?? [];
  $: unresolvedDraftConflicts = draft
    ? draft.conflicts.filter((item) => draft?.unresolvedConflictIds.includes(item.conflictId))
    : [];
  $: selectedDraftConflict = unresolvedDraftConflicts.find((item) => item.semanticPath === selectedPath)
    ?? (!selectedPath ? unresolvedDraftConflicts[0] : null)
    ?? null;
  $: selectedConflict = selectedDraftConflict ?? conflicts.find((item) => item.semanticPath === selectedPath)
    ?? (!selectedPath && !selectedDraftConflict ? conflicts[0] : null)
    ?? null;
  $: selectedChange = selectedPath
    ? changes.find((item) => item.semanticPath === selectedPath) ?? null
    : null;
  $: if (selectedConflict && manualConflictId !== selectedConflict.conflictId) {
    manualConflictId = selectedConflict.conflictId;
    manualOpen = false;
    manualValue = editableSemantic(selectedConflict.userValue);
    manualError = '';
  }
  $: effectiveEditStatus = workspace.editor.editStatus === 'conflict'
    || unresolvedDraftConflicts.length
    || draft?.rebaseRequired
    ? 'conflict'
    : draftDirty || workspace.editor.editStatus === 'modified'
      ? 'modified'
      : 'clean';
  $: effectiveCandidateStatus = draftDirty ? 'unsaved' : workspace.editor.candidateStatus;
  $: actionableBlockers = workspace.editor.blockers.filter((blocker) => ![
    'CONFIGURATION_CANDIDATE_UNSAVED',
    'CORE_VALIDATION_REQUIRED',
  ].includes(blocker.code));
  $: hasSaveBlocker = actionableBlockers.some((blocker) => blocker.blocks.includes('save'));
  $: visibleBlockers = (hasSaveBlocker
    ? actionableBlockers.filter((blocker) => blocker.blocks.includes('save'))
    : actionableBlockers
  ).filter((blocker, index, all) => all.findIndex((candidate) => (
    candidate.code === blocker.code && candidate.semanticPath === blocker.semanticPath
  )) === index).slice(0, 3);
  $: editorStatusLabel = effectiveEditStatus === 'conflict'
    ? 'Conflict'
    : effectiveCandidateStatus === 'invalid' || (editorHasErrors && effectiveCandidateStatus !== 'applied')
      ? 'Needs attention'
    : effectiveCandidateStatus === 'applied'
      ? 'Applied'
    : effectiveEditStatus === 'modified'
      ? 'Modified'
      : 'Clean';
  $: candidateStatusLabel = effectiveEditStatus === 'conflict'
    ? 'Resolve conflicts before continuing.'
    : syntaxInvalid
      ? 'Fix the highlighted configuration syntax.'
    : editorHasErrors && effectiveCandidateStatus !== 'applied'
      ? 'Fix the highlighted issue, then try again.'
    : effectiveCandidateStatus === 'invalid'
      ? 'Fix the highlighted issue, then try again.'
      : effectiveCandidateStatus === 'applied'
        ? 'This candidate is applied.'
        : 'Ready to apply.';

  function semanticPayload(value: SemanticValue): unknown {
    return value.state === 'present' ? value.value : undefined;
  }

  function displaySemantic(value: SemanticValue): string {
    if (value.state === 'missing') return $t('Not present');
    if (typeof value.value === 'string') return value.value;
    try { return JSON.stringify(value.value, null, 2); } catch { return String(value.value); }
  }

  function editableSemantic(value: SemanticValue): string {
    if (value.state === 'missing') return '';
    try { return JSON.stringify(value.value, null, 2) ?? 'null'; } catch { return 'null'; }
  }

  function display(value: unknown): string {
    if (value === undefined) return $t('Not present');
    if (value === null) return 'null';
    if (typeof value === 'string') return value;
    try { return JSON.stringify(value, null, 2); } catch { return String(value); }
  }

  function blockerMessage(messageKey: string): string {
    return coreAdmissionMessage(messageKey) ?? coreAssessmentMessage(messageKey) ?? blockerMessages[messageKey] ?? 'Review this issue before continuing.';
  }

  function recoverBlocker(blocker: ConfigurationWorkspaceView['editor']['blockers'][number]): void {
    if (blocker.recoveryAction === 'openCompatibility') {
      dispatch('navigateSurface', { surface: 'compatibility' });
      return;
    }
    if (blocker.recoveryAction === 'resolveSourceConflict') {
      dispatch('navigateSurface', { surface: 'sources' });
      return;
    }
    if (blocker.semanticPath) dispatch('focusPath', { path: blocker.semanticPath });
  }

  function resolveSelected(resolution: FinalConflictResolution): void {
    if (!selectedConflict) return;
    const detail = { conflictId: selectedConflict.conflictId, resolution };
    if (selectedDraftConflict) dispatch('resolveDraftConflict', detail);
    else dispatch('resolveConflict', detail);
  }

  function resolveManual(): void {
    if (!selectedConflict) return;
    try {
      const content = manualValue.trim();
      const value: SemanticValue = content
        ? { state: 'present', value: JSON.parse(content) }
        : { state: 'missing' };
      manualError = '';
      resolveSelected({ manualEdit: { value } });
    } catch {
      manualError = $t('Enter a valid JSON value for this path.');
    }
  }
</script>

<section class="final-editor-workspace" aria-labelledby="final-editor-title">
  <header class="workspace-heading">
    <div class="heading-copy">
      <p class="eyebrow">{$t('Configuration')}</p>
      <h2 id="final-editor-title">{$t('Final configuration')}</h2>
    </div>
    <div class:danger={effectiveEditStatus === 'conflict' || effectiveCandidateStatus === 'invalid' || (editorHasErrors && effectiveCandidateStatus !== 'applied')} class:modified={effectiveEditStatus === 'modified' && effectiveCandidateStatus !== 'invalid' && !editorHasErrors} class="editor-state" role="status" aria-live="polite">
      <strong>{$t(editorStatusLabel)}</strong>
      <span>{$t(candidateStatusLabel)}</span>
    </div>
  </header>

  {#if visibleBlockers.length > 0}
    <section class="editor-blockers" aria-label={$t('Configuration status')}>
      {#if editorStatusLabel !== 'Needs attention'}<strong class="blocker-heading">{$t('Needs attention')}</strong>{/if}
      {#each visibleBlockers as blocker (blocker.code + (blocker.semanticPath ?? ''))}
        {#if blocker.semanticPath || ['resolveSourceConflict', 'openCompatibility'].includes(blocker.recoveryAction)}
          <button type="button" on:click={() => recoverBlocker(blocker)}>
            <span aria-hidden="true">{blocker.blocks.includes('save') ? '!' : '→'}</span>
            <span><strong>{$t(blockerMessage(blocker.messageKey))}</strong>{#if blocker.semanticPath}<code>{blocker.semanticPath}</code>{/if}</span>
          </button>
        {:else}
          <div class="blocker-row">
            <span aria-hidden="true">{blocker.blocks.includes('save') ? '!' : '→'}</span>
            <span><strong>{$t(blockerMessage(blocker.messageKey))}</strong></span>
          </div>
        {/if}
        {#if blocker.details}
          <details class="blocker-details"><summary>{$t('Technical details')}</summary><pre>{blocker.details}</pre></details>
        {/if}
      {/each}
    </section>
  {/if}

  {#if draft?.rebaseRequired}
    <section class="draft-recovery" role="status">
      <div>
        <strong>{$t('Unfinished edit was kept safely.')}</strong>
        <span>{$t(draftDirty ? 'Repair the draft to merge it with the latest configuration.' : 'The latest configuration is shown. Resume the edit when you are ready.')}</span>
      </div>
      <div>
        {#if !draftDirty}<button type="button" on:click={() => dispatch('resumeDraft')} disabled={disabled}>{$t('Resume edit')}</button>{/if}
        <button type="button" on:click={() => dispatch('discardDraft')} disabled={disabled}>{$t('Discard')}</button>
      </div>
    </section>
  {/if}

  <div class="editor-slot"><slot name="editor" /></div>

  {#if selectedConflict || selectedChange || selectedDraftConflict}
    <aside class="path-inspector" aria-labelledby="path-inspector-title">
      <header>
        <div>
          <p class="eyebrow">{$t(selectedConflict || selectedDraftConflict ? 'Conflict' : 'Final edit')}</p>
          <h3 id="path-inspector-title"><code>{selectedConflict?.semanticPath ?? selectedChange?.semanticPath}</code></h3>
        </div>
        <button type="button" class="quiet-button" on:click={() => dispatch('focusPath', { path: selectedConflict?.semanticPath ?? selectedChange?.semanticPath ?? selectedDraftConflict?.semanticPath ?? '/' })}>{$t('Locate in editor')}</button>
      </header>

      {#if selectedConflict}
        <p class="inspector-summary">{$t('The upstream configuration and your edit changed this path. Choose the value to keep.')}</p>
        <div class="value-grid">
          <div><small>{$t('Updated configuration')}</small><pre>{displaySemantic(selectedConflict.upstreamValue)}</pre></div>
          <div><small>{$t('Your edit')}</small><pre>{displaySemantic(selectedConflict.userValue)}</pre></div>
        </div>
        <div class="action-row">
          <button type="button" on:click={() => resolveSelected('acceptUpstream')} disabled={disabled}>{$t('Accept updated')}</button>
          <button type="button" on:click={() => resolveSelected('keepMine')} disabled={disabled}>{$t('Keep mine')}</button>
          <button type="button" on:click={() => { manualOpen = true; }} disabled={disabled}>{$t('Merge manually')}</button>
        </div>
        {#if manualOpen}
        <label class="manual-merge"><span>{$t('Merge manually')}</span><textarea bind:value={manualValue} rows="4" spellcheck="false" disabled={disabled} aria-invalid={!!manualError}></textarea></label>
        {#if manualError}<p class="inline-error" role="alert">{manualError}</p>{/if}
        <div class="action-row"><button type="button" class="manual-button" on:click={resolveManual} disabled={disabled}>{$t('Use merged value')}</button><button type="button" on:click={() => { manualOpen = false; manualError = ''; manualValue = selectedConflict ? editableSemantic(selectedConflict.userValue) : ''; }} disabled={disabled}>{$t('Cancel')}</button></div>
        {/if}
        <details><summary>{$t('Technical details')}</summary><p><code>{selectedConflict.kind}</code></p><pre>{display(semanticPayload(selectedConflict.baseValue))}</pre></details>
      {:else if selectedChange}
        <p class="inspector-summary">{$t('This path differs from the current upstream configuration.')}</p>
        <div class="value-grid">
          <div><small>{$t('Upstream value')}</small><pre>{displaySemantic(selectedChange.upstreamValue)}</pre></div>
          <div><small>{$t('Final value')}</small><pre>{displaySemantic(selectedChange.finalValue)}</pre></div>
        </div>
      {/if}

    </aside>
  {/if}

  <div class="editor-actions"><slot name="actions" /></div>
</section>

<style>
  .final-editor-workspace { display: grid; gap: 10px; min-width: 0; }
  .workspace-heading, .path-inspector > header { display: flex; min-width: 0; align-items: flex-start; justify-content: space-between; gap: 12px; flex-wrap: wrap; }
  .heading-copy, .path-inspector header > div { min-width: 0; }.eyebrow { margin: 0; color: var(--ui-text-tertiary, #6b7280); font-size: .7rem; letter-spacing: .04em; text-transform: uppercase; }.heading-copy h2, .path-inspector h3 { min-width: 0; margin: 2px 0 0; overflow-wrap: anywhere; }.heading-copy h2 { font-size: 1.12rem; }
  .editor-state { display: grid; min-width: min(12rem, 100%); max-width: 100%; gap: 2px; padding: 7px 10px; border: 1px solid var(--ui-border-default); border-radius: 8px; background: var(--ui-surface-2); }.editor-state strong, .editor-state span { min-width: 0; overflow-wrap: anywhere; }.editor-state strong { color: var(--ui-success); font-size: .8rem; }.editor-state.modified strong { color: var(--ui-brand); }.editor-state.danger strong { color: var(--ui-warning); }.editor-state span { color: var(--ui-text-secondary); font-size: .72rem; }
  .editor-blockers { display: grid; gap: 6px; padding: 9px 10px; border: 1px solid color-mix(in srgb, var(--ui-warning) 38%, var(--ui-border-default)); border-radius: 8px; background: color-mix(in srgb, var(--ui-warning-soft) 72%, transparent); }.blocker-heading { color: var(--ui-warning); font-size: .78rem; }.editor-blockers button, .blocker-row { display: flex; min-width: 0; max-width: 100%; gap: 7px; align-items: baseline; padding: 3px; color: inherit; text-align: left; }.editor-blockers button { border: 0; background: transparent; }.editor-blockers button span:last-child, .blocker-row span:last-child { display: flex; min-width: 0; flex-wrap: wrap; gap: 4px 8px; }.editor-blockers strong, .editor-blockers code { min-width: 0; overflow-wrap: anywhere; }
  .draft-recovery { display: flex; min-width: 0; align-items: center; justify-content: space-between; gap: 10px; padding: 9px 10px; border: 1px solid color-mix(in srgb, var(--ui-warning) 28%, var(--ui-border-default)); border-radius: 8px; background: var(--ui-surface-2); }.draft-recovery > div { display: flex; min-width: 0; gap: 6px; flex-wrap: wrap; }.draft-recovery > div:first-child { display: grid; }.draft-recovery strong, .draft-recovery span { overflow-wrap: anywhere; }.draft-recovery span { color: var(--ui-text-secondary); font-size: .74rem; }.draft-recovery button { min-height: 32px; min-width: 0; max-width: 100%; border: 1px solid var(--ui-border-default); border-radius: 6px; background: transparent; padding: 5px 8px; color: inherit; white-space: normal; overflow-wrap: anywhere; }
  .editor-slot, .editor-actions { min-width: 0; overflow: hidden; }.path-inspector { display: grid; gap: 9px; min-width: 0; max-width: 100%; padding: 11px; border: 1px solid var(--ui-border-default); border-radius: 8px; background: var(--ui-surface-2); }.path-inspector h3 code { white-space: pre-wrap; overflow-wrap: anywhere; }.inspector-summary { margin: 0; color: var(--ui-text-secondary); font-size: .76rem; overflow-wrap: anywhere; }
  .value-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 7px; min-width: 0; }.value-grid > div { min-width: 0; }.value-grid small { color: var(--ui-text-tertiary); font-size: .68rem; }.value-grid pre, details pre { max-height: 150px; margin: 3px 0 0; overflow: auto; border: 1px solid var(--ui-border-subtle); border-radius: 5px; background: var(--ui-input); padding: 6px; color: inherit; font: .7rem/1.35 ui-monospace, SFMono-Regular, Consolas, monospace; white-space: pre-wrap; overflow-wrap: anywhere; }
  .action-row { display: flex; min-width: 0; flex-wrap: wrap; gap: 6px; }.quiet-button, .action-row button, .manual-button { min-width: 0; max-width: 100%; min-height: 32px; border: 1px solid var(--ui-border-default); border-radius: 6px; background: transparent; padding: 5px 8px; color: inherit; font-size: .74rem; white-space: normal; overflow-wrap: anywhere; }.manual-merge { display: grid; min-width: 0; gap: 4px; color: var(--ui-text-secondary); font-size: .72rem; }.manual-merge textarea { box-sizing: border-box; width: 100%; min-width: 0; max-width: 100%; resize: vertical; border: 1px solid var(--ui-border-default); border-radius: 6px; background: var(--ui-input); color: inherit; padding: 7px; font: .72rem/1.35 ui-monospace, SFMono-Regular, Consolas, monospace; }.inline-error { margin: 0; color: var(--ui-danger); } details { min-width: 0; color: var(--ui-text-secondary); font-size: .72rem; } details p { overflow-wrap: anywhere; }
  @media (max-width: 680px) { .editor-state { width: 100%; }.value-grid { grid-template-columns: 1fr; } }
  @media (max-width: 520px) { .workspace-heading, .path-inspector > header, .draft-recovery { align-items: stretch; flex-direction: column; }.quiet-button, .action-row button, .manual-button, .draft-recovery button { width: 100%; } }
</style>

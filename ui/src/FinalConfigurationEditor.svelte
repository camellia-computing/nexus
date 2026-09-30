<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { t } from './i18n';
  import { coreAdmissionMessage, coreAssessmentMessage } from './errors';
  import type {
    ConfigurationWorkspaceView,
    FinalEditorSession,
    FinalChangeProjection,
    SemanticValue,
  } from './types';

  export let workspace: ConfigurationWorkspaceView;
  export let draft: FinalEditorSession | null = null;
  export let selectedPath = '';
  export let selectedConflictId = '';
  export let disabled = false;
  export let draftDirty = false;
  export let syntaxInvalid = false;
  export let editorHasErrors = false;
  export let identityChecking = false;
  export let identityCheckFailed = false;

  const dispatch = createEventDispatcher<{
    focusPath: { path: string };
    selectConflict: { conflictId: string; path: string };
    rebaseDraft: void;
    resumeDraft: void;
    discardDraft: void;
    navigateSurface: { surface: 'intent' | 'details' | 'sources' | 'configuration' | 'compatibility'; sourceId?: string };
    adoptUpstream: FinalChangeProjection;
  }>();

  const blockerMessages: Record<string, string> = {
    CONFIGURATION_INVALID: 'Fix the highlighted issue and save again.',
    CONFIG_INVALID: 'Fix the highlighted issue and save again.',
    CORE_INVALID: 'The program could not use these changes.',
    CORE_VALIDATION_EVIDENCE_STALE: 'Your changes will be checked when you apply them.',
    CORE_PROFILE_MISMATCH: 'Apply again to check these changes with the current program.',
    CORE_TARGET_CHANGED: 'Review these changes for the current program.',
    SOURCE_VALUE_CONFLICT: 'Resolve the source conflict before continuing.',
    SOURCE_INVALID: 'Fix the invalid source before continuing.',
    SOURCE_UNAVAILABLE: 'Refresh or disable the unavailable source before continuing.',
    LAYER_OWNERSHIP_CONFLICT: 'Choose which setting to use before continuing.',
    FINAL_EDIT_CONFLICT: 'Resolve this edit conflict before continuing.',
    CONFIGURATION_IDENTITY_DUPLICATED: 'Remove the duplicate identity before continuing.',
  };

  $: changes = workspace.editor.changes ?? [];
  $: conflicts = workspace.editor.conflicts ?? [];
  $: activeConflictIndex = Math.max(0, conflicts.findIndex((item) => item.conflictId === selectedConflictId));
  $: selectedChange = selectedPath
    ? changes.find((item) => item.semanticPath === selectedPath) ?? null
    : null;
  $: selectedChangeIndex = selectedChange ? changes.findIndex((item) => item.editId === selectedChange.editId) : -1;
  $: repairingCandidateIssue = draftDirty && !conflicts.length
    && workspace.editor.blockers.some((blocker) => blocker.blocks.includes('save') && blocker.recoveryAction === 'reviewCandidate')
    && !workspace.editor.blockers.some((blocker) => blocker.blocks.includes('save') && blocker.recoveryAction !== 'reviewCandidate');
  $: effectiveEditStatus = (workspace.editor.editStatus === 'conflict' && !repairingCandidateIssue)
    || conflicts.length > 0
    || !!draft?.rebaseRequired
    ? 'conflict'
    : draftDirty || workspace.editor.editStatus === 'modified'
      ? 'modified'
      : 'clean';
  $: effectiveCandidateStatus = draftDirty ? 'unsaved' : workspace.editor.candidateStatus;
  $: actionableBlockers = workspace.editor.blockers.filter((blocker) => ![
    'CONFIGURATION_CANDIDATE_UNSAVED',
    'CORE_VALIDATION_REQUIRED',
  ].includes(blocker.code) && !(repairingCandidateIssue && blocker.blocks.includes('save') && blocker.recoveryAction === 'reviewCandidate'));
  $: hasSaveBlocker = actionableBlockers.some((blocker) => blocker.blocks.includes('save'));
  $: visibleBlockers = (hasSaveBlocker
    ? actionableBlockers.filter((blocker) => blocker.blocks.includes('save'))
    : actionableBlockers
  ).filter((blocker) => !(conflicts.length && blocker.code === 'FINAL_EDIT_CONFLICT'))
    .filter((blocker, index, all) => all.findIndex((candidate) => (
    candidate.code === blocker.code && candidate.semanticPath === blocker.semanticPath
  )) === index).slice(0, 1);
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
      : identityCheckFailed
        ? 'Check the program in Compatibility before applying.'
      : identityChecking
        ? 'Checking the current program before applying.'
      : effectiveCandidateStatus === 'applied'
        ? 'Configuration is up to date.'
        : 'Ready to apply.';

  function displaySemantic(value: SemanticValue): string {
    if (value.state === 'missing') return $t('Not present');
    if (typeof value.value === 'string') return value.value;
    try { return JSON.stringify(value.value, null, 2); } catch { return String(value.value); }
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
      dispatch('navigateSurface', { surface: 'sources', sourceId: blocker.scope.ownerId });
      return;
    }
    if (blocker.semanticPath) dispatch('focusPath', { path: blocker.semanticPath });
  }

  function navigateConflict(offset: number): void {
    if (!conflicts.length) return;
    const index = (activeConflictIndex + offset + conflicts.length) % conflicts.length;
    const conflict = conflicts[index];
    dispatch('selectConflict', { conflictId: conflict.conflictId, path: conflict.semanticPath });
  }

  function navigateChange(offset: number): void {
    if (!changes.length || selectedChangeIndex < 0) return;
    const change = changes[(selectedChangeIndex + offset + changes.length) % changes.length];
    dispatch('focusPath', { path: change.semanticPath });
  }
</script>

<section class="final-editor-workspace" aria-labelledby="final-editor-title">
  <header class="workspace-heading">
    <div class="heading-copy">
      <p class="eyebrow">{$t('Configuration')}</p>
      <h2 id="final-editor-title">{$t('Final configuration')}</h2>
    </div>
    {#if !conflicts.length}<div class:danger={effectiveEditStatus === 'conflict' || effectiveCandidateStatus === 'invalid' || (editorHasErrors && effectiveCandidateStatus !== 'applied')} class:modified={effectiveEditStatus === 'modified' && effectiveCandidateStatus !== 'invalid' && !editorHasErrors} class="editor-state" role="status" aria-live="polite">
      <strong>{$t(editorStatusLabel)}</strong>
      <span>{$t(candidateStatusLabel)}</span>
    </div>{/if}
  </header>

  {#if conflicts.length}
    <div class="merge-summary" role="status" aria-live="polite">
      <span><strong>{conflicts.length} {$t(conflicts.length === 1 ? 'setting needs a choice' : 'settings need a choice')}</strong></span>
      <div class="merge-navigation" aria-label={$t('Conflicts')}>
        <button type="button" on:click={() => navigateConflict(-1)} disabled={conflicts.length < 2}>{$t('Previous')}</button>
        <span>{activeConflictIndex + 1} / {conflicts.length}</span>
        <button type="button" on:click={() => navigateConflict(1)} disabled={conflicts.length < 2}>{$t('Next')}</button>
      </div>
    </div>
  {/if}

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
        {#if blocker.details}<details class="blocker-details"><summary>{$t('Information for support')}</summary><p>{$t('Share this only if support asks for it.')}</p><pre>{blocker.details}</pre></details>{/if}
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

  {#if selectedChange && !conflicts.some((item) => item.semanticPath === selectedPath)}
    <aside class="path-inspector" aria-labelledby="path-inspector-title">
      <header>
        <div>
          <p class="eyebrow">{$t('Your change')}</p>
          <h3 id="path-inspector-title"><code>{selectedChange.semanticPath}</code></h3>
        </div>
        <div class="change-navigation">
          {#if changes.length > 1}
            <button type="button" class="quiet-button" on:click={() => navigateChange(-1)} aria-label={$t('Previous change')}>←</button>
            <span>{selectedChangeIndex + 1} / {changes.length}</span>
            <button type="button" class="quiet-button" on:click={() => navigateChange(1)} aria-label={$t('Next change')}>→</button>
          {/if}
          <button type="button" class="quiet-button" on:click={() => dispatch('focusPath', { path: selectedChange.semanticPath })}>{$t('Locate in editor')}</button>
        </div>
      </header>

        <p class="inspector-summary">{$t('This path differs from the current upstream configuration.')}</p>
        <div class="value-grid">
          <div><small>{$t('Upstream value')}</small><pre>{displaySemantic(selectedChange.upstreamValue)}</pre></div>
          <div><small>{$t('Final value')}</small><pre>{displaySemantic(selectedChange.finalValue)}</pre></div>
        </div>
        <button type="button" class="quiet-button" on:click={() => dispatch('adoptUpstream', selectedChange)} disabled={disabled || draftDirty || !!draft?.rebaseRequired}>{$t('Use latest setting')}</button>

    </aside>
  {/if}

  <div class="editor-actions"><slot name="actions" /></div>
</section>

<style>
  .final-editor-workspace { display: grid; gap: 10px; min-width: 0; }
  .workspace-heading, .path-inspector > header { display: flex; min-width: 0; align-items: flex-start; justify-content: space-between; gap: 12px; flex-wrap: wrap; }
  .heading-copy, .path-inspector header > div { min-width: 0; }.eyebrow { margin: 0; color: var(--ui-text-tertiary, #6b7280); font-size: .7rem; letter-spacing: .04em; text-transform: uppercase; }.heading-copy h2, .path-inspector h3 { min-width: 0; margin: 2px 0 0; overflow-wrap: anywhere; }.heading-copy h2 { font-size: 1.12rem; }
  .editor-state { display: grid; min-width: min(12rem, 100%); max-width: 100%; gap: 2px; padding: 7px 10px; border: 1px solid var(--ui-border-default); border-radius: 8px; background: var(--ui-surface-2); }.editor-state strong, .editor-state span { min-width: 0; overflow-wrap: anywhere; }.editor-state strong { color: var(--ui-success); font-size: .8rem; }.editor-state.modified strong { color: var(--ui-brand); }.editor-state.danger strong { color: var(--ui-warning); }.editor-state span { color: var(--ui-text-secondary); font-size: .72rem; }
  .merge-summary { display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap; gap: 8px; min-width: 0; padding: 7px 10px; border-inline-start: 3px solid var(--ui-warning); border-radius: 6px; background: color-mix(in srgb, var(--ui-warning-soft) 55%, var(--ui-surface-2)); color: var(--ui-text-primary); font-size: .78rem; }
  .merge-summary span { min-width: 0; overflow-wrap: anywhere; }
  .merge-navigation { display: flex; align-items: center; gap: 5px; flex-wrap: wrap; }
  .merge-navigation button { width: auto; min-height: 28px; padding: 3px 7px; box-shadow: none; background: transparent; border: 1px solid var(--ui-border-default); border-radius: 5px; color: inherit; white-space: normal; }
  .editor-blockers { display: grid; justify-items: start; gap: 4px; min-width: 0; padding: 7px 10px; border-inline-start: 3px solid var(--ui-warning); border-radius: 6px; background: color-mix(in srgb, var(--ui-warning-soft) 45%, var(--ui-surface-2)); }.blocker-heading { color: var(--ui-warning); font-size: .78rem; }.editor-blockers button, .blocker-row { display: inline-flex; width: auto; min-height: 0; min-width: 0; max-width: 100%; gap: 7px; align-items: baseline; justify-content: flex-start; padding: 2px; box-shadow: none; border-radius: 4px; color: inherit; text-align: left; white-space: normal; }.editor-blockers button { border: 0; background: transparent; }.editor-blockers button span:last-child, .blocker-row span:last-child { display: flex; min-width: 0; flex-wrap: wrap; gap: 4px 8px; }.editor-blockers strong, .editor-blockers code { min-width: 0; overflow-wrap: anywhere; }
  .draft-recovery { display: flex; min-width: 0; align-items: center; justify-content: space-between; gap: 10px; padding: 9px 10px; border: 1px solid color-mix(in srgb, var(--ui-warning) 28%, var(--ui-border-default)); border-radius: 8px; background: var(--ui-surface-2); }.draft-recovery > div { display: flex; min-width: 0; gap: 6px; flex-wrap: wrap; }.draft-recovery > div:first-child { display: grid; }.draft-recovery strong, .draft-recovery span { overflow-wrap: anywhere; }.draft-recovery span { color: var(--ui-text-secondary); font-size: .74rem; }.draft-recovery button { min-height: 32px; min-width: 0; max-width: 100%; border: 1px solid var(--ui-border-default); border-radius: 6px; background: transparent; padding: 5px 8px; color: inherit; white-space: normal; overflow-wrap: anywhere; }
  .editor-slot, .editor-actions { min-width: 0; overflow: hidden; }.path-inspector { display: grid; gap: 9px; min-width: 0; max-width: 100%; padding: 11px; border: 1px solid var(--ui-border-default); border-radius: 8px; background: var(--ui-surface-2); }.path-inspector h3 code { white-space: pre-wrap; overflow-wrap: anywhere; }.inspector-summary { margin: 0; color: var(--ui-text-secondary); font-size: .76rem; overflow-wrap: anywhere; }
  .value-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 7px; min-width: 0; }.value-grid > div { min-width: 0; }.value-grid small { color: var(--ui-text-tertiary); font-size: .68rem; }.value-grid pre, details pre { max-height: 150px; margin: 3px 0 0; overflow: auto; border: 1px solid var(--ui-border-subtle); border-radius: 5px; background: var(--ui-input); padding: 6px; color: inherit; font: .7rem/1.35 ui-monospace, SFMono-Regular, Consolas, monospace; white-space: pre-wrap; overflow-wrap: anywhere; }
  .quiet-button { min-width: 0; max-width: 100%; min-height: 32px; border: 1px solid var(--ui-border-default); border-radius: 6px; background: transparent; padding: 5px 8px; color: inherit; font-size: .74rem; white-space: normal; overflow-wrap: anywhere; } details { min-width: 0; color: var(--ui-text-secondary); font-size: .72rem; } details p { overflow-wrap: anywhere; }
  .change-navigation { display: flex; align-items: center; flex-wrap: wrap; gap: 5px; min-width: 0; }.change-navigation span { color: var(--ui-text-secondary); font-size: .72rem; }.change-navigation button[aria-label] { min-width: 32px; padding: 4px; }
  @media (max-width: 680px) { .editor-state { width: 100%; }.value-grid { grid-template-columns: 1fr; } }
  @media (max-width: 520px) { .workspace-heading, .path-inspector > header, .draft-recovery { align-items: stretch; flex-direction: column; }.quiet-button, .draft-recovery button { width: 100%; } }
</style>

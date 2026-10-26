<script context="module" lang="ts">
  import { api } from './api';
  import { createIntentContextStore } from './intentContextStore';
  const contexts = createIntentContextStore(api.getConfigurationIntentContext);
</script>

<script lang="ts">
  import { onDestroy, tick } from 'svelte';
  import { t } from './i18n';
  import { configurationErrorInfo } from './errors';
  import { intentContextSummary, intentNodeLabel, intentNodeQualifier } from './intentPresentation';
  import NativeIntentField from './NativeIntentField.svelte';
  import NativeIntentObjects from './NativeIntentObjects.svelte';
  import NativeIntentHistory from './NativeIntentHistory.svelte';
  import Icon from './lib/components/Icon.svelte';
  import type { ConfigurationStateView, ConfigurationIntentAction, IntentChangePlan, IntentContext, IntentNode, IntentSubmission } from './types';
  export let programId: string;
  export let state: ConfigurationStateView;
  export let disabled = false;
  export let onChange: IntentSubmission;
  export let onPreview: (change: ConfigurationIntentAction) => Promise<IntentChangePlan>;
  export let onOpenFinal: () => void;
  export let onOpenDetails: () => void;
  export let onEditingChange: (editing: boolean) => void;
  let root: IntentContext | null = null;
  let context: IntentContext | null = null;
  let nodes = new Map<string, IntentNode>();
  let selectedId = '';
  let rootKey = '';
  let contextKey = '';
  let rootSequence = 0;
  let contextSequence = 0;
  let loading = false;
  let error = '';
  let editingId = '';
  let childChoice = '';
  let content: HTMLElement;
  $: revisionKey = JSON.stringify([programId, state.stateRevision, state.compatibilityProfile.profileHash]);
  $: if (rootKey !== revisionKey) { rootKey = revisionKey; void loadRoot(); }
  $: selectionKey = revisionKey + ':' + selectedId;
  $: if (selectedId && contextKey !== selectionKey) { contextKey = selectionKey; void loadSelected(); }
  $: chapters = root?.children ?? [];
  $: common = context?.children.filter((node) => node.settingId && node.disposition === 'guided') ?? [];
  $: further = context?.children.filter((node) => !common.some((field) => field.nodeId === node.nodeId)) ?? [];
  $: chapterId = context?.node ? chapters.find((node) => node.pattern[0] === context?.node?.pattern[0])?.nodeId ?? selectedId : selectedId;
  $: breadcrumbs = ancestry(context?.node);
  $: onEditingChange(!!editingId);
  onDestroy(() => { rootSequence += 1; contextSequence += 1; onEditingChange(false); });

  function ancestry(node: IntentNode | undefined) {
    const result: IntentNode[] = [];
    const visited = new Set<string>();
    while (node && !visited.has(node.nodeId)) {
      visited.add(node.nodeId); result.unshift(node);
      node = node.parentId ? nodes.get(node.parentId) : undefined;
    }
    return result;
  }
  function remember(result: IntentContext) {
    nodes = new Map(nodes);
    if (result.node) nodes.set(result.node.nodeId, result.node);
    for (const child of result.children) nodes.set(child.nodeId, child);
    for (const related of result.relatedNodes) nodes.set(related.nodeId, related);
  }
  function load(nodeId?: string) {
    return contexts.load(programId, state.stateRevision, state.compatibilityProfile.profileHash, nodeId);
  }
  async function loadRoot() {
    const sequence = ++rootSequence;
    try {
      const next = await load();
      if (sequence !== rootSequence) return;
      const changedCatalog = root?.catalogHash !== next.catalogHash;
      root = next;
      if (changedCatalog) nodes = new Map();
      remember(next);
      if (!selectedId || changedCatalog) selectedId = next.children.find((node) => ['log', 'log-level'].includes(node.nativeName))?.nodeId ?? next.children[0]?.nodeId ?? '';
    } catch (failure) { if (sequence === rootSequence) error = configurationErrorInfo(failure, 'guided-change').message; }
  }
  async function loadSelected() {
    const sequence = ++contextSequence;
    const target = selectedId;
    loading = true; error = '';
    try {
      const next = await load(target);
      if (sequence !== contextSequence || target !== selectedId) return;
      remember(next); context = next;
    } catch (failure) { if (sequence === contextSequence) error = configurationErrorInfo(failure, 'guided-change').message; }
    finally { if (sequence === contextSequence) loading = false; }
  }
  async function navigate(id: string, focus = false) {
    if (editingId || !id || id === selectedId) return;
    selectedId = id; childChoice = '';
    await tick();
    if (focus) content?.focus();
  }
  function editing(id: string, active: boolean) { if (active) editingId = id; else if (editingId === id) editingId = ''; }
  function label(node: IntentNode, localize: (message: string) => string) { return localize(node.nativeName === '*' ? 'Entry options' : intentNodeLabel(node)); }
</script>

<section class="native-intent-workspace" aria-label={$t('Intent')}>
  <NativeIntentHistory records={state.intentHistory} revision={state.stateRevision} disabled={disabled || !!editingId && editingId !== 'history'} submit={onChange} preview={onPreview} onEditingChange={(active) => editing('history', active)} />
  <div class="native-layout" class:initializing={!chapters.length}>
    {#if chapters.length}
    <nav class="chapter-navigation" aria-label={$t('Configuration chapters')}>
      {#each chapters as chapter (chapter.nodeId)}<button type="button" aria-current={chapter.nodeId === chapterId ? 'page' : undefined} disabled={!!editingId} on:click={() => void navigate(chapter.nodeId, true)}><span>{$t(intentNodeLabel(chapter))}</span>{#if chapter.nodeId === chapterId}<Icon name="chevron" size={14} />{/if}</button>{/each}
    </nav>
    {/if}
    <div class="native-content" bind:this={content} tabindex="-1">
      {#if chapters.length}
      <label class="chapter-selector"><span>{$t('Configuration chapter')}</span><select aria-label={$t('Configuration chapter')} value={chapterId} disabled={!!editingId} on:change={(event) => void navigate(event.currentTarget.value)}>{#each chapters as chapter (chapter.nodeId)}<option value={chapter.nodeId}>{$t(intentNodeLabel(chapter))}</option>{/each}</select></label>
      {/if}
      {#if breadcrumbs.length > 1}<nav class="breadcrumbs" aria-label={$t('Configuration location')}>{#each breadcrumbs as node, index (node.nodeId)}{#if index}<Icon name="chevron" size={12} />{/if}<button type="button" disabled={!!editingId || node.nodeId === selectedId} on:click={() => void navigate(node.nodeId)}>{label(node, $t)}</button>{/each}</nav>{/if}
      {#if error}<p class="context-error" role="alert">{$t(error)} <button type="button" on:click={() => { contexts.clear(); void loadRoot(); if (selectedId) void loadSelected(); }}>{$t('Retry')}</button></p>{/if}
      {#if context?.node?.nodeId === selectedId}
        <section class="native-chapter" aria-labelledby="native-chapter-heading">
          <header class="chapter-heading"><h3 id="native-chapter-heading">{label(context.node, $t)}</h3>{#if intentNodeQualifier(context.node)}<span>{intentNodeQualifier(context.node)}</span>{/if}{#if loading}<small role="status">{$t('Updating')}…</small>{/if}</header>
          {#if context.node.settingId || context.node.control || !context.children.length && !context.objectEditors.length}<NativeIntentField node={context.node} revision={state.stateRevision} disabled={disabled || loading} load={load} submit={onChange} preview={onPreview} openFinal={onOpenFinal} openDetails={onOpenDetails} openRelated={(id) => void navigate(id, true)} onEditingChange={(active) => editing(context?.node?.nodeId ?? 'field', active)} />{/if}
          {#if context.values.length && !context.node.control && !context.node.settingId}<p class="chapter-summary">{intentContextSummary(context.values[0], $t)}</p>{/if}
          {#each context.objectEditors as editor (editor.nodeId + ':' + editor.descriptor.kind)}<NativeIntentObjects {editor} targets={context.targets} program={state.kind} disabled={disabled || loading} blocked={!!editingId && editingId !== editor.nodeId + ':' + editor.descriptor.kind} submit={onChange} preview={onPreview} openFinal={onOpenFinal} onEditingChange={(active) => editing(editor.nodeId + ':' + editor.descriptor.kind, active)} />{/each}
          {#if common.length}<section class="common-fields" aria-label={$t('Common settings')}>{#each common as field (field.nodeId)}<NativeIntentField node={field} revision={state.stateRevision} disabled={disabled || loading || !!editingId && editingId !== field.nodeId} load={load} submit={onChange} preview={onPreview} openFinal={onOpenFinal} openDetails={onOpenDetails} openRelated={(id) => void navigate(id, true)} onEditingChange={(active) => editing(field.nodeId, active)} />{/each}</section>{/if}
          {#if further.length}<details class="native-options"><summary>{$t('More options')}</summary>
            {#if further.length > 24}<label class="child-choice"><span>{$t('Choose a setting')}</span><select aria-label={$t('Choose a setting')} bind:value={childChoice} disabled={!!editingId}><option value="">{$t('Select a value')}</option>{#each further as child (child.nodeId)}<option value={child.nodeId}>{label(child, $t)}{intentNodeQualifier(child) ? ' · ' + intentNodeQualifier(child) : ''}</option>{/each}</select></label><button type="button" disabled={!childChoice || !!editingId} on:click={() => void navigate(childChoice, true)}>{$t('Open setting')}</button>
            {:else}<div class="native-children">{#each further as child (child.nodeId)}<button type="button" disabled={!!editingId} on:click={() => void navigate(child.nodeId, true)}><span>{label(child, $t)}{#if intentNodeQualifier(child)}<small>{intentNodeQualifier(child)}</small>{/if}</span><Icon name="chevron" size={14} /></button>{/each}</div>{/if}
          </details>{/if}
        </section>
      {:else if !error}<p class="context-loading" role="status">{$t('Loading settings')}…</p>{/if}
    </div>
  </div>
</section>

<style>
  .native-intent-workspace { container-type: inline-size; min-width: 0; }
  .native-layout { display: grid; grid-template-columns: minmax(160px, 21%) minmax(0, 1fr); gap: 18px; align-items: start; }
  .native-layout.initializing { grid-template-columns: minmax(0, 1fr); }
  .chapter-navigation { display: grid; gap: 3px; border-right: 1px solid var(--ui-divider); padding-right: 12px; max-height: 580px; overflow-y: auto; }
  .chapter-navigation button { display: flex; justify-content: space-between; align-items: center; gap: 6px; width: 100%; text-align: left; font-size: .8rem; padding: 9px 10px; border-color: transparent; background: transparent; box-shadow: none; }
  .chapter-navigation button[aria-current='page'] { color: var(--ui-text-link); background: var(--ui-brand-soft); border-color: var(--ui-border-default); font-weight: var(--ui-weight-semibold); }
  .native-content { min-width: 0; }
  .chapter-selector { display: none; gap: 6px; font-size: .8rem; margin-bottom: 14px; }
  .native-chapter { display: grid; min-width: 0; gap: 14px; padding: 18px; border: 1px solid var(--ui-border-default); border-radius: var(--ui-radius-md); background: var(--ui-surface-1); }
  .chapter-heading { display: flex; flex-wrap: wrap; align-items: center; gap: 7px 12px; min-width: 0; }
  h3 { margin: 0; font-size: .98rem; font-weight: var(--ui-weight-semibold); }
  .chapter-heading span, .chapter-heading small { color: var(--ui-text-secondary); font-size: .77rem; }
  .chapter-summary { margin: 0; color: var(--ui-text-secondary); font-size: .8rem; }
  .common-fields { display: grid; min-width: 0; }
  .breadcrumbs { display: flex; flex-wrap: wrap; gap: 5px; align-items: center; margin-bottom: 12px; }
  .breadcrumbs button { font-size: .78rem; border: 0; background: transparent; padding: 3px; color: var(--ui-text-link); box-shadow: none; }
  .breadcrumbs button:disabled { opacity: 1; color: var(--ui-text-secondary); }
  .native-options { min-width: 0; border-top: 1px solid var(--ui-divider); padding-top: 12px; }
  summary { font-size: .8rem; color: var(--ui-text-secondary); cursor: pointer; }
  .native-children { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 6px 14px; margin-top: 12px; }
  .native-children button { display: flex; gap: 8px; justify-content: space-between; align-items: center; text-align: left; padding: 9px; font-size: .8rem; background: var(--ui-surface-2); box-shadow: none; }
  .native-children button > span { display: grid; gap: 3px; }
  .native-children small { font-size: .74rem; color: var(--ui-text-secondary); }
  .child-choice { display: grid; gap: 6px; padding: 12px 0; font-size: .8rem; }
  select { min-width: 0; max-width: 100%; width: 100%; }
  .context-error, .context-loading { font-size: .82rem; line-height: 1.5; }
  .context-error { color: var(--ui-danger); }
  button, span, summary, h3, p, label { min-width: 0; max-width: 100%; white-space: normal; overflow-wrap: anywhere; }
  @container (max-width: 959px) { .native-layout { grid-template-columns: 1fr; } .chapter-navigation { display: none; } .chapter-selector { display: grid; } }
  @container (max-width: 600px) { .native-children { grid-template-columns: 1fr; } .native-chapter { padding: 14px; } }
  @container (max-width: 440px) { .native-chapter { padding: 12px; } .native-options > button { width: 100%; } }
</style>

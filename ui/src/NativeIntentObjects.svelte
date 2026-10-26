<script lang="ts">
  import { tick } from 'svelte';
  import { t } from './i18n';
  import { intentOptionMessage } from './intentPresentation';
  import IntentObjectForm from './IntentObjectForm.svelte';
  import Icon from './lib/components/Icon.svelte';
  import type { ConfigurationIntentAction, IntentChangePlan, IntentObjectContext, IntentObjectOperation, IntentObjectProjection, IntentTarget, IntentSubmission, ProgramKind } from './types';
  export let editor: IntentObjectContext;
  export let targets: IntentTarget[];
  export let program: ProgramKind;
  export let disabled = false;
  export let blocked = false;
  export let submit: IntentSubmission;
  export let preview: (change: ConfigurationIntentAction) => Promise<IntentChangePlan>;
  export let openFinal: () => void;
  export let onEditingChange: (editing: boolean) => void;
  let selection: { object?: IntentObjectProjection } | null = null;
  let opener: HTMLButtonElement | null = null;
  let section: HTMLElement;
  let chosenId = '';
  let following = false;
  $: descriptor = editor.descriptor;
  $: chosen = editor.objects.find((object) => object.objectId === chosenId);
  function label(object: IntentObjectProjection, localize: (key: string) => string) { return object.label.startsWith('nexus-') ? localize(descriptor.label) : object.label; }
  function summary(object: IntentObjectProjection, localize: (key: string) => string) {
    return ['protocol', 'port', 'server', 'access', 'target'].flatMap((key) => {
      const value = object.values[key];
      return typeof value === 'string' || typeof value === 'number' ? [key === 'access' || key === 'protocol' ? localize(intentOptionMessage(String(value))) : String(value)] : [];
    }).join(' · ');
  }
  function native(operation: IntentObjectOperation): ConfigurationIntentAction {
    return { action: 'objectNode', nodeId: editor.nodeId, operation };
  }
  function open(object: IntentObjectProjection | undefined, button: HTMLButtonElement) {
    if (disabled || blocked || selection) return;
    if (object && !object.editable) { openFinal(); return; }
    opener = button;
    selection = { object };
    onEditingChange(true);
  }
  async function close() {
    selection = null; onEditingChange(false);
    await tick();
    if (opener?.isConnected) opener.focus();
    else section?.querySelector<HTMLButtonElement>('[data-add-kind]')?.focus();
  }
  async function follow(object: IntentObjectProjection) {
    if (disabled || blocked || following) return;
    following = true;
    try { await submit(native({ kind: 'follow', objectId: object.objectId, expectedHash: object.contentHash })); }
    finally { following = false; }
  }
</script>

<section class="native-objects" aria-label={$t(descriptor.label)} bind:this={section}>
  <header><h4>{$t(descriptor.label)}</h4>{#if descriptor.canCreate}<button type="button" data-add-kind={descriptor.kind} disabled={disabled || blocked || !!selection || following} on:click={(event) => open(undefined, event.currentTarget)}><Icon name="add" size={15} /><span>{$t('Add')} {$t(descriptor.label)}</span></button>{/if}</header>
  {#if editor.objects.length > 4}<div class="object-choice"><select aria-label={$t('Choose an entry') + ': ' + $t(descriptor.label)} bind:value={chosenId} disabled={!!selection || blocked}>{#if !chosen}<option value="">{$t('Choose an entry to edit')}</option>{/if}{#each editor.objects as object (object.objectId)}<option value={object.objectId}>{label(object, $t)}{object.values.port ? ' · ' + String(object.values.port) : ''}</option>{/each}</select><button type="button" disabled={disabled || !chosen || blocked || !!selection || following} on:click={(event) => chosen && (chosen.removed ? void follow(chosen) : open(chosen, event.currentTarget))}>{$t(chosen?.removed ? 'Follow source' : 'Edit')}</button></div>
  {:else if editor.objects.length}<div class="object-entries">{#each editor.objects as object (object.objectId)}<div class="object-row"><div><strong>{label(object, $t)}</strong><span>{object.removed ? $t('Removed') : summary(object, $t)}</span></div><button type="button" disabled={disabled || blocked || !!selection || following} on:click={(event) => object.removed ? void follow(object) : open(object, event.currentTarget)}>{$t(object.removed ? 'Follow source' : object.editable ? 'Edit' : 'Edit in Final configuration')}</button></div>{/each}</div>
  {:else if !selection}<p class="empty">{$t('No entries yet.')}</p>{/if}
  {#if selection}{#key selection.object?.objectId ?? editor.nodeId}<IntentObjectForm {descriptor} object={selection.object} latestObject={editor.objects.find((object) => object.objectId === selection?.object?.objectId)} {targets} {program} {disabled} {openFinal} preview={(change) => preview(native(change))} submit={(change, plan, onConfirmed) => submit(native(change), plan, onConfirmed)} close={() => void close()} />{/key}{/if}
</section>

<style>
  .native-objects { min-width: 0; border: 1px solid var(--ui-border-default); border-radius: var(--ui-radius-sm); background: var(--ui-surface-2); }
  header { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 8px; padding: 12px; }
  h4 { margin: 0; font-size: .85rem; font-weight: var(--ui-weight-semibold); }
  header button { border-color: transparent; background: var(--ui-brand-soft); color: var(--ui-text-link); }
  .object-entries { padding: 0 12px; }
  .object-row { display: flex; justify-content: space-between; align-items: center; gap: 12px; border-top: 1px solid var(--ui-divider); padding: 10px 0; }
  .object-row > div { display: grid; gap: 4px; min-width: 0; }
  strong { font-size: .83rem; font-weight: var(--ui-weight-medium); }
  .object-row span { font-size: .77rem; color: var(--ui-text-secondary); }
  .object-row button { flex-shrink: 0; max-width: 45%; }
  .object-choice { display: flex; gap: 8px; padding: 0 12px 12px; }
  select { width: 100%; min-width: 0; max-width: 100%; }
  .empty { margin: 0; padding: 0 12px 12px; color: var(--ui-text-secondary); font-size: .8rem; }
  button, h4, strong, span { min-width: 0; max-width: 100%; overflow-wrap: anywhere; white-space: normal; }
  button { box-shadow: none; min-height: 32px; font-size: .78rem; }
  @container (max-width: 440px) { header button { width: 100%; } .object-choice { flex-direction: column; } .object-row { flex-direction: column; align-items: stretch; } .object-row button { max-width: 100%; } }
</style>

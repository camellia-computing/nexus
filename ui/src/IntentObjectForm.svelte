<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from './i18n';
  import { dnsServerNeedsResolver, intentOptionMessage } from './intentPresentation';
  import Icon from './lib/components/Icon.svelte';
  import type { ConfigurationIntentAction, IntentObjectDescriptor, IntentObjectProjection, IntentObjectField, IntentTarget, ProgramKind } from './types';
  export let descriptor: IntentObjectDescriptor;
  export let object: IntentObjectProjection | undefined = undefined;
  export let targets: IntentTarget[] = [];
  export let program: ProgramKind;
  export let disabled = false;
  export let submit: (change: ConfigurationIntentAction) => Promise<boolean>;
  export let close: () => void;
  let values: Record<string, unknown> = object ? structuredClone(object.values) : {
    ...(descriptor.protocols.length ? { protocol: descriptor.protocols[0] } : {}),
    ...(descriptor.kind === 'listener' ? { access: 'local' } : {}),
  };
  let more = false;
  let pending = false;
  let inputError = '';
  let formElement: HTMLFormElement;
  onMount(() => formElement.querySelector<HTMLElement>('input:not([disabled]), select:not([disabled]), textarea:not([disabled])')?.focus());
  function visible(field: IntentObjectField, current: Record<string, unknown>, expanded: boolean) {
    if ((field.key === 'username' || field.key === 'password') && current.access !== 'lan' && !expanded) return false;
    if (['server', 'serverPort', 'bootstrap'].includes(field.key) && current.protocol === 'system') return false;
    if (field.key === 'udp' && (program !== 'xray' || current.protocol !== 'socks')) return false;
    if (field.key === 'bootstrap' && dnsServerNeedsResolver(current.server)) return true;
    return !field.advanced || expanded;
  }
  function options(field: IntentObjectField) {
    if (field.key === 'protocol') return descriptor.protocols;
    if (field.key === 'bootstrap') return targets.filter((target) => target.kind === 'dns' && target.id !== object?.label).map((target) => target.id);
    if (field.key === 'target') return targets.filter((target) => target.kind === 'connection').map((target) => target.id);
    return field.allowedValues;
  }
  function isRequired(field: IntentObjectField) {
    return field.required
      || (field.key === 'server' && descriptor.kind === 'dnsServer' && values.protocol !== 'system')
      || ((field.key === 'username' || field.key === 'password') && values.access === 'lan' && (!object || object.values.access !== 'lan'));
  }
  $: basicFields = descriptor.fields.filter((field) => visible(field, values, false) && !['username', 'password'].includes(field.key));
  $: extraFields = descriptor.fields.filter((field) => visible(field, values, true) && !basicFields.includes(field) && !['username', 'password'].includes(field.key));
  $: authFields = descriptor.fields.filter((field) => ['username', 'password'].includes(field.key) && visible(field, values, more));
  $: needsBootstrap = program === 'singBox' && dnsServerNeedsResolver(values.server) && basicFields.some((field) => field.key === 'bootstrap');
  $: missingReference = basicFields.find((field) => field.control === 'select' && (field.required || (field.key === 'bootstrap' && needsBootstrap)) && ['target', 'bootstrap'].includes(field.key) && !options(field).length);
  async function commit() {
    if (disabled || pending || missingReference) return;
    inputError = '';
    const patch = Object.fromEntries(Object.entries(values).filter(([key, value]) => value !== '' && value !== undefined && (!object || JSON.stringify(object.values[key]) !== JSON.stringify(value))));
    if (Object.values(patch).some((value) => typeof value === 'number' && !Number.isFinite(value))) { inputError = 'Enter a valid number.'; return; }
    if (object && !Object.keys(patch).length) { close(); return; }
    pending = true;
    try { if (await submit(object ? { action: 'updateObject', objectId: object.objectId, expectedHash: object.contentHash, values: patch } : { action: 'createObject', objectKind: descriptor.kind, values: patch })) close(); }
    finally { pending = false; }
  }
  async function objectAction(action: 'removeObject' | 'followObject') {
    if (!object || disabled || pending) return;
    inputError = '';
    pending = true;
    try { if (await submit({ action, objectId: object.objectId, expectedHash: object.contentHash })) close(); }
    finally { pending = false; }
  }
</script>

{#snippet fieldControl(field: IntentObjectField)}
    <label class:wide={field.key === 'address'} class:toggle-field={field.control === 'toggle'}>
      <span>{$t(field.label)}</span>
      {#if field.control === 'select'}
        <select aria-label={$t(field.label)} data-control-size="md" required={isRequired(field) || (field.key === 'bootstrap' && needsBootstrap)} disabled={disabled || pending || (!!object && field.key === 'protocol')} value={String(values[field.key] ?? '')} on:change={(event) => values = { ...values, [field.key]: event.currentTarget.value }}><option value="">{$t('Select a value')}</option>{#each options(field) as option}<option value={option}>{['target', 'bootstrap'].includes(field.key) ? targets.find((item) => item.id === option)?.label ?? option : $t(intentOptionMessage(option, field.key))}</option>{/each}</select>
      {:else if field.control === 'toggle'}
        <input type="checkbox" disabled={disabled || pending} checked={values[field.key] === true} on:change={(event) => values = { ...values, [field.key]: event.currentTarget.checked }} />
      {:else if field.key === 'address'}
        <textarea rows="2" required={isRequired(field)} disabled={disabled || pending} value={String(values[field.key] ?? '')} on:input={(event) => values = { ...values, [field.key]: event.currentTarget.value }}></textarea>
      {:else}
        <input type={field.secret ? 'password' : field.control === 'number' ? 'number' : 'text'} min={field.key === 'port' ? 1 : undefined} max={field.key === 'port' ? 65535 : undefined} required={isRequired(field)} autocomplete={field.secret ? 'new-password' : 'off'} disabled={disabled || pending} value={String(values[field.key] ?? '')} on:input={(event) => values = { ...values, [field.key]: field.control === 'number' && event.currentTarget.value !== '' ? Number(event.currentTarget.value) : event.currentTarget.value }} />
      {/if}
    </label>
{/snippet}

<form class="object-form" bind:this={formElement} aria-busy={pending} on:submit|preventDefault={() => void commit()}>
  <header class="form-heading"><Icon name={object ? 'settings' : 'add'} size={17} /><h5>{$t(object ? 'Edit' : 'Add')} {$t(descriptor.label)}</h5></header>
  <div class="field-grid">
  {#each basicFields as field (field.key)}
    {@render fieldControl(field)}
  {/each}
  </div>
  {#if authFields.length}
    <fieldset class="auth-fields"><legend><Icon name="lock" size={15} />{$t('Access protection')}</legend><div class="field-grid">{#each authFields as field (field.key)}{@render fieldControl(field)}{/each}</div>
      {#if values.access === 'lan'}<p>{$t('Other devices can use this proxy. Protect it with a username and password.')}</p>{/if}
    </fieldset>
  {/if}
  {#if more && extraFields.length}<div class="extra-fields"><div class="field-grid">{#each extraFields as field (field.key)}{@render fieldControl(field)}{/each}</div></div>{/if}
  {#if program === 'mihomo' && descriptor.kind === 'listener'}<p class="form-note"><Icon name="info" size={15} /><span>{$t('Network access and passwords apply to all local proxies.')}</span></p>{/if}
  {#if missingReference}<p class="form-note"><Icon name="info" size={15} /><span>{$t(missingReference.key === 'bootstrap' ? 'Add a DNS server to choose it here.' : 'Add a connection in Sources or Final configuration first.')}</span></p>{/if}
  {#if inputError}<p class="form-error" role="alert"><Icon name="alert" size={15} /><span>{$t(inputError)}</span></p>{/if}
  <div class="form-actions">
    <div class="form-secondary-actions">
      {#if extraFields.length || descriptor.fields.some((field) => ['username', 'password'].includes(field.key) && values.access !== 'lan')}<button class="text-action" type="button" aria-expanded={more} on:click={() => more = !more}><Icon name="settings" size={14} />{$t(more ? 'Fewer options' : 'More options')}</button>{/if}
      {#if object}<details class="entry-actions"><summary>{$t('Entry actions')}</summary><div><button class="remove-action" type="button" disabled={disabled || pending} on:click={() => void objectAction('removeObject')}><Icon name="trash" size={14} />{$t('Remove')}</button>{#if object.canFollow}<button type="button" disabled={disabled || pending} on:click={() => void objectAction('followObject')}><Icon name="undo" size={14} />{$t('Follow source')}</button>{/if}</div></details>{/if}
    </div>
    <div class="form-primary-actions"><button type="button" disabled={pending} on:click={close}>{$t('Cancel')}</button><button class="primary" type="submit" disabled={disabled || pending || !!missingReference}>{$t(object ? 'Done' : 'Add')}</button></div>
  </div>
</form>

<style>
  .object-form { display: grid; gap: 16px; margin: 0 12px 12px; padding: 16px; border: 1px solid color-mix(in srgb, var(--ui-brand) 35%, var(--ui-border-default)); border-radius: var(--ui-radius-sm); background: var(--ui-surface-raised); box-shadow: var(--ui-shadow-xs); }
  .form-heading { display: flex; align-items: center; gap: 7px; color: var(--ui-text-link); min-width: 0; }
  h5 { margin: 0; font-size: .85rem; font-weight: var(--ui-weight-semibold); overflow-wrap: anywhere; }
  .field-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 14px 18px; }
  label { display: grid; gap: 6px; min-width: 0; align-content: start; font-size: .8rem; font-weight: var(--ui-weight-medium); overflow-wrap: anywhere; }
  label.wide { grid-column: 1 / -1; }
  label.toggle-field { grid-template-columns: minmax(0, 1fr) auto; align-items: center; align-content: center; }
  input:not([type='checkbox']), select, textarea { width: 100%; min-width: 0; max-width: 100%; min-height: var(--ui-control-md); box-sizing: border-box; font-size: .82rem; }
  input, textarea { padding: 8px 10px; }
  textarea { resize: vertical; }
  input[type='checkbox'] { width: 18px; height: 18px; }
  .auth-fields { min-width: 0; margin: 0; padding: 12px; border: 1px solid var(--ui-border-default); border-radius: var(--ui-radius-sm); background: var(--ui-surface-2); }
  legend { display: inline-flex; align-items: center; gap: 6px; padding: 0 5px; font-size: .77rem; color: var(--ui-text-secondary); }
  .auth-fields p { margin-top: 10px; }
  .extra-fields { padding-top: 12px; border-top: 1px solid var(--ui-divider); }
  p { margin: 0; font-size: .78rem; color: var(--ui-text-secondary); line-height: 1.45; overflow-wrap: anywhere; }
  .form-note, .form-error { display: flex; align-items: flex-start; gap: 7px; min-width: 0; }
  .form-error { color: var(--ui-danger); }
  .form-actions { display: flex; min-width: 0; gap: 12px; flex-wrap: wrap; align-items: flex-start; justify-content: space-between; padding-top: 12px; border-top: 1px solid var(--ui-divider); }
  .form-secondary-actions, .form-primary-actions { display: flex; min-width: 0; gap: 6px; flex-wrap: wrap; }
  .form-primary-actions { margin-left: auto; }
  button { max-width: 100%; min-height: 34px; white-space: normal; overflow-wrap: anywhere; box-shadow: none; font-size: .78rem; }
  .text-action { padding: 5px 6px; border-color: transparent; background: transparent; color: var(--ui-text-secondary); }
  summary { cursor: pointer; min-height: 34px; padding: 8px 6px; font-size: .78rem; color: var(--ui-text-secondary); overflow-wrap: anywhere; }
  .entry-actions div { display: grid; gap: 6px; padding-top: 4px; }
  .remove-action { color: var(--ui-danger); }
  @container (max-width: 560px) { .field-grid { grid-template-columns: 1fr; } }
  @container (max-width: 440px) { .object-form { margin: 0 8px 8px; padding: 12px; } .form-primary-actions { width: 100%; } .form-primary-actions button { flex: 1; } }
</style>

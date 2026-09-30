<script lang="ts">
  import { t } from './i18n';
  import { intentOptionMessage } from './intentPresentation';
  import Icon from './lib/components/Icon.svelte';
  import type { ConfigurationIntentAction, GuidedProjection, GuidedSettingDescriptor, IntentTarget } from './types';
  export let descriptor: GuidedSettingDescriptor;
  export let projection: GuidedProjection | undefined;
  export let targets: IntentTarget[] = [];
  export let disabled = false;
  export let submit: (change: ConfigurationIntentAction) => Promise<boolean>;
  export let openFinal: () => void;
  let pending = false;
  let inputError = '';
  let actionsOpen = false;
  $: value = projection?.value ?? descriptor.defaultValue;
  $: isReference = descriptor.id === 'routing.final' || descriptor.id === 'dns.final';
  $: options = isReference ? targets.filter((item) => item.kind === (descriptor.id === 'dns.final' ? 'dns' : 'connection')).map((item) => item.id) : descriptor.allowedValues;
  $: unavailable = descriptor.available === false;
  async function set(value: unknown) {
    if (disabled || pending || unavailable) return;
    if (typeof value === 'number' && !Number.isFinite(value)) { inputError = 'Enter a valid number.'; return; }
    inputError = '';
    pending = true;
    try { if (await submit({ action: 'set', settingId: descriptor.id, value })) actionsOpen = false; }
    finally { pending = false; }
  }
  async function follow() {
    if (disabled || pending) return;
    inputError = '';
    pending = true;
    try { if (await submit({ action: 'follow', settingId: descriptor.id })) actionsOpen = false; }
    finally { pending = false; }
  }
</script>

<div class="setting-row" aria-busy={pending}>
  <label for={'intent-' + descriptor.id}>{$t(descriptor.label)}</label>
  <div class="setting-control">
    {#if descriptor.control === 'toggle' && typeof value === 'boolean'}
      <span class="toggle-control"><input id={'intent-' + descriptor.id} type="checkbox" checked={value === true} disabled={disabled || pending || unavailable} on:change={(event) => void set(event.currentTarget.checked)} /><span class="toggle-track" aria-hidden="true"></span><span class="toggle-value">{$t(value ? 'On' : 'Off')}</span></span>
    {:else if descriptor.control === 'toggle'}
      <select id={'intent-' + descriptor.id} data-control-size="md" disabled={disabled || pending || unavailable} on:change={(event) => event.currentTarget.value ? void set(event.currentTarget.value === 'true') : void follow()}><option value="">{$t('Use program default')}</option><option value="true">{$t('On')}</option><option value="false">{$t('Off')}</option></select>
    {:else if descriptor.control === 'select' || isReference}
      <select id={'intent-' + descriptor.id} data-control-size="md" value={typeof value === 'string' ? value : '__unset'} disabled={disabled || pending || unavailable || !options.length} on:change={(event) => void set(event.currentTarget.value)}><option value="__unset" disabled>{$t('Use program default')}</option>{#each options as option}<option value={option}>{isReference ? targets.find((item) => item.id === option)?.label ?? option : $t(intentOptionMessage(option, descriptor.id))}</option>{/each}</select>
    {:else}
      <input id={'intent-' + descriptor.id} type={descriptor.control === 'number' ? 'number' : 'text'} placeholder={descriptor.id === 'dns.timeout' ? '5s' : ''} value={value == null ? '' : String(value)} min={descriptor.minimum} max={descriptor.maximum} disabled={disabled || pending || unavailable} on:change={(event) => !event.currentTarget.value ? void follow() : void set(descriptor.control === 'number' ? Number(event.currentTarget.value) : event.currentTarget.value)} />
    {/if}
    <span class="setting-action-slot">{#if projection?.intentValue !== undefined}<button class="setting-menu" type="button" aria-label={$t('Setting actions') + ': ' + $t(descriptor.label)} aria-expanded={actionsOpen} aria-controls={'intent-actions-' + descriptor.id} on:click={() => actionsOpen = !actionsOpen}><Icon name="settings" size={16} /></button>{/if}</span>
  </div>
  {#if actionsOpen && projection?.intentValue !== undefined}<div class="setting-actions" id={'intent-actions-' + descriptor.id}>
    <button type="button" disabled={disabled || pending} on:click={() => void follow()}><Icon name="undo" size={14} />{$t('Follow source')}</button>
    {#if JSON.stringify(projection.intentValue) !== JSON.stringify(value)}<button type="button" disabled={disabled || pending || unavailable} on:click={() => void set(projection?.intentValue)}>{$t('Use this value')}</button>{/if}
  </div>{/if}
  {#if isReference && !options.length}<small>{$t(descriptor.id === 'dns.final' ? 'Add a DNS server to choose it here.' : 'Add a connection in Sources or Final configuration first.')} <button type="button" on:click={openFinal}>{$t('Edit in Final configuration')}</button></small>{/if}
  {#if unavailable}<small>{$t(descriptor.unavailableReason ?? 'This setting is not available in this program.')} <button type="button" on:click={openFinal}>{$t('Edit in Final configuration')}</button></small>{/if}
  {#if inputError}<small role="alert">{$t(inputError)}</small>{/if}
</div>

<style>
  .setting-row { display: grid; grid-template-columns: minmax(0, 1fr) minmax(150px, 44%); gap: 6px 18px; padding: 12px 0; border-top: 1px solid var(--ui-divider); align-items: center; }
  label, small, button { min-width: 0; overflow-wrap: anywhere; }
  label { font-size: .85rem; line-height: 1.45; font-weight: var(--ui-weight-medium); }
  .setting-control { display: flex; align-items: center; gap: 6px; min-width: 0; }
  input:not([type='checkbox']), select { width: 100%; min-width: 0; max-width: 100%; min-height: var(--ui-control-md); font-size: .82rem; }
  input:not([type='checkbox']) { padding: 7px 10px; }
  button { max-width: 100%; white-space: normal; box-shadow: none; }
  small { grid-column: 1 / -1; font-size: .78rem; color: var(--ui-text-secondary); line-height: 1.45; }
  small button { padding: 3px 5px; min-height: 28px; background: transparent; border-color: transparent; color: var(--ui-text-link); font-size: inherit; }
  small[role='alert'] { color: var(--ui-danger); }
  .setting-action-slot { width: 30px; flex: 0 0 30px; }
  .setting-menu { width: 30px; min-height: 30px; padding: 0; background: transparent; border-color: transparent; color: var(--ui-text-secondary); }
  .setting-actions { display: flex; justify-content: flex-end; flex-wrap: wrap; gap: 6px; grid-column: 1 / -1; }
  .setting-actions button { min-height: 30px; padding: 5px 8px; font-size: .76rem; }
  .toggle-control { display: flex; position: relative; align-items: center; gap: 9px; flex: 1; min-height: 38px; }
  .toggle-control input { position: absolute; z-index: 1; width: 38px; min-height: 24px; height: 24px; margin: 0; opacity: 0; cursor: pointer; }
  .toggle-track { position: relative; flex: 0 0 38px; height: 24px; border-radius: 20px; border: 1px solid var(--ui-border-strong); background: var(--ui-surface-inset); }
  .toggle-track::after { content: ''; position: absolute; left: 3px; top: 3px; width: 16px; height: 16px; border-radius: 50%; background: var(--ui-text-secondary); }
  .toggle-control input:checked + .toggle-track { background: var(--ui-brand); border-color: var(--ui-brand); }
  .toggle-control input:checked + .toggle-track::after { transform: translateX(14px); background: var(--ui-on-brand); }
  .toggle-control input:focus-visible + .toggle-track { outline: 3px solid var(--ui-focus-ring); outline-offset: 3px; }
  .toggle-control input:disabled + .toggle-track { opacity: .55; }
  .toggle-control input:disabled { cursor: default; }
  .toggle-value { font-size: .8rem; color: var(--ui-text-secondary); }
  @container (max-width: 440px) { .setting-row { grid-template-columns: 1fr; gap: 5px; } }
</style>

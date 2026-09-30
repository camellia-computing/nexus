<script lang="ts">
  import { onDestroy, tick } from 'svelte';
  import { t } from './i18n';
  import { intentGroups, intentOptionMessage } from './intentPresentation';
  import Icon, { type IconName } from './lib/components/Icon.svelte';
  import IntentSettingControl from './IntentSettingControl.svelte';
  import IntentObjectForm from './IntentObjectForm.svelte';
  import type { ConfigurationStateView, ConfigurationIntentAction, IntentObjectDescriptor, IntentObjectProjection } from './types';
  export let state: ConfigurationStateView;
  export let disabled = false;
  export let onChange: (change: ConfigurationIntentAction) => Promise<boolean>;
  export let onOpenFinal: () => void;
  export let onEditingChange: (editing: boolean) => void;
  let selection: { descriptor: IntentObjectDescriptor; object?: IntentObjectProjection } | null = null;
  let workspace: HTMLElement;
  let opener: HTMLButtonElement | null = null;
  let chosenIds: Record<string, string> = {};
  let collapsed: string[] = [];
  let following = '';
  $: onEditingChange(selection !== null);
  onDestroy(() => onEditingChange(false));
  const groupIcons: Record<string, IconName> = { local: 'home', dns: 'search', routing: 'sliders', tun: 'shield', logging: 'logs' };
  const emptyMessages: Record<string, string> = {
    listener: 'Add a local proxy for your apps to connect to.',
    dnsServer: 'Add a DNS server when you need your own resolver.',
    routeRule: 'Choose which connection a domain or IP address uses.',
    tun: 'Add a virtual adapter configuration. It stays off until you run the program.',
  };
  function objectLabel(object: IntentObjectProjection, descriptor: IntentObjectDescriptor, localize: (message: string) => string) {
    return object.label.startsWith('nexus-') ? localize(descriptor.label) : object.label;
  }
  function objectSummary(object: IntentObjectProjection, localize: (message: string) => string) {
    const { protocol, port, server, access, target } = object.values;
    return [protocol ? localize(intentOptionMessage(String(protocol))) : '', port ? String(port) : '',
      server ? String(server) : '', access ? localize(intentOptionMessage(String(access))) : '', target ? String(target) : ''].filter(Boolean).join(' · ');
  }
  function openObject(descriptor: IntentObjectDescriptor, object: IntentObjectProjection | undefined, button: HTMLButtonElement) {
    if (selection || disabled) return;
    opener = button;
    selection = { descriptor, object };
  }
  async function closeObject() {
    const kind = selection?.descriptor.kind;
    selection = null;
    await tick();
    if (opener?.isConnected) opener.focus();
    else if (kind) workspace?.querySelector<HTMLButtonElement>(`button[data-add-kind="${kind}"]`)?.focus();
  }
  async function followObject(object: IntentObjectProjection) {
    if (following || disabled || selection) return;
    following = object.objectId;
    try { await onChange({ action: 'followObject', objectId: object.objectId, expectedHash: object.contentHash }); }
    finally { following = ''; }
  }
  $: projection = new Map(state.guidedProjection.map((item) => [item.settingId, item]));
  function belongsToGroup(category: string, group: string) { return (category === 'network' ? 'routing' : category) === group; }
  $: groups = ['local', 'dns', 'routing', 'tun', 'logging'].filter((category) =>
    state.guidedDescriptors.some((item) => belongsToGroup(item.category, category))
      || state.intentObjectDescriptors?.some((item) => belongsToGroup(item.category, category) && (item.canCreate || state.intentObjects?.some((object) => object.kind === item.kind))));
</script>

<section class="guided-workspace" aria-label={$t('Intent')} bind:this={workspace}>
  <div class="intent-sections">
    {#each groups as category (category)}
      <section class="intent-group" data-group={category} aria-labelledby={'intent-heading-' + category}>
        <h3 id={'intent-heading-' + category}>
          <button class="group-heading" type="button" aria-expanded={!collapsed.includes(category)} aria-controls={'intent-content-' + category} on:click={() => collapsed = collapsed.includes(category) ? collapsed.filter((item) => item !== category) : [...collapsed, category]}>
            <span class="group-icon"><Icon name={groupIcons[category]} size={20} /></span>
            <span class="group-title">{$t(intentGroups[category])}</span>
            {#if selection && belongsToGroup(selection.descriptor.category, category)}<span class="editing-label" aria-hidden="true">{$t('Editing')}</span>{/if}
            <span class="group-chevron" class:expanded={!collapsed.includes(category)}><Icon name="chevron" size={16} /></span>
          </button>
        </h3>
        <div class="group-content" id={'intent-content-' + category} hidden={collapsed.includes(category)}>
        <div class="setting-list">
        {#each state.guidedDescriptors.filter((item) => belongsToGroup(item.category, category) && !item.advanced) as descriptor (descriptor.id)}
          <IntentSettingControl {descriptor} projection={projection.get(descriptor.id)} targets={state.intentTargets ?? []} {disabled} submit={onChange} openFinal={onOpenFinal} />
        {/each}
        </div>
        {#each (state.intentObjectDescriptors ?? []).filter((item) => belongsToGroup(item.category, category) && (item.canCreate || state.intentObjects?.some((object) => object.kind === item.kind))) as descriptor (descriptor.kind)}
          {@const objects = (state.intentObjects ?? []).filter((object) => object.kind === descriptor.kind)}
          {@const chosen = objects.find((object) => object.objectId === chosenIds[descriptor.kind])}
          <div class="object-list">
            <div class="object-toolbar">
              <h4>{$t(descriptor.label)}</h4>
              {#if descriptor.canCreate}<button class="add-object" data-add-kind={descriptor.kind} type="button" disabled={disabled || !!selection || !!following} title={selection ? $t('Finish or cancel this entry first.') : undefined} on:click={(event) => openObject(descriptor, undefined, event.currentTarget)}><Icon name="add" size={16} /><span>{$t('Add')} {$t(descriptor.label)}</span></button>{/if}
            </div>
            {#if objects.length > 4}
              <div class="object-choice">
                <select aria-label={$t('Choose an entry') + ': ' + $t(descriptor.label)} value={chosen?.objectId ?? ''} on:change={(event) => chosenIds = { ...chosenIds, [descriptor.kind]: event.currentTarget.value }}>
                  <option value="">{$t('Choose an entry to edit')}</option>
                  {#each objects as object (object.objectId)}<option value={object.objectId}>{objectLabel(object, descriptor, $t)}{#if object.values.port} · {String(object.values.port)}{/if}{#if object.removed} · {$t('Removed')}{/if}</option>{/each}
                </select>
                <button type="button" disabled={disabled || !chosen || !!selection || !!following} on:click={(event) => chosen && (chosen.removed ? void followObject(chosen) : chosen.editable ? openObject(descriptor, chosen, event.currentTarget) : onOpenFinal())}>{$t(chosen?.removed ? 'Follow source' : 'Edit')}</button>
              </div>
            {:else if objects.length}
            <div class="object-entries">
            {#each objects as object (object.objectId)}
              <div class="object-row" class:editing={selection?.object?.objectId === object.objectId}>
                <div class="object-description"><strong>{objectLabel(object, descriptor, $t)}</strong>{#if object.removed}<span>{$t('Removed')}</span>{:else if objectSummary(object, $t)}<span>{objectSummary(object, $t)}</span>{/if}</div>
                <button type="button" disabled={disabled || !!selection || !!following} on:click={(event) => object.removed ? void followObject(object) : object.editable ? openObject(descriptor, object, event.currentTarget) : onOpenFinal()}>{$t(object.removed ? 'Follow source' : object.editable ? 'Edit' : 'Edit in Final configuration')}</button>
              </div>
            {/each}
            </div>
            {:else if selection?.descriptor.kind !== descriptor.kind}
              <p class="object-empty">{$t(emptyMessages[descriptor.kind])}</p>
            {/if}
          {#if selection?.descriptor.kind === descriptor.kind}
            {#key selection.object?.objectId ?? descriptor.kind}
              <IntentObjectForm {descriptor} object={selection.object} targets={state.intentTargets ?? []} program={state.kind} {disabled} submit={onChange} close={() => void closeObject()} />
            {/key}
          {/if}
          </div>
        {/each}
        {#if state.guidedDescriptors.some((item) => belongsToGroup(item.category, category) && item.advanced)}
          <details class="group-options"><summary><Icon name="settings" size={15} /><span>{$t('More options')}</span></summary>
            <div class="setting-list">
              {#each state.guidedDescriptors.filter((item) => belongsToGroup(item.category, category) && item.advanced) as descriptor (descriptor.id)}
                <IntentSettingControl {descriptor} projection={projection.get(descriptor.id)} targets={state.intentTargets ?? []} {disabled} submit={onChange} openFinal={onOpenFinal} />
              {/each}
            </div>
          </details>
        {/if}
        </div>
      </section>
    {/each}
  </div>
</section>

<style>
  .guided-workspace { container-type: inline-size; min-width: 0; }
  .intent-sections { display: grid; gap: 14px; }
  .intent-group { --group-accent: var(--ui-brand); min-width: 0; border: 1px solid var(--ui-border-default); border-radius: var(--ui-radius-md); background: var(--ui-surface-1); }
  .intent-group[data-group='dns'] { --group-accent: var(--ui-accent-tertiary); }
  .intent-group[data-group='routing'] { --group-accent: var(--ui-accent-secondary); }
  .intent-group[data-group='logging'] { --group-accent: var(--ui-text-secondary); }
  h3, h4 { margin: 0; }
  .group-heading { display: flex; width: 100%; min-width: 0; gap: 12px; justify-content: flex-start; padding: 13px 18px; border: 0; border-radius: var(--ui-radius-md); box-shadow: none; background: transparent; text-align: left; }
  .group-heading:hover:not(:disabled) { background: var(--ui-state-hover); }
  .group-icon { display: grid; place-items: center; flex: 0 0 36px; height: 36px; border-radius: var(--ui-radius-sm); color: color-mix(in srgb, var(--group-accent) 78%, var(--ui-text-primary)); background: color-mix(in srgb, var(--group-accent) 12%, var(--ui-surface-1)); }
  .group-title { flex: 1; font-size: .94rem; font-weight: var(--ui-weight-semibold); }
  .editing-label { font-size: .72rem; font-weight: var(--ui-weight-medium); color: var(--ui-text-link); }
  .group-chevron { display: grid; color: var(--ui-text-secondary); flex-shrink: 0; }
  .group-chevron.expanded { transform: rotate(90deg); }
  .group-content { display: grid; gap: 14px; padding: 0 18px 16px; }
  .group-content[hidden] { display: none; }
  .setting-list { display: grid; min-width: 0; }
  .setting-list:empty { display: none; }
  .group-options { min-width: 0; border-top: 1px solid var(--ui-divider); }
  summary { display: flex; width: fit-content; max-width: 100%; align-items: center; gap: 7px; padding: 12px 0 0; font-size: .8rem; color: var(--ui-text-secondary); cursor: pointer; list-style: none; }
  summary::-webkit-details-marker { display: none; }
  details[open] summary { padding-bottom: 8px; }
  .object-list { min-width: 0; border: 1px solid var(--ui-border-subtle); border-radius: var(--ui-radius-sm); background: var(--ui-surface-2); }
  .object-toolbar { display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap; gap: 8px; padding: 10px 12px; }
  h4 { min-width: 0; font-size: .82rem; font-weight: var(--ui-weight-semibold); }
  .add-object { min-height: 32px; padding: 6px 9px; box-shadow: none; border-color: color-mix(in srgb, var(--ui-brand) 25%, var(--ui-border-subtle)); background: var(--ui-brand-soft); color: var(--ui-text-link); font-size: .78rem; }
  .object-entries { padding: 0 12px; }
  .object-row { display: flex; align-items: center; justify-content: space-between; gap: 14px; padding: 10px 0; border-top: 1px solid var(--ui-divider); min-width: 0; }
  .object-row.editing { border-color: var(--ui-border-focus); }
  .object-description { display: grid; gap: 3px; min-width: 0; }
  .object-description strong { font-size: .85rem; font-weight: var(--ui-weight-medium); }
  .object-description span { font-size: .77rem; color: var(--ui-text-secondary); }
  .object-row button { flex: 0 0 auto; max-width: 45%; min-height: 32px; font-size: .78rem; padding: 6px 10px; box-shadow: none; }
  .object-choice { display: flex; gap: 8px; min-width: 0; align-items: center; padding: 0 12px 12px; }
  .object-choice select { width: 100%; min-width: 0; max-width: 100%; }
  .object-choice button { flex: 0 0 auto; }
  .object-empty { margin: 0; padding: 0 12px 12px; font-size: .8rem; line-height: 1.45; color: var(--ui-text-secondary); }
  span, h4, button, p { min-width: 0; overflow-wrap: anywhere; }
  button { max-width: 100%; white-space: normal; overflow-wrap: anywhere; }
  @container (min-width: 840px) { .setting-list { grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 0 28px; } }
  @container (max-width: 440px) {
    .group-heading { padding: 12px; gap: 9px; }
    .group-content { padding: 0 12px 12px; }
    .group-icon { flex-basis: 30px; height: 30px; }
    .object-toolbar { align-items: flex-start; }
    .object-choice { flex-direction: column; align-items: stretch; }
  }
</style>

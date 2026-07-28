<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import type { EnvironmentEntry } from './drafts';
  import { t } from './i18n';
  import Icon from './lib/components/Icon.svelte';

  export let entries: EnvironmentEntry[] = [];
  const dispatch = createEventDispatcher<{ change: EnvironmentEntry[] }>();

  function setEntries(next: EnvironmentEntry[]) {
    entries = next;
    dispatch('change', entries);
  }

  function update(index: number, field: keyof EnvironmentEntry, value: string) {
    setEntries(entries.map((entry, entryIndex) =>
      entryIndex === index ? { ...entry, [field]: value } : entry,
    ));
  }
</script>

<div class="structured-input">
  {#each entries as entry, index}
    <div class="structured-row environment-row">
      <input value={entry.key} aria-label={`${$t('Environment key')} ${index + 1}`} placeholder="KEY" on:input={(event) => update(index, 'key', event.currentTarget.value)} />
      <span class="equals">=</span>
      <input value={entry.value} aria-label={`${$t('Environment value')} ${index + 1}`} placeholder="value" on:input={(event) => update(index, 'value', event.currentTarget.value)} />
      <button class="icon-button" type="button" aria-label={`${$t('Remove environment entry')} ${index + 1}`} on:click={() => setEntries(entries.filter((_, entryIndex) => entryIndex !== index))}><Icon name="trash" size={16} /></button>
    </div>
  {/each}
  {#if entries.length === 0}<div class="structured-empty">{$t('No custom environment variables.')}</div>{/if}
  <div class="structured-footer">
    <span>{$t('Unique names; no “=”.')}</span>
    <button type="button" on:click={() => setEntries([...entries, { key: '', value: '' }])}><Icon name="add" size={15} />{$t('Add variable')}</button>
  </div>
</div>

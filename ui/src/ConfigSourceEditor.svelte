<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { t } from './i18n';
  import { MAX_CONFIG_SOURCES_PER_PROGRAM } from './programs/shared/configuration';
  import OptionSelect from './lib/components/OptionSelect.svelte';
  import OverflowPreviewInput from './lib/components/OverflowPreviewInput.svelte';
  import type { ConfigSource, RemoteUpdate } from './types';

  export let sources: ConfigSource[] = [];
  export let platform = '';
  export let disabled = false;
  export let maxSources = MAX_CONFIG_SOURCES_PER_PROGRAM;
  export let remoteUpdate: RemoteUpdate | undefined = undefined;

  const dispatch = createEventDispatcher<{
    change: ConfigSource[];
    remoteUpdate: RemoteUpdate | undefined;
  }>();
  const remoteUpdateIntervals = [15, 60, 360, 720, 1440];
  let selectedRemoteUpdateInterval = 60;

  $: activeCount = sources.filter((source) => source.enabled).length;
  $: remoteCount = sources.filter((source) => source.mode === 'remote').length;
  $: selectedRemoteUpdateInterval = normalizeRemoteUpdateInterval(remoteUpdate?.intervalMinutes);
  $: remoteUpdateOptions = remoteUpdateIntervals.map((interval) => ({
    value: interval,
    label: $t(interval === 15 ? '15 minutes' : interval === 60 ? '1 hour' : interval === 360 ? '6 hours' : interval === 720 ? '12 hours' : '1 day'),
  }));
  $: sourceTypeOptions = [
    { value: 'local', label: $t('Local file') },
    { value: 'remote', label: $t('Remote URL') },
  ];

  function sourceId() {
    return `source-${crypto.randomUUID().replaceAll('-', '').slice(0, 12)}`;
  }

  function setSources(next: ConfigSource[]) {
    sources = next;
    dispatch('change', sources);
  }

  function add(mode: ConfigSource['mode']) {
    if (disabled || sources.length >= maxSources) return;
    const index = sources.length + 1;
    const common = {
      id: sourceId(),
      name: `${mode === 'local' ? 'Local' : 'Remote'} ${index}`,
      enabled: true,
    };
    setSources([
      ...sources,
      mode === 'local'
        ? { ...common, mode, path: '' }
        : { ...common, mode, url: '' },
    ]);
  }

  function update(index: number, field: string, value: string | boolean) {
    setSources(sources.map((source, sourceIndex) => {
      if (sourceIndex !== index) return source;
      if (field === 'mode') {
        return value === 'local'
          ? { mode: 'local', id: source.id, name: source.name, enabled: source.enabled, path: '' }
          : { mode: 'remote', id: source.id, name: source.name, enabled: source.enabled, url: '' };
      }
      return { ...source, [field]: value } as ConfigSource;
    }));
  }

  function move(index: number, offset: -1 | 1) {
    const target = index + offset;
    if (target < 0 || target >= sources.length) return;
    const next = [...sources];
    [next[index], next[target]] = [next[target], next[index]];
    setSources(next);
  }

  function toggleAuthentication(index: number, enabled: boolean) {
    setSources(sources.map((source, sourceIndex) =>
      sourceIndex === index && source.mode === 'remote'
        ? {
            ...source,
            authentication: enabled
              ? source.authentication ?? { scheme: 'basic', username: '', password: '' }
              : undefined,
          }
        : source,
    ));
  }

  function updateAuthentication(
    index: number,
    field: 'username' | 'password',
    value: string,
  ) {
    setSources(sources.map((source, sourceIndex) =>
      sourceIndex === index && source.mode === 'remote'
        ? {
            ...source,
            authentication: {
              scheme: 'basic',
              username: source.authentication?.username ?? '',
              credentialId: source.authentication?.credentialId,
              password: source.authentication?.password ?? '',
              [field]: value,
            },
          }
        : source,
    ));
  }

  function toggleRemoteUpdate(enabled: boolean) {
    dispatch('remoteUpdate', {
      enabled,
      intervalMinutes: normalizeRemoteUpdateInterval(
        remoteUpdate?.intervalMinutes ?? selectedRemoteUpdateInterval,
      ),
    });
  }

  function setRemoteUpdateInterval(intervalMinutes: number) {
    dispatch('remoteUpdate', {
      enabled: remoteUpdate?.enabled ?? false,
      intervalMinutes: normalizeRemoteUpdateInterval(intervalMinutes),
    });
  }

  function normalizeRemoteUpdateInterval(intervalMinutes: number | undefined) {
    const value = Number(intervalMinutes);
    return remoteUpdateIntervals.includes(value) ? value : 60;
  }

  function remove(index: number) {
    setSources(sources.filter((_, sourceIndex) => sourceIndex !== index));
  }
</script>

<div class="config-source-editor">
  <header class="source-editor-header">
    <div class="source-editor-title">
      <span><strong>{$t('Configuration sources')}</strong><small>{activeCount} / {sources.length} {$t('active')}</small></span>
    </div>
    <div class="source-editor-add">
      <button type="button" on:click={() => add('local')} disabled={disabled || sources.length >= maxSources}>
        <svg viewBox="0 0 20 20" aria-hidden="true"><path d="M5 3.5h6l4 4v9H5Z"></path><path d="M11 3.5v4h4M10 10v4M8 12h4"></path></svg>{$t('Local file')}
      </button>
      <button type="button" on:click={() => add('remote')} disabled={disabled || sources.length >= maxSources}>
        <svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="10" cy="10" r="6.5"></circle><path d="M3.8 10h12.4M10 3.5c2.6 2.8 2.6 10.2 0 13M10 3.5c-2.6 2.8-2.6 10.2 0 13"></path></svg>{$t('Remote URL')}
      </button>
    </div>
  </header>

  {#if remoteCount > 0}
    <div class:enabled={!!remoteUpdate?.enabled} class="remote-update-policy">
      <span class="remote-update-copy"><strong>{$t('Automatic updates')}</strong><small>{remoteCount} {$t(remoteCount === 1 ? 'remote source' : 'remote sources')}</small></span>
      <label class="remote-update-interval"><span>{$t('Update interval')}</span><OptionSelect bind:value={selectedRemoteUpdateInterval} options={remoteUpdateOptions} ariaLabel={$t('Update interval')} disabled={disabled || !remoteUpdate?.enabled} align="center" size="md" width="content" on:change={() => setRemoteUpdateInterval(selectedRemoteUpdateInterval)} /></label>
      <label class="compact-switch"><input type="checkbox" aria-label={$t('Automatic updates')} checked={!!remoteUpdate?.enabled} on:change={(event) => toggleRemoteUpdate(event.currentTarget.checked)} {disabled} /><span></span></label>
    </div>
  {/if}

  <div class="source-stack">
    {#each sources as source, index (source.id)}
      <article class:disabled={!source.enabled} class:remote={source.mode === 'remote'} class="config-source-row">
        <div class="source-primary">
          <input class="source-name" value={source.name} maxlength="128" aria-label={$t('Source name')} placeholder={$t('Source name')} on:input={(event) => update(index, 'name', event.currentTarget.value)} {disabled} />
          <span class="source-type-select"><OptionSelect value={source.mode} options={sourceTypeOptions} ariaLabel={$t('Source type')} {disabled} align="center" size="md" width="content" on:change={(event) => update(index, 'mode', String(event.detail.value))} /></span>
          {#if source.mode === 'local'}
            <OverflowPreviewInput className="source-address-field" value={source.path} maxlength={32000} ariaLabel={$t('Local configuration path')} placeholder={platform === 'Windows' ? 'config.json · C:/Configs/config.json' : 'config.json · /etc/proxy/config.json'} on:input={(event) => update(index, 'path', event.detail.value)} {disabled} />
          {:else}
            <OverflowPreviewInput className="source-address-field" value={source.url} maxlength={2048} ariaLabel={$t('Remote configuration URL')} placeholder="https://example.com/config.json" on:input={(event) => update(index, 'url', event.detail.value)} {disabled} />
          {/if}
          <label class="source-enabled" title={$t('Include source')}>
            <input type="checkbox" checked={source.enabled} aria-label={$t('Include source')} on:change={(event) => update(index, 'enabled', event.currentTarget.checked)} {disabled} />
            <span>{$t(source.enabled ? 'Enabled' : 'Paused')}</span>
          </label>
          <div class="source-actions">
            {#if source.mode === 'remote'}
              <button class:active={!!source.authentication} type="button" aria-label={$t('Basic authentication')} title={$t('Basic authentication')} aria-pressed={!!source.authentication} on:click={() => toggleAuthentication(index, !source.authentication)} {disabled}><svg viewBox="0 0 18 18"><circle cx="6.5" cy="9" r="2.5"></circle><path d="M9 9h5M12 9v2M14 9v2"></path></svg></button>
            {:else}
              <span class="source-auth-placeholder" aria-hidden="true"></span>
            {/if}
            <button type="button" aria-label={$t('Move up')} title={$t('Move up')} on:click={() => move(index, -1)} disabled={disabled || index === 0}><svg viewBox="0 0 18 18"><path d="m5 11 4-4 4 4"></path></svg></button>
            <button type="button" aria-label={$t('Move down')} title={$t('Move down')} on:click={() => move(index, 1)} disabled={disabled || index === sources.length - 1}><svg viewBox="0 0 18 18"><path d="m5 7 4 4 4-4"></path></svg></button>
            <button class="source-remove" type="button" aria-label={$t('Remove source')} title={$t('Remove source')} on:click={() => remove(index)} {disabled}><svg viewBox="0 0 18 18"><path d="M4.5 5.5h9M7 5.5V4h4v1.5M6 7.5l.5 6h5l.5-6"></path></svg></button>
          </div>
        </div>

        {#if source.mode === 'remote' && source.authentication}
          <div class="source-authentication">
            <span class="source-auth-label">{$t('Basic authentication')}</span>
            <div class="source-auth-fields">
              <label>{$t('Username')}<input value={source.authentication.username} maxlength="256" autocomplete="off" on:input={(event) => updateAuthentication(index, 'username', event.currentTarget.value)} {disabled} /></label>
              <label>{$t('Password')}<input type="password" value={source.authentication.password ?? ''} maxlength="4096" autocomplete="new-password" placeholder={source.authentication.credentialId ? '••••••••' : ''} on:input={(event) => updateAuthentication(index, 'password', event.currentTarget.value)} {disabled} /></label>
            </div>
          </div>
        {/if}
      </article>
    {/each}
  </div>

  {#if sources.length === 0}
    <div class="source-empty">
      <span aria-hidden="true"><svg viewBox="0 0 24 24"><path d="M5 5h5l2 2h7v12H5Z"></path><path d="M9 13h6M12 10v6"></path></svg></span>
      <div><strong>{$t('No configuration sources')}</strong><small>{$t('Add a local file or HTTPS source')}</small></div>
    </div>
  {/if}
</div>

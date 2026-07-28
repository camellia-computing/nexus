<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { t } from '../../i18n';
  import type { MihomoDashboard } from '../../types';

  export let value: MihomoDashboard | undefined;
  export let disabled = false;

  const dispatch = createEventDispatcher<{
    change: MihomoDashboard | undefined;
  }>();

  const defaultValue = (): MihomoDashboard => ({ listenPort: 9092 });

  function toggle(enabled: boolean) {
    dispatch('change', enabled ? value ?? defaultValue() : undefined);
  }

  function update(patch: Partial<MihomoDashboard>) {
    dispatch('change', { ...(value ?? defaultValue()), ...patch });
  }
</script>

<section class="dashboard-suite mihomo-dashboard-suite">
  <header>
    <div><h3>{$t('Mihomo Dashboard')}</h3></div>
    <span>{value ? $t('active') : $t('inactive')}</span>
  </header>
  <div class="dashboard-grid single">
    <article class:enabled={!!value} class="dashboard-option mihomo">
      <header>
        <span class="dashboard-kind-icon" aria-hidden="true">
          <svg viewBox="0 0 24 24">
            <path d="M4 14c2-5 7-8 12-7 1.7.3 3 1 4 2"></path>
            <path d="M20 10c-2 5-7 8-12 7-1.7-.3-3-1-4-2"></path>
            <path d="m7 14 2.5-4 2.5 4 2.5-4 2.5 4"></path>
          </svg>
        </span>
        <div>
          <strong>{$t('External Mihomo Dashboard')}</strong>
          <small>{$t('Loopback REST API with an external Web UI')}</small>
        </div>
        <label class="compact-switch">
          <input
            type="checkbox"
            aria-label={$t('Enable Mihomo Dashboard')}
            checked={!!value}
            on:change={(event) => toggle(event.currentTarget.checked)}
            {disabled}
          />
          <span></span>
        </label>
      </header>
      {#if value}
        <div class="dashboard-fields">
          <label>
            {$t('Port')}
            <input
              type="number"
              min="1024"
              max="65535"
              value={value.listenPort}
              on:input={(event) => update({ listenPort: event.currentTarget.valueAsNumber })}
              {disabled}
            />
          </label>
          <label class="wide">
            <span class="field-caption"><span>{$t('Download URL')}</span><em>{$t('(Optional)')}</em></span>
            <input
              maxlength="2048"
              value={value.downloadUrl ?? ''}
              on:input={(event) => update({ downloadUrl: event.currentTarget.value || undefined })}
              placeholder="https://example.com/dashboard.zip"
              {disabled}
            />
          </label>
        </div>
      {/if}
    </article>
  </div>
</section>

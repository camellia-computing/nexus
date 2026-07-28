<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { t } from '../../i18n';
  import type { XrayDashboard } from '../../types';

  export let value: XrayDashboard | undefined;
  export let disabled = false;

  const dispatch = createEventDispatcher<{
    change: XrayDashboard | undefined;
  }>();

  const defaultValue = (): XrayDashboard => ({ apiPort: 10085, metricsPort: 11111 });

  function toggle(enabled: boolean) {
    dispatch('change', enabled ? value ?? defaultValue() : undefined);
  }

  function update(patch: Partial<XrayDashboard>) {
    dispatch('change', { ...(value ?? defaultValue()), ...patch });
  }
</script>

<section class="dashboard-suite xray-dashboard-suite">
  <header>
    <div><h3>{$t('Xray Dashboard')}</h3></div>
    <span>{value ? $t('active') : $t('inactive')}</span>
  </header>
  <div class="dashboard-grid single">
    <article class:enabled={!!value} class="dashboard-option xray">
      <header>
        <span class="dashboard-kind-icon" aria-hidden="true">
          <svg viewBox="0 0 24 24">
            <path d="M4.5 15.5 12 4l7.5 11.5"></path>
            <path d="M7.2 15.5h9.6L12 20Z"></path>
            <path d="M9.2 13h5.6"></path>
          </svg>
        </span>
        <div>
          <strong>{$t('Built-in Xray Dashboard')}</strong>
          <small>{$t('Uses local telemetry, handler topology, routing and logger control')}</small>
        </div>
        <label class="compact-switch">
          <input
            type="checkbox"
            aria-label={$t('Enable Xray Dashboard')}
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
            {$t('API port')}
            <input
              type="number"
              min="1024"
              max="65535"
              value={value.apiPort}
              on:input={(event) => update({ apiPort: event.currentTarget.valueAsNumber })}
              {disabled}
            />
          </label>
          <label>
            {$t('Metrics port')}
            <input
              type="number"
              min="1024"
              max="65535"
              value={value.metricsPort}
              on:input={(event) => update({ metricsPort: event.currentTarget.valueAsNumber })}
              {disabled}
            />
          </label>
        </div>
      {/if}
    </article>
  </div>
</section>

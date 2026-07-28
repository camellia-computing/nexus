<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { t } from '../../i18n';
  import type { SingBoxDashboardChange, SingBoxDashboardOptions } from './index';

  export let value: SingBoxDashboardOptions = {};
  export let disabled = false;

  const dispatch = createEventDispatcher<{
    change: SingBoxDashboardChange;
  }>();

  function toggleNative(enabled: boolean) {
    dispatch('change', {
      kind: 'native',
      value: enabled ? value.native ?? { listenPort: 9090, updateInterval: '1d' } : undefined,
    });
  }

  function toggleClash(enabled: boolean) {
    dispatch('change', {
      kind: 'clash',
      value: enabled ? value.clash ?? { listenPort: 9091 } : undefined,
    });
  }

  function updateNative(patch: Partial<NonNullable<SingBoxDashboardOptions['native']>>) {
    dispatch('change', {
      kind: 'native',
      value: { ...(value.native ?? { listenPort: 9090, updateInterval: '1d' }), ...patch },
    });
  }

  function updateClash(patch: Partial<NonNullable<SingBoxDashboardOptions['clash']>>) {
    dispatch('change', {
      kind: 'clash',
      value: { ...(value.clash ?? { listenPort: 9091 }), ...patch },
    });
  }
</script>

<section class="dashboard-suite">
  <header>
    <div><h3>{$t('Dashboard access')}</h3></div>
    <span>{Number(!!value.native) + Number(!!value.clash)} {$t('active')}</span>
  </header>
  <div class="dashboard-grid">
    <article class:enabled={!!value.native} class="dashboard-option native">
      <header>
        <span class="dashboard-kind-icon" aria-hidden="true"><svg viewBox="0 0 24 24"><path d="M6 4.5h12v15H6Z"></path><path d="M9 8h6M9 12h6M9 16h3"></path></svg></span>
        <div><strong>{$t('sing-box API')}</strong><small>{$t('Native gRPC dashboard')}</small></div>
        <label class="compact-switch"><input type="checkbox" aria-label={$t('Enable sing-box API')} checked={!!value.native} on:change={(event) => toggleNative(event.currentTarget.checked)} {disabled} /><span></span></label>
      </header>
      {#if value.native}
        <div class="dashboard-fields">
          <label>{$t('Port')}<input type="number" min="1024" max="65535" value={value.native.listenPort} on:input={(event) => updateNative({ listenPort: event.currentTarget.valueAsNumber })} {disabled} /></label>
          <label>{$t('Update interval')}<input maxlength="16" value={value.native.updateInterval} on:input={(event) => updateNative({ updateInterval: event.currentTarget.value })} placeholder="1d" {disabled} /></label>
        </div>
      {/if}
    </article>
    <article class:enabled={!!value.clash} class="dashboard-option clash">
      <header>
        <span class="dashboard-kind-icon" aria-hidden="true"><svg viewBox="0 0 24 24"><path d="m5 8 7-4 7 4-7 4Z"></path><path d="m5 12 7 4 7-4M5 16l7 4 7-4"></path></svg></span>
        <div><strong>{$t('Clash API')}</strong><small>{$t('REST API with external UI')}</small></div>
        <label class="compact-switch"><input type="checkbox" aria-label={$t('Enable Clash API')} checked={!!value.clash} on:change={(event) => toggleClash(event.currentTarget.checked)} {disabled} /><span></span></label>
      </header>
      {#if value.clash}
        <div class="dashboard-fields">
          <label>{$t('Port')}<input type="number" min="1024" max="65535" value={value.clash.listenPort} on:input={(event) => updateClash({ listenPort: event.currentTarget.valueAsNumber })} {disabled} /></label>
          <label class="wide"><span class="field-caption"><span>{$t('Download URL')}</span><em>{$t('(Optional)')}</em></span><input maxlength="2048" value={value.clash.downloadUrl ?? ''} on:input={(event) => updateClash({ downloadUrl: event.currentTarget.value || undefined })} placeholder="https://example.com/dashboard.zip" {disabled} /></label>
        </div>
      {/if}
    </article>
  </div>
</section>

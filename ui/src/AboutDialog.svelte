<script lang="ts">
  import type { ErrorInfo } from './api';
  import BrandMark from './lib/components/BrandMark.svelte';
  import Icon from './lib/components/Icon.svelte';
  import { focusTrap } from './lib/actions/focusTrap';
  import ErrorNotice from './ErrorNotice.svelte';
  import { t } from './i18n';
  import type { ApplicationInfo } from './types';

  export let info: ApplicationInfo | null;
  export let error: ErrorInfo | null = null;
  export let onClose: () => void;

  function signatureLabel(status: ApplicationInfo['signatureStatus']) {
    if (status === 'verified') return 'Verified publisher signature';
    if (status === 'notVerified') return 'No verified publisher signature';
    return 'Not checked on this platform';
  }
</script>

<div class="modal-backdrop about-backdrop" role="presentation" on:click={(event) => event.currentTarget === event.target && onClose()}>
  <div use:focusTrap={{ onEscape: onClose }} class="about-dialog" role="dialog" aria-modal="true" aria-labelledby="about-title">
    <button class="icon-button about-close" type="button" aria-label={$t('Close')} on:click={onClose}><Icon name="close" /></button>
    <div class="about-mark"><BrandMark size={64} /></div>
    <div class="about-heading">
      <p class="eyebrow">{$t('About')}</p>
      <h2 id="about-title">{info?.name ?? 'Camellia Nexus'}</h2>
      <p>{$t(info?.description ?? 'Cross-platform binary lifecycle manager')}</p>
    </div>
    {#if error}
      <ErrorNotice {error} />
    {:else if info}
      <dl class="about-details">
        <div><dt>{$t('Version')}</dt><dd>{info.version}</dd></div>
        <div><dt>{$t('Author')}</dt><dd>{info.author}</dd></div>
        <div><dt>{$t('License')}</dt><dd>{info.license}</dd></div>
        <div><dt>{$t('Digital signature')}</dt><dd class:verified={info.signatureStatus === 'verified'}>{$t(signatureLabel(info.signatureStatus))}</dd></div>
      </dl>
      <p class="about-copyright">{info.copyright}</p>
    {:else}
      <div class="about-loading">{$t('Loading')}…</div>
    {/if}
  </div>
</div>

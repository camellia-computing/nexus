<script lang="ts">
  import { focusTrap } from './lib/actions/focusTrap';
  import { t } from './i18n';

  export let title: string;
  export let message: string;
  export let confirmLabel = 'Continue';
  export let danger = false;
  export let onResolve: (confirmed: boolean) => void;

</script>

<div class="modal-backdrop confirm-backdrop" role="presentation" on:click={(event) => event.currentTarget === event.target && onResolve(false)}>
  <div use:focusTrap={{ onEscape: () => onResolve(false), initialFocus: '[data-cancel]' }} class="confirm-dialog" role="alertdialog" aria-modal="true" aria-labelledby="confirm-title" aria-describedby="confirm-message">
    <span class:danger class="confirm-symbol">{danger ? '!' : '?'}</span>
    <div class="confirm-copy">
      <h2 id="confirm-title">{title}</h2>
      <p id="confirm-message">{message}</p>
    </div>
    <div class="confirm-actions">
      <button data-cancel type="button" on:click={() => onResolve(false)}>{$t('Cancel')}</button>
      <button class:danger class:primary={!danger} type="button" on:click={() => onResolve(true)}>{confirmLabel}</button>
    </div>
  </div>
</div>

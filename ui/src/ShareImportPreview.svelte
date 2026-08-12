<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { t } from './i18n';
  import Icon from './lib/components/Icon.svelte';
  import { focusTrap } from './lib/actions/focusTrap';
  import type { ShareImportPreview } from './types';

  export let preview: ShareImportPreview;
  export let busy = false;
  const dispatch = createEventDispatcher<{ confirm: void; close: void }>();
  $: accepted = preview.summary.acceptedItems;
  $: rejected = preview.summary.rejectedItems;
  $: canConfirm = accepted > 0 && preview.items.some((item) => !!item.fragment && item.summary.blockingIssues.length === 0);
</script>

<div class="share-preview-backdrop" role="presentation" on:click={(event) => event.target === event.currentTarget && dispatch('close')}>
  <div use:focusTrap={{ onEscape: () => dispatch('close') }} class="share-preview-dialog" role="dialog" aria-modal="true" aria-labelledby="share-preview-title" aria-describedby="share-preview-description">
    <header>
      <div>
        <p class="eyebrow">{$t('Configuration import')}</p>
        <h2 id="share-preview-title">{$t('Preview share import')}</h2>
      </div>
      <button type="button" class="icon-button" aria-label={$t('Close')} on:click={() => dispatch('close')}><Icon name="close" size={18} /></button>
    </header>
    <p id="share-preview-description" class="share-preview-description">{$t('Review compatible items and warnings before creating this Inline source.')}</p>
    <div class="share-preview-summary" role="status" aria-live="polite">
      <span class="summary-chip">{preview.summary.payload}</span>
      <span class="summary-chip">{accepted} {$t('accepted')}</span>
      <span class:problem={rejected > 0} class="summary-chip">{rejected} {$t('rejected')}</span>
      <span class="summary-chip">{preview.summary.fidelity}</span>
    </div>
    {#if preview.summary.issues.length > 0}
      <div class="share-preview-notice" role="note">
        {#each preview.summary.issues.slice(0, 4) as issue}
          <p><strong>{issue.code}</strong> {issue.message}</p>
        {/each}
      </div>
    {/if}
    <div class="share-preview-items" aria-label={$t('Imported items')}>
      {#each preview.items as item (item.itemId)}
        <article class:rejected={!item.fragment || item.summary.blockingIssues.length > 0}>
          <div><strong>{String(item.semantic.name ?? item.itemId)}</strong><span>{String(item.semantic.protocol ?? '')}</span></div>
          <small>{item.summary.fidelity}{item.summary.warnings.length ? ` · ${item.summary.warnings.length} ${$t('warnings')}` : ''}</small>
        </article>
      {/each}
    </div>
    <footer>
      <button type="button" on:click={() => dispatch('close')} disabled={busy}>{$t('Cancel')}</button>
      <button class="primary" type="button" on:click={() => dispatch('confirm')} disabled={busy || !canConfirm}>{busy ? `${$t('Working')}…` : $t('Use accepted items')}</button>
    </footer>
  </div>
</div>

<style>
  .share-preview-backdrop { position: fixed; inset: 0; z-index: 70; display: grid; place-items: center; padding: 20px; background: rgba(8, 12, 20, .48); }
  .share-preview-dialog { width: min(720px, 100%); max-height: min(760px, calc(100vh - 40px)); overflow: hidden; display: grid; grid-template-rows: auto auto auto auto minmax(96px, 1fr) auto; gap: 14px; margin: 0; padding: 22px; border: 1px solid var(--line, rgba(120,140,170,.22)); border-radius: 18px; background: var(--panel, #fff); color: var(--text, #18202c); box-shadow: 0 24px 80px rgba(0,0,0,.28); }
  header, footer, .share-preview-summary, .share-preview-items article, .share-preview-items article > div { display: flex; align-items: center; }
  header, footer { justify-content: space-between; gap: 12px; }
  h2 { margin: 2px 0 0; font-size: 1.2rem; }
  .eyebrow { margin: 0; font-size: .72rem; letter-spacing: .08em; text-transform: uppercase; opacity: .6; }
  .icon-button { display: grid; flex: 0 0 auto; width: 34px; height: 34px; place-items: center; padding: 0; border: 0; border-radius: 8px; background: transparent; cursor: pointer; }
  .icon-button:hover { background: rgba(90,110,150,.1); }
  .share-preview-description { margin: 0; color: var(--muted, #667085); font-size: .84rem; line-height: 1.45; }
  .share-preview-summary { flex-wrap: wrap; gap: 7px; }
  .summary-chip { padding: 4px 9px; border-radius: 999px; background: rgba(80,120,190,.12); font-size: .78rem; }
  .summary-chip.problem, article.rejected { background: rgba(210,70,70,.12); }
  .share-preview-notice { padding: 10px 12px; border-radius: 10px; background: rgba(220,170,40,.12); font-size: .82rem; }
  .share-preview-notice p { margin: 3px 0; }
  .share-preview-items { min-height: 0; display: grid; align-content: start; gap: 7px; overflow: auto; overscroll-behavior: contain; }
  .share-preview-items article { justify-content: space-between; gap: 12px; padding: 10px 12px; border-radius: 10px; background: rgba(100,130,170,.08); }
  .share-preview-items article > div { min-width: 0; gap: 9px; }
  .share-preview-items strong { max-width: 60%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .share-preview-items span, .share-preview-items small { opacity: .68; font-size: .78rem; }
  footer { justify-content: flex-end; padding-top: 2px; }
  button.primary { padding: 8px 14px; border: 0; border-radius: 9px; background: var(--accent, #496fe5); color: white; cursor: pointer; }
  button:disabled { opacity: .45; cursor: not-allowed; }
  @media (max-width: 680px) { .share-preview-backdrop { padding: 8px; } .share-preview-dialog { max-height: calc(100dvh - 16px); padding: 16px; border-radius: 14px; } .share-preview-items article { align-items: flex-start; flex-direction: column; gap: 4px; } footer { flex-wrap: wrap; } footer button { flex: 1 1 140px; } }
  @media (max-height: 560px) { .share-preview-backdrop { padding: 6px; } .share-preview-dialog { max-height: calc(100dvh - 12px); gap: 9px; padding: 12px 14px; } .share-preview-description { display: none; } }
</style>

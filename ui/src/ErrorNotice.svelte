<script lang="ts">
  import { onDestroy } from 'svelte';
  import type { ErrorInfo } from './api';
  import { hasChineseTranslation, t, uiLanguage } from './i18n';
  import Icon from './lib/components/Icon.svelte';

  export let error: ErrorInfo;
  export let dismissible = false;
  export let autoDismissMs = 0;
  export let onDismiss: () => void = () => {};

  let dismissTimer: number | undefined;
  let scheduledKey = '';
  let pointerPaused = false;
  let focusPaused = false;

  function scheduleDismiss() {
    if (dismissTimer !== undefined) window.clearTimeout(dismissTimer);
    dismissTimer = scheduledKey && !pointerPaused && !focusPaused
      ? window.setTimeout(() => {
          dismissTimer = undefined;
          onDismiss();
        }, autoDismissMs)
      : undefined;
  }

  $: {
    const nextKey = autoDismissMs > 0
      ? `${error.code ?? ''}\u0000${error.title}\u0000${error.message}\u0000${autoDismissMs}`
      : '';
    if (nextKey !== scheduledKey) {
      scheduledKey = nextKey;
      scheduleDismiss();
    }
  }

  $: message =
    $uiLanguage === 'zh-CN' && !hasChineseTranslation(error.message)
      ? error.fallbackMessage
      : error.message;

  function dismiss() {
    if (dismissTimer !== undefined) window.clearTimeout(dismissTimer);
    dismissTimer = undefined;
    onDismiss();
  }

  function pauseForPointer(paused: boolean) {
    pointerPaused = paused;
    scheduleDismiss();
  }

  function updateFocusPause(event: FocusEvent) {
    const owner = event.currentTarget as HTMLElement;
    focusPaused = event.type === 'focusin'
      || (event.relatedTarget instanceof Node && owner.contains(event.relatedTarget));
    scheduleDismiss();
  }

  onDestroy(() => {
    if (dismissTimer !== undefined) window.clearTimeout(dismissTimer);
  });
</script>

<div
  class="error-notice"
  role="alert"
  aria-live="assertive"
  on:pointerenter={() => pauseForPointer(true)}
  on:pointerleave={() => pauseForPointer(false)}
  on:focusin={updateFocusPause}
  on:focusout={updateFocusPause}
>
  <span class="error-symbol">!</span>
  <div>
    <strong>{$t(error.title)}</strong>
    <p>{$t(message)}</p>
    {#if error.details}
      <details class="error-details"><summary>{$t('Technical details')}</summary><pre>{error.details}</pre></details>
    {/if}
    <small>{$t(error.suggestion)}</small>
  </div>
  {#if dismissible}<button class="icon-button" type="button" aria-label={$t('Dismiss error')} on:click={dismiss}><Icon name="close" /></button>{/if}
</div>

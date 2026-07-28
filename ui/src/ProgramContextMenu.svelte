<script lang="ts">
  import { afterUpdate, onMount } from 'svelte';
  import Icon from './lib/components/Icon.svelte';
  import { programDefinition } from './programs/registry';
  import type { ProgramSummary } from './types';
  import { t } from './i18n';

  export let program: ProgramSummary;
  export let x: number;
  export let y: number;
  export let busy = false;
  export let stopBusy = false;
  export let canActivate = false;
  export let licenseActionHint = '';
  export let canMoveUp = false;
  export let canMoveDown = false;
  export let onOpenFolder: () => void;
  export let onStart: () => void;
  export let onStop: () => void;
  export let onRestart: () => void;
  export let onMoveUp: () => void;
  export let onMoveDown: () => void;
  export let onDelete: () => void;
  export let onClose: () => void;

  let menuElement: HTMLDivElement;
  let menuX = x;
  let menuY = y;
  let menuResizeObserver: ResizeObserver | undefined;

  $: canStart = ['stopped', 'exited', 'error'].includes(program.state.status);
  $: canStop = program.state.status !== 'stopped';
  $: canRestart = program.state.status === 'running' || program.state.status === 'backoff';
  $: hasLifecycleAction = canStart || canRestart;
  $: kindLabel = programDefinition(program.kind).displayName;
  $: showKindLabel = normalizeIdentityLabel(program.name) !== normalizeIdentityLabel(kindLabel);

  function normalizeIdentityLabel(value: string) {
    return value.normalize('NFKC').trim().toLowerCase().replace(/[\s_-]+/g, '');
  }

  function clampMenuToViewport() {
    if (!menuElement) return;

    // Keep one extra CSS pixel for fractional layout and animated transforms.
    const safeInset = 9;
    const bounds = menuElement.getBoundingClientRect();
    const requestedX = Number.isFinite(x) ? x : safeInset;
    const requestedY = Number.isFinite(y) ? y : safeInset;
    const measuredScaleX = menuElement.offsetWidth > 0 ? bounds.width / menuElement.offsetWidth : 1;
    const measuredScaleY = menuElement.offsetHeight > 0 ? bounds.height / menuElement.offsetHeight : 1;
    const scaleX = Number.isFinite(measuredScaleX) && measuredScaleX > 0 ? measuredScaleX : 1;
    const scaleY = Number.isFinite(measuredScaleY) && measuredScaleY > 0 ? measuredScaleY : 1;
    const maximumX = Math.max(safeInset, window.innerWidth - bounds.width - safeInset);
    const maximumY = Math.max(safeInset, window.innerHeight - bounds.height - safeInset);
    const nextX = Math.min(Math.max(requestedX * scaleX, safeInset), maximumX) / scaleX;
    const nextY = Math.min(Math.max(requestedY * scaleY, safeInset), maximumY) / scaleY;

    if (menuX !== nextX) menuX = nextX;
    if (menuY !== nextY) menuY = nextY;
  }

  afterUpdate(clampMenuToViewport);

  onMount(() => {
    (menuElement.querySelector<HTMLButtonElement>('[role="menuitem"]:not(:disabled)') ?? menuElement).focus();
    menuResizeObserver = new ResizeObserver(clampMenuToViewport);
    menuResizeObserver.observe(menuElement);
    window.addEventListener('resize', clampMenuToViewport);
    menuElement.addEventListener('animationend', clampMenuToViewport);
    clampMenuToViewport();

    return () => {
      menuResizeObserver?.disconnect();
      window.removeEventListener('resize', clampMenuToViewport);
      menuElement.removeEventListener('animationend', clampMenuToViewport);
    };
  });

  function handleMenuKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      onClose();
      return;
    }
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;

    const menu = event.currentTarget as HTMLElement;
    const items = Array.from(menu.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)'));
    if (!items.length) return;
    event.preventDefault();
    const current = items.indexOf(document.activeElement as HTMLButtonElement);
    const next = event.key === 'Home'
      ? 0
      : event.key === 'End'
        ? items.length - 1
        : event.key === 'ArrowDown'
          ? (current + 1 + items.length) % items.length
          : (current - 1 + items.length) % items.length;
    items[next].focus();
  }
</script>

<div bind:this={menuElement} class="program-context-menu" role="menu" tabindex="-1" aria-label={`${program.name} ${$t('actions')}`} style={`left:${menuX}px;top:${menuY}px`} on:keydown={handleMenuKeydown}>
  <div class="context-menu-title"><span class:error-state={program.state.status === 'error' || program.state.status === 'stopFailed'} class:running={program.state.status === 'running'} class="dot"></span><span class="context-menu-copy"><strong>{program.name}</strong>{#if showKindLabel}<small>{kindLabel}</small>{/if}</span></div>
  {#if canStart}<button role="menuitem" type="button" on:click={onStart} disabled={busy || !canActivate} title={$t(canActivate ? 'Start' : licenseActionHint)}><Icon name="start" size={16} />{$t('Start')}</button>
  {:else if program.state.status === 'backoff'}<button role="menuitem" type="button" on:click={onRestart} disabled={busy || !canActivate} title={$t(canActivate ? 'Retry now' : licenseActionHint)}><Icon name="restart" size={16} />{$t('Retry now')}</button>
  {:else if canRestart}<button role="menuitem" type="button" on:click={onRestart} disabled={busy || !canActivate} title={$t(canActivate ? 'Restart' : licenseActionHint)}><Icon name="restart" size={16} />{$t('Restart')}</button>
  {:else}<button role="menuitem" type="button" on:click={onOpenFolder}><Icon name="folder" size={16} />{$t('Open working folder')}</button>{/if}
  {#if canStop}<button role="menuitem" type="button" on:click={onStop} disabled={stopBusy}><Icon name="stop" size={15} />{$t(program.state.status === 'backoff' ? 'Stop retries' : 'Stop')}</button>{/if}
  <div class="context-separator"></div>
  {#if hasLifecycleAction}<button role="menuitem" type="button" on:click={onOpenFolder}><Icon name="folder" size={16} />{$t('Open working folder')}</button>{/if}
  <div class="context-order-actions"><button role="menuitem" type="button" on:click={onMoveUp} disabled={!canMoveUp}><Icon name="arrow-up" size={15} />{$t('Move up')}</button><button role="menuitem" type="button" on:click={onMoveDown} disabled={!canMoveDown}><Icon name="arrow-down" size={15} />{$t('Move down')}</button></div>
  <div class="context-separator"></div>
  <button class="context-danger" role="menuitem" type="button" on:click={onDelete} disabled={busy}><Icon name="trash" size={16} />{$t('Delete program')}</button>
</div>

<style>
  .program-context-menu {
    max-width: calc(100vw - 18px);
    max-height: calc(100vh - 18px);
    overflow-x: hidden;
    overflow-y: auto;
    overscroll-behavior: contain;
  }
</style>

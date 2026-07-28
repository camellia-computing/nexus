<script context="module" lang="ts">
  let nextPreviewId = 0;
</script>

<script lang="ts">
  import { createEventDispatcher, onMount, tick } from 'svelte';

  export let value = '';
  export let ariaLabel = '';
  export let placeholder = '';
  export let maxlength = 2048;
  export let disabled = false;
  export let className = '';

  const previewId = `overflow-preview-${++nextPreviewId}`;
  const dispatch = createEventDispatcher<{
    input: { value: string; event: Event };
  }>();

  let wrapper: HTMLSpanElement;
  let input: HTMLInputElement;
  let preview: HTMLSpanElement;
  let hovered = false;
  let focused = false;
  let dismissed = false;
  let overflowing = false;
  let previewStyle = '';

  $: visible = !!value && overflowing && !dismissed && (hovered || focused);

  function measure() {
    if (!input) return;
    overflowing = input.scrollWidth - input.clientWidth > 1;
  }

  async function positionPreview() {
    measure();
    if (!wrapper || !value || !overflowing || dismissed || (!hovered && !focused)) return;
    const bounds = wrapper.getBoundingClientRect();
    const viewportGap = 12;
    const maxWidth = Math.max(0, window.innerWidth - viewportGap * 2);
    const width = Math.min(maxWidth, Math.max(bounds.width, 360));
    const left = Math.min(
      Math.max(viewportGap, bounds.left),
      Math.max(viewportGap, window.innerWidth - width - viewportGap),
    );
    let top = bounds.bottom + 6;
    previewStyle = `left:${left}px;top:${top}px;width:${width}px`;
    await tick();
    if (!preview || !value || dismissed || (!hovered && !focused)) return;
    const previewHeight = preview.getBoundingClientRect().height;
    const spaceBelow = window.innerHeight - bounds.bottom;
    const spaceAbove = bounds.top;
    if (top + previewHeight > window.innerHeight - viewportGap && spaceAbove > spaceBelow) {
      top = Math.max(viewportGap, bounds.top - previewHeight - 6);
      previewStyle = `left:${left}px;top:${top}px;width:${width}px`;
    }
  }

  function inputValue(event: Event) {
    value = (event.currentTarget as HTMLInputElement).value;
    dismissed = false;
    dispatch('input', { value, event });
    void tick().then(positionPreview);
  }

  function enter() {
    hovered = true;
    dismissed = false;
    void tick().then(positionPreview);
  }

  function leave() {
    hovered = false;
  }

  function focus() {
    focused = true;
    dismissed = false;
    void tick().then(positionPreview);
  }

  function blur() {
    focused = false;
  }

  function keydown(event: KeyboardEvent) {
    if (event.key !== 'Escape' || !visible) return;
    dismissed = true;
    event.stopPropagation();
  }

  onMount(() => {
    const observer = new ResizeObserver(() => void positionPreview());
    observer.observe(input);
    window.addEventListener('resize', positionPreview);
    window.addEventListener('scroll', positionPreview, true);
    measure();
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', positionPreview);
      window.removeEventListener('scroll', positionPreview, true);
    };
  });
</script>

<span
  bind:this={wrapper}
  class={`overflow-preview-field ${className}`.trim()}
  data-overflowing={overflowing}
>
  <input
    bind:this={input}
    class="source-address"
    {value}
    {maxlength}
    {placeholder}
    {disabled}
    aria-label={ariaLabel || undefined}
    aria-describedby={visible ? previewId : undefined}
    on:input={inputValue}
    on:mouseenter={enter}
    on:mouseleave={leave}
    on:focus={focus}
    on:blur={blur}
    on:keydown={keydown}
  />
  {#if visible}
    <span
      bind:this={preview}
      id={previewId}
      class="overflow-preview"
      role="tooltip"
      style={previewStyle}
    >{value}</span>
  {/if}
</span>

<style>
  .overflow-preview-field {
    display: grid;
    min-width: 0;
  }

  input {
    min-width: 0;
  }

  .overflow-preview {
    position: fixed;
    z-index: 90;
    max-height: min(220px, calc(100dvh - 24px));
    overflow: auto;
    border: var(--ui-border-width) solid var(--ui-border-strong);
    border-radius: var(--ui-radius-sm);
    background: var(--ui-surface-overlay);
    padding: 9px 11px;
    color: var(--ui-text-primary);
    box-shadow: var(--ui-shadow-overlay);
    font-family: var(--ui-font-mono);
    font-size: var(--ui-font-size-xs);
    font-weight: var(--ui-weight-regular);
    line-height: var(--ui-line-height-body);
    overflow-wrap: anywhere;
    pointer-events: none;
    white-space: normal;
  }
</style>

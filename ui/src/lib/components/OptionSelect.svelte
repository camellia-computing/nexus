<script lang="ts">
  import { createEventDispatcher } from 'svelte';

  interface OptionItem {
    value: string | number;
    label: string;
    disabled?: boolean;
  }

  export let value: string | number = '';
  export let options: OptionItem[] = [];
  export let ariaLabel = '';
  export let disabled = false;
  export let align: 'center' | 'start' = 'center';
  export let size: 'md' | 'lg' = 'lg';
  export let width: 'content' | 'fill' = 'content';
  export let title = '';

  const dispatch = createEventDispatcher<{
    change: { value: string | number; event: Event };
  }>();

  function changed(event: Event) {
    value = (event.currentTarget as HTMLSelectElement).value;
    const option = options.find((candidate) => String(candidate.value) === String(value));
    const selectedValue = option?.value ?? value;
    value = selectedValue;
    dispatch('change', { value: selectedValue, event });
  }
</script>

<span
  class:content={width === 'content'}
  class:fill={width === 'fill'}
  class:center={align === 'center'}
  class:start={align === 'start'}
  class="option-select"
>
  {#if width === 'content'}
    <span class="option-measure" aria-hidden="true">
      {#each options as option}<span>{option.label}</span>{/each}
    </span>
  {/if}
  <select
    {disabled}
    aria-label={ariaLabel || undefined}
    data-option-align={align}
    data-control-size={size}
    {title}
    {value}
    on:change={changed}
  >
    {#each options as option}
      <option value={option.value} disabled={option.disabled}>{option.label}</option>
    {/each}
  </select>
</span>

<style>
  .option-select {
    display: grid;
    min-width: 0;
    max-width: 100%;
  }

  .option-select.content {
    width: max-content;
  }

  .option-select.fill {
    width: 100%;
  }

  select,
  .option-measure {
    grid-area: 1 / 1;
  }

  select {
    width: 100%;
    min-width: 0;
  }

  .center select {
    text-align: center;
    text-align-last: center;
  }

  .start select {
    text-align: start;
    text-align-last: start;
  }

  .option-measure {
    display: grid;
    height: 0;
    overflow: hidden;
    padding-inline: 38px;
    visibility: hidden;
    white-space: nowrap;
  }

  .option-measure > span {
    width: max-content;
  }

  @media (max-width: 540px), (pointer: coarse) {
    select {
      min-height: 44px;
    }
  }
</style>

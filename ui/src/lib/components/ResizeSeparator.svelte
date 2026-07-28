<script lang="ts">
  interface Props {
    label: string;
    value: number;
    min: number;
    max: number;
    onPointerDown: (event: PointerEvent) => void;
    onKeyDown: (event: KeyboardEvent) => void;
  }

  let { label, value, min, max, onPointerDown, onKeyDown }: Props = $props();
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex (WAI-ARIA Window Splitter pattern) -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions (WAI-ARIA Window Splitter pattern) -->
<div
  class="resize-separator"
  role="separator"
  tabindex="0"
  aria-orientation="horizontal"
  aria-valuemin={min}
  aria-valuemax={max}
  aria-valuenow={value}
  aria-label={`${label} (${value} px)`}
  onpointerdown={onPointerDown}
  onkeydown={onKeyDown}
></div>

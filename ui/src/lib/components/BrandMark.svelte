<script lang="ts">
  interface Props {
    size?: number;
  }

  let { size = 34 }: Props = $props();
</script>

<svg class="brand-mark" width={size} height={size} viewBox="0 0 36 36" aria-hidden="true">
  <path class="petal petal-one" d="M18 17.6C12.2 17.6 7.8 14 7.8 9.6 7.8 6 10.5 3.5 14 3.5c3.6 0 5.2 3 4 6.6Z" />
  <path class="petal petal-two" d="M18.4 18c0-5.8 3.5-10.1 8-10.1 3.5 0 6.1 2.7 6.1 6.2 0 3.6-3.1 5.2-6.7 4Z" />
  <path class="petal petal-three" d="M18 18.4c5.8 0 10.1 3.5 10.1 8 0 3.5-2.7 6.1-6.2 6.1-3.6 0-5.2-3.1-4-6.7Z" />
  <path class="petal petal-four" d="M17.6 18c0 5.8-3.5 10.1-8 10.1-3.5 0-6.1-2.7-6.1-6.2 0-3.6 3.1-5.2 6.7-4Z" />
  <circle cx="18" cy="18" r="5.2" />
  <circle class="core" cx="18" cy="18" r="1.7" />
</svg>

<style>
  .brand-mark {
    display: block;
    overflow: visible;
  }

  .petal,
  circle {
    fill: var(--ui-brand, #b53e52);
  }

  .petal-two,
  .petal-four {
    opacity: 0.78;
  }

  .core {
    fill: var(--ui-on-brand, white);
  }
</style>

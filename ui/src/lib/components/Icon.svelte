<script lang="ts">
  export type IconName =
    | 'add'
    | 'activity'
    | 'alert'
    | 'arrow-down'
    | 'arrow-up'
    | 'back'
    | 'check'
    | 'chevron'
    | 'clock'
    | 'close'
    | 'config'
    | 'dashboard'
    | 'details'
    | 'external'
    | 'filter'
    | 'folder'
    | 'format'
    | 'general'
    | 'grid'
    | 'home'
    | 'info'
    | 'logs'
    | 'lock'
    | 'menu'
    | 'moon'
    | 'palette'
    | 'redo'
    | 'restart'
    | 'replace'
    | 'search'
    | 'settings'
    | 'shield'
    | 'sliders'
    | 'start'
    | 'stop'
    | 'suggestions'
    | 'sun'
    | 'trash'
    | 'undo';

  interface Props {
    name: IconName;
    size?: number;
    strokeWidth?: number;
    label?: string;
  }

  let { name, size = 18, strokeWidth = 1.7, label }: Props = $props();
</script>

<svg
  class="icon"
  width={size}
  height={size}
  viewBox="0 0 20 20"
  fill="none"
  stroke="currentColor"
  stroke-width={strokeWidth}
  stroke-linecap="round"
  stroke-linejoin="round"
  role={label ? 'img' : undefined}
  aria-label={label}
  aria-hidden={label ? undefined : 'true'}
>
  {#if name === 'add'}
    <path d="M10 4v12M4 10h12" />
  {:else if name === 'activity'}
    <path d="M2.5 10h3l2-5.2 4.2 10.4 2.1-5.2h3.7" />
  {:else if name === 'alert'}
    <path d="M10 2.8 18 17H2Z" /><path d="M10 7v4.5M10 14.3v.1" />
  {:else if name === 'arrow-down'}
    <path d="M10 3.5v13M5.5 12l4.5 4.5 4.5-4.5" />
  {:else if name === 'arrow-up'}
    <path d="M10 16.5v-13M5.5 8 10 3.5 14.5 8" />
  {:else if name === 'back'}
    <path d="m11.5 4.5-5.5 5.5 5.5 5.5M6 10h9" />
  {:else if name === 'check'}
    <path d="m4.5 10.2 3.3 3.3 7.7-7.7" />
  {:else if name === 'chevron'}
    <path d="m7.5 4.5 5.5 5.5-5.5 5.5" />
  {:else if name === 'clock'}
    <circle cx="10" cy="10" r="7" /><path d="M10 6v4.2l2.8 1.7" />
  {:else if name === 'close'}
    <path d="m5 5 10 10M15 5 5 15" />
  {:else if name === 'config'}
    <path d="M3 3h9.5L17 7.5V17H3Z" /><path d="M12.5 3v4.5H17M6.5 10.2h7M6.5 13.3h4.8" />
  {:else if name === 'dashboard'}
    <rect x="3" y="3" width="14" height="14" rx="2" /><path d="M3 8h14M8 8v9" />
  {:else if name === 'details'}
    <rect x="3" y="3" width="14" height="14" rx="2" /><path d="M7 7h6M7 10h6M7 13h4" />
  {:else if name === 'external'}
    <path d="M11 4h5v5M9 11l7-7M15 11v4.5H4.5v-11H9" />
  {:else if name === 'filter'}
    <path d="M3 4h14l-5.3 6v5l-3.4 1v-6Z" />
  {:else if name === 'folder'}
    <path d="M2.8 6.3h5l1.5 1.6h7.9v7.3H2.8Z" /><path d="M2.8 6.3V4.8h5.1l1.4 1.5" />
  {:else if name === 'format'}
    <path d="M7 3.5H5.5v4L3.5 10l2 2.5v4H7M13 3.5h1.5v4l2 2.5-2 2.5v4H13M9 7h2M9 10h2M9 13h2" />
  {:else if name === 'general'}
    <circle cx="10" cy="6.3" r="3" /><path d="M4.5 16.5c.5-3.1 2.3-4.7 5.5-4.7s5 1.6 5.5 4.7" />
  {:else if name === 'grid'}
    <rect x="3" y="3" width="5.5" height="5.5" rx="1" /><rect x="11.5" y="3" width="5.5" height="5.5" rx="1" /><rect x="3" y="11.5" width="5.5" height="5.5" rx="1" /><rect x="11.5" y="11.5" width="5.5" height="5.5" rx="1" />
  {:else if name === 'home'}
    <path d="m2.8 9.2 7.2-6 7.2 6" /><path d="M4.6 8v8.5h10.8V8M8 16.5v-5h4v5" />
  {:else if name === 'info'}
    <circle cx="10" cy="10" r="7" /><path d="M10 9v5M10 6.2v.1" />
  {:else if name === 'logs'}
    <rect x="3" y="3" width="14" height="14" rx="1.5" /><path d="m6.5 7.5 2 2-2 2M10.5 12h3" />
  {:else if name === 'lock'}
    <path d="M6.5 8V6.8a3.5 3.5 0 0 1 7 0V8" /><rect x="4.5" y="8" width="11" height="8.5" rx="2.5" /><path d="M10 11.2v2.2" />
  {:else if name === 'menu'}
    <path d="M4 6h12M4 10h12M4 14h12" />
  {:else if name === 'moon'}
    <path d="M15.8 12.6A6.6 6.6 0 0 1 7.4 4.2 6.7 6.7 0 1 0 15.8 12.6Z" />
  {:else if name === 'palette'}
    <path d="M10 2.8a7.2 7.2 0 1 0 0 14.4h1.2a1.7 1.7 0 0 0 0-3.4h-.8a1.4 1.4 0 0 1 0-2.8h2.8A4 4 0 0 0 17.2 7c0-2.3-3.2-4.2-7.2-4.2Z" /><path d="M6.2 7v.1M9 5.4v.1M5.7 10.2v.1" />
  {:else if name === 'redo'}
    <path d="M13 5.5h3.5V2M16.2 5.5a7 7 0 1 0 .6 7" />
  {:else if name === 'restart'}
    <path d="M15.6 9a5.6 5.6 0 1 0-1.3 4.1" /><path d="M15.8 5.4V9h-3.6" />
  {:else if name === 'replace'}
    <path d="M3 6.5h11.5M11.5 3.5l3 3-3 3M17 13.5H5.5M8.5 10.5l-3 3 3 3" />
  {:else if name === 'search'}
    <circle cx="8.5" cy="8.5" r="5.2" /><path d="m12.4 12.4 4.1 4.1" />
  {:else if name === 'settings'}
    <circle cx="10" cy="10" r="2.8" /><path d="M16 11.4a6 6 0 0 0 0-2.8l1.7-1.3-1.6-2.7-2.1.8a6.8 6.8 0 0 0-2.3-1.3L11.4 2H8.6l-.3 2.1A6.8 6.8 0 0 0 6 5.4l-2.1-.8-1.6 2.7L4 8.6a6 6 0 0 0 0 2.8l-1.7 1.3 1.6 2.7 2.1-.8a6.8 6.8 0 0 0 2.3 1.3l.3 2.1h2.8l.3-2.1a6.8 6.8 0 0 0 2.3-1.3l2.1.8 1.6-2.7Z" />
  {:else if name === 'shield'}
    <path d="M10 2.5c2 1.4 4.1 2.1 6.2 2.3v4.7c0 3.7-2.1 6.4-6.2 8-4.1-1.6-6.2-4.3-6.2-8V4.8C5.9 4.6 8 3.9 10 2.5Z" /><path d="m7.2 10 1.8 1.8 3.8-4" />
  {:else if name === 'sliders'}
    <path d="M4 5h12M4 10h12M4 15h12" /><circle cx="7" cy="5" r="1.5" fill="var(--ui-surface-1, white)" /><circle cx="13" cy="10" r="1.5" fill="var(--ui-surface-1, white)" /><circle cx="9" cy="15" r="1.5" fill="var(--ui-surface-1, white)" />
  {:else if name === 'start'}
    <path d="m7 4.5 8 5.5-8 5.5Z" />
  {:else if name === 'stop'}
    <rect x="5" y="5" width="10" height="10" rx="1.5" />
  {:else if name === 'suggestions'}
    <path d="m10 2 .9 2.7L13.5 6l-2.6 1.3L10 10l-.9-2.7L6.5 6l2.6-1.3ZM15.5 10.5l.6 1.8 1.9.7-1.9.7-.6 1.8-.6-1.8L13 13l1.9-.7ZM5 11.5l.7 2.1 2.1.9-2.1.8L5 17.5l-.7-2.2-2.1-.8 2.1-.9Z" />
  {:else if name === 'sun'}
    <circle cx="10" cy="10" r="3.2" /><path d="M10 2v2M10 16v2M2 10h2M16 10h2M4.3 4.3l1.4 1.4M14.3 14.3l1.4 1.4M15.7 4.3l-1.4 1.4M5.7 14.3l-1.4 1.4" />
  {:else if name === 'trash'}
    <path d="M5.5 6.5h9M8 6.5v-2h4v2M7 8.5l.5 7h5l.5-7" />
  {:else if name === 'undo'}
    <path d="M7 5.5H3.5V2M3.8 5.5a7 7 0 1 1-.6 7" />
  {/if}
</svg>

<style>
  .icon {
    display: block;
    flex: 0 0 auto;
    pointer-events: none;
  }
</style>

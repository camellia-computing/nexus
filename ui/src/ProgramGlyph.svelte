<script lang="ts">
  import type { ProgramKind } from './types';
  import GenericGlyph from './programs/generic/Glyph.svelte';
  import MihomoGlyph from './programs/mihomo/Glyph.svelte';
  import SingBoxGlyph from './programs/sing-box/Glyph.svelte';
  import XrayGlyph from './programs/xray/Glyph.svelte';

  export let kind: ProgramKind;
</script>

{#if kind === 'generic'}
  <GenericGlyph />
{:else if kind === 'singBox'}
  <SingBoxGlyph />
{:else if kind === 'xray'}
  <XrayGlyph />
{:else if kind === 'mihomo'}
  <MihomoGlyph />
{/if}

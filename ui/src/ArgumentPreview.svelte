<script lang="ts">
  import type { ArgumentParseResult } from './arguments';
  import { t } from './i18n';

  export let result: ArgumentParseResult;
</script>

<div class:error={!!result.error} class="argument-preview" aria-live="polite">
  {#if result.error}
    <strong>{$t('Cannot parse arguments')}</strong><span>{$t(result.error)}</span>
  {:else}
    <div class="preview-heading"><strong>{$t('argv preview')}</strong><span>{result.args.length} {$t(result.args.length === 1 ? 'value' : 'values')}</span></div>
    <div class="argument-chips">
      {#each result.args as argument, index}
        <span><i>{index + 1}</i><code>{argument || $t('(empty)')}</code></span>
      {/each}
      {#if result.args.length === 0}<small>{$t('No arguments')}</small>{/if}
    </div>
    {#each result.warnings as warning}<p class="argument-warning">{$t(warning)}</p>{/each}
  {/if}
</div>

<script lang="ts">
  import ProgramGlyph from '../../ProgramGlyph.svelte';
  import { t } from '../../i18n';
  import type { ProgramSummary } from '../../types';

  interface Props {
    program: ProgramSummary | null;
    onCancel: () => void;
  }

  let { program, onCancel }: Props = $props();
</script>

<section class="program-detail-loading" aria-busy="true">
  <header class="page-header program-hero program-loading-hero">
    <div class="page-title program-hero-identity">
      <div class="detail-heading">
        <span class={`detail-program-icon ${program?.kind ?? 'generic'}`} aria-hidden="true">
          <ProgramGlyph kind={program?.kind ?? 'generic'} />
        </span>
        <div class="detail-heading-copy">
          <p class="eyebrow">
            {program
              ? program.kind === 'singBox'
                ? 'sing-box'
                : program.kind
              : $t('Programs')}
          </p>
          <h1>{program?.name ?? $t('Opening program')}</h1>
          <p class="program-loading-state">
            <span class="program-selection-spinner" aria-hidden="true"></span>
            <span>{$t('Opening program')}</span>
          </p>
        </div>
      </div>
    </div>
  </header>

  <div class="panel program-loading-panel">
    <div
      class="program-loading-message"
      role="status"
      aria-live="polite"
      aria-atomic="true"
    >
      <span class="program-detail-spinner" aria-hidden="true"></span>
      <span>
        <strong>{$t('Loading program details')}</strong>
        <small>{$t('Retrieving program status and available actions.')}</small>
      </span>
    </div>
    <button type="button" onclick={onCancel}>{$t('Cancel')}</button>
  </div>
</section>

<script lang="ts">
  import Icon from '../../lib/components/Icon.svelte';
  import ProgramGlyph from '../../ProgramGlyph.svelte';
  import { t } from '../../i18n';
  import { isRuntimeActive } from '../../programState';
  import { programDefinitions } from '../../programs/registry';
  import type { InvalidProgram, ProgramKind, ProgramSummary } from '../../types';

  interface Props {
    programs: ProgramSummary[];
    invalidPrograms: InvalidProgram[];
    runningCount: number;
    issueCount: number;
    autoStartCount: number;
    createBlockReason?: string;
    stateNameKey: (state: ProgramSummary['state']) => string;
    onCreate: (kind: ProgramKind) => void;
    onSelect: (id: string) => void;
  }

  let {
    programs,
    invalidPrograms,
    runningCount,
    issueCount,
    autoStartCount,
    createBlockReason = '',
    stateNameKey,
    onCreate,
    onSelect,
  }: Props = $props();

  let activePrograms = $derived(programs.filter((program) => isRuntimeActive(program.state)));
  let visibleActivePrograms = $derived(activePrograms.slice(0, 6));
  let runtimeTone = $derived(issueCount > 0 ? 'attention' : runningCount > 0 ? 'active' : 'idle');
</script>

<div class="home-workspace home-dashboard" data-runtime-tone={runtimeTone}>
  <header class="home-heading home-hero">
    <div class="home-hero-copy">
      <p class="eyebrow">Camellia Nexus</p>
      <h1>{$t('Workspace')}</h1>
      <p>{$t('Manage programs, configuration and runtime health from one place.')}</p>
    </div>
    <div class="home-hero-context" aria-label={$t('Current status')}>
      <Icon name={issueCount > 0 ? 'alert' : 'activity'} size={18} />
      <span>{$t(issueCount > 0 ? 'Issues' : runningCount > 0 ? 'Running' : 'Ready')}</span>
    </div>
  </header>

  {#if invalidPrograms.length}
    <section class="workspace-recovery" aria-labelledby="workspace-recovery-title">
      <span aria-hidden="true"><Icon name="alert" size={20} /></span>
      <div>
        <h2 id="workspace-recovery-title">{$t('Workspace recovery required')}</h2>
        <p>{$t('Some program folders were not loaded. Their files were left unchanged.')}</p>
        <details>
          <summary>{invalidPrograms.length} {$t('invalid program folders')}</summary>
          <ul>{#each invalidPrograms as invalid}<li><code>{invalid.path}</code><span>{invalid.error}</span></li>{/each}</ul>
        </details>
      </div>
    </section>
  {/if}

  <section class="runtime-stage status-bento" aria-labelledby="runtime-overview-title">
    <h2 id="runtime-overview-title" class="visually-hidden">{$t('Current status')}</h2>

    <article class="status-card status-card-primary status-running">
      <div class="status-card-heading">
        <span class="status-card-icon" aria-hidden="true"><Icon name="activity" size={21} /></span>
        <span>{$t('Running')}</span>
      </div>
      <strong class="status-card-value">{runningCount}</strong>
      <p class="status-card-meta">{runningCount}/{programs.length} {$t('Programs')}</p>
    </article>

    <div class="status-support-grid">
      <article class:has-issues={issueCount > 0} class="status-card status-issues">
        <span class="status-card-icon" aria-hidden="true"><Icon name="alert" size={18} /></span>
        <div class="status-card-copy"><span>{$t('Issues')}</span><strong>{issueCount}</strong></div>
      </article>
      <article class="status-card status-automatic">
        <span class="status-card-icon" aria-hidden="true"><Icon name="clock" size={18} /></span>
        <div class="status-card-copy"><span>{$t('Automatic')}</span><strong>{autoStartCount}</strong></div>
      </article>
      <article class="status-card status-total">
        <span class="status-card-icon" aria-hidden="true"><Icon name="grid" size={18} /></span>
        <div class="status-card-copy"><span>{$t('Programs')}</span><strong>{programs.length}</strong></div>
      </article>
    </div>
  </section>

  <div class="home-content-grid">
    <section class="home-panel activity-panel" aria-labelledby="active-programs-title">
      <div class="section-heading panel-heading">
        <div>
          <p class="eyebrow">{$t('Runtime')}</p>
          <h2 id="active-programs-title">{$t('Active programs')}</h2>
        </div>
        <span class="section-count" aria-label={`${activePrograms.length} ${$t('Active programs')}`}>{activePrograms.length}</span>
      </div>

      {#if visibleActivePrograms.length}
        <div class="activity-list program-activity-list">
          {#each visibleActivePrograms as program (program.id)}
            <button
              type="button"
              class={`activity-row program-activity ${program.kind}`}
              title={`${program.name} · ${$t(stateNameKey(program.state))}`}
              onclick={() => onSelect(program.id)}
            >
              <span class="activity-icon program-activity-icon" aria-hidden="true"><ProgramGlyph kind={program.kind} /></span>
              <span class="activity-copy"><strong>{program.name}</strong><small>{$t(stateNameKey(program.state))}</small></span>
              <span
                class:running={program.state.status === 'running'}
                class:error-state={program.state.status === 'error' || program.state.status === 'stopFailed'}
                class="activity-state"
                aria-hidden="true"
              ><i></i></span>
              <span class="activity-disclosure" aria-hidden="true"><Icon name="chevron" size={15} /></span>
            </button>
          {/each}
          {#if activePrograms.length > visibleActivePrograms.length}
            <div class="activity-more" aria-label={`${activePrograms.length - visibleActivePrograms.length} ${$t('Programs')}`}>
              <strong>+{activePrograms.length - visibleActivePrograms.length}</strong>
              <small>{$t('More active programs are available in the navigator.')}</small>
            </div>
          {/if}
        </div>
      {:else}
        <div class="workspace-empty activity-empty">
          <span class="empty-state-icon" aria-hidden="true"><Icon name="activity" size={21} /></span>
          <div><strong>{$t('No active programs')}</strong><small>{$t('Start a program from the navigator when you are ready.')}</small></div>
        </div>
      {/if}
    </section>

    <section class="home-panel template-panel" aria-labelledby="program-profiles-title">
      <div class="section-heading panel-heading">
        <div>
          <p class="eyebrow">{$t('Create program')}</p>
          <h2 id="program-profiles-title">{$t('Program profiles')}</h2>
        </div>
      </div>

      <div class="profile-list template-grid">
        {#each programDefinitions as definition, index (definition.kind)}
          <button
            class:template-featured={index === 0}
            class={`profile-row template-card ${definition.kind === 'singBox' ? 'singbox' : definition.kind}`}
            data-template-kind={definition.kind}
            type="button"
            onclick={() => onCreate(definition.kind)}
            title={createBlockReason ? $t(createBlockReason) : `${$t(definition.templateName)} — ${$t(definition.templateDescription)}`}
          >
            <span class="profile-icon template-icon" aria-hidden="true"><ProgramGlyph kind={definition.kind} /></span>
            <span class="template-copy"><strong>{$t(definition.templateName)}</strong><small>{$t(definition.templateDescription)}</small></span>
            <span class="template-action" aria-hidden="true"><Icon name="add" size={16} /></span>
          </button>
        {/each}
      </div>
    </section>
  </div>
</div>

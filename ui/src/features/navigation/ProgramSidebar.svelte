<script lang="ts">
  import BrandMark from '../../lib/components/BrandMark.svelte';
  import Icon from '../../lib/components/Icon.svelte';
  import ProgramGlyph from '../../ProgramGlyph.svelte';
  import { t } from '../../i18n';
  import type { ProgramKind, ProgramState, ProgramSummary } from '../../types';

  type ProgramFilter = 'all' | 'running' | 'inactive' | 'issues' | ProgramKind;

  interface Props {
    collapsed: boolean;
    drawerMode: boolean;
    drawerOpen: boolean;
    programs: ProgramSummary[];
    visiblePrograms: ProgramSummary[];
    selectedId: string;
    loadingId: string;
    query: string;
    filter: ProgramFilter;
    toolsOpen: boolean;
    bulkMode: boolean;
    bulkSelectedIds: Set<string>;
    allVisibleSelected: boolean;
    bulkBusy: string;
    canReorder: boolean;
    canActivate: boolean;
    licenseHint: string;
    createBlockReason: string;
    draggingId: string;
    dropTargetId: string;
    dropAfter: boolean;
    listElement: HTMLElement | null;
    stateNameKey: (state: ProgramState) => string;
    isRunning: (state: ProgramState | undefined) => boolean;
    isIssue: (state: ProgramState | undefined) => boolean;
    onToggleCollapsed: () => void;
    onHome: () => void;
    onCreate: () => void;
    onToggleBulk: () => void;
    onSelectAll: () => void;
    onBulkLifecycle: (action: 'start' | 'stop' | 'restart') => void;
    onActivate: (event: MouseEvent, id: string) => void;
    onContextMenu: (event: MouseEvent | KeyboardEvent, program: ProgramSummary) => void;
    onStartDrag: (event: PointerEvent, id: string) => void;
    onSettings: () => void;
  }

  let {
    collapsed,
    drawerMode,
    drawerOpen,
    programs,
    visiblePrograms,
    selectedId,
    loadingId,
    query = $bindable(),
    filter = $bindable(),
    toolsOpen = $bindable(),
    bulkMode,
    bulkSelectedIds,
    allVisibleSelected,
    bulkBusy,
    canReorder,
    canActivate,
    licenseHint,
    createBlockReason,
    draggingId,
    dropTargetId,
    dropAfter,
    listElement = $bindable(),
    stateNameKey,
    isRunning,
    isIssue,
    onToggleCollapsed,
    onHome,
    onCreate,
    onToggleBulk,
    onSelectAll,
    onBulkLifecycle,
    onActivate,
    onContextMenu,
    onStartDrag,
    onSettings,
  }: Props = $props();
</script>

<aside
  id="primary-sidebar"
  class:collapsed={collapsed && !drawerMode}
  class="sidebar-shell"
  role={drawerMode && drawerOpen ? 'dialog' : undefined}
  aria-modal={drawerMode && drawerOpen ? 'true' : undefined}
  aria-label={$t('Programs')}
  aria-hidden={drawerMode && !drawerOpen ? 'true' : undefined}
  inert={drawerMode && !drawerOpen}
>
  <header class="sidebar-brand-zone">
    <button class="brand brand-home sidebar-brand" type="button" aria-label={$t('Dashboard')} title={$t('Dashboard')} onclick={onHome}>
      <span class="sidebar-brand-mark" aria-hidden="true"><BrandMark size={34} /></span>
      <span class="brand-name sidebar-brand-copy"><strong>Camellia Nexus</strong><small>Desktop control</small></span>
    </button>
    <button
      class="sidebar-toggle sidebar-icon-button"
      type="button"
      aria-label={$t(drawerMode ? 'Close navigation' : collapsed ? 'Expand sidebar' : 'Collapse sidebar')}
      title={$t(drawerMode ? 'Close navigation' : collapsed ? 'Expand sidebar' : 'Collapse sidebar')}
      aria-expanded={drawerMode ? true : !collapsed}
      onclick={onToggleCollapsed}
    ><Icon name={drawerMode ? 'close' : 'menu'} size={17} /></button>
  </header>

  <section class="sidebar-command-zone" aria-label={$t('Create program')}>
    <button class="primary full add-program sidebar-command" type="button" aria-label={$t('Add program')} title={$t(createBlockReason || 'Add program')} onclick={onCreate}>
      <span class="sidebar-command-icon" aria-hidden="true"><Icon name="add" size={17} /></span>
      <span class="sidebar-command-label">{$t('Add program')}</span>
    </button>
  </section>

  <section class:open={toolsOpen} class:filtered={!!query || filter !== 'all'} class="catalog-tools sidebar-discovery-zone" aria-label={$t('Search and filter')}>
    <button
      class="catalog-tools-toggle sidebar-command sidebar-command-secondary"
      type="button"
      aria-label={$t('Search and filter')}
      title={$t('Search and filter')}
      aria-expanded={toolsOpen}
      onclick={() => (toolsOpen = !toolsOpen)}
    >
      <span class="sidebar-command-icon" aria-hidden="true"><Icon name="search" size={16} /></span>
      <span class="sidebar-command-label">{$t('Search and filter')}</span>
      {#if query || filter !== 'all'}<span class="sidebar-filter-indicator" aria-hidden="true"></span>{/if}
    </button>

    <div class="catalog-filter-panel sidebar-filter-panel">
      <div class="program-search sidebar-field">
        <span aria-hidden="true"><Icon name="search" size={15} /></span>
        <input bind:value={query} aria-label={$t('Search programs')} placeholder={$t('Search programs')} />
        {#if query}<button type="button" aria-label={$t('Clear search')} title={$t('Clear search')} onclick={() => (query = '')}><Icon name="close" size={13} /></button>{/if}
      </div>
      <label class:active={filter !== 'all'} class="program-filter sidebar-field">
        <span aria-hidden="true"><Icon name="filter" size={15} /></span>
        <select class="option-align-start" bind:value={filter} aria-label={$t('Filter programs')}>
          <option value="all">{$t('All programs')}</option>
          <option value="running">{$t('Running')}</option>
          <option value="inactive">{$t('Inactive')}</option>
          <option value="issues">{$t('Issues')}</option>
          <option value="generic">{$t('Generic')}</option>
          <option value="singBox">sing-box</option>
          <option value="xray">Xray</option>
          <option value="mihomo">Mihomo</option>
        </select>
      </label>
    </div>
  </section>

  <section class="sidebar-program-zone" aria-labelledby="sidebar-programs-heading">
    {#if loadingId}
      {@const loadingProgram = programs.find((program) => program.id === loadingId)}
      <span class="visually-hidden" role="status" aria-live="polite" aria-atomic="true">
        {$t('Loading program details')}{loadingProgram ? `: ${loadingProgram.name}` : ''}
      </span>
    {/if}
    <div class="nav-heading sidebar-section-heading">
      <span id="sidebar-programs-heading">{$t('Programs')}</span>
      <div class="sidebar-heading-actions">
        <small>{visiblePrograms.length === programs.length ? programs.length : `${visiblePrograms.length}/${programs.length}`}</small>
        {#if programs.length > 1}
          <button class:active={bulkMode} class="bulk-mode-toggle sidebar-icon-button" type="button" aria-pressed={bulkMode} aria-label={$t(bulkMode ? 'Finish selection' : 'Select multiple')} title={$t(bulkMode ? 'Finish selection' : 'Select multiple')} onclick={onToggleBulk}>
            <Icon name="check" size={15} />
          </button>
        {/if}
      </div>
    </div>

    <nav bind:this={listElement} class:reordering={!!draggingId} class="program-navigation" aria-label={$t('Programs')}>
      {#each visiblePrograms as program (program.id)}
        <button
          class:active={!bulkMode && selectedId === program.id}
          class:selection-pending={!bulkMode && loadingId === program.id}
          class:bulk-selected={bulkMode && bulkSelectedIds.has(program.id)}
          class:dragging={draggingId === program.id}
          class:drop-target={dropTargetId === program.id && draggingId !== program.id}
          class:drop-after={dropTargetId === program.id && draggingId !== program.id && dropAfter}
          class:drop-before={dropTargetId === program.id && draggingId !== program.id && !dropAfter}
          class={`program-item sidebar-program-item ${program.kind}`}
          data-program-id={program.id}
          type="button"
          aria-pressed={bulkMode ? bulkSelectedIds.has(program.id) : undefined}
          aria-busy={!bulkMode && loadingId === program.id ? 'true' : undefined}
          aria-label={`${program.name}, ${$t(stateNameKey(program.state))}${!bulkMode && loadingId === program.id ? `, ${$t('Loading program details')}` : ''}`}
          title={`${program.name} · ${$t(stateNameKey(program.state))}${!bulkMode && loadingId === program.id ? ` · ${$t('Loading program details')}` : ''}`}
          onclick={(event) => onActivate(event, program.id)}
          oncontextmenu={(event) => onContextMenu(event, program)}
          onkeydown={(event) => (event.key === 'ContextMenu' || (event.shiftKey && event.key === 'F10')) && onContextMenu(event, program)}
        >
          <span class={`program-avatar sidebar-program-icon ${program.kind}`} aria-hidden="true">
            <ProgramGlyph kind={program.kind} />
            <i class:running={isRunning(program.state)} class:error-state={isIssue(program.state)}></i>
          </span>
          <span class="program-copy sidebar-program-copy"><strong>{program.name}</strong><small>{$t(stateNameKey(program.state))}</small></span>
          <span class="sidebar-program-trailing">
            {#if bulkMode}
              <span class:checked={bulkSelectedIds.has(program.id)} class="bulk-check" aria-hidden="true"><Icon name="check" size={12} /></span>
            {:else if loadingId === program.id}
              <span class="program-selection-spinner" aria-hidden="true"></span>
            {:else if canReorder}
              <span class="program-grip" title={$t('Drag to reorder')} onpointerdown={(event) => onStartDrag(event, program.id)} aria-hidden="true"><i></i><i></i><i></i><i></i><i></i><i></i></span>
            {/if}
          </span>
        </button>
      {/each}

      {#if programs.length === 0}
        <div class="empty-list sidebar-empty-list"><span aria-hidden="true"><Icon name="grid" size={18} /></span><strong>{$t('No programs yet')}</strong><small>{$t('Register a binary to begin lifecycle management.')}</small></div>
      {:else if visiblePrograms.length === 0}
        <div class="empty-list filtered-empty sidebar-empty-list"><span aria-hidden="true"><Icon name="search" size={18} /></span><strong>{$t('No matching programs')}</strong><button type="button" onclick={() => { query = ''; filter = 'all'; }}>{$t('Clear filters')}</button></div>
      {/if}
    </nav>
  </section>

  {#if bulkMode}
    <section class="bulk-toolbar sidebar-bulk-zone" aria-label={$t('Select multiple')}>
      <div class="bulk-summary">
        <span><strong>{bulkSelectedIds.size}</strong><small>{$t('selected')}</small></span>
        <button class="icon-button sidebar-icon-button" type="button" aria-label={$t('Finish selection')} title={$t('Finish selection')} onclick={onToggleBulk}><Icon name="close" /></button>
      </div>
      <div class="bulk-actions">
        <button type="button" title={$t(allVisibleSelected ? 'Clear visible' : 'Select visible')} onclick={onSelectAll} disabled={!visiblePrograms.length}><Icon name="check" size={14} /><span>{$t(allVisibleSelected ? 'Clear' : 'All')}</span></button>
        <button type="button" title={$t(canActivate ? 'Start' : licenseHint)} onclick={() => onBulkLifecycle('start')} disabled={!bulkSelectedIds.size || !!bulkBusy || !canActivate}><Icon name="start" size={14} /><span>{$t('Start')}</span></button>
        <button type="button" title={$t(canActivate ? 'Restart' : licenseHint)} onclick={() => onBulkLifecycle('restart')} disabled={!bulkSelectedIds.size || !!bulkBusy || !canActivate}><Icon name="restart" size={14} /><span>{$t('Restart')}</span></button>
        <button class="stop-button" type="button" title={$t('Stop')} onclick={() => onBulkLifecycle('stop')} disabled={!bulkSelectedIds.size || bulkBusy === 'stop'}><Icon name="stop" size={13} /><span>{$t('Stop')}</span></button>
      </div>
    </section>
  {/if}

  <footer class="sidebar-footer sidebar-settings-zone">
    <button class="settings-button sidebar-command sidebar-command-secondary" type="button" aria-label={$t('Settings')} title={$t('Settings')} onclick={onSettings}>
      <span class="sidebar-command-icon" aria-hidden="true"><Icon name="settings" size={18} /></span>
      <span class="sidebar-command-label"><strong>{$t('Settings')}</strong></span>
      <span class="sidebar-command-disclosure" aria-hidden="true"><Icon name="chevron" size={15} /></span>
    </button>
  </footer>
</aside>

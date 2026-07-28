<script lang="ts">
  import { tick } from 'svelte';
  import { t } from '../../i18n';
  import Icon, { type IconName } from '../../lib/components/Icon.svelte';

  type ProgramTab = 'overview' | 'dashboard' | 'configuration' | 'logs';

  interface Props {
    active: ProgramTab;
    label: string;
    dashboardVisible: boolean;
    dashboardDisabled: boolean;
    dashboardTitle: string;
    configurationVisible: boolean;
    onSelect: (tab: ProgramTab) => void | Promise<void>;
  }

  interface TabDefinition {
    id: ProgramTab;
    label: string;
    icon: IconName;
    disabled: boolean;
    title?: string;
  }

  let {
    active,
    label,
    dashboardVisible,
    dashboardDisabled,
    dashboardTitle,
    configurationVisible,
    onSelect,
  }: Props = $props();

  const tabs: TabDefinition[] = $derived([
    { id: 'overview', label: $t('Details'), icon: 'details', disabled: false },
    ...(dashboardVisible
      ? [{ id: 'dashboard' as const, label: $t('Dashboard'), icon: 'dashboard' as const, disabled: dashboardDisabled, title: dashboardTitle }]
      : []),
    ...(configurationVisible
      ? [{ id: 'configuration' as const, label: $t('Configuration'), icon: 'config' as const, disabled: false }]
      : []),
    { id: 'logs', label: $t('Logs'), icon: 'logs', disabled: false },
  ]);
  const enabledTabs = $derived(tabs.filter((tab) => !tab.disabled));
  const selectedTabId = $derived(
    tabs.some((tab) => tab.id === active && !tab.disabled)
      ? active
      : enabledTabs[0]?.id ?? active,
  );

  let redirectedInvalidTab: ProgramTab | null = null;

  $effect(() => {
    const activeTab = tabs.find((tab) => tab.id === active);
    if (activeTab && !activeTab.disabled) {
      redirectedInvalidTab = null;
      return;
    }

    const fallback = tabs.find((tab) => tab.id === 'overview' && !tab.disabled) ?? enabledTabs[0];
    if (!fallback || redirectedInvalidTab === active) return;

    redirectedInvalidTab = active;
    void onSelect(fallback.id);
    void tick().then(() => document.getElementById(`program-tab-${fallback.id}`)?.focus());
  });

  function select(tab: ProgramTab) {
    void onSelect(tab);
  }

  function handleKeydown(event: KeyboardEvent) {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
    const available = enabledTabs;
    if (!available.length) return;
    event.preventDefault();
    const focusedId = (event.target as HTMLElement).id.replace('program-tab-', '') as ProgramTab;
    const focusedIndex = available.findIndex((tab) => tab.id === focusedId);
    const activeIndex = available.findIndex((tab) => tab.id === selectedTabId);
    const current = focusedIndex >= 0 ? focusedIndex : Math.max(0, activeIndex);
    const next = event.key === 'Home'
      ? 0
      : event.key === 'End'
        ? available.length - 1
        : event.key === 'ArrowRight'
          ? (current + 1) % available.length
          : (current - 1 + available.length) % available.length;
    const id = available[next].id;
    select(id);
    void tick().then(() => document.getElementById(`program-tab-${id}`)?.focus());
  }
</script>

<div class="tabs program-tabs" role="tablist" tabindex="-1" aria-label={label} aria-orientation="horizontal" onkeydown={handleKeydown}>
  {#each tabs as tab (tab.id)}
    <button
      id={`program-tab-${tab.id}`}
      type="button"
      role="tab"
      aria-selected={selectedTabId === tab.id}
      aria-controls={selectedTabId === tab.id ? `program-panel-${tab.id}` : undefined}
      tabindex={selectedTabId === tab.id ? 0 : -1}
      class:active={selectedTabId === tab.id}
      disabled={tab.disabled}
      title={tab.title}
      onclick={() => select(tab.id)}
    >
      <Icon name={tab.icon} size={16} />
      <span class="tab-label">{tab.label}</span>
      <span class="tab-selected-indicator" aria-hidden="true"></span>
    </button>
  {/each}
</div>

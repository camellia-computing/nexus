<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { t, translate, translateConfigurationValue, uiLanguage } from './i18n';
  import type {
    ConfigurationStateView,
    GuidedProjection,
    GuidedSettingDescriptor,
  } from './types';

  export let state: ConfigurationStateView;
  export let disabled = false;

  const dispatch = createEventDispatcher<{
    change: {
      settingId: string;
      value?: unknown;
    };
  }>();

  $: projectionById = new Map(
    state.guidedProjection.map((projection) => [projection.settingId, projection]),
  );
  $: categories = [...new Set(state.guidedDescriptors.map((descriptor) => descriptor.category))];
  $: intentDiagnostics = state.desired.diagnostics.filter(belongsToIntent);
  $: intentConflicts = state.desired.conflicts.filter(belongsToIntent);

  function projectionFor(descriptor: GuidedSettingDescriptor): GuidedProjection {
    return projectionById.get(descriptor.id) ?? {
      settingId: descriptor.id,
      status: 'inherited',
    };
  }

  function projectedValue(descriptor: GuidedSettingDescriptor): unknown {
    return projectionFor(descriptor).value;
  }

  function dependencyEnabled(descriptor: GuidedSettingDescriptor): boolean {
    if (!descriptor.enabledWhen) return true;
    const dependency = projectionById.get(descriptor.enabledWhen);
    // Keep the dependency editable while Final configuration also changes it,
    // so the user can still update the upstream Intent value.
    if (dependency?.status === 'overridden' || dependency?.status === 'finalEdit' || dependency?.status === 'custom') return true;
    return dependency?.value === true;
  }

  function dependencyHint(descriptor: GuidedSettingDescriptor): string | undefined {
    if (!descriptor.enabledWhen) return undefined;
    const dependency = projectionById.get(descriptor.enabledWhen);
    if (dependency?.status === 'overridden' || dependency?.status === 'finalEdit') return 'The parent setting has a Final configuration edit.';
    return dependency?.value === true ? undefined : 'Dependency unavailable';
  }

  function change(descriptor: GuidedSettingDescriptor, value: unknown) {
    dispatch('change', {
      settingId: descriptor.id,
      value,
    });
  }

  function reset(descriptor: GuidedSettingDescriptor) {
    dispatch('change', {
      settingId: descriptor.id,
      value: undefined,
    });
  }

  function statusLabel(projection: GuidedProjection): string {
    switch (projection.status) {
      case 'explicit': return 'Explicit';
      case 'custom': return 'Custom / Advanced';
      case 'overridden':
      case 'finalEdit': return 'Final configuration edit';
      default: return 'Following source';
    }
  }

  function categoryLabel(category: string): string {
    return ({
      logging: 'Logging',
      dns: 'DNS',
      routing: 'Routing',
      network: 'Network',
      tun: 'TUN',
    } as Record<string, string>)[category] ?? category;
  }

  function settingValueLabel(descriptor: GuidedSettingDescriptor, value: string): string {
    // Reference the store in this component so Svelte re-renders option
    // labels immediately when the language changes while the tab remains
    // mounted.
    return $uiLanguage === 'zh-CN'
      ? translateConfigurationValue(descriptor.id, value)
      : value;
  }

  const diagnosticMessageKeys: Record<string, string> = {
    SOURCE_INVALID: 'The latest source content is invalid; Applied and Last Known Good were retained.',
    SOURCE_UNAVAILABLE: 'No parsed snapshot is available for this source.',
    CORE_PROFILE_MISMATCH: 'The candidate was validated for a different compatibility profile.',
    CORE_VALIDATION_EVIDENCE_STALE: 'The binary, compatibility profile, or configuration changed after native validation.',
    CORE_INVALID: 'The Core rejected this candidate; Applied and Last Known Good were retained.',
    CORE_TARGET_CHANGED: 'The Core compatibility target changed; review and validate the candidate again.',
    CORE_TARGET_SOURCE_REJECTED: 'A source item is not expressible for the selected Core compatibility target.',
  };

  function diagnosticMessage(code: string, message: string, messageKey?: string): string {
    const key = messageKey && diagnosticMessageKeys[messageKey]
      ? diagnosticMessageKeys[messageKey]
      : diagnosticMessageKeys[code];
    return localizedMessage(
      key
        ?? 'Structured diagnostic details are unavailable. Review the highlighted settings or editor problems and validator output. Applied and Last Known Good were retained.',
    )
      + (key ? '' : ` (${code})`);
  }

  function localizedMessage(source: string): string {
    // The shared translator intentionally removes terminal punctuation for
    // compact labels.  Diagnostics and source details are sentences; retain
    // their English punctuation while still translating the Chinese view.
    return $uiLanguage === 'en' ? source : translate(source);
  }

  function conflictMessage(reason: string, messageKey?: string): string {
    if (messageKey === 'CONFIGURATION_IDENTITY_DUPLICATED') {
      return translate('A Final configuration identity is duplicated and cannot be applied safely.');
    }
    return translate(
      'Structured diagnostic details are unavailable. Review the highlighted settings or editor problems and validator output. Applied and Last Known Good were retained.',
    );
  }

  function belongsToIntent(issue: { scope?: { surface: string } }): boolean {
    return issue.scope?.surface === 'intent';
  }
</script>

<section class="guided-workspace" aria-label={$t('Common settings')}>
  <div class="guided-grid">
    {#each categories as category (category)}
      {#each state.guidedDescriptors.filter((descriptor) => descriptor.category === category) as descriptor, index (descriptor.id)}
          {@const projection = projectionFor(descriptor)}
          {@const controlDisabled = disabled || !dependencyEnabled(descriptor)}
          {@const dependencyMessage = dependencyHint(descriptor)}
          <article class:custom={projection.status === 'custom'} class:overridden={projection.status === 'overridden' || projection.status === 'finalEdit'}>
            {#if index === 0}<h3 class="setting-category">{$t(categoryLabel(category))}</h3>{/if}
            <div class="setting-copy">
              <strong>{$t(descriptor.label)}</strong>
              <small>{$t(descriptor.description)}</small>
              <span class="projection-status">{$t(statusLabel(projection))}</span>
            </div>
            <div class="setting-control">
              {#if descriptor.control === 'toggle'}
                <label class="guided-toggle">
                  <input
                    type="checkbox"
                    checked={projectedValue(descriptor) === true}
                    disabled={controlDisabled}
                    aria-label={$t(descriptor.label)}
                    on:change={(event) => change(descriptor, event.currentTarget.checked)}
                  />
                  <span></span>
                </label>
              {:else if descriptor.control === 'select'}
                <select
                  value={typeof projectedValue(descriptor) === 'string' && descriptor.allowedValues.includes(String(projectedValue(descriptor))) ? String(projectedValue(descriptor)) : ''}
                  disabled={controlDisabled}
                  aria-label={$t(descriptor.label)}
                  on:change={(event) => change(descriptor, event.currentTarget.value)}
                >
                  <option value="" disabled>{$t(projection.status === 'custom' ? 'Custom / Advanced' : 'Select a value')}</option>
                  {#each descriptor.allowedValues as value (value)}
                    <option {value}>{settingValueLabel(descriptor, value)}</option>
                  {/each}
                </select>
              {:else if descriptor.control === 'number'}
                <input type="number" value={typeof projectedValue(descriptor) === 'number' ? Number(projectedValue(descriptor)) : undefined} disabled={controlDisabled} aria-label={$t(descriptor.label)} on:change={(event) => change(descriptor, event.currentTarget.valueAsNumber)} />
              {:else}
                <input type="text" value={typeof projectedValue(descriptor) === 'string' ? String(projectedValue(descriptor)) : ''} disabled={controlDisabled} aria-label={$t(descriptor.label)} on:change={(event) => change(descriptor, event.currentTarget.value)} />
              {/if}
              <button type="button" on:click={() => reset(descriptor)} disabled={controlDisabled || projection.status === 'inherited'}>
                {$t('Follow source')}
              </button>
              {#if projection.intentValue !== undefined && JSON.stringify(projection.intentValue) !== JSON.stringify(projection.value)}
                <button type="button" on:click={() => change(descriptor, projection.intentValue)} disabled={controlDisabled}>{$t('Use this value')}</button>
              {/if}
            </div>
            {#if projection.status === 'overridden' || projection.status === 'finalEdit'}
              <p>{$t('Final configuration also edits this setting. Changes here may require conflict resolution there.')}</p>
            {:else if projection.status === 'custom'}
              <p>{$t('The effective configuration cannot be represented safely by this simple control.')}</p>
            {/if}
            {#if dependencyMessage}<p class="dependency-note">{$t(dependencyMessage)}</p>{/if}
          </article>
      {/each}
    {/each}
  </div>

  {#if intentDiagnostics.length > 0 || intentConflicts.length > 0}
    <div class="guided-diagnostics" role="status">
      {#each intentDiagnostics as diagnostic (`diagnostic-${diagnostic.code}`)}
        <span><strong>{diagnostic.code}</strong>{diagnosticMessage(diagnostic.code, diagnostic.message, diagnostic.messageKey)}</span>
      {/each}
      {#each intentConflicts as conflict (`conflict-${conflict.semanticPath}`)}
        <span><strong>{conflict.semanticPath}</strong>{conflictMessage(conflict.reason, conflict.messageKey)}</span>
      {/each}
    </div>
  {/if}
</section>

<style>
  .guided-workspace { display: grid; gap: 14px; margin-bottom: var(--ui-gap-md, 16px); padding: 16px; border: 1px solid var(--border-color, rgba(127,127,127,.28)); border-radius: 14px; background: var(--panel-background, rgba(127,127,127,.045)); container-type: inline-size; }
  .setting-copy { display: grid; gap: 3px; }
  .setting-copy small { opacity: .72; line-height: 1.35; }
  .projection-status { width: fit-content; border-radius: 999px; padding: 3px 8px; font-size: .78rem; background: rgba(127,127,127,.12); }
  /* The workspace is narrower than the viewport once the program sidebar and
     panel padding are accounted for.  Flexible tracks let two short settings
     share that real width instead of making auto-fill reserve a 300px track
     and leaving a large empty column on the right. */
  .guided-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 210px), 1fr)); gap: 10px; align-items: stretch; }
  .setting-category { margin: 0; font-size: .78rem; font-weight: 700; letter-spacing: .02em; text-transform: capitalize; opacity: .7; }
  article { display: grid; gap: 8px; align-content: start; min-width: 0; min-height: 0; padding: 11px; border: 1px solid var(--border-color, rgba(127,127,127,.22)); border-radius: 11px; background: var(--surface-background, rgba(255,255,255,.025)); }
  article.custom, article.overridden { border-style: dashed; }
  .projection-status { margin-top: 4px; }
  .setting-control { display: flex; flex-wrap: wrap; align-items: center; gap: 7px; min-width: 0; }
  .setting-control select, .setting-control input[type='number'], .setting-control input[type='text'] { flex: 1 1 150px; width: auto; min-width: 0; min-height: 34px; }
  .setting-control button { flex: 0 0 auto; min-height: 32px; max-width: 100%; }
  .guided-toggle { display: inline-flex; flex: 0 0 auto; align-items: center; justify-self: start; margin-inline: 0 auto; }
  .guided-toggle input { width: 18px; height: 18px; margin: 0; }
  article p { margin: 0; font-size: .8rem; line-height: 1.4; opacity: .78; }
  .dependency-note { color: var(--ui-text-warning, inherit); }
  .guided-diagnostics { display: grid; gap: 5px; padding: 10px; border-radius: 9px; background: rgba(210,70,70,.09); }
  .guided-diagnostics span { display: flex; gap: 8px; font-size: .82rem; }
  @container (max-width: 480px) { .guided-grid { grid-template-columns: 1fr; } }
  @media (max-width: 520px) { .guided-grid { grid-template-columns: 1fr; } .setting-control { align-items: stretch; } .setting-control button { margin-inline-start: 0; } }
</style>

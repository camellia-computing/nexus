<script lang="ts">
  import { t } from './i18n';
  import { coreAssessmentMessage } from './errors';
  import type { ConfigurationStateView, ConfigurationSurface } from './types';

  export let state: ConfigurationStateView | null = null;
  export let surface: ConfigurationSurface;
  export let includeAll = false;

  const diagnosticMessages: Record<string, string> = {
    SOURCE_INVALID: 'Fix or disable this source before continuing.',
    SOURCE_UNAVAILABLE: 'Refresh or disable this source before continuing.',
    CORE_PROFILE_MISMATCH: 'Apply again to check these changes with the current program.',
    CORE_VALIDATION_EVIDENCE_STALE: 'Your changes will be checked when you apply them.',
    CORE_INVALID: 'The program could not use these changes. Your current configuration was kept.',
    CORE_TARGET_CHANGED: 'Review these changes for the current program.',
    CORE_TARGET_SOURCE_REJECTED: 'This source cannot be used with the selected program target.',
    FINAL_EDIT_CONFLICT: 'Review this path in Final configuration.',
    SOURCE_VALUE_CONFLICT: 'Choose a source value before continuing.',
    LAYER_OWNERSHIP_CONFLICT: 'Choose which setting owns this path.',
  };

  const diagnosticTitles: Record<string, string> = {
    SOURCE_INVALID: 'Source needs attention',
    SOURCE_UNAVAILABLE: 'Source is unavailable',
    CORE_PROFILE_MISMATCH: 'Configuration needs a new check',
    CORE_VALIDATION_EVIDENCE_STALE: 'Configuration needs a new check',
    CORE_INVALID: 'Configuration was not applied',
    CORE_BUILD_CAPABILITY_UNAVAILABLE: 'Configuration needs attention',
    CORE_BUILD_CAPABILITY_UNCONFIRMED: 'Configuration needs attention',
    CONFIGURATION_VALUE_NOT_ALLOWED: 'Configuration needs attention',
    CONFIGURATION_ASSESSMENT_LIMIT: 'Configuration needs attention',
    CORE_TARGET_CHANGED: 'Program changed',
    CORE_TARGET_SOURCE_REJECTED: 'Source is not supported by this target',
    FINAL_EDIT_CONFLICT: 'Your edit needs review',
    SOURCE_VALUE_CONFLICT: 'Source values conflict',
    LAYER_OWNERSHIP_CONFLICT: 'Settings conflict',
  };

  function owns(issue: { scope?: { surface: ConfigurationSurface } }): boolean {
    return includeAll || issue.scope?.surface === surface;
  }

  function hasSourceStatusFor(code: string): boolean {
    if (surface !== 'sources' || includeAll) return false;
    const freshness = code === 'SOURCE_INVALID' ? 'invalid'
      : code === 'SOURCE_UNAVAILABLE' ? 'unavailable' : null;
    return freshness !== null && (state?.sourceStatuses ?? []).some(
      (source) => source.freshness === freshness && !!source.messageKey,
    );
  }

  function diagnosticMessage(code: string, messageKey?: string): string {
    return coreAssessmentMessage(messageKey ?? code) ?? diagnosticMessages[messageKey ?? '']
      ?? diagnosticMessages[code]
      ?? 'Review the highlighted setting. Your current configuration was kept.';
  }

  function conflictMessage(messageKey?: string): string {
    if (messageKey === 'FINAL_EDIT_CONFLICT') {
      return 'This setting changed in two places. Choose which value to use.';
    }
    if (messageKey === 'SOURCE_VALUE_CONFLICT') {
      return 'Two sources set different values. Choose one in Sources.';
    }
    if (messageKey === 'LAYER_OWNERSHIP_CONFLICT') {
      return 'Two settings change the same value. Choose which one to use.';
    }
    if (messageKey === 'CONFIGURATION_IDENTITY_DUPLICATED') {
      return 'Two entries have the same name. Rename or remove one.';
    }
    return 'Review this conflict in Final configuration.';
  }

  $: diagnostics = (state?.desired.diagnostics ?? []).filter(
    (issue) => owns(issue) && !hasSourceStatusFor(issue.messageKey ?? issue.code),
  );
  $: conflicts = (state?.desired.conflicts ?? []).filter(owns);
</script>

{#if diagnostics.length > 0 || conflicts.length > 0}
  <section class="surface-issues" aria-label={$t('Configuration issues')} role="status">
    {#each diagnostics as diagnostic, index (`diagnostic-${diagnostic.code}-${index}`)}
      <article>
        <div><strong>{$t(diagnosticTitles[diagnostic.messageKey ?? diagnostic.code] ?? diagnosticTitles[diagnostic.code] ?? 'Configuration issue')}</strong></div>
        <p>{$t(diagnosticMessage(diagnostic.code, diagnostic.messageKey))}</p>
        <details><summary>{$t('Information for support')}</summary><code>{diagnostic.code}</code>{#if diagnostic.scope?.ownerId}<code>{diagnostic.scope.ownerId}</code>{/if}{#if diagnostic.details}<pre>{diagnostic.details}</pre>{/if}</details>
      </article>
    {/each}
    {#each conflicts as conflict, index (`conflict-${conflict.semanticPath}-${index}`)}
      <article>
        <div><strong>{$t('Configuration conflict')}</strong><code>{conflict.semanticPath}</code></div>
        <p>{$t(conflictMessage(conflict.messageKey))}</p>
        <details><summary>{$t('Information for support')}</summary>{#if conflict.messageKey}<code>{conflict.messageKey}</code>{/if}{#if conflict.scope?.ownerId}<code>{conflict.scope.ownerId}</code>{/if}</details>
      </article>
    {/each}
  </section>
{/if}

<style>
  .surface-issues { display: grid; gap: 7px; padding: 10px; border: 1px solid rgba(210,75,75,.32); border-radius: 11px; background: rgba(210,75,75,.06); }
  article { display: grid; gap: 4px; min-width: 0; }
  article + article { padding-top: 7px; border-top: 1px solid rgba(127,127,127,.18); }
  article > div { display: flex; flex-wrap: wrap; align-items: center; gap: 7px; }
  p { margin: 0; font-size: .84rem; line-height: 1.4; }
  code { max-width: 100%; overflow-wrap: anywhere; font-size: .76rem; opacity: .78; }
  details { min-width: 0; font-size: .78rem; }
  details code + code { margin-left: 7px; }
  pre { max-height: 180px; overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; }
</style>

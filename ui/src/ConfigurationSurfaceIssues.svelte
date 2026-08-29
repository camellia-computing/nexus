<script lang="ts">
  import { t } from './i18n';
  import type { ConfigurationStateView, ConfigurationSurface } from './types';

  export let state: ConfigurationStateView | null = null;
  export let surface: ConfigurationSurface;
  export let includeAll = false;

  const diagnosticMessages: Record<string, string> = {
    SOURCE_INVALID: 'The latest source content is invalid; Applied and Last Known Good were retained.',
    SOURCE_UNAVAILABLE: 'No parsed snapshot is available for this source.',
    CORE_PROFILE_MISMATCH: 'The candidate was validated for a different compatibility profile.',
    CORE_VALIDATION_EVIDENCE_STALE: 'The binary, compatibility profile, or configuration changed after native validation.',
    CORE_INVALID: 'The Core rejected this candidate; Applied and Last Known Good were retained.',
    CORE_TARGET_CHANGED: 'The Core compatibility target changed; review and validate the candidate again.',
    CORE_TARGET_SOURCE_REJECTED: 'A source item is not expressible for the selected Core compatibility target.',
    RAW_DECISION_SUPERSEDED: 'The upstream value changed after this Final configuration decision was created. Review it before saving or applying.',
    SOURCE_VALUE_CONFLICT: 'A source value conflict must be resolved before this candidate can be saved.',
    LAYER_OWNERSHIP_CONFLICT: 'The same semantic path is owned by more than one configuration layer. Choose one owner before saving.',
  };

  const diagnosticTitles: Record<string, string> = {
    SOURCE_INVALID: 'Source needs attention',
    SOURCE_UNAVAILABLE: 'Source is unavailable',
    CORE_PROFILE_MISMATCH: 'Compatibility profile mismatch',
    CORE_VALIDATION_EVIDENCE_STALE: 'Native validation evidence is stale',
    CORE_INVALID: 'Candidate rejected by Core',
    CORE_TARGET_CHANGED: 'Compatibility target changed',
    CORE_TARGET_SOURCE_REJECTED: 'Source is not supported by this target',
    RAW_DECISION_SUPERSEDED: 'Final decision needs review',
    SOURCE_VALUE_CONFLICT: 'Source values conflict',
    LAYER_OWNERSHIP_CONFLICT: 'Configuration ownership conflict',
  };

  function owns(issue: { scope?: { surface: ConfigurationSurface } }): boolean {
    return includeAll || issue.scope?.surface === surface;
  }

  function diagnosticMessage(code: string, messageKey?: string): string {
    return diagnosticMessages[messageKey ?? '']
      ?? diagnosticMessages[code]
      ?? 'Structured diagnostic details are unavailable. Review the highlighted settings or editor problems and validator output. Applied and Last Known Good were retained.';
  }

  function conflictMessage(messageKey?: string): string {
    if (messageKey === 'RAW_DECISION_SUPERSEDED') {
      return 'The upstream value changed after this Final configuration decision was created. Review it before saving or applying.';
    }
    if (messageKey === 'SOURCE_VALUE_CONFLICT') {
      return 'A source value conflict must be resolved before this candidate can be saved.';
    }
    if (messageKey === 'LAYER_OWNERSHIP_CONFLICT') {
      return 'The same semantic path is owned by more than one configuration layer. Choose one owner before saving.';
    }
    if (messageKey === 'CONFIGURATION_RAW_OVERRIDE') {
      return 'Final configuration decisions overlap fields owned by this integration. Confirm takeover before Details can replace them.';
    }
    if (messageKey === 'CONFIGURATION_IDENTITY_DUPLICATED') {
      return 'A Final configuration identity is duplicated and cannot be applied safely.';
    }
    return 'The Final configuration decision conflicts with the current Source or Intent value.';
  }

  $: diagnostics = (state?.desired.diagnostics ?? []).filter(owns);
  $: conflicts = (state?.desired.conflicts ?? []).filter(owns);
</script>

{#if diagnostics.length > 0 || conflicts.length > 0}
  <section class="surface-issues" aria-label={$t('Configuration issues')} role="status">
    {#each diagnostics as diagnostic, index (`diagnostic-${diagnostic.code}-${index}`)}
      <article>
        <div><strong>{$t(diagnosticTitles[diagnostic.messageKey ?? diagnostic.code] ?? diagnosticTitles[diagnostic.code] ?? 'Configuration issue')}</strong><code>{diagnostic.code}</code>{#if diagnostic.scope?.ownerId}<code>{diagnostic.scope.ownerId}</code>{/if}</div>
        <p>{$t(diagnosticMessage(diagnostic.code, diagnostic.messageKey))}</p>
        {#if diagnostic.details}<details><summary>{$t('Technical details')}</summary><pre>{diagnostic.details}</pre></details>{/if}
      </article>
    {/each}
    {#each conflicts as conflict, index (`conflict-${conflict.semanticPath}-${index}`)}
      <article>
        <div><strong>{$t('Configuration conflict')}</strong><code>{conflict.semanticPath}</code>{#if conflict.scope?.ownerId}<code>{conflict.scope.ownerId}</code>{/if}</div>
        <p>{$t(conflictMessage(conflict.messageKey))}</p>
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
  details { font-size: .78rem; }
  pre { max-height: 180px; overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; }
</style>

<script lang="ts">
  import { onDestroy } from 'svelte';
  import { t } from './i18n';
  import { configurationErrorInfo, coreAssessmentMessage } from './errors';
  import { intentContextSummary, intentEffectLabel, intentNodeLabel, intentOptionMessage, type IntentSettingChange } from './intentPresentation';
  import IntentSettingControl from './IntentSettingControl.svelte';
  import type { ConfigurationIntentAction, IntentChangePlan, IntentContext, IntentNode, IntentSubmission } from './types';

  export let node: IntentNode;
  export let revision: number;
  export let disabled = false;
  export let load: (nodeId: string) => Promise<IntentContext>;
  export let submit: IntentSubmission;
  export let preview: (change: ConfigurationIntentAction) => Promise<IntentChangePlan>;
  export let openFinal: () => void;
  export let openDetails: () => void;
  export let openRelated: (nodeId: string) => void;
  export let onEditingChange: (editing: boolean) => void = () => {};
  let context: IntentContext | null = null;
  let loading = false;
  let error = '';
  let requestSequence = 0;
  let selectedPath = '';
  let text = '';
  let editing = false;
  let editProtected = false;
  let revealReplacement = false;
  let editReference = '';
  let pending = false;
  let impact: { action: ConfigurationIntentAction; plan: IntentChangePlan } | null = null;
  let observedKey = '';
  $: key = node.nodeId + ':' + revision;
  $: if (key !== observedKey) { observedKey = key; void read(); }
  $: current = selectedPath ? context?.values.find((value) => value.semanticPath === selectedPath)
    : context?.values.length === 1 ? context.values[0] : undefined;
  $: nativeControl = context?.node?.control ?? node.control;
  $: control = context?.setting?.control ?? nativeControl?.kind;
  $: usesSettingControl = node.disposition === 'guided' && !!context?.setting && (['select', 'toggle'].includes(context.setting.control)
    || ['routing.final', 'dns.final'].includes(context.setting.id));
  $: label = context?.setting?.label ?? intentNodeLabel(node);
  $: outdated = context?.stateRevision !== revision;
  $: onEditingChange(editing || !!impact);
  onDestroy(() => { requestSequence += 1; text = ''; onEditingChange(false); });

  async function read() {
    const sequence = ++requestSequence;
    loading = true;
    error = '';
    try {
      const result = await load(node.nodeId);
      if (sequence !== requestSequence) return;
      context = result;
    } catch (failure) {
      if (sequence === requestSequence) error = configurationErrorInfo(failure, 'guided-change').message;
    } finally { if (sequence === requestSequence) loading = false; }
  }
  function cancelEdit() { impact = null; editing = false; editProtected = false; revealReplacement = false; text = ''; error = ''; }
  function confirmed() { cancelEdit(); void read(); }
  async function change(action: ConfigurationIntentAction) {
    if (!current || disabled || pending || loading || outdated) return false;
    pending = true;
    error = '';
    try {
      const plan = await preview(action);
      if (!plan.canCommit) {
        error = configurationErrorInfo({ code: 'CONFIG_INVALID', messageKey: plan.blockers[0]?.messageKey }, 'guided-change').message;
        return false;
      }
      if (plan.effects.some((effect) => effect.role === 'related')) { selectedPath = current.semanticPath; impact = { action, plan }; return false; }
      return await submit(action, plan, confirmed);
    } catch (failure) {
      error = configurationErrorInfo(failure, 'guided-change').message;
      return false;
    } finally { pending = false; }
  }
  async function confirmImpact() {
    if (!impact || disabled || pending || loading || outdated) return;
    pending = true; error = '';
    try {
      if (impact.plan.stateRevision !== revision) {
        if (!current) { error = 'This entry is no longer available here. Your input is kept.'; return; }
        const action = 'referenceId' in impact.action ? { ...impact.action, referenceId: current.referenceId } : impact.action;
        const plan = await preview(action);
        if (!plan.canCommit) { error = configurationErrorInfo({ code: 'CONFIG_INVALID', messageKey: plan.blockers[0]?.messageKey }, 'guided-change').message; return; }
        impact = { action, plan };
        return;
      }
      await submit(impact.action, impact.plan, confirmed);
    } catch (failure) { error = configurationErrorInfo(failure, 'guided-change').message; }
    finally { pending = false; }
  }
  function edit() {
    if (!current) return;
    selectedPath = current.semanticPath;
    editProtected = current.protected;
    text = !current.protected && current.present ? String(current.value ?? '') : '';
    revealReplacement = false;
    editReference = current.referenceId;
    editing = true;
  }
  async function save() {
    if (!current) return;
    if (current.protected && !text) { error = 'Enter a replacement value, or keep the existing one.'; return; }
    const value = control === 'number' ? Number(text) : text;
    if (control === 'number' && (!text.trim() || !Number.isFinite(value)
      || (typeof value === 'number' && Number.isInteger(value) && !Number.isSafeInteger(value)))) {
      error = 'Enter a valid number.'; return;
    }
    await change({ action: 'editNode', nodeId: node.nodeId, referenceId: editReference, value: { state: 'present', value } });
  }
  function curated(action: IntentSettingChange) {
    if (!current) return Promise.resolve(false);
    if (action.kind === 'set') return changeValue(action.value);
    return change({ action: 'followNode', nodeId: node.nodeId, referenceId: current.referenceId });
  }
  function changeValue(value: unknown) {
    if (!current) return Promise.resolve(false);
    return change({ action: 'editNode', nodeId: node.nodeId, referenceId: current.referenceId, value: { state: 'present', value } });
  }
</script>

<div class="native-field" aria-busy={loading || pending} data-node-id={node.nodeId}>
  {#if context?.values.length && (context.values.length > 1 || selectedPath && !current)}
    <label class="value-choice"><span>{$t('Choose an entry')}</span><select disabled={editing || !!impact || pending || disabled} value={current?.semanticPath ?? ''} on:change={(event) => selectedPath = event.currentTarget.value}><option value="" disabled>{$t('Choose an entry to edit')}</option>{#each context.values as value (value.referenceId)}<option value={value.semanticPath}>{value.semanticPath}</option>{/each}</select></label>
  {/if}
  {#if usesSettingControl && context?.setting && current && node.disposition === 'guided' && !current.protected}
    <IntentSettingControl descriptor={context.setting} projection={{ settingId: context.setting.id, status: 'inherited', value: current.present ? current.value : undefined, intentValue: current.intentValue?.state === 'present' ? current.intentValue.value : undefined }} targets={context.targets} canFollow={current.canFollow} remove={current.present ? () => current ? change({ action: 'editNode', nodeId: node.nodeId, referenceId: current.referenceId, value: { state: 'missing' } }) : Promise.resolve(false) : undefined} disabled={disabled || loading || pending || outdated || !!impact} submit={curated} {openFinal} />
  {:else}
    <div class="field-heading"><strong>{$t(label)}</strong>{#if current}<span>{intentContextSummary(current, $t)}</span>{:else if loading}<span role="status">{$t('Loading')}…</span>{/if}</div>
    {#if editing && !current}
      <form on:submit|preventDefault><label><span>{$t('Value')}</span><input type={editProtected ? 'password' : control === 'number' ? 'number' : 'text'} value={text} on:input={(event) => text = event.currentTarget.value} disabled={pending || disabled} /></label><p role="status">{$t('This entry is no longer available here. Your input is kept.')}</p><div class="field-actions"><button type="button" disabled={pending} on:click={() => { cancelEdit(); selectedPath = ''; }}>{$t('Cancel')}</button><button type="button" disabled={pending} on:click={openFinal}>{$t('Open Final configuration')}</button></div></form>
    {:else if node.disposition === 'guided' && control === 'text' && current?.protected}
      {#if editing}
        <form class="protected-value" on:submit|preventDefault={() => void save()}>
          <label><span>{$t('Replacement value')}</span><textarea class:masked={!revealReplacement} rows="2" autocomplete="off" spellcheck="false" value={text} on:input={(event) => text = event.currentTarget.value} disabled={pending || disabled}></textarea></label>
          <button type="button" class="reveal-value" aria-pressed={revealReplacement} disabled={pending} on:click={() => revealReplacement = !revealReplacement}>{$t(revealReplacement ? 'Hide replacement' : 'Show replacement')}</button>
          {#if editReference !== current.referenceId}<p role="status">{$t('This setting changed while you were editing.')}</p>{/if}
          <div class="field-actions"><button type="button" disabled={pending} on:click={cancelEdit}>{$t('Keep existing')}</button>{#if editReference !== current.referenceId}<button type="button" class="primary" disabled={pending || disabled || loading || outdated || !!impact} on:click={() => { if (current) editReference = current.referenceId; void save(); }}>{$t('Use this value')}</button>{:else}<button class="primary" type="submit" disabled={pending || disabled || loading || outdated || !!impact}>{$t('Replace value')}</button>{/if}</div>
        </form>
      {:else}
        <div class="field-actions">
          <button type="button" disabled={disabled || loading || pending || outdated} on:click={edit}>{$t('Replace value')}</button>
          {#if current.present}<button type="button" disabled={disabled || loading || pending || outdated} on:click={() => current && void change({ action: 'editNode', nodeId: node.nodeId, referenceId: current.referenceId, value: { state: 'missing' } })}>{$t('Remove field')}</button>{/if}
          {#if current.canFollow}<button type="button" disabled={disabled || loading || pending || outdated} on:click={() => current && void change({ action: 'followNode', nodeId: node.nodeId, referenceId: current.referenceId })}>{$t('Follow source')}</button>{/if}
        </div>
      {/if}
    {:else if node.disposition === 'guided' && control && current && !current.protected}
      {#if control === 'toggle'}
        <label class="boolean-choice"><span>{$t('Value')}</span><select aria-label={$t(label)} value={!current.present ? 'missing' : current.value === null ? 'null' : String(current.value)} disabled={disabled || loading || pending || outdated || !!impact} on:change={(event) => { const requested = event.currentTarget.value; event.currentTarget.value = !current?.present ? 'missing' : current.value === null ? 'null' : String(current.value); void changeValue(requested === 'null' ? null : requested === 'true'); }}><option value="missing" disabled>{$t('Use program default')}</option>{#if nativeControl?.nullable}<option value="null">null</option>{/if}<option value="true">{$t('On')}</option><option value="false">{$t('Off')}</option></select></label>
      {:else if control === 'select'}
        <label><span>{$t('Value')}</span><select aria-label={$t(label)} value={current.present ? JSON.stringify(current.value) : 'missing'} disabled={disabled || loading || pending || outdated || !!impact} on:change={(event) => { const requested = event.currentTarget.value; event.currentTarget.value = current?.present ? JSON.stringify(current.value) : 'missing'; void changeValue(JSON.parse(requested)); }}>
          <option value="missing" disabled>{$t('Use program default')}</option>
          {#if current.present && (typeof current.value !== 'string' || !nativeControl?.allowedValues.includes(current.value))}<option value={JSON.stringify(current.value)} disabled>{intentContextSummary(current, $t)}</option>{/if}
          {#if nativeControl?.nullable && current.value !== null}<option value="null">null</option>{/if}
          {#each nativeControl?.allowedValues ?? [] as option}<option value={JSON.stringify(option)}>{option === '' ? $t('Use program default') : context?.targets.find((target) => target.id === option)?.label ?? $t(intentOptionMessage(option))}</option>{/each}
        </select></label>
      {:else if editing}
        <form on:submit|preventDefault={() => void save()}><label><span>{$t('Value')}</span><input type={control === 'number' ? 'number' : 'text'} value={text} on:input={(event) => text = event.currentTarget.value} disabled={pending || disabled} /></label>
          {#if editReference !== current.referenceId}<p role="status">{$t('This setting changed while you were editing.')} {$t('Current value')}: {intentContextSummary(current, $t)}</p>{/if}
          <div class="field-actions"><button type="button" disabled={pending} on:click={() => { editing = false; impact = null; }}>{$t('Cancel')}</button>{#if editReference !== current.referenceId}<button type="button" class="primary" disabled={pending || disabled || loading || outdated || !!impact} on:click={() => { if (current) editReference = current.referenceId; void save(); }}>{$t('Use this value')}</button>{:else}<button class="primary" type="submit" disabled={pending || disabled || loading || outdated || !!impact}>{$t('Done')}</button>{/if}</div></form>
      {:else}<button type="button" disabled={disabled || loading || pending || outdated || !!impact} on:click={edit}>{$t('Edit')}</button>{/if}
    {:else if node.disposition === 'detailsManaged'}
      <p>{$t('This setting is managed in Details.')} <button type="button" on:click={openDetails}>{$t('Open Details')}</button></p>
    {:else if !loading && context && (node.disposition !== 'guided' || !control || !!current)}
      <p>{$t(node.disposition === 'inapplicable' ? 'This setting is not available in this program.' : 'Edit this setting in Final configuration to keep its existing structure.')} <button type="button" on:click={openFinal}>{$t('Edit in Final configuration')}</button></p>
    {/if}
  {/if}
  {#if current && !editing && !impact && !usesSettingControl && (current.canFollow || control && current.present && !current.protected)}
    <details class="field-options"><summary>{$t('Setting actions')}</summary><div class="field-actions">
      {#if current.canFollow}<button type="button" disabled={disabled || pending || loading} on:click={() => current && void change({ action: 'followNode', nodeId: node.nodeId, referenceId: current.referenceId })}>{$t('Follow source')}</button>{/if}
      {#if control && current.present && node.disposition !== 'detailsManaged' && !current.protected}<button type="button" disabled={disabled || pending || loading} on:click={() => current && void change({ action: 'editNode', nodeId: node.nodeId, referenceId: current.referenceId, value: { state: 'missing' } })}>{$t('Remove field')}</button>{/if}
    </div></details>
  {/if}
  {#if impact}<section class="linked-impact" aria-label={$t('Related changes')}>
    <p>{$t(impact.plan.effects.some((effect) => effect.role === 'related') ? 'This change also updates:' : 'Only this setting will change.')}</p><ul>{#each impact.plan.effects.filter((effect) => effect.role === 'related') as effect (effect.semanticPath)}<li><strong>{$t(intentEffectLabel(effect.semanticPath))}</strong><span>{effect.protected ? $t('Already set') : intentContextSummary({ present: effect.after?.state === 'present', protected: false, value: effect.after?.state === 'present' ? effect.after.value : undefined }, $t)}</span></li>{/each}</ul>
    <div class="field-actions"><button type="button" disabled={pending} on:click={() => impact = null}>{$t('Cancel')}</button><button type="button" class="primary" disabled={disabled || pending || loading || outdated} on:click={() => void confirmImpact()}>{$t(impact.plan.stateRevision === revision ? 'Done' : 'Review latest settings')}</button></div>
  </section>{/if}
  {#if context?.issues.length}<p class="field-issue" role="status">{$t(coreAssessmentMessage(context.issues[0].messageKey) ?? 'Review this setting in Final configuration.')}</p>{/if}
  {#if context?.relatedNodes.length}<details class="field-options"><summary>{$t('Related settings')}</summary><div class="field-actions">{#each context.relatedNodes as related (related.nodeId)}<button type="button" disabled={pending || editing || !!impact} on:click={() => openRelated(related.nodeId)}>{$t(intentNodeLabel(related))}</button>{/each}</div></details>{/if}
  {#if error}<p class="field-error" role="alert">{$t(error)}{#if !context || outdated}<button type="button" on:click={() => void read()}>{$t('Retry')}</button>{/if}</p>{/if}
</div>

<style>
  .native-field { min-width: 0; padding: 8px 0; }
  .field-heading { display: flex; flex-wrap: wrap; align-items: baseline; justify-content: space-between; gap: 6px 14px; margin-bottom: 10px; }
  strong, label { font-size: .85rem; font-weight: var(--ui-weight-medium); }
  .field-heading span, p { font-size: .8rem; color: var(--ui-text-secondary); }
  p { margin: 8px 0 0; line-height: 1.5; }
  .field-error { color: var(--ui-danger); }
  .field-issue { color: var(--ui-warning-text); }
  label { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 52%); align-items: center; gap: 8px 16px; }
  .value-choice { margin-bottom: 12px; }
  select, input, textarea { min-width: 0; max-width: 100%; width: 100%; box-sizing: border-box; }
  textarea { resize: vertical; }
  textarea.masked { -webkit-text-security: disc; }
  .reveal-value { display: block; margin: 6px 0 0 auto; padding: 4px 6px; border-color: transparent; background: transparent; color: var(--ui-text-secondary); }
  button, summary, p, strong, span { min-width: 0; max-width: 100%; overflow-wrap: anywhere; white-space: normal; }
  button { box-shadow: none; font-size: .8rem; }
  .field-actions { display: flex; flex-wrap: wrap; gap: 8px; justify-content: flex-end; padding-top: 10px; }
  .field-options { margin-top: 8px; }
  .linked-impact { min-width: 0; margin-top: 10px; padding: 12px; border: 1px solid var(--ui-border-default); border-radius: var(--ui-radius-sm); background: var(--ui-brand-soft); }
  .linked-impact p { margin: 0; }
  .linked-impact ul { display: grid; gap: 6px; margin: 10px 0 0; padding: 0; list-style: none; }
  .linked-impact li { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 6px 12px; min-width: 0; font-size: .8rem; }
  summary { font-size: .78rem; color: var(--ui-text-secondary); cursor: pointer; }
  @container (max-width: 600px) { label { grid-template-columns: 1fr; } }
  @container (max-width: 440px) { .field-actions button { flex: 1 1 100%; } }
</style>

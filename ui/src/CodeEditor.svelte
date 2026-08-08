<script context="module" lang="ts">
  let nextEditorInstance = 0;
</script>

<script lang="ts">
  import { createEventDispatcher, onMount } from 'svelte';
  import { startCompletion } from '@codemirror/autocomplete';
  import { Compartment, EditorState, Prec, type Extension } from '@codemirror/state';
  import { EditorView, keymap, type Command, type ViewUpdate } from '@codemirror/view';
  import {
    copyLineDown,
    copyLineUp,
    deleteLine,
    indentLess,
    indentMore,
    isolateHistory,
    moveLineDown,
    moveLineUp,
    redo,
    redoDepth,
    toggleComment,
    undo,
    undoDepth,
  } from '@codemirror/commands';
  import { json } from '@codemirror/lang-json';
  import { yaml } from '@codemirror/lang-yaml';
  import {
    foldAll,
    HighlightStyle,
    indentUnit,
    syntaxHighlighting,
    unfoldAll,
  } from '@codemirror/language';
  import {
    forEachDiagnostic,
    forceLinting,
    lintGutter,
    lintKeymap,
    linter,
    openLintPanel,
    previousDiagnostic,
    setDiagnostics,
    type Diagnostic,
  } from '@codemirror/lint';
  import {
    gotoLine,
    openSearchPanel,
    search,
    searchKeymap,
    searchPanelOpen,
  } from '@codemirror/search';
  import { oneDarkTheme } from '@codemirror/theme-one-dark';
  import { tags } from '@lezer/highlight';
  import { basicSetup } from 'codemirror';
  import type {
    ConfigurationDiagnostic,
    ConfigurationLanguage,
  } from './editor/configurationLanguage';
  import { ConfigurationLanguageService } from './editor/configurationLanguageService';
  import {
    parseJsonSchemaDocument,
    type JsonSchemaCompletionSemantics,
    type JsonSchemaNode,
  } from './editor/jsonSchema';
  import { jsonSchemaCompletionExtension } from './editor/jsonSchemaCompletion';
  import { t, translate, uiLanguage, type UiLanguage } from './i18n';
  import Icon from './lib/components/Icon.svelte';
  import type { ConfigurationSchemaDocument } from './types';

  export let value = '';
  export let readOnly = false;
  export let theme: 'light' | 'dark' = 'dark';
  export let language: ConfigurationLanguage = 'text';
  export let revision = '';
  export let configurationSchema: ConfigurationSchemaDocument | null = null;
  export let configurationSchemaLoading = false;
  export let configurationSchemaError = false;
  export let jsonSchemaSemantics: JsonSchemaCompletionSemantics | undefined;

  const dispatch = createEventDispatcher<{
    retrySchema: void;
    save: void;
    validate: void;
  }>();

  const instanceId = ++nextEditorInstance;
  const editorDescriptionId = `program-configuration-editor-description-${instanceId}`;
  const editorStatusId = `program-configuration-editor-status-${instanceId}`;
  const themeCompartment = new Compartment();
  const syntaxCompartment = new Compartment();
  const lintCompartment = new Compartment();
  const phrasesCompartment = new Compartment();
  const readOnlyCompartment = new Compartment();
  const accessibilityCompartment = new Compartment();
  const wrappingCompartment = new Compartment();
  const schemaCompletionCompartment = new Compartment();
  const accessibleDarkHighlightStyle = HighlightStyle.define([
    { tag: tags.keyword, color: '#d99bff' },
    {
      tag: [tags.name, tags.deleted, tags.character, tags.propertyName, tags.macroName],
      color: '#ff7b86',
    },
    { tag: [tags.function(tags.variableName), tags.labelName], color: '#78bfff' },
    {
      tag: [tags.color, tags.constant(tags.name), tags.standard(tags.name)],
      color: '#e4aa73',
    },
    { tag: [tags.definition(tags.name), tags.separator], color: '#c7ccd6' },
    {
      tag: [
        tags.typeName,
        tags.className,
        tags.number,
        tags.changed,
        tags.annotation,
        tags.modifier,
        tags.self,
        tags.namespace,
      ],
      color: '#f0cb85',
    },
    {
      tag: [
        tags.operator,
        tags.operatorKeyword,
        tags.url,
        tags.escape,
        tags.regexp,
        tags.link,
        tags.special(tags.string),
      ],
      color: '#67c7d4',
    },
    { tag: [tags.meta, tags.comment], color: '#a6afc0' },
    { tag: tags.strong, fontWeight: 'bold' },
    { tag: tags.emphasis, fontStyle: 'italic' },
    { tag: tags.strikethrough, textDecoration: 'line-through' },
    { tag: tags.link, color: '#a6afc0', textDecoration: 'underline' },
    { tag: tags.heading, fontWeight: 'bold', color: '#ff7b86' },
    {
      tag: [tags.atom, tags.bool, tags.special(tags.variableName)],
      color: '#e4aa73',
    },
    {
      tag: [tags.processingInstruction, tags.string, tags.inserted],
      color: '#a8d486',
    },
    { tag: tags.invalid, color: '#ffb4ab' },
  ]);
  const accessibleDarkTheme: Extension = [
    oneDarkTheme,
    syntaxHighlighting(accessibleDarkHighlightStyle),
  ];

  let host: HTMLDivElement;
  let view: EditorView | undefined;
  let languageService: ConfigurationLanguageService | undefined;
  let externalValue = value;
  let wrapLines = true;
  let cursorLine = 1;
  let cursorColumn = 1;
  let selectedCharacters = 0;
  let lineCount = 1;
  let characterCount = value.length;
  let errorCount = 0;
  let warningCount = 0;
  let canUndo = false;
  let canRedo = false;
  let checking = false;
  let formatting = false;
  let schemaCompiling = false;
  let schemaReady = false;
  let schemaCompileFailed = false;
  let schemaGeneration = 0;
  let appliedSchemaIdentity: string | null = null;
  let appliedSchemaSemantics: JsonSchemaCompletionSemantics | undefined;
  let formatStatusKey = '';
  let formatStatusTimeout: ReturnType<typeof setTimeout> | undefined;
  let languageTaskCount = 0;
  let mounted = false;
  let appliedTheme = theme;
  let appliedLanguage = language;
  let appliedUiLanguage: UiLanguage = $uiLanguage;
  let appliedReadOnly = readOnly;
  let appliedRevision = revision;

  $: formatAvailable = language === 'jsonc' || language === 'yaml';
  $: schemaEnhancementExpected = configurationSchemaLoading
    || configurationSchemaError
    || configurationSchema !== null
    || schemaCompiling
    || schemaCompileFailed
    || schemaReady;
  $: languageLabel = language === 'jsonc'
    ? 'JSON'
    : language === 'yaml'
      ? 'YAML'
      : language === 'toml'
        ? 'TOML'
        : $t('Plain text');
  $: problemSummary = errorCount
    ? `${errorCount} ${$t(errorCount === 1 ? 'error' : 'errors')}${warningCount ? ` · ${warningCount} ${$t(warningCount === 1 ? 'warning' : 'warnings')}` : ''}`
    : warningCount
      ? `${warningCount} ${$t(warningCount === 1 ? 'warning' : 'warnings')}`
      : $t('No problems');

  function editorSyntax(value: ConfigurationLanguage): Extension {
    if (value === 'jsonc') return json();
    if (value === 'yaml') return yaml();
    return [];
  }

  function editorPhrases(language: UiLanguage) {
    return EditorState.phrases.of(
      language === 'zh-CN'
        ? {
            Find: '查找',
            Replace: '替换',
            next: '下一个',
            previous: '上一个',
            all: '全部',
            'match case': '区分大小写',
            regexp: '正则表达式',
            'by word': '全字匹配',
            replace: '替换',
            'replace all': '全部替换',
            close: '关闭',
            'current match': '当前匹配',
            'on line': '位于行',
            Diagnostics: '问题',
            'No diagnostics': '没有问题',
          }
        : {
            next: 'Next',
            previous: 'Previous',
            all: 'All',
            'match case': 'Match case',
            regexp: 'Regular expression',
            'by word': 'Whole word',
            replace: 'Replace',
            'replace all': 'Replace all',
            close: 'Close',
          },
    );
  }

  function openReplace(editor: EditorView): boolean {
    openSearchPanel(editor);
    requestAnimationFrame(() => {
      const replace = editor.dom.querySelector<HTMLInputElement>('.cm-search input[name="replace"]');
      replace?.focus();
      replace?.select();
    });
    return true;
  }

  function editorAccessibility() {
    return EditorView.contentAttributes.of({
      'aria-label': translate('Configuration editor'),
      'aria-describedby': `${editorDescriptionId} ${editorStatusId}`,
      spellcheck: 'false',
    });
  }

  function editorReadOnly(value: boolean): Extension {
    return [
      EditorState.readOnly.of(value),
      EditorView.editable.of(!value),
    ];
  }

  function editorLint(value: ConfigurationLanguage): Extension {
    if (value !== 'jsonc' && value !== 'yaml') return [];
    return [
      linter(
        async (editor) => {
          languageTaskCount += 1;
          if (mounted) checking = true;
          try {
            const analysis = await languageService?.analyze(value, editor.state.doc.toString());
            return analysis?.diagnostics.map(
              (diagnostic) => editorDiagnostic(diagnostic, value),
            ) ?? [];
          } catch {
            return [
              {
                from: 0,
                to: Math.min(editor.state.doc.length, 1),
                severity: 'warning',
                source: languageSource(value),
                message: translate('Local syntax diagnostics are temporarily unavailable.'),
              },
            ];
          } finally {
            languageTaskCount = Math.max(0, languageTaskCount - 1);
            if (mounted) checking = languageTaskCount > 0;
          }
        },
        { delay: 450 },
      ),
      lintGutter({ hoverTime: 250 }),
    ];
  }

  function editorDiagnostic(
    diagnostic: ConfigurationDiagnostic,
    sourceLanguage: ConfigurationLanguage,
  ): Diagnostic {
    return {
      from: diagnostic.from,
      to: diagnostic.to,
      severity: diagnostic.severity,
      source: diagnostic.code.startsWith('jsonSchema.')
        ? 'JSON Schema'
        : languageSource(sourceLanguage),
      message: localizedDiagnosticMessage(diagnostic),
    };
  }

  function localizedDiagnosticMessage(diagnostic: ConfigurationDiagnostic): string {
    if (diagnostic.code.startsWith('jsonSchema.')) {
      return localizedJsonSchemaDiagnostic(diagnostic);
    }
    const messageKey = DIAGNOSTIC_MESSAGES[diagnostic.code];
    if (messageKey) return translate(messageKey);
    if (diagnostic.code.startsWith('yaml.')) {
      return `${translate('YAML syntax error')}: ${diagnostic.message}`;
    }
    if (diagnostic.code.startsWith('json.')) return translate('Invalid JSON syntax.');
    return translate('The document could not be analyzed.');
  }

  function localizedJsonSchemaDiagnostic(diagnostic: ConfigurationDiagnostic): string {
    const property = String(diagnostic.parameters?.property ?? '');
    const type = String(diagnostic.parameters?.type ?? '');
    const pattern = String(diagnostic.parameters?.pattern ?? '');
    const limit = String(diagnostic.parameters?.limit ?? '');
    switch (diagnostic.code) {
      case 'jsonSchema.additionalProperties':
      case 'jsonSchema.unevaluatedProperties':
        return property
          ? `${translate('Unknown property')}: ${property}`
          : translate('The object contains an unsupported property.');
      case 'jsonSchema.required':
        return property
          ? `${translate('Required property is missing')}: ${property}`
          : translate('A required property is missing.');
      case 'jsonSchema.type':
        return type
          ? `${translate('Expected value type')}: ${type}`
          : translate('The value has the wrong type.');
      case 'jsonSchema.enum':
        return translate('Choose one of the values allowed by the program schema.');
      case 'jsonSchema.const':
        return translate('The value must match the constant required by the program schema.');
      case 'jsonSchema.pattern':
        return pattern
          ? `${translate('The value does not match the required pattern')}: ${pattern}`
          : translate('The value does not match the required pattern.');
      case 'jsonSchema.minimum':
      case 'jsonSchema.exclusiveMinimum':
        return limit
          ? `${translate('The value is below the allowed minimum')}: ${limit}`
          : translate('The value is below the allowed minimum.');
      case 'jsonSchema.maximum':
      case 'jsonSchema.exclusiveMaximum':
        return limit
          ? `${translate('The value is above the allowed maximum')}: ${limit}`
          : translate('The value is above the allowed maximum.');
      case 'jsonSchema.minLength':
      case 'jsonSchema.minItems':
      case 'jsonSchema.minProperties':
        return limit
          ? `${translate('The value has fewer entries than allowed')}: ${limit}`
          : translate('The value has fewer entries than allowed.');
      case 'jsonSchema.maxLength':
      case 'jsonSchema.maxItems':
      case 'jsonSchema.maxProperties':
        return limit
          ? `${translate('The value has more entries than allowed')}: ${limit}`
          : translate('The value has more entries than allowed.');
      case 'jsonSchema.oneOf':
        return translate('The value must match exactly one supported configuration shape.');
      case 'jsonSchema.anyOf':
        return translate('The value does not match any supported configuration shape.');
      default:
        return translate('The value does not match the program configuration schema.');
    }
  }

  function languageSource(value: ConfigurationLanguage): string {
    return value === 'yaml' ? 'YAML' : value === 'jsonc' ? 'JSON' : 'Configuration';
  }

  function updateEditorStatus(state: EditorState) {
    const selection = state.selection.main;
    const line = state.doc.lineAt(selection.head);
    cursorLine = line.number;
    cursorColumn = selection.head - line.from + 1;
    selectedCharacters = Math.abs(selection.to - selection.from);
    lineCount = state.doc.lines;
    characterCount = state.doc.length;
    canUndo = undoDepth(state) > 0;
    canRedo = redoDepth(state) > 0;

    let nextErrors = 0;
    let nextWarnings = 0;
    forEachDiagnostic(state, (diagnostic) => {
      if (diagnostic.severity === 'error') nextErrors += 1;
      else if (diagnostic.severity === 'warning') nextWarnings += 1;
    });
    errorCount = nextErrors;
    warningCount = nextWarnings;
  }

  function updateScrollableRegionAccessibility(editor: EditorView) {
    if (searchPanelOpen(editor.state)) {
      editor.scrollDOM.tabIndex = 0;
      editor.scrollDOM.setAttribute('aria-label', translate('Configuration document'));
    } else {
      editor.scrollDOM.tabIndex = -1;
      editor.scrollDOM.removeAttribute('aria-label');
    }
  }

  function runEditorCommand(command: Command) {
    if (!view) return;
    view.focus();
    command(view);
  }

  function requestSave(): boolean {
    dispatch('save');
    return true;
  }

  function requestValidation(): boolean {
    dispatch('validate');
    return true;
  }

  function toggleLineWrapping(): boolean {
    wrapLines = !wrapLines;
    view?.dispatch({
      effects: wrappingCompartment.reconfigure(wrapLines ? EditorView.lineWrapping : []),
    });
    view?.focus();
    return true;
  }

  async function formatDocument(): Promise<boolean> {
    const editor = view;
    if (!editor || readOnly || !formatAvailable || formatting) return false;
    const original = editor.state.doc.toString();
    const originalLanguage = language;
    const originalLine = editor.state.doc.lineAt(editor.state.selection.main.head);
    const originalColumn = editor.state.selection.main.head - originalLine.from;
    formatting = true;
    setFormatStatus('');
    try {
      const result = await languageService?.format(originalLanguage, original);
      if (!result || !view || view !== editor) return true;
      if (editor.state.doc.toString() !== original || language !== originalLanguage) {
        setFormatStatus('The document changed before formatting completed.');
        return true;
      }

      const diagnostics = result.diagnostics.map(
        (diagnostic) => editorDiagnostic(diagnostic, originalLanguage),
      );
      editor.dispatch(setDiagnostics(editor.state, diagnostics));
      if (diagnostics.some((diagnostic) => diagnostic.severity === 'error')) {
        setFormatStatus('Fix syntax errors before formatting.');
        openLintPanel(editor);
        editor.focus();
        return true;
      }
      if (!result.changed) {
        setFormatStatus('Configuration is already formatted.');
        editor.focus();
        return true;
      }

      editor.dispatch({
        changes: { from: 0, to: editor.state.doc.length, insert: result.content },
        annotations: isolateHistory.of('full'),
      });
      const nextLine = editor.state.doc.line(Math.min(originalLine.number, editor.state.doc.lines));
      editor.dispatch({
        selection: {
          anchor: Math.min(nextLine.to, nextLine.from + originalColumn),
        },
        scrollIntoView: true,
      });
      forceLinting(editor);
      setFormatStatus('Formatting complete. Save to keep these changes.');
      editor.focus();
      return true;
    } catch {
      setFormatStatus('The document could not be formatted.');
      return true;
    } finally {
      formatting = false;
    }
  }

  function setFormatStatus(key: string) {
    formatStatusKey = key;
    if (formatStatusTimeout) clearTimeout(formatStatusTimeout);
    if (key) {
      formatStatusTimeout = setTimeout(() => {
        formatStatusKey = '';
      }, 6_000);
    }
  }

  async function synchronizeJsonSchema(
    document: ConfigurationSchemaDocument | null,
    semantics: JsonSchemaCompletionSemantics | undefined,
    currentLanguage: ConfigurationLanguage,
  ): Promise<void> {
    const schemaDocument = currentLanguage === 'jsonc' ? document : null;
    const identity = schemaDocument
      ? `${schemaDocument.source}:${schemaDocument.dialect}:${schemaDocument.contentHash}`
      : '';
    if (
      identity === appliedSchemaIdentity
      && semantics === appliedSchemaSemantics
    ) {
      return;
    }
    appliedSchemaIdentity = identity;
    appliedSchemaSemantics = semantics;
    const generation = ++schemaGeneration;
    schemaReady = false;
    schemaCompileFailed = false;
    schemaCompiling = schemaDocument !== null;
    view?.dispatch({
      effects: schemaCompletionCompartment.reconfigure([]),
    });

    if (!schemaDocument) {
      try {
        await languageService?.configureJsonSchema(null);
      } catch {
        // Syntax analysis remains available when the optional schema worker is unavailable.
      }
      if (generation === schemaGeneration) {
        schemaCompiling = false;
        if (view) forceLinting(view);
      }
      return;
    }

    try {
      const parsedSchema: JsonSchemaNode = parseJsonSchemaDocument(schemaDocument);
      if (!languageService) throw new Error('Configuration language service is unavailable');
      await languageService.configureJsonSchema(
        schemaDocument,
        semantics?.annotationKeywords,
      );
      if (
        generation !== schemaGeneration
        || identity !== appliedSchemaIdentity
        || semantics !== appliedSchemaSemantics
      ) {
        return;
      }
      view?.dispatch({
        effects: schemaCompletionCompartment.reconfigure(
          jsonSchemaCompletionExtension(parsedSchema, semantics),
        ),
      });
      schemaReady = true;
      if (view) forceLinting(view);
    } catch {
      if (generation !== schemaGeneration) return;
      schemaCompileFailed = true;
      try {
        await languageService?.configureJsonSchema(null);
      } catch {
        // The editor deliberately falls back to syntax-only analysis.
      }
      if (view) forceLinting(view);
    } finally {
      if (generation === schemaGeneration) schemaCompiling = false;
    }
  }

  function retryConfigurationSchema() {
    appliedSchemaIdentity = null;
    dispatch('retrySchema');
  }

  onMount(() => {
    mounted = true;
    languageService = new ConfigurationLanguageService();
    view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: value,
        extensions: [
          basicSetup,
          search({ top: true }),
          indentUnit.of('  '),
          Prec.highest(keymap.of([
            ...searchKeymap,
            ...lintKeymap,
            { key: 'Alt-ArrowUp', run: moveLineUp },
            { key: 'Alt-ArrowDown', run: moveLineDown },
            { key: 'Shift-Alt-ArrowUp', run: copyLineUp },
            { key: 'Shift-Alt-ArrowDown', run: copyLineDown },
            { key: 'Shift-Alt-f', run: () => { void formatDocument(); return true; } },
            { key: 'Shift-F8', run: previousDiagnostic },
            { key: 'Alt-z', run: toggleLineWrapping },
            { key: 'Mod-s', run: requestSave },
            { key: 'Mod-Enter', run: requestValidation },
            { key: 'Mod-g', run: gotoLine },
            { key: 'Mod-h', run: openReplace },
            { key: 'Mod-Shift-k', run: deleteLine },
            { key: 'Mod-/', run: toggleComment },
            { key: 'Mod-[', run: indentLess },
            { key: 'Mod-]', run: indentMore },
            { key: 'Mod-Alt-[', run: foldAll },
            { key: 'Mod-Alt-]', run: unfoldAll },
          ])),
          syntaxCompartment.of(editorSyntax(language)),
          lintCompartment.of(editorLint(language)),
          themeCompartment.of(theme === 'dark' ? accessibleDarkTheme : []),
          phrasesCompartment.of(editorPhrases($uiLanguage)),
          accessibilityCompartment.of(editorAccessibility()),
          wrappingCompartment.of(EditorView.lineWrapping),
          schemaCompletionCompartment.of([]),
          readOnlyCompartment.of(editorReadOnly(readOnly)),
          EditorView.updateListener.of((update: ViewUpdate) => {
            if (update.docChanged) {
              if (!formatting) setFormatStatus('');
              externalValue = update.state.doc.toString();
              value = externalValue;
            }
            updateEditorStatus(update.state);
            updateScrollableRegionAccessibility(update.view);
          }),
        ],
      }),
    });
    updateEditorStatus(view.state);
    updateScrollableRegionAccessibility(view);
    forceLinting(view);
    return () => {
      mounted = false;
      if (formatStatusTimeout) clearTimeout(formatStatusTimeout);
      languageService?.dispose();
      languageService = undefined;
      view?.destroy();
      view = undefined;
    };
  });

  $: if (view && value !== externalValue) {
    externalValue = value;
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: value },
    });
  }

  $: if (view && revision !== appliedRevision) {
    appliedRevision = revision;
    setFormatStatus('');
  }

  $: if (view) {
    if (theme !== appliedTheme) {
      appliedTheme = theme;
      view.dispatch({
        effects: themeCompartment.reconfigure(theme === 'dark' ? accessibleDarkTheme : []),
      });
    }
  }

  $: if (mounted) {
    void synchronizeJsonSchema(
      configurationSchema,
      jsonSchemaSemantics,
      language,
    );
  }

  $: if (view) {
    if (language !== appliedLanguage) {
      appliedLanguage = language;
      view.dispatch({
        effects: [
          syntaxCompartment.reconfigure(editorSyntax(language)),
          lintCompartment.reconfigure(editorLint(language)),
        ],
      });
      forceLinting(view);
    }
  }

  $: if (view) {
    if ($uiLanguage !== appliedUiLanguage) {
      appliedUiLanguage = $uiLanguage;
      view.dispatch({
        effects: [
          phrasesCompartment.reconfigure(editorPhrases($uiLanguage)),
          accessibilityCompartment.reconfigure(editorAccessibility()),
        ],
      });
      forceLinting(view);
    }
  }

  $: if (view) {
    if (readOnly !== appliedReadOnly) {
      appliedReadOnly = readOnly;
      view.dispatch({
        effects: readOnlyCompartment.reconfigure(editorReadOnly(readOnly)),
      });
    }
  }

  const DIAGNOSTIC_MESSAGES: Record<string, string> = {
    'configuration.duplicateKey': 'Duplicate object key.',
    'configuration.rootObjectExpected': 'The top-level configuration should be an object or mapping.',
    'format.failed': 'The document could not be formatted.',
    'json.InvalidCharacter': 'Invalid character in JSON.',
    'json.InvalidCommentToken': 'Comments are not supported by native JSON configuration.',
    'json.InvalidEscapeCharacter': 'Invalid escape sequence in JSON string.',
    'json.InvalidNumberFormat': 'Invalid number format in JSON.',
    'json.InvalidSymbol': 'Invalid symbol in JSON.',
    'json.InvalidUnicode': 'Invalid Unicode escape in JSON string.',
    'json.UnexpectedEndOfComment': 'The JSON comment is not closed.',
    'json.UnexpectedEndOfNumber': 'The JSON number is incomplete.',
    'json.UnexpectedEndOfString': 'The JSON string is not closed.',
    'json.PropertyNameExpected': 'A quoted property name is expected.',
    'json.ValueExpected': 'A JSON value is expected.',
    'json.ColonExpected': 'A colon is expected after the property name.',
    'json.CommaExpected': 'A comma is expected between values.',
    'json.CloseBraceExpected': 'A closing brace is expected.',
    'json.CloseBracketExpected': 'A closing bracket is expected.',
    'json.EndOfFileExpected': 'Unexpected content after the JSON document.',
    'yaml.BAD_ALIAS': 'The YAML alias is invalid or references an unknown anchor.',
    'yaml.BAD_DQ_ESCAPE': 'The YAML string contains an invalid escape sequence.',
    'yaml.BAD_INDENT': 'The YAML indentation is inconsistent.',
    'yaml.DUPLICATE_KEY': 'Duplicate mapping key.',
    'yaml.MISSING_CHAR': 'The YAML document is missing required punctuation.',
    'yaml.MULTIPLE_DOCS': 'Only one YAML document is supported for a program configuration.',
    'yaml.TAB_AS_INDENT': 'Use spaces instead of tabs for YAML indentation.',
    'yaml.UNEXPECTED_TOKEN': 'The YAML document contains an unexpected token.',
  };
</script>

<p id={editorDescriptionId} class="visually-hidden">
  {$t(readOnly
    ? 'Read-only program configuration. Standard editor keyboard shortcuts are available.'
    : 'Edit the program configuration. Use the command bar or standard editor keyboard shortcuts.')}
  {#if schemaReady}
    {$t('Schema suggestions appear as you type. Press Ctrl Space or use Show suggestions to open them.')}
  {/if}
</p>

<div class:read-only={readOnly} class="code-editor-shell">
  <div class="editor-command-bar" role="toolbar" aria-label={$t('Configuration editor commands')}>
    <div class="editor-command-group">
      <button
        class="editor-command secondary-command"
        type="button"
        disabled={readOnly || !canUndo}
        aria-label={$t('Undo')}
        title={`${$t('Undo')} · Ctrl/⌘ Z`}
        on:click={() => runEditorCommand(undo)}
      >
        <span class="editor-command-icon" aria-hidden="true"><Icon name="undo" size={16} /></span>
        <span class="command-label">{$t('Undo')}</span>
      </button>
      <button
        class="editor-command secondary-command"
        type="button"
        disabled={readOnly || !canRedo}
        aria-label={$t('Redo')}
        title={`${$t('Redo')} · Ctrl/⌘ Shift Z`}
        on:click={() => runEditorCommand(redo)}
      >
        <span class="editor-command-icon" aria-hidden="true"><Icon name="redo" size={16} /></span>
        <span class="command-label">{$t('Redo')}</span>
      </button>
      <span class="editor-command-divider" aria-hidden="true"></span>
      <button
        class="editor-command secondary-command"
        type="button"
        aria-label={$t('Find')}
        title={`${$t('Find')} · Ctrl/⌘ F`}
        on:click={() => runEditorCommand(openSearchPanel)}
      >
        <span class="editor-command-icon" aria-hidden="true"><Icon name="search" size={16} /></span>
        <span class="command-label">{$t('Find')}</span>
      </button>
      <button
        class="editor-command secondary-command"
        type="button"
        aria-label={$t('Replace')}
        title={`${$t('Replace')} · Ctrl/⌘ H`}
        on:click={() => runEditorCommand(openReplace)}
      >
        <span class="editor-command-icon" aria-hidden="true"><Icon name="replace" size={16} /></span>
        <span class="command-label">{$t('Replace')}</span>
      </button>
    </div>
    <div class="editor-command-group editor-command-primary">
      {#if readOnly}<span class="editor-read-only">{$t('Read only')}</span>{/if}
      <span class:visible={checking || formatting || schemaCompiling} class="editor-language-activity" aria-hidden="true"></span>
      {#if schemaEnhancementExpected}
        <button
          class="editor-command secondary-command suggestion-command"
          type="button"
          disabled={readOnly || !schemaReady}
          aria-label={$t('Show suggestions')}
          title={schemaReady
            ? `${$t('Show suggestions')} · Ctrl Space`
            : $t(configurationSchemaError || schemaCompileFailed
              ? 'Program schema unavailable'
              : 'Loading program schema')}
          on:click={() => runEditorCommand(startCompletion)}
        >
          <span class="editor-command-icon" aria-hidden="true"><Icon name="suggestions" size={16} /></span>
          <span class="command-label">{$t('Show suggestions')}</span>
        </button>
      {/if}
      <button
        class="editor-command format-command"
        type="button"
        disabled={readOnly || !formatAvailable || formatting}
        aria-label={$t('Format document')}
        title={$t(formatAvailable ? 'Format document' : 'Formatting is unavailable for this format.')}
        on:click={() => void formatDocument()}
      >
        <span class="editor-command-icon" aria-hidden="true"><Icon name="format" size={16} /></span>
        <span>{formatting ? `${$t('Formatting')}…` : $t('Format document')}</span>
        <kbd aria-hidden="true">⇧ Alt F</kbd>
      </button>
    </div>
  </div>

  <div class="editor" bind:this={host}></div>

  <div
    id={editorStatusId}
    class="editor-status-bar"
    role="group"
    aria-label={$t('Editor status')}
  >
    <button
      class:has-problems={errorCount + warningCount > 0}
      class="editor-status-problems"
      type="button"
      aria-label={`${$t('Problems')}: ${problemSummary}`}
      title={`${$t('Open problems')} · Ctrl/⌘ Shift M`}
      on:click={() => runEditorCommand(openLintPanel)}
    >
      <span class:error={errorCount > 0} aria-hidden="true">{errorCount ? '×' : warningCount ? '△' : '✓'}</span>
      <span>{problemSummary}</span>
    </button>
    {#if formatStatusKey}
      <span class="editor-format-status" role="status" aria-live="polite">{$t(formatStatusKey)}</span>
    {:else if checking}
      <span class="editor-format-status">{$t('Checking syntax')}…</span>
    {/if}
    {#if configurationSchemaLoading || schemaCompiling}
      <span class="editor-schema-status" role="status">{$t('Loading program schema')}…</span>
    {:else if configurationSchemaError || schemaCompileFailed}
      <span class="editor-schema-status schema-unavailable" role="status">
        {$t('Program schema unavailable')}
        <button type="button" on:click={retryConfigurationSchema}>
          {$t('Retry')}
        </button>
      </span>
    {:else if schemaReady}
      <span
        class="editor-schema-status schema-ready"
        role="status"
        title={$t('Schema suggestions appear as you type. Press Ctrl Space or use Show suggestions to open them.')}
      >
        <span class="schema-ready-icon" aria-hidden="true"><Icon name="suggestions" size={13} /></span>
        <span>{$t('Schema suggestions ready')}</span>
        <kbd aria-hidden="true">Ctrl Space</kbd>
      </span>
    {/if}
    <span class="editor-status-spacer"></span>
    <span title={$t('Cursor position')}>
      {$t('Ln')} {cursorLine}, {$t('Col')} {cursorColumn}
      {#if selectedCharacters} · {selectedCharacters} {$t('selected')}{/if}
    </span>
    <span>{lineCount} {$t(lineCount === 1 ? 'line' : 'lines')} · {characterCount.toLocaleString()} {$t('characters')}</span>
    <span>{$t('Spaces')}: 2</span>
    <button
      class="editor-status-toggle"
      type="button"
      aria-pressed={wrapLines}
      title={`${$t('Toggle line wrapping')} · Alt Z`}
      on:click={toggleLineWrapping}
    >
      {$t(wrapLines ? 'Wrap' : 'No wrap')}
    </button>
    <strong>{languageLabel}</strong>
  </div>
</div>

<style>
  .code-editor-shell {
    container: configuration-editor / inline-size;
    display: grid;
    width: 100%;
    min-width: 0;
    min-height: var(--code-editor-min-height, 360px);
    height: var(--code-editor-height, min(55vh, 620px));
    grid-template-rows: auto minmax(0, 1fr) auto;
    overflow: hidden;
    border: 1px solid color-mix(in srgb, var(--ui-brand) 14%, var(--ui-border-default));
    border-radius: var(--ui-radius-md);
    background: color-mix(in srgb, var(--ui-input) 96%, var(--ui-surface-1));
    box-shadow: var(--ui-shadow-sm);
  }

  .code-editor-shell:focus-within {
    border-color: var(--ui-focus-ring);
    outline: 3px solid var(--ui-focus-ring);
    outline-offset: 2px;
  }

  .editor-command-bar,
  .editor-status-bar {
    display: flex;
    min-width: 0;
    align-items: center;
    color: var(--ui-text-secondary);
    font-family: var(--ui-font-body, system-ui, sans-serif);
  }

  .editor-command-bar {
    min-height: 43px;
    justify-content: space-between;
    gap: 12px;
    overflow: hidden;
    border-bottom: 1px solid color-mix(in srgb, var(--ui-border-default) 76%, transparent);
    background: color-mix(in srgb, var(--ui-surface-2) 92%, var(--ui-input));
    padding: 5px 7px;
  }

  .editor-command-group {
    display: flex;
    min-width: 0;
    align-items: center;
    gap: 3px;
  }

  .editor-command-primary {
    margin-left: auto;
    justify-content: flex-end;
  }

  .editor-command {
    display: inline-flex;
    width: auto;
    min-width: var(--ui-control-sm);
    min-height: var(--ui-control-sm);
    align-items: center;
    justify-content: center;
    gap: 6px;
    margin: 0;
    border: 1px solid transparent;
    border-radius: var(--ui-radius-xs);
    background: transparent;
    padding: 4px 8px;
    color: var(--ui-text-secondary);
    box-shadow: none;
    font: var(--ui-weight-semibold, 600) var(--ui-font-size-xs)/1.2 var(--ui-font-body, system-ui, sans-serif);
    white-space: nowrap;
  }

  .editor-command:hover:not(:disabled),
  .editor-command:focus-visible {
    border-color: color-mix(in srgb, var(--ui-brand) 22%, var(--ui-border-default));
    background: var(--ui-state-hover);
    color: var(--ui-text-primary);
    transform: none;
  }

  .editor-command:focus-visible,
  .editor-status-bar button:focus-visible {
    outline: 2px solid var(--ui-focus-ring);
    outline-offset: 1px;
  }

  .editor-command:disabled {
    opacity: .38;
  }

  .editor-command-icon {
    display: grid;
    width: 16px;
    height: 16px;
    flex: 0 0 16px;
    place-items: center;
    color: currentColor;
  }

  .editor-command-divider {
    width: 1px;
    height: 20px;
    margin: 0 4px;
    background: var(--ui-divider);
  }

  .format-command {
    border-color: color-mix(in srgb, var(--ui-brand) 20%, var(--ui-border-default));
    background: color-mix(in srgb, var(--ui-brand-soft) 76%, transparent);
    color: var(--ui-brand);
  }

  .suggestion-command:not(:disabled) {
    color: var(--ui-brand);
  }

  .format-command kbd {
    border: 1px solid color-mix(in srgb, currentColor 18%, transparent);
    border-radius: var(--ui-radius-xs);
    background: color-mix(in srgb, var(--ui-surface-1) 68%, transparent);
    padding: 2px 4px;
    color: var(--ui-text-tertiary);
    font: 500 calc(var(--ui-font-size-xs) * .88)/1.1 var(--ui-font-mono, "SFMono-Regular", "Cascadia Code", monospace);
  }

  .editor-language-activity {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--ui-brand);
    opacity: 0;
    transform: scale(.7);
  }

  .editor-language-activity.visible {
    animation: editor-pulse 1s ease-in-out infinite;
    opacity: 1;
  }

  .editor {
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    background: color-mix(in srgb, var(--ui-input) 96%, var(--ui-surface-1));
  }

  .editor :global(.cm-editor) {
    position: relative;
    height: 100%;
    font: 12.5px/1.68 "SFMono-Regular", "Cascadia Code", "JetBrains Mono", Consolas, monospace;
    letter-spacing: -.01em;
  }

  .editor :global(.cm-scroller) {
    font-family: inherit;
    overscroll-behavior: contain;
  }

  .editor :global(.cm-gutters) {
    min-width: 48px;
    border-right: 1px solid color-mix(in srgb, var(--ui-border-default) 74%, transparent);
    background: color-mix(in srgb, var(--ui-surface-2) 88%, transparent);
    color: var(--ui-text-tertiary);
  }

  .editor :global(.cm-lineNumbers .cm-gutterElement) {
    padding: 0 10px 0 8px;
  }

  .editor :global(.cm-activeLine),
  .editor :global(.cm-activeLineGutter) {
    background: color-mix(in srgb, var(--ui-brand) 7%, transparent);
  }

  .editor :global(.cm-content) {
    padding: 12px 0 28px;
    caret-color: var(--ui-brand);
  }

  .editor :global(.cm-line) {
    padding: 0 17px;
  }

  .editor :global(.cm-selectionBackground) {
    background: color-mix(in srgb, var(--ui-brand) 24%, transparent) !important;
  }

  .editor :global(.cm-focused) {
    outline: none;
  }

  .editor :global(.cm-lint-marker-error),
  .editor :global(.cm-lint-marker-warning) {
    width: 11px;
    height: 11px;
  }

  .editor :global(.cm-lintRange-error) {
    text-decoration-color: var(--ui-danger);
  }

  .editor :global(.cm-lintRange-warning) {
    text-decoration-color: var(--ui-warning);
  }

  .editor :global(.cm-panels-top) {
    position: relative;
    z-index: 20;
    width: 100%;
    min-width: 0;
    overflow: visible;
    border-bottom: 1px solid color-mix(in srgb, var(--ui-brand) 18%, var(--ui-border-default));
    background: color-mix(in srgb, var(--ui-surface-raised) 94%, var(--ui-input));
    box-shadow: 0 5px 14px color-mix(in srgb, var(--ui-shadow-color, #000) 8%, transparent);
  }

  .editor :global(.cm-panel.cm-search) {
    position: relative;
    display: flex;
    width: 100%;
    min-width: 0;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    box-sizing: border-box;
    padding: 7px calc(var(--ui-control-sm) + 15px) 7px 8px;
    color: var(--ui-text-primary);
    font-family: var(--ui-font-body, system-ui, sans-serif);
  }

  .editor :global(.cm-panel.cm-search br) {
    display: block;
    width: 100%;
    height: 0;
    flex: 0 0 100%;
    margin: 0;
  }

  .editor :global(.cm-panel.cm-search input[type='text']) {
    width: clamp(180px, 36cqi, 300px);
    max-width: 100%;
    min-width: 0;
    min-height: var(--ui-control-sm);
    flex: 0 1 clamp(180px, 36cqi, 300px);
    margin: 0;
    border: 1px solid var(--ui-border-default);
    border-radius: var(--ui-radius-xs);
    background: var(--ui-input);
    padding: 4px 9px;
    color: var(--ui-text-primary);
    font: var(--ui-font-size-sm)/1.25 var(--ui-font-mono, "SFMono-Regular", "Cascadia Code", Consolas, monospace);
  }

  .editor :global(.cm-panel.cm-search input[type='text']::placeholder) {
    color: var(--ui-text-tertiary);
    font-family: var(--ui-font-body, system-ui, sans-serif);
    font-weight: var(--ui-weight-regular, 400);
    letter-spacing: 0;
    opacity: .88;
  }

  .editor :global(.cm-panel.cm-search input[type='text']:focus) {
    border-color: var(--ui-focus-ring);
    outline: 0;
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--ui-focus-ring) 34%, transparent);
  }

  .editor :global(.cm-panel.cm-search button) {
    width: auto;
    max-width: 100%;
    min-height: var(--ui-control-sm);
    flex: 0 0 auto;
    margin: 0;
    border: 1px solid transparent;
    border-radius: var(--ui-radius-xs);
    background: var(--ui-surface-2);
    padding: 5px 10px;
    color: var(--ui-text-secondary);
    box-shadow: none;
    font: var(--ui-weight-semibold, 600) var(--ui-font-size-xs)/1.2 var(--ui-font-body, system-ui, sans-serif);
    white-space: nowrap;
  }

  .editor :global(.cm-panel.cm-search button:hover) {
    border-color: color-mix(in srgb, var(--ui-brand) 22%, var(--ui-border-default));
    background: var(--ui-brand-soft);
    color: var(--ui-brand);
    transform: none;
  }

  .editor :global(.cm-panel.cm-search label) {
    display: inline-flex;
    width: fit-content;
    max-width: 100%;
    min-height: var(--ui-control-sm);
    flex: 0 1 auto;
    flex-direction: row;
    align-items: center;
    gap: 6px;
    margin: 0;
    color: var(--ui-text-secondary);
    font: var(--ui-weight-medium, 500) var(--ui-font-size-xs)/1.2 var(--ui-font-body, system-ui, sans-serif);
    white-space: nowrap;
  }

  .editor :global(.cm-panel.cm-search input[type='checkbox']) {
    width: 16px;
    min-height: 16px;
    flex: 0 0 16px;
    margin: 0;
    accent-color: var(--ui-brand);
  }

  .editor :global(.cm-panel.cm-search button[name='close']) {
    top: 10px;
    right: 10px;
    display: grid;
    width: var(--ui-control-sm);
    min-height: var(--ui-control-sm);
    place-items: center;
    border-radius: var(--ui-radius-xs);
    padding: 0;
    background: transparent;
    color: var(--ui-text-secondary);
    font-size: var(--ui-font-size-md);
  }

  .editor :global(.cm-searchMatch) {
    border-radius: 2px;
    background: color-mix(in srgb, var(--ui-warning) 34%, transparent) !important;
    outline: 1px solid color-mix(in srgb, var(--ui-warning) 48%, transparent);
  }

  .editor :global(.cm-searchMatch-selected) {
    background: color-mix(in srgb, var(--ui-brand) 34%, transparent) !important;
    outline-color: var(--ui-brand);
  }

  .editor :global(.cm-panels-bottom) {
    max-height: min(38%, 240px);
    overflow: auto;
    border-top: 1px solid var(--ui-border-default);
    background: color-mix(in srgb, var(--ui-surface-raised) 98%, transparent);
    color: var(--ui-text-primary);
  }

  .editor :global(.cm-panel.cm-panel-lint) {
    padding: 0;
    font-family: var(--ui-font-body, system-ui, sans-serif);
  }

  .editor :global(.cm-panel.cm-panel-lint ul) {
    max-height: 210px;
    margin: 0;
    padding: 4px;
  }

  .editor :global(.cm-diagnostic) {
    min-height: 36px;
    border-bottom: 1px solid var(--ui-divider);
    border-left-width: 3px;
    padding: 7px 10px;
    color: var(--ui-text-primary);
    font-size: var(--ui-font-size-xs);
  }

  .editor :global(.cm-diagnostic-error) {
    border-left-color: var(--ui-danger);
    background: color-mix(in srgb, var(--ui-danger-soft) 52%, transparent);
  }

  .editor :global(.cm-diagnostic-warning) {
    border-left-color: var(--ui-warning);
    background: color-mix(in srgb, var(--ui-warning-soft) 52%, transparent);
  }

  .editor :global(.cm-panel.cm-panel-lint ul:focus [aria-selected]) {
    background: color-mix(in srgb, var(--ui-brand-soft) 78%, var(--ui-surface-2));
    color: var(--ui-text-primary);
  }

  .editor :global(.cm-diagnosticSource) {
    color: var(--ui-text-tertiary);
    font: 500 var(--ui-font-size-xs)/1.3 var(--ui-font-mono, "SFMono-Regular", "Cascadia Code", monospace);
  }

  .editor :global(.cm-panel.cm-panel-lint > button[name='close']) {
    top: 7px;
    right: 7px;
    width: 26px;
    min-height: 26px;
    border-radius: 7px;
    background: var(--ui-surface-2);
    color: var(--ui-text-secondary);
  }

  .editor :global(.cm-tooltip) {
    max-width: min(520px, calc(100% - 16px));
    overflow: hidden;
    border: 1px solid var(--ui-border-strong);
    border-radius: var(--ui-radius-xs);
    background: var(--ui-surface-raised);
    color: var(--ui-text-primary);
    box-shadow: var(--ui-shadow-md);
  }

  .editor :global(.cm-tooltip-autocomplete > ul) {
    max-width: 100%;
    max-height: min(280px, 42vh);
    font: var(--ui-font-size-xs)/1.45 var(--ui-font-mono, "SFMono-Regular", "Cascadia Code", monospace);
  }

  .editor :global(.cm-tooltip-autocomplete > ul > li) {
    min-width: 0;
    min-height: 28px;
    padding: 5px 8px;
  }

  .editor :global(.cm-completionLabel),
  .editor :global(.cm-completionDetail) {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .editor :global(.cm-completionDetail) {
    max-width: 24ch;
    color: var(--ui-text-tertiary);
    font-family: var(--ui-font-body, system-ui, sans-serif);
  }

  .editor :global(.cm-tooltip-autocomplete ul li[aria-selected]) {
    background: var(--ui-brand-soft);
    color: var(--ui-text-primary);
  }

  .editor-status-bar {
    min-height: 32px;
    gap: 10px;
    overflow: hidden;
    border-top: 1px solid color-mix(in srgb, var(--ui-border-default) 76%, transparent);
    background: color-mix(in srgb, var(--ui-surface-2) 92%, var(--ui-input));
    padding: 3px 8px;
    font-size: var(--ui-font-size-xs);
    white-space: nowrap;
  }

  .editor-status-bar button {
    width: auto;
    min-height: 22px;
    margin: 0;
    border: 0;
    border-radius: 5px;
    background: transparent;
    padding: 2px 5px;
    color: inherit;
    box-shadow: none;
    font: inherit;
  }

  .editor-status-bar button:hover {
    background: var(--ui-state-hover);
    color: var(--ui-text-primary);
    transform: none;
  }

  .editor-status-problems {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }

  .editor-status-problems.has-problems {
    color: var(--ui-warning);
  }

  .editor-status-problems span.error {
    color: var(--ui-danger);
  }

  .editor-format-status {
    max-width: min(44ch, 40%);
    overflow: hidden;
    color: var(--ui-brand);
    text-overflow: ellipsis;
  }

  .editor-schema-status {
    display: inline-flex;
    min-width: 0;
    align-items: center;
    gap: 4px;
  }

  .editor-schema-status.schema-ready {
    color: var(--ui-success);
  }

  .editor-schema-status kbd {
    border: 1px solid color-mix(in srgb, currentColor 22%, transparent);
    border-radius: var(--ui-radius-xs);
    background: color-mix(in srgb, var(--ui-surface-1) 74%, transparent);
    padding: 2px 5px;
    color: var(--ui-text-secondary);
    font: 500 calc(var(--ui-font-size-xs) * .88)/1.1 var(--ui-font-mono, monospace);
  }

  .editor-schema-status.schema-unavailable {
    color: var(--ui-warning);
  }

  .editor-schema-status button {
    min-height: 20px;
    padding-inline: 4px;
    color: currentColor;
    text-decoration: underline;
    text-underline-offset: 2px;
  }

  .editor-status-spacer {
    flex: 1;
  }

  .editor-status-bar strong {
    color: var(--ui-brand);
    font-weight: 700;
  }

  .editor-read-only {
    color: var(--ui-text-tertiary);
    font-size: var(--ui-font-size-xs);
    text-transform: uppercase;
  }

  @keyframes editor-pulse {
    0%, 100% { box-shadow: 0 0 0 0 color-mix(in srgb, var(--ui-brand) 0%, transparent); transform: scale(.8); }
    50% { box-shadow: 0 0 0 4px color-mix(in srgb, var(--ui-brand) 12%, transparent); transform: scale(1); }
  }

  @container configuration-editor (max-width: 720px) {
    .secondary-command .command-label,
    .format-command kbd,
    .editor-command-divider {
      display: none;
    }

    .editor-command-bar {
      gap: 7px;
    }

    .editor-command {
      padding-inline: 7px;
    }

    .editor-format-status,
    .editor-status-bar > span:nth-last-of-type(2) {
      display: none;
    }
  }

  @container configuration-editor (max-width: 540px) {
    .editor :global(.cm-panel.cm-search input[type='text']) {
      width: 100%;
      max-width: none;
      flex-basis: 100%;
    }
  }

  @container configuration-editor (max-width: 410px) {
    .format-command > span:nth-child(2),
    .editor-status-bar > span:not(.editor-status-spacer):not(.editor-schema-status) {
      display: none;
    }

    .editor-status-bar {
      gap: 6px;
    }

    .editor-schema-status kbd {
      display: none;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .editor-language-activity.visible {
      animation: none;
      opacity: 1;
      transform: none;
    }
  }
</style>

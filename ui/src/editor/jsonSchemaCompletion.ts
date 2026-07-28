import {
  startCompletion,
  type Completion,
  type CompletionContext,
  type CompletionResult,
  type CompletionSource,
} from '@codemirror/autocomplete';
import { jsonLanguage } from '@codemirror/lang-json';
import type { Extension } from '@codemirror/state';
import type { EditorView } from '@codemirror/view';
import {
  completeJsonSchema,
  type JsonSchemaCompletion,
  type JsonSchemaCompletionSemantics,
  type JsonSchemaNode,
  type JsonSchemaSuggestedValue,
} from './jsonSchema';

const AUTOMATIC_TRIGGER_CHARACTERS = new Set(['"', ':', ',']);

export function jsonSchemaCompletionExtension(
  schema: JsonSchemaNode,
  semantics?: JsonSchemaCompletionSemantics,
): Extension {
  return jsonLanguage.data.of({
    autocomplete: jsonSchemaCompletionSource(schema, semantics),
  });
}

export function jsonSchemaCompletionSource(
  schema: JsonSchemaNode,
  semantics?: JsonSchemaCompletionSemantics,
): CompletionSource {
  return (context: CompletionContext): CompletionResult | null => {
    if (!context.explicit) {
      const previous = context.pos > 0
        ? context.state.doc.sliceString(context.pos - 1, context.pos)
        : '';
      if (!AUTOMATIC_TRIGGER_CHARACTERS.has(previous)) return null;
    }

    const result = completeJsonSchema(
      context.state.doc.toString(),
      context.pos,
      schema,
      semantics,
    );
    if (!result) return null;
    const options = result.options
      .filter((option) =>
        option.kind === 'property'
        || !result.quoted
        || typeof option.value === 'string')
      .map((option) => completionOption(option, result.quoted));
    if (!options.length) return null;
    return {
      from: result.from,
      options,
      filter: true,
    };
  };
}

function completionOption(
  option: JsonSchemaCompletion,
  quoted: boolean,
): Completion {
  if (option.kind === 'property') {
    return {
      label: option.label,
      type: 'property',
      detail: option.detail,
      boost: option.required ? 2 : 0,
      apply: (view, _completion, from, to) => {
        applyProperty(view, from, to, option.label, option.scaffold, quoted);
      },
    };
  }
  return {
    label: option.label,
    type: option.detail.endsWith('reference') ? 'variable' : 'constant',
    detail: option.detail,
    boost: option.boost,
    apply: (view, _completion, from, to) => {
      applyValue(view, from, to, option.value, quoted);
    },
  };
}

function applyProperty(
  view: EditorView,
  from: number,
  to: number,
  key: string,
  scaffold: JsonSchemaSuggestedValue | undefined,
  quoted: boolean,
): void {
  const keyText = JSON.stringify(key);
  const scaffoldText = scaffold === undefined ? '' : JSON.stringify(scaffold);
  const propertyText = quoted
    ? `${keyText.slice(1)}: ${scaffoldText}`
    : `${keyText}: ${scaffoldText}`;
  const replaceTo = quoted ? closingQuoteEnd(view, to) : to;
  const cursorBack = scaffoldCursorBack(scaffold);
  view.dispatch({
    changes: { from, to: replaceTo, insert: propertyText },
    selection: { anchor: from + propertyText.length - cursorBack },
    userEvent: 'input.complete',
  });
  if (scaffold !== undefined && (Array.isArray(scaffold) || isRecord(scaffold))) {
    queueMicrotask(() => startCompletion(view));
  }
}

function applyValue(
  view: EditorView,
  from: number,
  to: number,
  value: JsonSchemaSuggestedValue,
  quoted: boolean,
): void {
  let replaceFrom = from;
  let replaceTo = to;
  let text: string;
  if (quoted && typeof value === 'string') {
    text = JSON.stringify(value).slice(1);
    replaceTo = closingQuoteEnd(view, to);
  } else {
    text = JSON.stringify(value);
    if (quoted) {
      replaceFrom = Math.max(0, from - 1);
      replaceTo = closingQuoteEnd(view, to);
    }
  }
  const cursorBack = scaffoldCursorBack(value);
  view.dispatch({
    changes: { from: replaceFrom, to: replaceTo, insert: text },
    selection: { anchor: replaceFrom + text.length - cursorBack },
    userEvent: 'input.complete',
  });
  if (Array.isArray(value) || isRecord(value)) {
    queueMicrotask(() => startCompletion(view));
  }
}

function closingQuoteEnd(view: EditorView, from: number): number {
  const document = view.state.doc;
  let escaped = false;
  for (let position = from; position < document.length; position += 1) {
    const character = document.sliceString(position, position + 1);
    if (escaped) {
      escaped = false;
    } else if (character === '\\') {
      escaped = true;
    } else if (character === '"') {
      return position + 1;
    } else if (character === '\n' || character === '\r') {
      break;
    }
  }
  return from;
}

function scaffoldCursorBack(value: JsonSchemaSuggestedValue | undefined): number {
  if (value === undefined) return 0;
  if (typeof value === 'string' || Array.isArray(value) || isRecord(value)) return 1;
  return 0;
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

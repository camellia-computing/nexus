import {
  applyEdits,
  format as formatJson,
  parseTree,
  printParseErrorCode,
  type Node as JsonNode,
  type ParseError,
} from 'jsonc-parser';
import { isMap, parseDocument } from 'yaml';
import type { ConfigurationSchemaDocument } from '../types';

export type ConfigurationLanguage = 'jsonc' | 'yaml' | 'toml' | 'text';
export type ConfigurationDiagnosticSeverity = 'error' | 'warning' | 'info';

export interface ConfigurationDiagnostic {
  from: number;
  to: number;
  severity: ConfigurationDiagnosticSeverity;
  code: string;
  message: string;
  parameters?: Record<string, string | number | boolean>;
}

export interface ConfigurationAnalysis {
  diagnostics: ConfigurationDiagnostic[];
}

export interface ConfigurationFormatResult extends ConfigurationAnalysis {
  content: string;
  changed: boolean;
}

export interface JsonConfigurationAnalysis extends ConfigurationAnalysis {
  root?: JsonNode;
}

export type ConfigurationLanguageTask =
  | {
      id: number;
      kind: 'analyze';
      language: ConfigurationLanguage;
      content: string;
    }
  | {
      id: number;
      kind: 'configureJsonSchema';
      schema: ConfigurationSchemaDocument | null;
      annotationKeywords: string[];
    }
  | {
      id: number;
      kind: 'format';
      language: ConfigurationLanguage;
      content: string;
    };

export type ConfigurationLanguageTaskResult =
  | {
      id: number;
      ok: true;
      kind: 'analyze';
      result: ConfigurationAnalysis;
    }
  | {
      id: number;
      ok: true;
      kind: 'format';
      result: ConfigurationFormatResult;
    }
  | {
      id: number;
      ok: true;
      kind: 'configureJsonSchema';
    }
  | {
      id: number;
      ok: false;
      error: string;
    };

const JSON_PARSE_OPTIONS = {
  allowEmptyContent: false,
  allowTrailingComma: false,
  disallowComments: true,
} as const;

export function analyzeConfiguration(
  language: ConfigurationLanguage,
  content: string,
): ConfigurationAnalysis {
  switch (language) {
    case 'jsonc':
      return analyzeJsonDocument(content);
    case 'yaml':
      return { diagnostics: analyzeYaml(content) };
    default:
      return { diagnostics: [] };
  }
}

export function formatConfiguration(
  language: ConfigurationLanguage,
  content: string,
): ConfigurationFormatResult {
  const analysis = analyzeConfiguration(language, content);
  if (analysis.diagnostics.some((diagnostic) => diagnostic.severity === 'error')) {
    return { ...analysis, content, changed: false };
  }

  try {
    const endOfLine = content.includes('\r\n') ? '\r\n' : '\n';
    let formatted = content;
    if (language === 'jsonc') {
      formatted = applyEdits(
        content,
        formatJson(content, undefined, {
          eol: endOfLine,
          insertFinalNewline: true,
          insertSpaces: true,
          keepLines: false,
          tabSize: 2,
        }),
      );
    } else if (language === 'yaml') {
      const document = parseDocument(content, {
        keepSourceTokens: true,
        prettyErrors: false,
        strict: true,
        uniqueKeys: true,
        version: '1.2',
      });
      if (document.contents !== null) {
        formatted = document.toString({
          indent: 2,
          indentSeq: true,
          lineWidth: 0,
          minContentWidth: 0,
        });
        if (endOfLine === '\r\n') formatted = formatted.replace(/\n/g, '\r\n');
      }
    }
    return {
      ...analysis,
      content: formatted,
      changed: formatted !== content,
    };
  } catch (error) {
    return {
      content,
      changed: false,
      diagnostics: [
        ...analysis.diagnostics,
        {
          from: 0,
          to: Math.min(content.length, 1),
          severity: 'error',
          code: 'format.failed',
          message: error instanceof Error ? error.message : 'Configuration formatting failed',
        },
      ],
    };
  }
}

export function analyzeJsonDocument(content: string): JsonConfigurationAnalysis {
  const errors: ParseError[] = [];
  const root = parseTree(content, errors, JSON_PARSE_OPTIONS);
  const diagnostics: ConfigurationDiagnostic[] = errors.map((error) => ({
    ...diagnosticRange(content.length, error.offset, error.length),
    severity: 'error' as const,
    code: `json.${printParseErrorCode(error.error)}`,
    message: printParseErrorCode(error.error),
  }));

  if (root) {
    findDuplicateJsonKeys(root, diagnostics);
    if (!errors.length && root.type !== 'object') {
      diagnostics.push({
        ...diagnosticRange(content.length, root.offset, root.length),
        severity: 'warning',
        code: 'configuration.rootObjectExpected',
        message: 'The top-level configuration should be an object',
      });
    }
  }
  return {
    diagnostics: uniqueDiagnostics(diagnostics),
    root,
  };
}

function findDuplicateJsonKeys(
  node: JsonNode,
  diagnostics: ConfigurationDiagnostic[],
): void {
  if (node.type === 'object') {
    const keys = new Set<string>();
    for (const property of node.children ?? []) {
      const [key, value] = property.children ?? [];
      if (typeof key?.value === 'string') {
        if (keys.has(key.value)) {
          diagnostics.push({
            ...diagnosticRange(key.offset + key.length, key.offset, key.length),
            severity: 'warning',
            code: 'configuration.duplicateKey',
            message: 'Duplicate object key',
          });
        } else {
          keys.add(key.value);
        }
      }
      if (value) findDuplicateJsonKeys(value, diagnostics);
    }
    return;
  }
  for (const child of node.children ?? []) findDuplicateJsonKeys(child, diagnostics);
}

function analyzeYaml(content: string): ConfigurationDiagnostic[] {
  const document = parseDocument(content, {
    keepSourceTokens: true,
    prettyErrors: false,
    strict: true,
    uniqueKeys: true,
    version: '1.2',
  });
  const diagnostics: ConfigurationDiagnostic[] = [
    ...document.errors.map((error) => ({
      ...diagnosticRange(content.length, error.pos[0], error.pos[1] - error.pos[0]),
      severity: 'error' as const,
      code: `yaml.${error.code}`,
      message: error.message,
    })),
    ...document.warnings.map((warning) => ({
      ...diagnosticRange(content.length, warning.pos[0], warning.pos[1] - warning.pos[0]),
      severity: 'warning' as const,
      code: `yaml.${warning.code}`,
      message: warning.message,
    })),
  ];

  if (!document.errors.length && !isMap(document.contents)) {
    const range = document.contents?.range;
    diagnostics.push({
      ...diagnosticRange(content.length, range?.[0] ?? 0, (range?.[1] ?? 1) - (range?.[0] ?? 0)),
      severity: 'warning',
      code: 'configuration.rootObjectExpected',
      message: 'The top-level configuration should be a mapping',
    });
  }
  return uniqueDiagnostics(diagnostics);
}

function diagnosticRange(documentLength: number, offset: number, length: number) {
  const from = Math.min(documentLength, Math.max(0, offset));
  const requestedTo = Math.max(from, offset + Math.max(0, length));
  const to = Math.min(documentLength, Math.max(requestedTo, from < documentLength ? from + 1 : from));
  return { from, to };
}

function uniqueDiagnostics(
  diagnostics: ConfigurationDiagnostic[],
): ConfigurationDiagnostic[] {
  const seen = new Set<string>();
  return diagnostics.filter((diagnostic) => {
    const identity = [
      diagnostic.from,
      diagnostic.to,
      diagnostic.severity,
      diagnostic.code,
    ].join(':');
    if (seen.has(identity)) return false;
    seen.add(identity);
    return true;
  });
}

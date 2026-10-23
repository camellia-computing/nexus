import {
  findNodeAtLocation,
  getNodeValue,
  parseTree,
  type Node as JsonNode,
} from 'jsonc-parser';
import {
  isMap,
  isNode,
  isScalar,
  isSeq,
  parseDocument,
  type Node as YamlNode,
} from 'yaml';
import type { SemanticPathSegment } from '../types';
import type { ConfigurationLanguage } from './configurationLanguage';

export interface ConfigurationMarkerRange {
  from: number;
  to: number;
  status: 'exact' | 'parentAnchor' | 'documentUnavailable';
}

const JSON_PARSE_OPTIONS = {
  allowEmptyContent: false,
  allowTrailingComma: true,
  disallowComments: false,
} as const;

export function resolveConfigurationMarkerRange(
  language: ConfigurationLanguage,
  content: string,
  segments: SemanticPathSegment[],
  documentPath?: string[],
): ConfigurationMarkerRange {
  return createConfigurationPathIndex(language, content)(segments, documentPath);
}

export function createConfigurationPathIndex(language: ConfigurationLanguage, content: string) {
  const jsonErrors: import('jsonc-parser').ParseError[] = [];
  const parsedJson = language === 'jsonc' ? parseTree(content, jsonErrors, JSON_PARSE_OPTIONS) : undefined;
  const jsonRoot = jsonErrors.length ? undefined : parsedJson;
  const parsedYaml = language === 'yaml' ? parseDocument(content, {
    keepSourceTokens: true,
    prettyErrors: false,
    strict: true,
    uniqueKeys: true,
    version: '1.2',
  }) : undefined;
  const yamlRoot = parsedYaml?.errors.length ? undefined : parsedYaml?.contents;
  return (segments: SemanticPathSegment[], documentPath?: string[]): ConfigurationMarkerRange => {
  const path = documentPath ?? segments;
  if (language === 'jsonc') return resolveJsonRange(content, jsonRoot, path);
  if (language === 'yaml') return resolveYamlRange(content, yamlRoot, path);
  return { ...boundedRange(content.length, 0, Math.min(content.length, 1)), status: 'documentUnavailable' };
  };
}

function resolveJsonRange(
  content: string,
  root: JsonNode | undefined,
  segments: (SemanticPathSegment | string)[],
): ConfigurationMarkerRange {
  let current = root;
  if (!current) return { ...boundedRange(content.length, 0, Math.min(content.length, 1)), status: 'documentUnavailable' };
  let deepest = current;
  let exact = true;
  for (const segment of segments) {
    const next = jsonChild(current, segment);
    if (next === 'ambiguous') return { from: 0, to: 0, status: 'documentUnavailable' };
    if (!next) { exact = false; break; }
    current = next;
    deepest = next;
  }
  const from = exact ? deepest.offset : Math.max(deepest.offset, deepest.offset + deepest.length - 1);
  const to = exact ? deepest.offset + deepest.length : Math.min(content.length, from + 1);
  return { ...boundedRange(content.length, from, to), status: exact ? 'exact' : 'parentAnchor' };
}

function jsonChild(
  current: JsonNode,
  segment: SemanticPathSegment | string,
): JsonNode | 'ambiguous' | undefined {
  if (typeof segment === 'string') {
    if (current.type === 'object') return findNodeAtLocation(current, [segment]);
    if (current.type === 'array' && /^(0|[1-9][0-9]*)$/u.test(segment)) {
      return current.children?.[Number(segment)];
    }
    return undefined;
  }
  if (segment.kind === 'key') {
    return current.type === 'object'
      ? findNodeAtLocation(current, [segment.key])
      : undefined;
  }
  if (current.type !== 'array') return undefined;
  const matches = current.children?.filter((item) => {
    if (item.type !== 'object') return false;
    const identity = findNodeAtLocation(item, [segment.field]);
    return identity !== undefined && String(getNodeValue(identity)) === segment.value;
  });
  return matches && matches.length > 1 ? 'ambiguous' : matches?.[0];
}

function resolveYamlRange(
  content: string,
  root: unknown,
  segments: (SemanticPathSegment | string)[],
): ConfigurationMarkerRange {
  const contents = root;
  if (!contents || !isNode(contents)) {
    return { ...boundedRange(content.length, 0, Math.min(content.length, 1)), status: 'documentUnavailable' };
  }
  let current: YamlNode = contents as YamlNode;
  let deepest: YamlNode = current;
  let exact = true;
  for (const segment of segments) {
    const next = yamlChild(current, segment);
    if (next === 'ambiguous') return { from: 0, to: 0, status: 'documentUnavailable' };
    if (!next) { exact = false; break; }
    current = next;
    deepest = next;
  }
  const range = deepest.range;
  const from = exact ? (range?.[0] ?? 0) : Math.max(0, (range?.[1] ?? 1) - 1);
  const to = exact ? (range?.[1] ?? Math.min(content.length, 1)) : Math.min(content.length, from + 1);
  return { ...boundedRange(
    content.length,
    from,
    to,
  ), status: exact ? 'exact' : 'parentAnchor' };
}

function yamlChild(
  current: YamlNode,
  segment: SemanticPathSegment | string,
): YamlNode | 'ambiguous' | undefined {
  if (typeof segment === 'string') {
    const next = isMap(current) ? current.get(segment, true)
      : isSeq(current) && /^(0|[1-9][0-9]*)$/u.test(segment) ? current.items[Number(segment)] : undefined;
    return next && isNode(next) ? next as YamlNode : undefined;
  }
  if (segment.kind === 'key') {
    if (!isMap(current)) return undefined;
    const next = current.get(segment.key, true);
    return next !== undefined && isNode(next) ? next : undefined;
  }
  if (!isSeq(current)) return undefined;
  const matches = current.items.filter((item): item is YamlNode => {
    if (!item || !isMap(item)) return false;
    const identity = item.get(segment.field, true);
    return isScalar(identity) && String(identity.value) === segment.value;
  });
  return matches.length > 1 ? 'ambiguous' : matches[0];
}

function boundedRange(documentLength: number, from: number, to: number): Pick<ConfigurationMarkerRange, 'from' | 'to'> {
  const boundedFrom = Math.min(documentLength, Math.max(0, from));
  const boundedTo = Math.min(documentLength, Math.max(boundedFrom, to));
  if (boundedTo > boundedFrom || boundedFrom === documentLength) {
    return { from: boundedFrom, to: boundedTo };
  }
  return { from: boundedFrom, to: Math.min(documentLength, boundedFrom + 1) };
}

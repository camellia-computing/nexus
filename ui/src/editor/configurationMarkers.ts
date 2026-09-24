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
  exact?: boolean;
}

const JSON_PARSE_OPTIONS = {
  allowEmptyContent: false,
  allowTrailingComma: false,
  disallowComments: true,
} as const;

export function resolveConfigurationMarkerRange(
  language: ConfigurationLanguage,
  content: string,
  segments: SemanticPathSegment[],
  documentPath?: string[],
): ConfigurationMarkerRange {
  const path = documentPath ?? segments;
  if (language === 'jsonc') return resolveJsonRange(content, path);
  if (language === 'yaml') return resolveYamlRange(content, path);
  return boundedRange(content.length, 0, Math.min(content.length, 1));
}

function resolveJsonRange(
  content: string,
  segments: (SemanticPathSegment | string)[],
): ConfigurationMarkerRange {
  let current = parseTree(content, [], JSON_PARSE_OPTIONS);
  if (!current) return boundedRange(content.length, 0, Math.min(content.length, 1));
  let deepest = current;
  let exact = true;
  for (const segment of segments) {
    const next = jsonChild(current, segment);
    if (!next) { exact = false; break; }
    current = next;
    deepest = next;
  }
  return { ...boundedRange(content.length, deepest.offset, deepest.offset + deepest.length), exact };
}

function jsonChild(
  current: JsonNode,
  segment: SemanticPathSegment | string,
): JsonNode | undefined {
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
  return current.children?.find((item) => {
    if (item.type !== 'object') return false;
    const identity = findNodeAtLocation(item, [segment.field]);
    return identity !== undefined && getNodeValue(identity) === segment.value;
  });
}

function resolveYamlRange(
  content: string,
  segments: (SemanticPathSegment | string)[],
): ConfigurationMarkerRange {
  const document = parseDocument(content, {
    keepSourceTokens: true,
    prettyErrors: false,
    strict: true,
    uniqueKeys: true,
    version: '1.2',
  });
  const contents = document.contents;
  if (!contents || !isNode(contents)) {
    return boundedRange(content.length, 0, Math.min(content.length, 1));
  }
  let current: YamlNode = contents as YamlNode;
  let deepest: YamlNode = current;
  let exact = true;
  for (const segment of segments) {
    const next = yamlChild(current, segment);
    if (!next) { exact = false; break; }
    current = next;
    deepest = next;
  }
  const range = deepest.range;
  return { ...boundedRange(
    content.length,
    range?.[0] ?? 0,
    range?.[1] ?? Math.min(content.length, 1),
  ), exact };
}

function yamlChild(
  current: YamlNode,
  segment: SemanticPathSegment | string,
): YamlNode | undefined {
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
  return current.items.find((item): item is YamlNode => {
    if (!item || !isMap(item)) return false;
    const identity = item.get(segment.field, true);
    return isScalar(identity) && String(identity.value) === segment.value;
  });
}

function boundedRange(documentLength: number, from: number, to: number): ConfigurationMarkerRange {
  const boundedFrom = Math.min(documentLength, Math.max(0, from));
  const boundedTo = Math.min(documentLength, Math.max(boundedFrom, to));
  if (boundedTo > boundedFrom || boundedFrom === documentLength) {
    return { from: boundedFrom, to: boundedTo };
  }
  return { from: boundedFrom, to: Math.min(documentLength, boundedFrom + 1) };
}

import {
  findNodeAtLocation,
  getLocation,
  getNodeValue,
  parseTree,
  type JSONPath,
  type Node as JsonNode,
} from 'jsonc-parser';
import type { ConfigurationSchemaDocument } from '../types';

export type JsonSchemaNode = boolean | Record<string, unknown>;
export type JsonPrimitive = string | number | boolean | null;
export type JsonSchemaSuggestedValue =
  | JsonPrimitive
  | readonly unknown[]
  | Readonly<Record<string, unknown>>;

export interface JsonSchemaSemanticSuggestion {
  value: JsonSchemaSuggestedValue;
  detail?: string;
  boost?: number;
}

export interface JsonSchemaValueCompletionContext {
  document: unknown;
  path: readonly (string | number)[];
  annotations: (keyword: string) => readonly unknown[];
}

export interface JsonSchemaCompletionSemantics {
  annotationKeywords: readonly string[];
  completeValue: (
    context: JsonSchemaValueCompletionContext,
  ) => readonly JsonSchemaSemanticSuggestion[];
}

export interface JsonSchemaPropertyCompletion {
  kind: 'property';
  label: string;
  detail: string;
  required: boolean;
  scaffold: JsonSchemaSuggestedValue | undefined;
}

export interface JsonSchemaValueCompletion {
  kind: 'value';
  label: string;
  detail: string;
  value: JsonSchemaSuggestedValue;
  boost: number;
}

export type JsonSchemaCompletion =
  | JsonSchemaPropertyCompletion
  | JsonSchemaValueCompletion;

export interface JsonSchemaCompletionResult {
  from: number;
  replaceTo: number;
  quoted: boolean;
  options: JsonSchemaCompletion[];
}

const JSON_PARSE_OPTIONS = {
  allowEmptyContent: false,
  allowTrailingComma: false,
  disallowComments: true,
} as const;
const MAX_SCHEMA_RECURSION = 64;
const schemaAnchorIndexes = new WeakMap<
  Record<string, unknown>,
  ReadonlyMap<string, JsonSchemaNode>
>();

export function parseJsonSchemaDocument(
  document: ConfigurationSchemaDocument,
): JsonSchemaNode {
  if (document.dialect !== 'draft2020-12') {
    throw new Error(`Unsupported JSON Schema dialect: ${document.dialect}`);
  }
  const parsed: unknown = JSON.parse(document.content);
  if (!isSchemaNode(parsed)) {
    throw new Error('JSON Schema root must be an object or boolean');
  }
  return parsed;
}

export function completeJsonSchema(
  content: string,
  position: number,
  rootSchema: JsonSchemaNode,
  semantics?: JsonSchemaCompletionSemantics,
): JsonSchemaCompletionResult | null {
  const cursor = Math.min(content.length, Math.max(0, position));
  const location = getLocation(content, cursor);
  const rootNode = parseTree(content, [], JSON_PARSE_OPTIONS);
  const document = rootNode ? getNodeValue(rootNode) : undefined;

  if (location.isAtPropertyKey) {
    const containerPath = location.path.slice(0, -1);
    const container = valueAtPath(document, containerPath);
    const schemas = schemaNodesAtPath(rootSchema, containerPath, document);
    const range = completionRange(content, cursor, location.previousNode, true);
    const options = propertyCompletions(rootSchema, schemas, container);
    return options.length
      ? { ...range, options }
      : null;
  }

  const schemas = schemaNodesAtPath(rootSchema, location.path, document);
  const range = completionRange(content, cursor, location.previousNode, false);
  const options = valueCompletions(
    rootSchema,
    schemas,
    document,
    location.path,
    semantics,
  );
  return options.length
    ? { ...range, options }
    : null;
}

function propertyCompletions(
  rootSchema: JsonSchemaNode,
  schemas: readonly JsonSchemaNode[],
  container: unknown,
): JsonSchemaPropertyCompletion[] {
  const existing = isRecord(container) ? new Set(Object.keys(container)) : new Set<string>();
  const properties = new Map<string, {
    schemas: JsonSchemaNode[];
    required: boolean;
  }>();

  for (const schema of schemas) {
    const object = asSchemaObject(schema);
    if (!object) continue;
    const schemaProperties = asRecord(object.properties);
    if (!schemaProperties) continue;
    const required = new Set(
      Array.isArray(object.required)
        ? object.required.filter((value): value is string => typeof value === 'string')
        : [],
    );
    for (const [name, property] of Object.entries(schemaProperties)) {
      if (!isSchemaNode(property)) continue;
      const current = properties.get(name);
      if (current) {
        current.schemas.push(property);
        current.required ||= required.has(name);
      } else {
        properties.set(name, {
          schemas: [property],
          required: required.has(name),
        });
      }
    }
  }

  const options: JsonSchemaPropertyCompletion[] = [];
  for (const [label, property] of properties) {
    if (existing.has(label)) continue;
    const expanded = expandSchemaNodes(rootSchema, property.schemas, undefined);
    options.push({
      kind: 'property',
      label,
      detail: describeSchemas(expanded, property.required),
      required: property.required,
      scaffold: schemaScaffold(expanded),
    });
  }
  return options.sort((left, right) =>
    Number(right.required) - Number(left.required)
      || left.label.localeCompare(right.label));
}

function valueCompletions(
  rootSchema: JsonSchemaNode,
  schemas: readonly JsonSchemaNode[],
  document: unknown,
  path: readonly (string | number)[],
  semantics?: JsonSchemaCompletionSemantics,
): JsonSchemaValueCompletion[] {
  const options = new Map<string, JsonSchemaValueCompletion>();
  const add = (
    value: JsonSchemaSuggestedValue,
    detail: string,
    boost: number,
  ) => {
    const identity = stableValueIdentity(value);
    const existing = options.get(identity);
    if (!existing || boost > existing.boost) {
      options.set(identity, {
        kind: 'value',
        label: displayValue(value),
        detail,
        value,
        boost,
      });
    }
  };

  for (const schema of schemas) {
    const object = asSchemaObject(schema);
    if (!object) continue;
    if ('const' in object && isSuggestedValue(object.const)) {
      add(object.const, 'constant', 5);
    }
    if (Array.isArray(object.enum)) {
      for (const value of object.enum) {
        if (isSuggestedValue(value)) add(value, 'allowed value', 3);
      }
    }
    if (Array.isArray(object.examples)) {
      for (const value of object.examples) {
        if (isSuggestedValue(value)) add(value, 'example', 1);
      }
    }
    if ('default' in object && isSuggestedValue(object.default)) {
      add(object.default, 'default', 4);
    }
  }

  if (semantics) {
    const annotations = new Map<string, readonly unknown[]>();
    for (const keyword of semantics.annotationKeywords) {
      annotations.set(keyword, collectAnnotations(rootSchema, schemas, keyword));
    }
    for (const suggestion of semantics.completeValue({
      document,
      path,
      annotations: (keyword) => annotations.get(keyword) ?? [],
    })) {
      add(suggestion.value, suggestion.detail ?? 'reference', suggestion.boost ?? 5);
    }
  }

  const kinds = schemaTypes(schemas);
  if (kinds.has('boolean')) {
    add(true, 'boolean', 2);
    add(false, 'boolean', 2);
  }
  if (options.size === 0) {
    if (kinds.has('object')) add({}, 'object', 0);
    if (kinds.has('array')) add([], 'array', 0);
  }

  return [...options.values()].sort((left, right) =>
    right.boost - left.boost || left.label.localeCompare(right.label));
}

function schemaNodesAtPath(
  rootSchema: JsonSchemaNode,
  path: readonly (string | number)[],
  document: unknown,
): JsonSchemaNode[] {
  let schemas: JsonSchemaNode[] = [rootSchema];
  let instance = document;
  for (const segment of path) {
    const expanded = expandSchemaNodes(rootSchema, schemas, instance);
    const next: JsonSchemaNode[] = [];
    for (const schema of expanded) {
      const object = asSchemaObject(schema);
      if (!object) continue;
      if (typeof segment === 'number') {
        const prefixItems = schemaArray(object.prefixItems);
        if (segment < prefixItems.length) {
          next.push(prefixItems[segment]);
        } else if (isSchemaNode(object.items)) {
          next.push(object.items);
        }
        continue;
      }
      let matched = false;
      const properties = asRecord(object.properties);
      if (properties && isSchemaNode(properties[segment])) {
        next.push(properties[segment]);
        matched = true;
      }
      const patternProperties = asRecord(object.patternProperties);
      if (patternProperties) {
        for (const [pattern, patternSchema] of Object.entries(patternProperties)) {
          if (
            isSchemaNode(patternSchema)
            && patternMatches(pattern, segment)
          ) {
            next.push(patternSchema);
            matched = true;
          }
        }
      }
      if (!matched && isSchemaNode(object.additionalProperties)) {
        next.push(object.additionalProperties);
      }
    }
    schemas = next;
    instance = valueAtSegment(instance, segment);
  }
  return expandSchemaNodes(rootSchema, schemas, instance);
}

function expandSchemaNodes(
  rootSchema: JsonSchemaNode,
  schemas: readonly JsonSchemaNode[],
  instance: unknown,
): JsonSchemaNode[] {
  const output: JsonSchemaNode[] = [];
  for (const schema of schemas) {
    expandSchemaNode(rootSchema, schema, instance, output, [], 0);
  }
  return uniqueSchemas(output);
}

function expandSchemaNode(
  rootSchema: JsonSchemaNode,
  schema: JsonSchemaNode,
  instance: unknown,
  output: JsonSchemaNode[],
  stack: JsonSchemaNode[],
  depth: number,
): void {
  if (depth >= MAX_SCHEMA_RECURSION || stack.includes(schema)) return;
  if (typeof schema === 'boolean') {
    if (schema) output.push(schema);
    return;
  }
  stack.push(schema);
  output.push(schema);

  for (const keyword of ['$ref', '$dynamicRef'] as const) {
    if (typeof schema[keyword] === 'string') {
      const target = resolveLocalReference(rootSchema, schema[keyword]);
      if (target !== null) {
        expandSchemaNode(rootSchema, target, instance, output, stack, depth + 1);
      }
    }
  }
  for (const branch of schemaArray(schema.allOf)) {
    expandSchemaNode(rootSchema, branch, instance, output, stack, depth + 1);
  }
  for (const keyword of ['oneOf', 'anyOf'] as const) {
    const branches = schemaArray(schema[keyword]);
    if (!branches.length) continue;
    const viable = branches.filter(
      (branch) => !schemaConflicts(rootSchema, branch, instance, [], depth + 1),
    );
    for (const branch of viable.length ? viable : branches) {
      expandSchemaNode(rootSchema, branch, instance, output, stack, depth + 1);
    }
  }
  if (isRecord(instance)) {
    const dependentSchemas = asRecord(schema.dependentSchemas);
    if (dependentSchemas) {
      for (const property of Object.keys(instance)) {
        const dependentSchema = dependentSchemas[property];
        if (isSchemaNode(dependentSchema)) {
          expandSchemaNode(
            rootSchema,
            dependentSchema,
            instance,
            output,
            stack,
            depth + 1,
          );
        }
      }
    }
  }
  if (isSchemaNode(schema.if)) {
    const conditionConflicts = schemaConflicts(
      rootSchema,
      schema.if,
      instance,
      [],
      depth + 1,
    );
    if (conditionConflicts) {
      if (isSchemaNode(schema.else)) {
        expandSchemaNode(rootSchema, schema.else, instance, output, stack, depth + 1);
      }
    } else {
      if (isSchemaNode(schema.then)) {
        expandSchemaNode(rootSchema, schema.then, instance, output, stack, depth + 1);
      }
      if (isSchemaNode(schema.else)) {
        // Known values can reject a condition, but an incomplete document cannot prove it true.
        expandSchemaNode(rootSchema, schema.else, instance, output, stack, depth + 1);
      }
    }
  }
  stack.pop();
}

function schemaConflicts(
  rootSchema: JsonSchemaNode,
  schema: JsonSchemaNode,
  instance: unknown,
  stack: JsonSchemaNode[],
  depth: number,
): boolean {
  if (depth >= MAX_SCHEMA_RECURSION || stack.includes(schema)) return false;
  if (typeof schema === 'boolean') return !schema;
  stack.push(schema);
  try {
    if ('const' in schema && instance !== undefined && !sameJsonValue(schema.const, instance)) {
      return true;
    }
    if (
      Array.isArray(schema.enum)
      && instance !== undefined
      && !schema.enum.some((value) => sameJsonValue(value, instance))
    ) {
      return true;
    }
    if (typeof schema.type === 'string' && instance !== undefined) {
      if (!instanceMatchesType(instance, schema.type)) return true;
    }
    if (
      Array.isArray(schema.type)
      && instance !== undefined
      && !schema.type.some(
        (type) => typeof type === 'string' && instanceMatchesType(instance, type),
      )
    ) {
      return true;
    }
    for (const keyword of ['$ref', '$dynamicRef'] as const) {
      if (typeof schema[keyword] === 'string') {
        const target = resolveLocalReference(rootSchema, schema[keyword]);
        if (
          target !== null
          && schemaConflicts(rootSchema, target, instance, stack, depth + 1)
        ) {
          return true;
        }
      }
    }
    if (isRecord(instance)) {
      const properties = asRecord(schema.properties);
      if (properties) {
        for (const [key, value] of Object.entries(instance)) {
          const property = properties[key];
          if (
            isSchemaNode(property)
            && schemaConflicts(rootSchema, property, value, stack, depth + 1)
          ) {
            return true;
          }
        }
      }
      const patternProperties = asRecord(schema.patternProperties);
      if (patternProperties) {
        for (const [key, value] of Object.entries(instance)) {
          for (const [pattern, property] of Object.entries(patternProperties)) {
            if (
              patternMatches(pattern, key)
              && isSchemaNode(property)
              && schemaConflicts(rootSchema, property, value, stack, depth + 1)
            ) {
              return true;
            }
          }
        }
      }
    }
    for (const branch of schemaArray(schema.allOf)) {
      if (schemaConflicts(rootSchema, branch, instance, stack, depth + 1)) return true;
    }
    for (const keyword of ['oneOf', 'anyOf'] as const) {
      const branches = schemaArray(schema[keyword]);
      if (
        branches.length
        && branches.every(
          (branch) => schemaConflicts(rootSchema, branch, instance, stack, depth + 1),
        )
      ) {
        return true;
      }
    }
    return false;
  } finally {
    stack.pop();
  }
}

function collectAnnotations(
  rootSchema: JsonSchemaNode,
  schemas: readonly JsonSchemaNode[],
  keyword: string,
): unknown[] {
  const annotations: unknown[] = [];
  for (const schema of expandSchemaNodes(rootSchema, schemas, undefined)) {
    const object = asSchemaObject(schema);
    if (object && keyword in object) annotations.push(object[keyword]);
  }
  return annotations;
}

function resolveLocalReference(
  rootSchema: JsonSchemaNode,
  reference: string,
): JsonSchemaNode | null {
  if (reference === '#') return rootSchema;
  if (!reference.startsWith('#')) return null;
  if (!reference.startsWith('#/')) {
    let anchor: string;
    try {
      anchor = decodeURIComponent(reference.slice(1));
    } catch {
      return null;
    }
    const root = asSchemaObject(rootSchema);
    return root ? schemaAnchors(root).get(anchor) ?? null : null;
  }
  let current: unknown = rootSchema;
  for (const rawSegment of reference.slice(2).split('/')) {
    if (!isRecord(current)) return null;
    const segment = rawSegment.replaceAll('~1', '/').replaceAll('~0', '~');
    current = current[segment];
  }
  return isSchemaNode(current) ? current : null;
}

function schemaAnchors(
  rootSchema: Record<string, unknown>,
): ReadonlyMap<string, JsonSchemaNode> {
  const cached = schemaAnchorIndexes.get(rootSchema);
  if (cached) return cached;
  const anchors = new Map<string, JsonSchemaNode>();
  const visit = (value: unknown) => {
    if (Array.isArray(value)) {
      for (const item of value) visit(item);
      return;
    }
    if (!isRecord(value)) return;
    for (const keyword of ['$anchor', '$dynamicAnchor'] as const) {
      if (typeof value[keyword] === 'string') anchors.set(value[keyword], value);
    }
    for (const child of Object.values(value)) visit(child);
  };
  visit(rootSchema);
  schemaAnchorIndexes.set(rootSchema, anchors);
  return anchors;
}

function completionRange(
  content: string,
  position: number,
  previousNode: JsonNode | undefined,
  property: boolean,
): Pick<JsonSchemaCompletionResult, 'from' | 'replaceTo' | 'quoted'> {
  const node = property && previousNode?.type === 'property'
    ? previousNode.children?.[0] ?? previousNode
    : previousNode;
  if (node && content[node.offset] === '"') {
    const from = Math.min(position, node.offset + 1);
    const closingQuote = findClosingQuote(content, from);
    return {
      from,
      replaceTo: closingQuote === null ? position : closingQuote + 1,
      quoted: true,
    };
  }
  let from = position;
  while (from > 0 && /[\w.+-]/.test(content[from - 1])) from -= 1;
  return { from, replaceTo: position, quoted: false };
}

function findClosingQuote(content: string, from: number): number | null {
  let escaped = false;
  for (let index = from; index < content.length; index += 1) {
    const character = content[index];
    if (escaped) {
      escaped = false;
    } else if (character === '\\') {
      escaped = true;
    } else if (character === '"') {
      return index;
    } else if (character === '\n' || character === '\r') {
      return null;
    }
  }
  return null;
}

function schemaScaffold(
  schemas: readonly JsonSchemaNode[],
): JsonSchemaSuggestedValue | undefined {
  const constants: JsonSchemaSuggestedValue[] = [];
  const defaults: JsonSchemaSuggestedValue[] = [];
  for (const schema of schemas) {
    const object = asSchemaObject(schema);
    if (!object) continue;
    if ('const' in object && isSuggestedValue(object.const)) constants.push(object.const);
    if ('default' in object && isSuggestedValue(object.default)) defaults.push(object.default);
  }
  if (constants.length === 1) return constants[0];
  if (defaults.length === 1) return defaults[0];

  const kinds = schemaTypes(schemas);
  if (kinds.size !== 1) return undefined;
  const [kind] = kinds;
  if (kind === 'object') return {};
  if (kind === 'array') return [];
  if (kind === 'string') return '';
  if (kind === 'boolean') return false;
  if (kind === 'integer' || kind === 'number') return 0;
  if (kind === 'null') return null;
  return undefined;
}

function schemaTypes(schemas: readonly JsonSchemaNode[]): Set<string> {
  const types = new Set<string>();
  for (const schema of schemas) {
    const object = asSchemaObject(schema);
    if (!object) continue;
    if (typeof object.type === 'string') types.add(object.type);
    if (Array.isArray(object.type)) {
      for (const value of object.type) {
        if (typeof value === 'string') types.add(value);
      }
    }
    if ('const' in object) types.add(jsonType(object.const));
    if (Array.isArray(object.enum)) {
      for (const value of object.enum) types.add(jsonType(value));
    }
    if (asRecord(object.properties)) types.add('object');
    if (isSchemaNode(object.items) || schemaArray(object.prefixItems).length) types.add('array');
  }
  types.delete('undefined');
  return types;
}

function describeSchemas(
  schemas: readonly JsonSchemaNode[],
  required: boolean,
): string {
  const types = [...schemaTypes(schemas)].sort();
  const type = types.length ? types.join(' | ') : 'value';
  return required ? `${type} · required` : type;
}

function valueAtPath(
  value: unknown,
  path: readonly (string | number)[],
): unknown {
  let current = value;
  for (const segment of path) current = valueAtSegment(current, segment);
  return current;
}

function valueAtSegment(value: unknown, segment: string | number): unknown {
  if (typeof segment === 'number') {
    return Array.isArray(value) ? value[segment] : undefined;
  }
  return isRecord(value) ? value[segment] : undefined;
}

function instanceMatchesType(value: unknown, type: string): boolean {
  switch (type) {
    case 'array': return Array.isArray(value);
    case 'object': return isRecord(value);
    case 'integer': return typeof value === 'number' && Number.isInteger(value);
    case 'number': return typeof value === 'number';
    case 'null': return value === null;
    default: return typeof value === type;
  }
}

function patternMatches(pattern: string, value: string): boolean {
  try {
    return new RegExp(pattern, 'u').test(value);
  } catch {
    return false;
  }
}

function schemaArray(value: unknown): JsonSchemaNode[] {
  return Array.isArray(value)
    ? value.filter(isSchemaNode)
    : [];
}

function uniqueSchemas(schemas: readonly JsonSchemaNode[]): JsonSchemaNode[] {
  const seen = new Set<JsonSchemaNode>();
  return schemas.filter((schema) => {
    if (seen.has(schema)) return false;
    seen.add(schema);
    return true;
  });
}

function isSchemaNode(value: unknown): value is JsonSchemaNode {
  return typeof value === 'boolean' || isRecord(value);
}

function asSchemaObject(value: JsonSchemaNode): Record<string, unknown> | null {
  return typeof value === 'boolean' ? null : value;
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return isRecord(value) ? value : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isSuggestedValue(value: unknown): value is JsonSchemaSuggestedValue {
  return value === null
    || typeof value === 'string'
    || typeof value === 'number'
    || typeof value === 'boolean'
    || Array.isArray(value)
    || isRecord(value);
}

function sameJsonValue(left: unknown, right: unknown): boolean {
  if (Object.is(left, right)) return true;
  if (
    (Array.isArray(left) || isRecord(left))
    && (Array.isArray(right) || isRecord(right))
  ) {
    return stableValueIdentity(left) === stableValueIdentity(right);
  }
  return false;
}

function stableValueIdentity(value: unknown): string {
  return JSON.stringify(value) ?? String(value);
}

function displayValue(value: JsonSchemaSuggestedValue): string {
  return typeof value === 'string' ? value || '""' : stableValueIdentity(value);
}

function jsonType(value: unknown): string {
  if (value === null) return 'null';
  if (Array.isArray(value)) return 'array';
  if (typeof value === 'number' && Number.isInteger(value)) return 'integer';
  return typeof value;
}

export function jsonNodeAtPath(
  root: JsonNode,
  path: JSONPath,
): JsonNode | undefined {
  return findNodeAtLocation(root, path);
}

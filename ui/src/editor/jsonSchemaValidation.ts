import Ajv2020, {
  type ErrorObject,
  type ValidateFunction,
} from 'ajv/dist/2020.js';
import {
  findNodeAtLocation,
  type JSONPath,
  type Node as JsonNode,
} from 'jsonc-parser';
import type { ConfigurationDiagnostic } from './configurationLanguage';
import type { JsonSchemaNode } from './jsonSchema';

const MAX_SCHEMA_DIAGNOSTICS = 100;
const ANNOTATION_KEYWORD_PATTERN = /^x-[a-z][a-z0-9-]*$/;

export class JsonSchemaValidator {
  private readonly validate: ValidateFunction;

  private constructor(validate: ValidateFunction) {
    this.validate = validate;
  }

  static compile(
    schema: JsonSchemaNode,
    annotationKeywords: readonly string[],
  ): JsonSchemaValidator {
    const ajv = new Ajv2020({
      allowUnionTypes: true,
      allErrors: true,
      logger: false,
      strict: true,
      validateFormats: false,
    });
    for (const keyword of new Set(annotationKeywords)) {
      if (!ANNOTATION_KEYWORD_PATTERN.test(keyword)) {
        throw new Error(`Invalid JSON Schema annotation keyword: ${keyword}`);
      }
      ajv.addKeyword({ keyword, valid: true, errors: false });
    }
    return new JsonSchemaValidator(ajv.compile(schema));
  }

  analyze(value: unknown, root: JsonNode): ConfigurationDiagnostic[] {
    if (this.validate(value)) return [];
    return normalizeErrors(this.validate.errors ?? [])
      .slice(0, MAX_SCHEMA_DIAGNOSTICS)
      .map((error) => diagnosticForError(error, root));
  }
}

function normalizeErrors(errors: readonly ErrorObject[]): ErrorObject[] {
  const combinatorPaths = new Set(
    errors
      .filter((error) => error.keyword === 'oneOf' || error.keyword === 'anyOf')
      .map((error) => error.instancePath),
  );
  const strongParentPaths = new Set(
    errors
      .filter((error) => [
        'additionalProperties',
        'unevaluatedProperties',
        'enum',
        'pattern',
        'minimum',
        'maximum',
        'type',
      ].includes(error.keyword))
      .map((error) => pointerParent(error.instancePath)),
  );
  const constCounts = countByPath(errors, 'const');
  const requiredCounts = countByPath(errors, 'required');
  const emittedConstPaths = new Set<string>();
  const seen = new Set<string>();
  const normalized: ErrorObject[] = [];

  for (let error of errors) {
    if (
      (error.keyword === 'oneOf' || error.keyword === 'anyOf')
      && errors.some((candidate) =>
        candidate !== error
        && candidate.keyword !== 'oneOf'
        && candidate.keyword !== 'anyOf'
        && pointerContains(error.instancePath, candidate.instancePath))
    ) {
      continue;
    }
    if (
      error.keyword === 'required'
      && ((requiredCounts.get(error.instancePath) ?? 0) > 1
        || strongParentPaths.has(error.instancePath))
      && combinatorPaths.has(error.instancePath)
    ) {
      continue;
    }
    if (error.keyword === 'const') {
      const parent = pointerParent(error.instancePath);
      if (strongParentPaths.has(parent)) continue;
      if ((constCounts.get(error.instancePath) ?? 0) > 1) {
        if (emittedConstPaths.has(error.instancePath)) continue;
        emittedConstPaths.add(error.instancePath);
        error = {
          ...error,
          keyword: 'enum',
          message: 'must be one of the allowed values',
          params: {},
        };
      }
    }
    const identity = [
      error.instancePath,
      error.keyword,
      JSON.stringify(error.params),
    ].join(':');
    if (seen.has(identity)) continue;
    seen.add(identity);
    normalized.push(error);
  }

  return normalized.sort((left, right) =>
    left.instancePath.localeCompare(right.instancePath)
      || diagnosticPriority(left.keyword) - diagnosticPriority(right.keyword));
}

function diagnosticForError(
  error: ErrorObject,
  root: JsonNode,
): ConfigurationDiagnostic {
  const path = pointerPath(error.instancePath);
  const property = errorProperty(error);
  const targetPath = property === undefined ? path : [...path, property];
  const valueNode = findNodeAtLocation(root, targetPath)
    ?? findNodeAtLocation(root, path)
    ?? root;
  const target = property !== undefined
    ? propertyKeyNode(valueNode) ?? valueNode
    : valueNode;
  return {
    from: target.offset,
    to: Math.max(target.offset + 1, target.offset + target.length),
    severity: 'error',
    code: `jsonSchema.${error.keyword}`,
    message: error.message ?? 'does not match the configuration schema',
    parameters: diagnosticParameters(error, property),
  };
}

function diagnosticParameters(
  error: ErrorObject,
  property: string | undefined,
): Record<string, string | number | boolean> | undefined {
  const parameters: Record<string, string | number | boolean> = {};
  if (property !== undefined) parameters.property = property;
  for (const key of ['type', 'pattern', 'limit', 'comparison']) {
    const value = (error.params as Record<string, unknown>)[key];
    if (
      typeof value === 'string'
      || typeof value === 'number'
      || typeof value === 'boolean'
    ) {
      parameters[key] = value;
    }
  }
  return Object.keys(parameters).length ? parameters : undefined;
}

function errorProperty(error: ErrorObject): string | undefined {
  const parameters = error.params as Record<string, unknown>;
  if (
    error.keyword === 'additionalProperties'
    && typeof parameters.additionalProperty === 'string'
  ) {
    return parameters.additionalProperty;
  }
  if (
    error.keyword === 'unevaluatedProperties'
    && typeof parameters.unevaluatedProperty === 'string'
  ) {
    return parameters.unevaluatedProperty;
  }
  if (error.keyword === 'required' && typeof parameters.missingProperty === 'string') {
    return parameters.missingProperty;
  }
  return undefined;
}

function propertyKeyNode(node: JsonNode): JsonNode | undefined {
  if (node.type === 'property') return node.children?.[0];
  if (node.parent?.type === 'property') return node.parent.children?.[0];
  return undefined;
}

function pointerPath(pointer: string): JSONPath {
  if (!pointer) return [];
  return pointer
    .slice(1)
    .split('/')
    .map((segment) => segment.replaceAll('~1', '/').replaceAll('~0', '~'))
    .map((segment) => /^(0|[1-9]\d*)$/.test(segment) ? Number(segment) : segment);
}

function pointerParent(pointer: string): string {
  const separator = pointer.lastIndexOf('/');
  return separator <= 0 ? '' : pointer.slice(0, separator);
}

function pointerContains(parent: string, child: string): boolean {
  return parent === child || child.startsWith(`${parent}/`);
}

function countByPath(
  errors: readonly ErrorObject[],
  keyword: string,
): Map<string, number> {
  const counts = new Map<string, number>();
  for (const error of errors) {
    if (error.keyword !== keyword) continue;
    counts.set(error.instancePath, (counts.get(error.instancePath) ?? 0) + 1);
  }
  return counts;
}

function diagnosticPriority(keyword: string): number {
  if (keyword === 'additionalProperties' || keyword === 'unevaluatedProperties') return 0;
  if (keyword === 'required' || keyword === 'type' || keyword === 'enum') return 1;
  if (keyword === 'oneOf' || keyword === 'anyOf') return 3;
  return 2;
}

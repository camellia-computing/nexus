import type {
  JsonSchemaCompletionSemantics,
  JsonSchemaSemanticSuggestion,
} from '../../editor/jsonSchema';

const TAG_REFERENCE_KEYWORD = 'x-tag-reference';

const REFERENCE_CONTAINERS: Readonly<Record<string, readonly (readonly string[])[]>> = {
  outbound: [['outbounds'], ['endpoints']],
  inbound: [['inbounds']],
  dns_server: [['dns', 'servers']],
  rule_set: [['route', 'rule_set']],
  certificate_provider: [['certificate_providers']],
  http_client: [['http_clients']],
  network_namespace: [['network_namespaces']],
};

export const singBoxJsonSchemaSemantics: JsonSchemaCompletionSemantics = {
  annotationKeywords: [TAG_REFERENCE_KEYWORD],
  completeValue: ({ document, path, annotations }) => {
    const kinds = new Set(
      annotations(TAG_REFERENCE_KEYWORD)
        .filter((value): value is string => typeof value === 'string'),
    );
    const suggestions: JsonSchemaSemanticSuggestion[] = [];
    const seen = new Set<string>();
    for (const kind of kinds) {
      const containers = REFERENCE_CONTAINERS[kind] ?? [];
      const ownTag = enclosingTag(document, path, containers);
      for (const container of containers) {
        const entries = valueAtPath(document, container);
        if (!Array.isArray(entries)) continue;
        for (const entry of entries) {
          if (!isRecord(entry) || typeof entry.tag !== 'string' || !entry.tag) continue;
          if (entry.tag === ownTag || seen.has(entry.tag)) continue;
          seen.add(entry.tag);
          suggestions.push({
            value: entry.tag,
            detail: `${kind.replaceAll('_', ' ')} reference`,
            boost: 5,
          });
        }
      }
    }
    return suggestions;
  },
};

function enclosingTag(
  document: unknown,
  path: readonly (string | number)[],
  containers: readonly (readonly string[])[],
): string | undefined {
  for (const container of containers) {
    if (
      path.length <= container.length
      || !container.every((segment, index) => path[index] === segment)
      || typeof path[container.length] !== 'number'
    ) {
      continue;
    }
    const entry = valueAtPath(document, [
      ...container,
      path[container.length],
    ]);
    if (isRecord(entry) && typeof entry.tag === 'string') return entry.tag;
  }
  return undefined;
}

function valueAtPath(
  value: unknown,
  path: readonly (string | number)[],
): unknown {
  let current = value;
  for (const segment of path) {
    if (typeof segment === 'number') {
      current = Array.isArray(current) ? current[segment] : undefined;
    } else {
      current = isRecord(current) ? current[segment] : undefined;
    }
  }
  return current;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

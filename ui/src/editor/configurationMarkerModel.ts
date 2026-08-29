import type { ConfigurationLayer, RawDecisionStatus, SemanticPathSegment } from '../types';

export type ConfigurationEditorMarkerKind = 'conflict' | 'validation' | 'warning' | 'source';
export type ConfigurationEditorMarkerResolution = 'keepMine' | 'useUpdated' | 'combine';

export interface ConfigurationEditorMarker {
  id: string;
  kind: ConfigurationEditorMarkerKind;
  severity: 'error' | 'warning' | 'info';
  message: string;
  semanticPath: string;
  segments: SemanticPathSegment[];
  resolvable?: boolean;
  canCombine?: boolean;
  layer?: ConfigurationLayer;
  status?: RawDecisionStatus;
  messageKey?: string;
  sourceIds?: string[];
  issueIds?: string[];
}

export function semanticPathSegments(path: string): SemanticPathSegment[] {
  if (!path || path === '/') return [];
  const segments: SemanticPathSegment[] = [];
  for (const encodedPart of path.split('/').slice(1)) {
    const part = encodedPart.replaceAll('~1', '/').replaceAll('~0', '~');
    const selectorStart = part.indexOf('[');
    const key = selectorStart < 0 ? part : part.slice(0, selectorStart);
    if (key) segments.push({ kind: 'key', key });
    if (selectorStart < 0) continue;
    const selectors = part.slice(selectorStart);
    const expression = /\[([^=\]]+)=([^\]]*)\]/g;
    let match: RegExpExecArray | null;
    while ((match = expression.exec(selectors)) !== null) {
      segments.push({ kind: 'identity', field: match[1], value: match[2] });
    }
  }
  return segments;
}

import type { ProgramKind } from '../types.ts';

const EXECUTABLE_NAMES: Record<ProgramKind, string> = {
  generic: 'program',
  singBox: 'sing-box',
  xray: 'xray',
  mihomo: 'mihomo',
};

export function executableNameFor(kind: ProgramKind): string {
  return EXECUTABLE_NAMES[kind];
}

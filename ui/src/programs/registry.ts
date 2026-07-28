import type { ProgramKind } from '../types';
import { genericProgram } from './generic';
import { mihomoProgram } from './mihomo';
import { singBoxProgram } from './sing-box';
import type { ProgramDefinition } from './types';
import { xrayProgram } from './xray';

const programs: Record<ProgramKind, ProgramDefinition> = {
  generic: genericProgram,
  singBox: singBoxProgram,
  xray: xrayProgram,
  mihomo: mihomoProgram,
};

export const programDefinitions = [
  genericProgram,
  singBoxProgram,
  xrayProgram,
  mihomoProgram,
] as const;

export function programDefinition(kind: ProgramKind): ProgramDefinition {
  return programs[kind];
}

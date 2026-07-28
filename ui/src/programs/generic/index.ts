import type { ProgramDefinition } from '../types';
import { executableNameFor } from '../executableNames';

export const genericProgram: ProgramDefinition = {
  kind: 'generic',
  displayName: 'Generic',
  executableName: executableNameFor('generic'),
  templateName: 'Generic binary',
  templateDescription: 'Exact argument-vector execution for arbitrary binaries.',
};

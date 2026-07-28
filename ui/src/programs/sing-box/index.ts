import { enrichConfigurationArguments } from '../shared/configuration';
import type { ProgramDefinition } from '../types';
import type { SingBoxClashDashboard, SingBoxDashboard } from '../../types';
import { executableNameFor } from '../executableNames';
import { singBoxJsonSchemaSemantics } from './jsonSchema';
export { applySingBoxDashboardChange } from './dashboard-state';

export interface SingBoxDashboardOptions {
  native?: SingBoxDashboard;
  clash?: SingBoxClashDashboard;
}

export type SingBoxDashboardChange =
  | { kind: 'native'; value?: SingBoxDashboard }
  | { kind: 'clash'; value?: SingBoxClashDashboard };


const configurationFlags = ['-c', '--config', '-C', '--config-directory'] as const;

export const singBoxProgram: ProgramDefinition = {
  kind: 'singBox',
  displayName: 'sing-box',
  executableName: executableNameFor('singBox'),
  templateName: 'sing-box',
  templateDescription: 'Managed configuration with native validation and formatting.',
  configuration: {
    flags: configurationFlags,
    manualConfigPath: 'config/manual-override.json',
    managedConfigPath: 'config/managed.json',
    language: 'jsonc',
    initialConfigPlaceholder: '{\n  "log": {}\n}',
    storedConfigurationMode: 'merge',
    jsonSchemaSemantics: singBoxJsonSchemaSemantics,
    enrichArguments: (result, context) =>
      enrichConfigurationArguments(result, configurationFlags, context.managedConfiguration),
  },
};

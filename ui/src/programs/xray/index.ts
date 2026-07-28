import { enrichConfigurationArguments } from '../shared/configuration';
import type { ProgramDefinition } from '../types';
import { executableNameFor } from '../executableNames';

const configurationFlags = ['-c', '-config', '-confdir'] as const;

export const xrayProgram: ProgramDefinition = {
  kind: 'xray',
  displayName: 'Xray',
  executableName: executableNameFor('xray'),
  templateName: 'Xray',
  templateDescription: 'Configuration validation, resolved output and diagnostics.',
  configuration: {
    flags: configurationFlags,
    manualConfigPath: 'config/manual-override.json',
    managedConfigPath: 'config/managed.json',
    language: 'jsonc',
    initialConfigPlaceholder: '{\n  "log": {}\n}',
    storedConfigurationMode: 'merge',
    enrichArguments: (result, context) => {
      const enriched = enrichConfigurationArguments(
        result,
        configurationFlags,
        context.managedConfiguration,
      );
      if (enriched.error) return enriched;
      if (
        enriched.args.some((argument) => argument === '-test' || argument === '-dump')
      ) {
        return {
          ...enriched,
          warnings: [
            ...enriched.warnings,
            'Xray diagnostic flags such as -test or -dump normally exit instead of staying active.',
          ],
        };
      }
      return enriched;
    },
  },
};

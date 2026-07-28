import {
  enrichConfigurationArguments,
  hasConfigurationArgument,
} from '../shared/configuration.ts';
import { executableNameFor } from '../executableNames.ts';
import type { ProgramDefinition } from '../types.ts';

const configurationFlags = ['-f', '--f', '-config', '--config'] as const;

export const mihomoProgram: ProgramDefinition = {
  kind: 'mihomo',
  displayName: 'Mihomo',
  executableName: executableNameFor('mihomo'),
  templateName: 'Mihomo',
  templateDescription: 'Managed YAML configuration with native validation and external UI.',
  configuration: {
    flags: configurationFlags,
    manualConfigPath: 'config/manual-override.yaml',
    managedConfigPath: 'config/managed.yaml',
    language: 'yaml',
    initialConfigPlaceholder: 'mode: rule\nlog-level: info',
    storedConfigurationMode: 'exclusive',
    enrichArguments: (result, context) => {
      const enriched = enrichConfigurationArguments(
        result,
        configurationFlags,
        context.managedConfiguration,
      );
      if (enriched.error) return enriched;
      if (
        enriched.args.some((argument) =>
          ['-age-secret-key', '--age-secret-key'].some(
            (flag) => argument === flag || argument.startsWith(`${flag}=`),
          ),
        )
      ) {
        return {
          ...enriched,
          error: 'Mihomo age secret keys must not be stored in program arguments or environment variables.',
        };
      }
      if (
        enriched.args.some((argument) =>
          ['-config', '--config'].some(
            (flag) => argument === flag || argument.startsWith(`${flag}=`),
          ),
        )
      ) {
        return {
          ...enriched,
          error: 'Mihomo inline configuration is unavailable; use a configuration file.',
        };
      }
      if (
        context.storedConfiguration &&
        hasConfigurationArgument(configurationFlags, enriched.args)
      ) {
        return {
          ...enriched,
          error: 'Configuration path arguments conflict with the stored Mihomo configuration.',
        };
      }
      const diagnostics = enriched.args.filter((argument) =>
        ['-t', '--t', '-v', '--v'].includes(argument),
      );
      return diagnostics.length
        ? {
            ...enriched,
            warnings: [
              ...enriched.warnings,
              'Mihomo diagnostic flags such as -t or -v normally exit instead of staying active.',
            ],
          }
        : enriched;
    },
  },
};

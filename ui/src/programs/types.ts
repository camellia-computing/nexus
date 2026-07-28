import type { ArgumentParseResult } from '../arguments';
import type { ProgramKind } from '../types';
import type { JsonSchemaCompletionSemantics } from '../editor/jsonSchema';

export interface ProgramDefinition {
  kind: ProgramKind;
  displayName: string;
  executableName: string;
  templateName: string;
  templateDescription: string;
  configuration?: {
    flags: readonly string[];
    manualConfigPath: string;
    managedConfigPath: string;
    language: 'jsonc' | 'yaml';
    initialConfigPlaceholder: string;
    storedConfigurationMode: 'merge' | 'exclusive';
    jsonSchemaSemantics?: JsonSchemaCompletionSemantics;
    enrichArguments: (
      result: ArgumentParseResult,
      context: {
        managedConfiguration: boolean;
        storedConfiguration: boolean;
      },
    ) => ArgumentParseResult;
  };
}

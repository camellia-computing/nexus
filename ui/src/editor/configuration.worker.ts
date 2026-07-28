import {
  analyzeConfiguration,
  analyzeJsonDocument,
  formatConfiguration,
  type ConfigurationLanguageTask,
  type ConfigurationLanguageTaskResult,
} from './configurationLanguage';
import { getNodeValue } from 'jsonc-parser';
import { parseJsonSchemaDocument } from './jsonSchema';
import { JsonSchemaValidator } from './jsonSchemaValidation';

type ConfigurationWorkerScope = {
  onmessage: ((event: MessageEvent<ConfigurationLanguageTask>) => void) | null;
  postMessage: (message: ConfigurationLanguageTaskResult) => void;
};

const workerScope = globalThis as unknown as ConfigurationWorkerScope;
let jsonSchemaValidator: JsonSchemaValidator | null = null;

workerScope.onmessage = (event) => {
  const task = event.data;
  try {
    if (task.kind === 'analyze') {
      if (task.language === 'jsonc') {
        const analysis = analyzeJsonDocument(task.content);
        if (
          jsonSchemaValidator
          && analysis.root
          && !analysis.diagnostics.some((diagnostic) => diagnostic.severity === 'error')
        ) {
          analysis.diagnostics.push(
            ...jsonSchemaValidator.analyze(getNodeValue(analysis.root), analysis.root),
          );
        }
        workerScope.postMessage({
          id: task.id,
          ok: true,
          kind: task.kind,
          result: { diagnostics: analysis.diagnostics },
        });
        return;
      }
      workerScope.postMessage({
        id: task.id,
        ok: true,
        kind: task.kind,
        result: analyzeConfiguration(task.language, task.content),
      });
    } else if (task.kind === 'format') {
      workerScope.postMessage({
        id: task.id,
        ok: true,
        kind: task.kind,
        result: formatConfiguration(task.language, task.content),
      });
    } else {
      jsonSchemaValidator = null;
      if (task.schema) {
        jsonSchemaValidator = JsonSchemaValidator.compile(
          parseJsonSchemaDocument(task.schema),
          task.annotationKeywords,
        );
      }
      workerScope.postMessage({
        id: task.id,
        ok: true,
        kind: task.kind,
      });
    }
  } catch (error) {
    workerScope.postMessage({
      id: task.id,
      ok: false,
      error: error instanceof Error ? error.message : 'Configuration language task failed',
    });
  }
};

export {};

import type {
  ConfigurationAnalysis,
  ConfigurationFormatResult,
  ConfigurationLanguage,
  ConfigurationLanguageTask,
  ConfigurationLanguageTaskResult,
} from './configurationLanguage';
import type { ConfigurationSchemaDocument } from '../types';

type PendingTask = {
  reject: (error: Error) => void;
  resolve: (result: ConfigurationLanguageTaskResult) => void;
  timeout: ReturnType<typeof setTimeout>;
};

type JsonSchemaConfiguration = {
  schema: ConfigurationSchemaDocument;
  annotationKeywords: string[];
};

type ConfigurationLanguageRequest =
  ConfigurationLanguageTask extends infer Task
    ? Task extends { id: number }
      ? Omit<Task, 'id'>
      : never
    : never;

const LANGUAGE_TASK_TIMEOUT_MS = 15_000;

export class ConfigurationLanguageService {
  private worker: Worker | null = null;
  private disposed = false;
  private nextId = 1;
  private readonly pending = new Map<number, PendingTask>();
  private jsonSchemaConfiguration: JsonSchemaConfiguration | null = null;
  private jsonSchemaConfigurationVersion = 0;
  private jsonSchemaWorker: Worker | null = null;
  private jsonSchemaSetup: Promise<void> | null = null;

  constructor() {
    this.startWorker();
  }

  private startWorker(): void {
    if (this.disposed || this.worker) return;
    try {
      const worker = new Worker(new URL('./configuration.worker.ts', import.meta.url), {
        name: 'camellia-configuration-language-service',
        type: 'module',
      });
      this.worker = worker;
      worker.onmessage = (event: MessageEvent<ConfigurationLanguageTaskResult>) => {
        if (this.worker !== worker) return;
        const pending = this.pending.get(event.data.id);
        if (!pending) return;
        clearTimeout(pending.timeout);
        this.pending.delete(event.data.id);
        if (event.data.ok) pending.resolve(event.data);
        else pending.reject(new Error(event.data.error));
      };
      worker.onerror = () => {
        this.failWorker(new Error('Configuration language worker failed'), worker);
      };
      worker.onmessageerror = () => {
        this.failWorker(
          new Error('Configuration language worker returned an invalid response'),
          worker,
        );
      };
    } catch {
      this.worker = null;
    }
  }

  async analyze(
    language: ConfigurationLanguage,
    content: string,
  ): Promise<ConfigurationAnalysis> {
    try {
      const response = await this.request({ kind: 'analyze', language, content });
      if (response.ok && response.kind === 'analyze') return response.result;
      throw new Error('Configuration language worker returned the wrong result');
    } catch {
      const { analyzeConfiguration } = await import('./configurationLanguage');
      return analyzeConfiguration(language, content);
    }
  }

  async format(
    language: ConfigurationLanguage,
    content: string,
  ): Promise<ConfigurationFormatResult> {
    try {
      const response = await this.request({ kind: 'format', language, content });
      if (response.ok && response.kind === 'format') return response.result;
      throw new Error('Configuration language worker returned the wrong result');
    } catch {
      const { formatConfiguration } = await import('./configurationLanguage');
      return formatConfiguration(language, content);
    }
  }

  async configureJsonSchema(
    schema: ConfigurationSchemaDocument | null,
    annotationKeywords: readonly string[] = [],
  ): Promise<void> {
    this.startWorker();
    const worker = this.worker;
    if (!worker) {
      throw new Error('Configuration language worker is unavailable');
    }

    this.jsonSchemaConfigurationVersion += 1;
    this.jsonSchemaConfiguration = schema
      ? { schema, annotationKeywords: [...annotationKeywords] }
      : null;
    this.jsonSchemaWorker = null;
    this.jsonSchemaSetup = null;
    if (this.jsonSchemaConfiguration) {
      await this.ensureJsonSchemaConfigured(worker);
      return;
    }

    const response = await this.requestOnWorker(worker, {
      kind: 'configureJsonSchema',
      schema: null,
      annotationKeywords: [],
    });
    if (!response.ok || response.kind !== 'configureJsonSchema') {
      throw new Error('Configuration language worker returned the wrong result');
    }
  }

  dispose(): void {
    this.disposed = true;
    this.failWorker(new Error('Configuration language service was disposed'));
  }

  private async request(
    task: ConfigurationLanguageRequest,
  ): Promise<ConfigurationLanguageTaskResult> {
    this.startWorker();
    const worker = this.worker;
    if (!worker) {
      throw new Error('Configuration language worker is unavailable');
    }
    await this.ensureJsonSchemaConfigured(worker);
    return this.requestOnWorker(worker, task);
  }

  private requestOnWorker(
    worker: Worker,
    task: ConfigurationLanguageRequest,
  ): Promise<ConfigurationLanguageTaskResult> {
    if (this.worker !== worker) {
      return Promise.reject(new Error('Configuration language worker was replaced'));
    }
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error('Configuration language task timed out'));
        this.failWorker(new Error('Configuration language worker timed out'), worker);
      }, LANGUAGE_TASK_TIMEOUT_MS);
      this.pending.set(id, { reject, resolve, timeout });
      worker.postMessage({ ...task, id } as ConfigurationLanguageTask);
    });
  }

  private async ensureJsonSchemaConfigured(worker: Worker): Promise<void> {
    const configuration = this.jsonSchemaConfiguration;
    if (!configuration || this.jsonSchemaWorker === worker) return;
    if (this.jsonSchemaSetup) {
      await this.jsonSchemaSetup;
      return;
    }

    const version = this.jsonSchemaConfigurationVersion;
    const setup = (async () => {
      const response = await this.requestOnWorker(worker, {
        kind: 'configureJsonSchema',
        schema: configuration.schema,
        annotationKeywords: configuration.annotationKeywords,
      });
      if (!response.ok || response.kind !== 'configureJsonSchema') {
        throw new Error('Configuration language worker returned the wrong result');
      }
      if (
        this.worker === worker
        && this.jsonSchemaConfigurationVersion === version
      ) {
        this.jsonSchemaWorker = worker;
      }
    })();
    this.jsonSchemaSetup = setup;
    try {
      await setup;
    } finally {
      if (this.jsonSchemaSetup === setup) this.jsonSchemaSetup = null;
    }
  }

  private failWorker(error: Error, expected?: Worker): void {
    if (expected && this.worker !== expected) return;
    this.worker?.terminate();
    this.worker = null;
    this.jsonSchemaWorker = null;
    this.jsonSchemaSetup = null;
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timeout);
      pending.reject(error);
    }
    this.pending.clear();
  }
}

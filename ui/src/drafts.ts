import type { ConfigSource, ProgramKind, RestartPolicy } from './types';
import { executableNameFor } from './programs/executableNames.ts';

export const CREATE_DRAFT_STORAGE_KEY = 'camellia-nexus.create-drafts';

export interface EnvironmentEntry {
  key: string;
  value: string;
}

export interface CreateDraft {
  kind: ProgramKind;
  id: string;
  name: string;
  mode: 'managed' | 'external';
  executable: string;
  packageSource: string;
  argumentLine: string;
  environment: EnvironmentEntry[];
  autoStart: boolean;
  restartPolicy: RestartPolicy;
  privilegeMode: 'standard' | 'automatic' | 'elevated';
  managedConfiguration: boolean;
  configSources: ConfigSource[];
  remoteAutoUpdate: boolean;
  remoteUpdateIntervalMinutes: number;
  dashboardEnabled: boolean;
  dashboardPort: number;
  dashboardUpdateInterval: string;
  clashDashboardEnabled: boolean;
  clashDashboardPort: number;
  clashDashboardDownloadUrl: string;
  xrayDashboardEnabled: boolean;
  xrayApiPort: number;
  xrayMetricsPort: number;
  mihomoDashboardEnabled: boolean;
  mihomoDashboardPort: number;
  mihomoDashboardDownloadUrl: string;
  initialConfig: string;
}

function executableName(kind: ProgramKind, platform: string): string {
  const suffix = platform === 'Windows' ? '.exe' : '';
  return `${executableNameFor(kind)}${suffix}`;
}

export function defaultDraft(kind: ProgramKind, platform = ''): CreateDraft {
  return {
    kind,
    id: '',
    name: '',
    mode: 'managed',
    executable: executableName(kind, platform),
    packageSource: '',
    argumentLine: '',
    environment: [],
    autoStart: false,
    restartPolicy: 'onFailure',
    privilegeMode: 'automatic',
    managedConfiguration: false,
    configSources: [],
    remoteAutoUpdate: false,
    remoteUpdateIntervalMinutes: 60,
    dashboardEnabled: false,
    dashboardPort: 9090,
    dashboardUpdateInterval: '1d',
    clashDashboardEnabled: false,
    clashDashboardPort: 9091,
    clashDashboardDownloadUrl: '',
    xrayDashboardEnabled: false,
    xrayApiPort: 10085,
    xrayMetricsPort: 11111,
    mihomoDashboardEnabled: false,
    mihomoDashboardPort: 9092,
    mihomoDashboardDownloadUrl: '',
    initialConfig: '',
  };
}

function sanitize(value: unknown, fallback: CreateDraft): CreateDraft {
  if (!value || typeof value !== 'object') return fallback;
  const draft = value as Partial<CreateDraft>;
  const text = (candidate: unknown, defaultValue: string, max: number) =>
    typeof candidate === 'string' ? candidate.slice(0, max) : defaultValue;
  const environment = Array.isArray(draft.environment)
    ? draft.environment
        .filter(
          (entry): entry is EnvironmentEntry =>
            !!entry &&
            typeof entry === 'object' &&
            typeof (entry as EnvironmentEntry).key === 'string' &&
            typeof (entry as EnvironmentEntry).value === 'string',
        )
        .slice(0, 256)
        .map((entry) => ({ key: entry.key.slice(0, 256), value: '' }))
    : fallback.environment;
  const configSources = Array.isArray(draft.configSources)
    ? draft.configSources
        .filter((source): source is ConfigSource => {
          if (!source || typeof source !== 'object') return false;
          const value = source as Partial<ConfigSource>;
          return (value.mode === 'local' || value.mode === 'remote')
            && typeof value.id === 'string'
            && typeof value.name === 'string';
        })
        .map((source) => source.mode === 'local'
          ? {
              mode: 'local' as const,
              id: source.id.slice(0, 64),
              name: source.name.slice(0, 128),
              enabled: source.enabled !== false,
              path: typeof source.path === 'string' ? source.path.slice(0, 32_000) : '',
            }
          : {
              mode: 'remote' as const,
              id: source.id.slice(0, 64),
              name: source.name.slice(0, 128),
              enabled: source.enabled !== false,
              url: safePersistedUrl(source.url),
              authentication:
                source.authentication?.scheme === 'basic'
                  ? {
                      scheme: 'basic' as const,
                      username:
                        typeof source.authentication.username === 'string'
                          ? source.authentication.username.slice(0, 256)
                          : '',
                      credentialId:
                        typeof source.authentication.credentialId === 'string'
                          ? source.authentication.credentialId.slice(0, 160)
                          : undefined,
                      password: '',
                    }
                  : undefined,
            })
    : fallback.configSources;
  return {
    kind: fallback.kind,
    id: text(draft.id, fallback.id, 63),
    name: text(draft.name, fallback.name, 128),
    mode: draft.mode === 'external' ? 'external' : 'managed',
    executable: text(draft.executable, fallback.executable, 32_000),
    packageSource: text(draft.packageSource, fallback.packageSource, 32_000),
    argumentLine: '',
    environment,
    autoStart: draft.autoStart === true,
    restartPolicy: ['never', 'onFailure', 'always'].includes(draft.restartPolicy ?? '')
      ? (draft.restartPolicy as RestartPolicy)
      : fallback.restartPolicy,
    privilegeMode: ['standard', 'automatic', 'elevated'].includes(draft.privilegeMode ?? '')
      ? (draft.privilegeMode as CreateDraft['privilegeMode'])
      : fallback.privilegeMode,
    managedConfiguration:
      draft.managedConfiguration === true ||
      (typeof draft.managedConfiguration !== 'boolean' && !!draft.initialConfig),
    configSources,
    remoteAutoUpdate: draft.remoteAutoUpdate === true,
    remoteUpdateIntervalMinutes: [15, 60, 360, 720, 1440].includes(
      Number(draft.remoteUpdateIntervalMinutes),
    )
      ? Number(draft.remoteUpdateIntervalMinutes)
      : fallback.remoteUpdateIntervalMinutes,
    dashboardEnabled: draft.dashboardEnabled === true,
    dashboardPort:
      Number.isInteger(draft.dashboardPort) && Number(draft.dashboardPort) >= 1024
        ? Number(draft.dashboardPort)
        : fallback.dashboardPort,
    dashboardUpdateInterval: text(
      draft.dashboardUpdateInterval,
      fallback.dashboardUpdateInterval,
      16,
    ),
    clashDashboardEnabled: draft.clashDashboardEnabled === true,
    clashDashboardPort:
      Number.isInteger(draft.clashDashboardPort) && Number(draft.clashDashboardPort) >= 1024
        ? Number(draft.clashDashboardPort)
        : fallback.clashDashboardPort,
    clashDashboardDownloadUrl: safePersistedUrl(draft.clashDashboardDownloadUrl),
    xrayDashboardEnabled: draft.xrayDashboardEnabled === true,
    xrayApiPort:
      Number.isInteger(draft.xrayApiPort) && Number(draft.xrayApiPort) >= 1024
        ? Number(draft.xrayApiPort)
        : fallback.xrayApiPort,
    xrayMetricsPort:
      Number.isInteger(draft.xrayMetricsPort) && Number(draft.xrayMetricsPort) >= 1024
        ? Number(draft.xrayMetricsPort)
        : fallback.xrayMetricsPort,
    mihomoDashboardEnabled: draft.mihomoDashboardEnabled === true,
    mihomoDashboardPort:
      Number.isInteger(draft.mihomoDashboardPort) && Number(draft.mihomoDashboardPort) >= 1024
        ? Number(draft.mihomoDashboardPort)
        : fallback.mihomoDashboardPort,
    mihomoDashboardDownloadUrl: safePersistedUrl(draft.mihomoDashboardDownloadUrl),
    initialConfig: '',
  };
}

function safePersistedUrl(value: unknown): string {
  if (typeof value !== 'string' || !value) return '';
  try {
    const url = new URL(value);
    if (url.protocol !== 'https:' && url.protocol !== 'http:') return '';
    url.username = '';
    url.password = '';
    url.search = '';
    url.hash = '';
    return url.toString().slice(0, 2048);
  } catch {
    return '';
  }
}

function readAll(): Partial<Record<ProgramKind, unknown>> {
  try {
    const value = JSON.parse(localStorage.getItem(CREATE_DRAFT_STORAGE_KEY) ?? '{}');
    return value && typeof value === 'object' ? value : {};
  } catch {
    return {};
  }
}

export function loadCreateDraft(kind: ProgramKind, platform = ''): CreateDraft {
  const fallback = defaultDraft(kind, platform);
  return sanitize(readAll()[kind], fallback);
}

export function saveCreateDraft(draft: CreateDraft): void {
  try {
    const drafts = readAll();
    drafts[draft.kind] = {
      ...draft,
      argumentLine: '',
      environment: draft.environment.map((entry) => ({ key: entry.key, value: '' })),
      initialConfig: '',
      clashDashboardDownloadUrl: safePersistedUrl(draft.clashDashboardDownloadUrl),
      mihomoDashboardDownloadUrl: safePersistedUrl(draft.mihomoDashboardDownloadUrl),
      configSources: draft.configSources.map((source) =>
        source.mode === 'remote'
          ? {
              ...source,
              url: safePersistedUrl(source.url),
              authentication: source.authentication
                ? { ...source.authentication, password: undefined }
                : undefined,
            }
          : source,
      ),
    };
    localStorage.setItem(CREATE_DRAFT_STORAGE_KEY, JSON.stringify(drafts));
  } catch {
    // Draft persistence is a convenience and must never block program creation.
  }
}

export function clearCreateDraft(kind: ProgramKind, platform = ''): CreateDraft {
  const draft = defaultDraft(kind, platform);
  try {
    const drafts = readAll();
    delete drafts[kind];
    localStorage.setItem(CREATE_DRAFT_STORAGE_KEY, JSON.stringify(drafts));
  } catch {
    // See saveCreateDraft.
  }
  return draft;
}

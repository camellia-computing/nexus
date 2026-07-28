import assert from 'node:assert/strict';
import { getNodeValue } from 'jsonc-parser';
import { formatArgumentLine, parseArgumentLine } from '../src/arguments.ts';
import {
  APPEARANCE_STORAGE_KEY,
  THEME_IDS,
  UI_SCALES,
  loadAppearancePreferences,
  normalizeAppearancePreferences,
  resolveColorScheme,
  saveAppearancePreferences,
} from '../src/lib/theme/preferences.ts';
import { managedWorkingDirectory } from '../src/paths.ts';
import {
  enrichConfigurationArguments,
  effectiveConfigSourceLimit,
  hasConfigurationArgument,
  MAX_CONFIG_SOURCES_PER_PROGRAM,
} from '../src/programs/shared/configuration.ts';
import { applySingBoxDashboardChange } from '../src/programs/sing-box/dashboard-state.ts';
import { mihomoProgram } from '../src/programs/mihomo/index.ts';
import { errorInfoOf } from '../src/errors.ts';
import {
  clientVersionAdvisory,
  compareCanonicalSemVer,
  hasRefreshableLicenseSession,
  isNewerEntitlementSnapshot,
  licenseNoticeKey,
  licenseNoticeRequiresPersistentAttention,
  licenseRuntimeImpact,
  licenseRuntimeNotice,
  licenseStateNotice,
  signedLicenseStatusPresentation,
} from '../src/license.ts';
import { moveCatalogItem } from '../src/catalog.ts';
import {
  CREATE_DRAFT_STORAGE_KEY,
  clearCreateDraft,
  defaultDraft,
  loadCreateDraft,
  saveCreateDraft,
} from '../src/drafts.ts';
import { createAsyncListenerScope } from '../src/lib/asyncListenerScope.ts';
import { canUseProgramLifecycleAction, deriveLicenseAccess } from '../src/licenseAccess.ts';
import { compactPaymentReference, formatBillingAmount } from '../src/billingPresentation.ts';
import { isRuntimeActive } from '../src/programState.ts';
import {
  analyzeConfiguration,
  analyzeJsonDocument,
  formatConfiguration,
} from '../src/editor/configurationLanguage.ts';
import {
  completeJsonSchema,
  parseJsonSchemaDocument,
} from '../src/editor/jsonSchema.ts';
import { JsonSchemaValidator } from '../src/editor/jsonSchemaValidation.ts';
import { singBoxJsonSchemaSemantics } from '../src/programs/sing-box/jsonSchema.ts';

const validJsonAnalysis = analyzeConfiguration(
  'jsonc',
  '{"log":{"level":"info"},"outbounds":[{"tag":"direct"}]}',
);
assert.deepEqual(validJsonAnalysis.diagnostics, []);
const invalidJsonAnalysis = analyzeConfiguration('jsonc', '{"log": {"level": "info",}}');
assert.equal(
  invalidJsonAnalysis.diagnostics.some((diagnostic) => diagnostic.severity === 'error'),
  true,
);
assert.equal(
  invalidJsonAnalysis.diagnostics.some((diagnostic) => diagnostic.code === 'json.PropertyNameExpected'),
  true,
);
assert.equal(
  analyzeConfiguration('jsonc', '{/* comment */"mode":"rule"}').diagnostics.some(
    (diagnostic) => diagnostic.code === 'json.InvalidCommentToken',
  ),
  true,
  'native JSON configuration must not silently accept JSONC comments',
);
const duplicateJson = '{"route":{"final":"direct","final":"proxy"}}';
const duplicateJsonResult = formatConfiguration('jsonc', duplicateJson);
assert.equal(
  duplicateJsonResult.diagnostics.some(
    (diagnostic) => diagnostic.code === 'configuration.duplicateKey',
  ),
  true,
);
assert.equal(duplicateJsonResult.changed, true);
assert.equal(
  duplicateJsonResult.content.match(/"final"/g)?.length,
  2,
  'formatting must preserve duplicate keys for explicit user resolution',
);
assert.equal(
  formatConfiguration('jsonc', '{"route":}').changed,
  false,
  'formatting must not rewrite syntactically invalid JSON',
);

const yamlWithCommentsAndAliases = [
  '# outbound pool',
  'proxies:',
  '  - &edge { name: edge, server: example.test }',
  'selected: *edge',
  '',
].join('\n');
const yamlFormatResult = formatConfiguration('yaml', yamlWithCommentsAndAliases);
assert.deepEqual(yamlFormatResult.diagnostics, []);
assert.match(yamlFormatResult.content, /# outbound pool/);
assert.match(yamlFormatResult.content, /&edge/);
assert.match(yamlFormatResult.content, /\*edge/);
assert.equal(
  analyzeConfiguration('yaml', 'mode: rule\nmode: direct\n').diagnostics.some(
    (diagnostic) => diagnostic.code === 'yaml.DUPLICATE_KEY',
  ),
  true,
);
assert.equal(
  analyzeConfiguration('yaml', '---\nmode: rule\n---\nmode: direct\n').diagnostics.some(
    (diagnostic) => diagnostic.code === 'yaml.MULTIPLE_DOCS',
  ),
  true,
);
assert.equal(
  analyzeConfiguration('yaml', '- rule\n- direct\n').diagnostics.some(
    (diagnostic) => diagnostic.code === 'configuration.rootObjectExpected',
  ),
  true,
);
const crlfYamlResult = formatConfiguration('yaml', 'mode: rule\r\nrules:\r\n - MATCH,DIRECT\r\n');
assert.equal(crlfYamlResult.content.includes('\r\n'), true);
assert.doesNotMatch(crlfYamlResult.content, /(^|[^\r])\n/);
assert.deepEqual(
  formatConfiguration('yaml', '# Keep an intentionally empty configuration.\n'),
  {
    content: '# Keep an intentionally empty configuration.\n',
    changed: false,
    diagnostics: [{
      from: 0,
      to: 1,
      severity: 'warning',
      code: 'configuration.rootObjectExpected',
      message: 'The top-level configuration should be a mapping',
    }],
  },
  'formatting an empty YAML document must not invent a null value or remove comments',
);

const testConfigurationSchema = {
  $schema: 'https://json-schema.org/draft/2020-12/schema',
  type: 'object',
  properties: {
    log: {
      type: 'object',
      properties: {
        level: { enum: ['debug', 'info', 'warn'] },
      },
      additionalProperties: false,
    },
    outbounds: {
      type: 'array',
      items: { $ref: '#/$defs/outbound' },
    },
    route: {
      type: 'object',
      properties: {
        final: { type: 'string', 'x-tag-reference': 'outbound' },
      },
      additionalProperties: false,
    },
  },
  required: ['outbounds'],
  additionalProperties: false,
  $defs: {
    outbound: {
      oneOf: [
        {
          type: 'object',
          properties: {
            type: { const: 'direct' },
            tag: { type: 'string' },
          },
          required: ['type', 'tag'],
          additionalProperties: false,
        },
        {
          type: 'object',
          properties: {
            type: { const: 'socks' },
            tag: { type: 'string' },
            server: { type: 'string' },
            server_port: { type: 'integer', minimum: 1, maximum: 65_535 },
            detour: { type: 'string', 'x-tag-reference': 'outbound' },
          },
          required: ['type', 'tag', 'server', 'server_port'],
          additionalProperties: false,
        },
      ],
    },
  },
};

function schemaCompletionAtMarker(source, semantics) {
  const position = source.indexOf('|');
  assert.notEqual(position, -1, 'completion fixture must contain a cursor marker');
  return completeJsonSchema(
    source.slice(0, position) + source.slice(position + 1),
    position,
    testConfigurationSchema,
    semantics,
  );
}

const rootPropertyCompletion = schemaCompletionAtMarker('{\n  "|"\n}');
assert.deepEqual(
  rootPropertyCompletion.options.map((option) => option.label),
  ['outbounds', 'log', 'route'],
  'generic schema completion should prioritize required properties',
);
assert.equal(rootPropertyCompletion.options[0].kind, 'property');
assert.deepEqual(rootPropertyCompletion.options[0].scaffold, []);

const noDuplicatePropertyCompletion = schemaCompletionAtMarker(
  '{"log": {}, "|"}',
);
assert.equal(
  noDuplicatePropertyCompletion.options.some((option) => option.label === 'log'),
  false,
  'generic property completion must not duplicate an existing key',
);

const enumValueCompletion = schemaCompletionAtMarker(
  '{"log": {"level": "|"}}',
);
assert.deepEqual(
  enumValueCompletion.options.map((option) => option.label),
  ['debug', 'info', 'warn'],
);

const narrowedBranchCompletion = schemaCompletionAtMarker(
  '{"outbounds": [{"type": "socks", "|"}]}',
);
assert.equal(
  narrowedBranchCompletion.options.some((option) => option.label === 'server'),
  true,
  'oneOf completion should retain the branch selected by a discriminator value',
);

const anchoredSchema = {
  $schema: 'https://json-schema.org/draft/2020-12/schema',
  type: 'object',
  properties: {
    mode: { $ref: '#mode', default: 'rule' },
  },
  $defs: {
    mode: {
      $anchor: 'mode',
      enum: ['direct', 'rule'],
    },
  },
};
const anchoredCompletion = completeJsonSchema(
  '{"mode": ""}',
  '{"mode": "'.length,
  anchoredSchema,
);
assert.deepEqual(
  anchoredCompletion.options.map((option) => option.label),
  ['rule', 'direct'],
  'generic completion should resolve local anchors and prioritize defaults',
);

const structuralSchema = {
  $schema: 'https://json-schema.org/draft/2020-12/schema',
  type: 'object',
  properties: {
    mode: { type: 'string' },
    tuple: {
      type: 'array',
      prefixItems: [
        { type: 'boolean' },
        { enum: ['primary', 'secondary'] },
      ],
      items: false,
    },
    labels: {
      type: 'object',
      patternProperties: {
        '^x-': { enum: ['enabled', 'disabled'] },
      },
      additionalProperties: false,
    },
  },
  dependentSchemas: {
    mode: {
      properties: {
        modeOptions: { type: 'object' },
      },
    },
  },
};
const dependentCompletionSource = '{"mode": "rule", "|"}';
const dependentCompletionPosition = dependentCompletionSource.indexOf('|');
const dependentCompletion = completeJsonSchema(
  dependentCompletionSource.slice(0, dependentCompletionPosition)
    + dependentCompletionSource.slice(dependentCompletionPosition + 1),
  dependentCompletionPosition,
  structuralSchema,
);
assert.equal(
  dependentCompletion.options.some((option) => option.label === 'modeOptions'),
  true,
  'dependentSchemas should contribute properties when their trigger is present',
);
const prefixCompletionSource = '{"tuple": [true, ""]}';
const prefixCompletion = completeJsonSchema(
  prefixCompletionSource,
  prefixCompletionSource.indexOf('""') + 1,
  structuralSchema,
);
assert.deepEqual(
  prefixCompletion.options.map((option) => option.label),
  ['primary', 'secondary'],
);
const patternCompletionSource = '{"labels": {"x-feature": ""}}';
const patternCompletion = completeJsonSchema(
  patternCompletionSource,
  patternCompletionSource.lastIndexOf('""') + 1,
  structuralSchema,
);
assert.deepEqual(
  patternCompletion.options.map((option) => option.label),
  ['disabled', 'enabled'],
);

const referenceFixture = [
  '{',
  '  "outbounds": [',
  '    {"type": "direct", "tag": "direct"},',
  '    {"type": "socks", "tag": "edge", "server": "127.0.0.1", "server_port": 1080, "detour": "|"}',
  '  ],',
  '  "route": {"final": "edge"}',
  '}',
].join('\n');
assert.equal(
  schemaCompletionAtMarker(referenceFixture),
  null,
  'generic JSON Schema completion must not invent program-specific tag references',
);
const referenceCompletion = schemaCompletionAtMarker(
  referenceFixture,
  singBoxJsonSchemaSemantics,
);
assert.deepEqual(
  referenceCompletion.options.map((option) => option.label),
  ['direct'],
  'sing-box semantics should add compatible tags while excluding the enclosing outbound',
);

const schemaDocument = {
  source: 'programBinary',
  dialect: 'draft2020-12',
  content: JSON.stringify(testConfigurationSchema),
  contentHash: '0'.repeat(64),
};
assert.deepEqual(parseJsonSchemaDocument(schemaDocument), testConfigurationSchema);
assert.throws(
  () => JsonSchemaValidator.compile(testConfigurationSchema, []),
  /strict mode: unknown keyword/,
  'generic validation must explicitly register program annotation keywords',
);
const schemaValidator = JsonSchemaValidator.compile(
  testConfigurationSchema,
  singBoxJsonSchemaSemantics.annotationKeywords,
);
const invalidSchemaConfiguration = analyzeJsonDocument(
  '{"outbounds": [], "unsupported": true}',
);
assert.ok(invalidSchemaConfiguration.root);
const schemaDiagnostics = schemaValidator.analyze(
  getNodeValue(invalidSchemaConfiguration.root),
  invalidSchemaConfiguration.root,
);
assert.equal(schemaDiagnostics.length, 1);
assert.equal(schemaDiagnostics[0].code, 'jsonSchema.additionalProperties');
assert.equal(schemaDiagnostics[0].parameters.property, 'unsupported');
assert.equal(
  '{"outbounds": [], "unsupported": true}'.slice(
    schemaDiagnostics[0].from,
    schemaDiagnostics[0].to,
  ),
  '"unsupported"',
  'schema diagnostics should highlight the offending property key',
);

const unevaluatedSchema = {
  $schema: 'https://json-schema.org/draft/2020-12/schema',
  type: 'object',
  allOf: [{
    type: 'object',
    properties: { known: { type: 'boolean' } },
  }],
  unevaluatedProperties: false,
};
const unevaluatedValidator = JsonSchemaValidator.compile(unevaluatedSchema, []);
const unevaluatedDocument = analyzeJsonDocument('{"known": true, "extra": false}');
assert.ok(unevaluatedDocument.root);
assert.equal(
  unevaluatedValidator.analyze(
    getNodeValue(unevaluatedDocument.root),
    unevaluatedDocument.root,
  )[0].code,
  'jsonSchema.unevaluatedProperties',
  'validation must use Draft 2020-12 unevaluatedProperties semantics',
);
assert.doesNotThrow(
  () => JsonSchemaValidator.compile({
    $schema: 'https://json-schema.org/draft/2020-12/schema',
    type: ['string', 'number'],
  }, []),
  'valid Draft 2020-12 union types must not be rejected by validator lint settings',
);

assert.equal(formatBillingAmount('19.99000000'), '19.99');
assert.equal(formatBillingAmount('19.90000000'), '19.90');
assert.equal(formatBillingAmount('19.0'), '19.00');
assert.equal(formatBillingAmount('19.00000000'), '19.00');
assert.equal(formatBillingAmount('0.00000001'), '0.00000001');
assert.equal(formatBillingAmount('9007199254740993.12340000'), '9007199254740993.1234');
assert.equal(formatBillingAmount('not-a-decimal'), 'not-a-decimal');
assert.equal(compactPaymentReference('short-reference'), 'short-reference');
assert.equal(compactPaymentReference('CNX-PAY_F76430CD5208A6B2A78C01C8D4E3C190'), 'CNX-PAY_F76…D4E3C190');

let resolveLateListener;
let lateListenerReleased = 0;
const listenerErrors = [];
const listenerScope = createAsyncListenerScope((error) => listenerErrors.push(error));
listenerScope.track(new Promise((resolve) => { resolveLateListener = resolve; }));
listenerScope.dispose();
listenerScope.dispose();
resolveLateListener(() => { lateListenerReleased += 1; });
await Promise.resolve();
assert.equal(listenerScope.active(), false);
assert.equal(lateListenerReleased, 1);
assert.deepEqual(listenerErrors, []);

const activeScope = createAsyncListenerScope((error) => listenerErrors.push(error));
activeScope.track(Promise.resolve(() => { lateListenerReleased += 1; }));
await Promise.resolve();
activeScope.dispose();
assert.equal(lateListenerReleased, 2);

assert.deepEqual(
  parseArgumentLine('program.exe --arg1 value --arg2 --arg3 value3', 'program.exe', 'Windows').args,
  ['--arg1', 'value', '--arg2', '--arg3', 'value3'],
);
assert.deepEqual(
  parseArgumentLine('arg1 arg2 --arg3 value3 -arg4').args,
  ['arg1', 'arg2', '--arg3', 'value3', '-arg4'],
);
assert.deepEqual(
  parseArgumentLine('--name "value with spaces" --path C:\\Tools\\app').args,
  ['--name', 'value with spaces', '--path', 'C:\\Tools\\app'],
);
assert.match(parseArgumentLine('--name "unfinished').error, /closing/);
assert.equal(parseArgumentLine('--flag value | next').warnings.length, 1);
assert.deepEqual(
  parseArgumentLine('D:/Other/program.exe --flag', 'C:/Tools/program.exe', 'Windows').args,
  ['D:/Other/program.exe', '--flag'],
);
assert.deepEqual(
  parseArgumentLine('program.exe --flag', 'C:/Tools/program.exe', 'Windows').args,
  ['--flag'],
);

const exact = ['', 'two words', 'C:/Program Files/app', 'a\\"b', "don't"];
assert.deepEqual(parseArgumentLine(formatArgumentLine(exact)).args, exact);
assert.equal(resolveColorScheme('system', false), 'light');
assert.equal(resolveColorScheme('system', true), 'dark');
assert.equal(resolveColorScheme('light', true), 'light');

const draftStorage = new Map();
globalThis.localStorage = {
  getItem: (key) => draftStorage.get(key) ?? null,
  setItem: (key, value) => draftStorage.set(key, value),
  removeItem: (key) => draftStorage.delete(key),
};
const sensitiveDraft = {
  ...defaultDraft('generic', 'Linux'),
  argumentLine: '--token command-secret',
  environment: [{ key: 'API_TOKEN', value: 'environment-secret' }],
  initialConfig: '{"password":"configuration-secret"}',
  clashDashboardDownloadUrl: 'https://user:password@example.test/ui.zip?token=url-secret#fragment',
  configSources: [{
    mode: 'remote',
    id: 'remote',
    name: 'Remote',
    enabled: true,
    url: 'https://user:password@example.test/config.json?token=url-secret#fragment',
    authentication: { scheme: 'basic', username: 'operator', password: 'basic-secret' },
  }],
};
saveCreateDraft(sensitiveDraft);
const persistedDraft = draftStorage.get(CREATE_DRAFT_STORAGE_KEY);
for (const secret of ['command-secret', 'environment-secret', 'configuration-secret', 'password', 'url-secret', 'basic-secret']) {
  assert.doesNotMatch(persistedDraft, new RegExp(secret));
}
const restoredDraft = loadCreateDraft('generic', 'Linux');
assert.equal(restoredDraft.argumentLine, '');
assert.deepEqual(restoredDraft.environment, [{ key: 'API_TOKEN', value: '' }]);
assert.equal(restoredDraft.initialConfig, '');
assert.equal(restoredDraft.configSources[0].mode, 'remote');
assert.equal(restoredDraft.configSources[0].url, 'https://example.test/config.json');
assert.equal(restoredDraft.configSources[0].authentication?.password, '');
const mihomoDraft = defaultDraft('mihomo', 'Windows');
assert.equal(mihomoDraft.executable, 'mihomo.exe');
assert.equal(mihomoDraft.mihomoDashboardPort, 9092);
assert.equal(mihomoDraft.mihomoDashboardEnabled, false);
const oversizedSources = Array.from({ length: MAX_CONFIG_SOURCES_PER_PROGRAM + 1 }, (_, index) => ({
  mode: 'local',
  id: `source-${index + 1}`,
  name: `Source ${index + 1}`,
  enabled: true,
  path: `config-${index + 1}.json`,
}));
saveCreateDraft({
  ...defaultDraft('xray', 'Linux'),
  managedConfiguration: true,
  configSources: oversizedSources,
});
assert.equal(
  loadCreateDraft('xray', 'Linux').configSources.length,
  MAX_CONFIG_SOURCES_PER_PROGRAM + 1,
  'draft loading must preserve over-limit sources so the user can resolve them explicitly',
);
assert.equal(effectiveConfigSourceLimit(), 50);
assert.equal(effectiveConfigSourceLimit(20), 20);
assert.equal(effectiveConfigSourceLimit(50), 50);
assert.equal(effectiveConfigSourceLimit(500), 50);
clearCreateDraft('xray', 'Linux');
clearCreateDraft('generic', 'Linux');
assert.deepEqual(THEME_IDS, ['cupertino', 'material', 'aurora']);
assert.deepEqual(UI_SCALES, [0.95, 1.05, 1.15, 1.3]);
assert.deepEqual(normalizeAppearancePreferences({ version: 3, theme: 'material', colorMode: 'dark', scale: 1.15 }), {
  version: 3,
  theme: 'material',
  colorMode: 'dark',
  scale: 1.15,
});
assert.deepEqual(normalizeAppearancePreferences({ version: 2, theme: 'material', colorMode: 'dark', scale: 1.15 }), {
  version: 3,
  theme: 'cupertino',
  colorMode: 'system',
  scale: 1.05,
});
const appearanceValues = new Map();
const appearanceStorage = {
  getItem: (key) => appearanceValues.get(key) ?? null,
  setItem: (key, value) => appearanceValues.set(key, value),
  removeItem: (key) => appearanceValues.delete(key),
};
const auroraAppearance = { version: 3, theme: 'aurora', colorMode: 'light', scale: 1.3 };
assert.equal(saveAppearancePreferences(auroraAppearance, appearanceStorage), true);
assert.equal(appearanceValues.has(APPEARANCE_STORAGE_KEY), true);
assert.deepEqual(loadAppearancePreferences(appearanceStorage), auroraAppearance);
for (const scale of UI_SCALES) {
  const preferences = { version: 3, theme: 'cupertino', colorMode: 'system', scale };
  assert.equal(saveAppearancePreferences(preferences, appearanceStorage), true);
  assert.deepEqual(loadAppearancePreferences(appearanceStorage), preferences);
}
appearanceValues.set(APPEARANCE_STORAGE_KEY, '{broken');
assert.deepEqual(loadAppearancePreferences(appearanceStorage), {
  version: 3,
  theme: 'cupertino',
  colorMode: 'system',
  scale: 1.05,
});
assert.equal(managedWorkingDirectory('bin/program.exe'), 'bin');
assert.equal(managedWorkingDirectory('bin/tools/program.exe'), 'bin/tools');
assert.equal(managedWorkingDirectory('bin\\tools\\program.exe'), 'bin/tools');
assert.equal(
  enrichConfigurationArguments(
    { args: ['-c', 'config.json'], error: '', warnings: [] },
    ['-c', '--config'],
    true,
  ).error,
  'Configuration path arguments are unavailable in managed mode.',
);
assert.equal(
  enrichConfigurationArguments(
    { args: ['--config='], error: '', warnings: [] },
    ['-c', '--config'],
    false,
  ).error,
  '--config= requires a configuration path after “=”.',
);
assert.equal(hasConfigurationArgument(['-c', '--config'], ['run', '--config=a.json']), true);
assert.equal(mihomoProgram.configuration.language, 'yaml');
assert.equal(mihomoProgram.configuration.managedConfigPath, 'config/managed.yaml');
assert.equal(
  mihomoProgram.configuration.enrichArguments(
    { args: ['-f', '/etc/mihomo.yaml'], error: '', warnings: [] },
    { managedConfiguration: false, storedConfiguration: true },
  ).error,
  'Configuration path arguments conflict with the stored Mihomo configuration.',
);
assert.equal(
  mihomoProgram.configuration.enrichArguments(
    { args: ['-f', '/etc/mihomo.yaml'], error: '', warnings: [] },
    { managedConfiguration: false, storedConfiguration: false },
  ).error,
  '',
);
assert.match(
  mihomoProgram.configuration.enrichArguments(
    { args: ['-config=Zm9v'], error: '', warnings: [] },
    { managedConfiguration: false, storedConfiguration: false },
  ).error,
  /inline configuration/,
);
assert.match(
  mihomoProgram.configuration.enrichArguments(
    { args: ['-age-secret-key', 'AGE-SECRET-KEY-TEST'], error: '', warnings: [] },
    { managedConfiguration: false, storedConfiguration: false },
  ).error,
  /must not be stored/,
);
assert.equal(
  mihomoProgram.configuration.enrichArguments(
    { args: ['-t'], error: '', warnings: [] },
    { managedConfiguration: false, storedConfiguration: false },
  ).warnings.length,
  1,
);
for (const status of ['starting', 'running', 'stopping', 'backoff', 'stopFailed']) {
  assert.equal(isRuntimeActive({ status }), true, `${status} should be runtime-active`);
}
for (const state of [
  { status: 'stopped' },
  { status: 'exited', code: 0, success: true },
  { status: 'error', code: 'FAILED', message: 'failed' },
]) {
  assert.equal(isRuntimeActive(state), false, `${state.status} should not be runtime-active`);
}
assert.equal(canUseProgramLifecycleAction({ canUseLocalPrograms: false }, 'start'), false);
assert.equal(canUseProgramLifecycleAction({ canUseLocalPrograms: false }, 'restart'), false);
assert.equal(canUseProgramLifecycleAction({ canUseLocalPrograms: false }, 'stop'), true);
assert.equal(canUseProgramLifecycleAction({ canUseLocalPrograms: true }, 'start'), true);

let dashboards = applySingBoxDashboardChange({}, {
  kind: 'native',
  value: { listenPort: 9090, updateInterval: '1d' },
});
dashboards = applySingBoxDashboardChange(dashboards, {
  kind: 'clash',
  value: { listenPort: 9091 },
});
assert.deepEqual(dashboards, {
  native: { listenPort: 9090, updateInterval: '1d' },
  clash: { listenPort: 9091 },
});
dashboards = applySingBoxDashboardChange(dashboards, { kind: 'native' });
assert.equal(dashboards.native, undefined);
assert.deepEqual(dashboards.clash, { listenPort: 9091 });

const storageError = errorInfoOf({
  code: 'STORAGE',
  message: 'Storage operation failed',
  details: 'permission denied',
});
assert.equal(storageError.title, 'Storage error');
assert.equal(storageError.fallbackMessage, 'Application data could not be accessed.');
assert.equal(storageError.details, 'permission denied');
assert.notEqual(storageError.title, 'storage');
assert.doesNotMatch(storageError.title, /_/);
const configurationSchemaError = errorInfoOf({
  code: 'CONFIGURATION_SCHEMA_INVALID',
  message: 'Program could not generate a configuration schema',
});
assert.equal(configurationSchemaError.title, 'Program schema unavailable');
assert.match(configurationSchemaError.suggestion, /schema command/);
for (const [code, title, message] of [
  ['TIMEOUT', 'Operation timed out', 'The operation did not finish in time.'],
  ['NETWORK', 'Network error', 'The network request could not be completed.'],
]) {
  const serviceError = errorInfoOf({
    code,
    message: 'License service operation failed',
  });
  assert.equal(serviceError.title, title);
  assert.equal(serviceError.message, message);
}
const licenseError = errorInfoOf({
  code: 'LICENSE_DEVICE_DENIED',
  message: 'License service operation failed',
  details: 'the device is not authorized',
});
assert.equal(licenseError.title, 'Device authorization revoked');
assert.equal(licenseError.message, 'This device is not authorized for the current license.');
assert.equal(licenseError.code, 'LICENSE_DEVICE_DENIED');
assert.equal(licenseError.details, '');
const registeredIdentityError = errorInfoOf({
  code: 'LICENSE_IDENTITY_ALREADY_REGISTERED',
  message: 'License service operation failed',
});
assert.equal(registeredIdentityError.title, 'License identity already registered');
assert.match(registeredIdentityError.message, /existing license identity/);
assert.match(registeredIdentityError.suggestion, /Use another license/);
const clientUpgradeError = errorInfoOf({
  code: 'LICENSE_CLIENT_UPGRADE_REQUIRED',
  message: 'License service operation failed',
});
assert.equal(clientUpgradeError.title, 'Camellia Nexus update required');
assert.match(clientUpgradeError.suggestion, /supported Camellia Nexus version/);
const workspaceConflictError = errorInfoOf({
  code: 'LICENSE_WORKSPACE_CONFLICT',
  message: 'License service operation failed',
});
assert.equal(workspaceConflictError.title, 'Workspace changed');
assert.equal(workspaceConflictError.message, 'The team workspace was updated by another session.');
assert.match(workspaceConflictError.suggestion, /Reload the team workspace/);
const operationConflictError = errorInfoOf({
  code: 'LICENSE_OPERATION_CONFLICT',
  message: 'License service operation failed',
});
assert.equal(operationConflictError.title, 'Request changed');
assert.equal(operationConflictError.message, 'This operation ID was already used for a different request.');
assert.match(operationConflictError.suggestion, /current feature data/);
for (const [code, title, suggestion] of [
  ['LICENSE_WORKSPACE_QUOTA_EXCEEDED', 'Workspace storage full', /purge eligible deleted data/],
  ['LICENSE_WORKSPACE_DOCUMENT_LIMIT_REACHED', 'Shared configuration limit reached', /Delete an unused active shared configuration/],
  ['LICENSE_WORKSPACE_ALERT_RULE_LIMIT_REACHED', 'Alert rule limit reached', /Delete an unused alert rule/],
  ['LICENSE_WORKSPACE_RETENTION_ACTIVE', 'Recovery period still active', /30 days/],
  ['LICENSE_WORKSPACE_NOT_FOUND', 'Workspace item not found', /Reload/],
  ['LICENSE_WORKSPACE_INTEGRITY_FAILED', 'Workspace integrity check failed', /Stop editing/],
  ['LICENSE_WORKSPACE_KEY_UNAVAILABLE', 'Workspace key unavailable', /keyring/],
  ['LICENSE_WEBHOOK_INVALID_URL', 'Webhook URL rejected', /public HTTPS/],
  ['LICENSE_WEBHOOK_ENDPOINT_LIMIT_REACHED', 'Webhook endpoint limit reached', /Delete an unused endpoint/],
  ['LICENSE_WEBHOOK_NOT_FOUND', 'Webhook endpoint not found', /Reload/],
  ['LICENSE_WEBHOOK_KEY_UNAVAILABLE', 'Webhook key unavailable', /keyring/],
  ['REQUEST_TOO_LARGE', 'Request too large', /Reduce/],
]) {
  const workspaceError = errorInfoOf({ code, message: 'License service operation failed' });
  assert.equal(workspaceError.title, title);
  assert.match(workspaceError.suggestion, suggestion);
}
for (const [code, title, message] of [
  ['LICENSE_ACTIVATION_CODE_INVALID', 'Invalid activation code', 'The activation code was not recognized.'],
  ['LICENSE_ACTIVATION_CODE_EXPIRED', 'Activation code expired', 'This activation code has expired.'],
  ['LICENSE_ACTIVATION_CODE_CONSUMED', 'Activation code already used', 'This activation code has already been used.'],
  ['LICENSE_ACTIVATION_CODE_REVOKED', 'Activation code revoked', 'This activation code is no longer valid.'],
]) {
  const activationCodeError = errorInfoOf({
    code,
    message: 'License service operation failed',
  });
  assert.equal(activationCodeError.title, title);
  assert.equal(activationCodeError.message, message);
  assert.match(activationCodeError.suggestion, /activation code/);
}

const clientVersionPolicy = {
  minimumVersion: '2.0.0',
  recommendedVersion: '2.1.0',
  enforceAfter: 2_000,
};
const currentLicenseLimits = {
  max_programs: 50,
  max_config_sources_per_program: 20,
  max_team_members: 1,
  max_remote_monitors: 3,
  max_shared_programs: 0,
  max_webhook_endpoints: 0,
  max_workspace_storage_bytes: 0,
  max_alert_rules: 0,
  max_audit_export_events: 0,
};
const currentEntitlementClaims = {
  schemaVersion: 3,
  iss: 'issuer',
  aud: 'audience',
  sub: 'account',
  licenseId: 'license',
  deviceId: 'device',
  deviceKeyThumbprint: 'thumbprint',
  plan: 'pro',
  planRevision: 2,
  policyHash: '0'.repeat(64),
  licenseStatus: 'active',
  capabilities: [],
  workspacePermissions: [],
  limits: currentLicenseLimits,
  licenseEpoch: 1,
  deviceLimit: 3,
  memberLimit: 1,
  offlineAccessEndsAt: 4,
  iat: 1,
  refreshAfter: 2,
  exp: 3,
  tokenId: 'token',
  keyId: 'k',
  clientVersionPolicy,
};
assert.equal(compareCanonicalSemVer('2.0.0', '2.0.0'), 0);
assert.equal(compareCanonicalSemVer('2.0.0-alpha.10', '2.0.0-alpha.2'), 1);
assert.equal(compareCanonicalSemVer('2.0.0-1', '2.0.0-alpha'), -1);
assert.equal(compareCanonicalSemVer('2.0.0+build.7', '2.0.0+build.8'), 0);
assert.equal(compareCanonicalSemVer('100000000000000000000.0.0', '99999999999999999999.0.0'), 1);
assert.equal(compareCanonicalSemVer('02.0.0', '2.0.0'), null);
assert.equal(compareCanonicalSemVer('2.0.0\n', '2.0.0'), null);
assert.equal(compareCanonicalSemVer('2.0.0-01', '2.0.0-1'), null);
assert.equal(compareCanonicalSemVer('2.0.0-alpha..1', '2.0.0-alpha.1'), null);
assert.equal(compareCanonicalSemVer('2.0.0+build..1', '2.0.0'), null);
assert.equal(
  compareCanonicalSemVer(`0.0.0-0.${'--.'.repeat(10_000)}`, '0.0.0'),
  null,
);

const currentActiveEntitlementState = {
  status: 'active',
  entitlement: { keyId: 'k', claims: currentEntitlementClaims },
};
assert.equal(licenseRuntimeImpact(currentActiveEntitlementState), 'active');
assert.equal(deriveLicenseAccess(currentActiveEntitlementState, 1).configurationValid, true);
for (const requiredLimit of [
  'max_workspace_storage_bytes',
  'max_alert_rules',
  'max_audit_export_events',
]) {
  const incompleteLimits = { ...currentLicenseLimits };
  delete incompleteLimits[requiredLimit];
  const incompleteState = {
    status: 'active',
    entitlement: {
      keyId: 'k',
      claims: { ...currentEntitlementClaims, limits: incompleteLimits },
    },
  };
  assert.equal(deriveLicenseAccess(incompleteState, 1).configurationValid, false);
  assert.equal(deriveLicenseAccess(incompleteState, 1).canUseLocalPrograms, false);
}
assert.equal(hasRefreshableLicenseSession({ status: 'sessionOnly' }), false);
assert.equal(hasRefreshableLicenseSession({ status: 'activationPending' }), true);
assert.equal(licenseRuntimeImpact({ status: 'activationPending' }), 'hardInactive');

let entitlementGeneration = 0;
const newerResponse = {
  generation: 2,
  entitlementState: { status: 'unauthenticated' },
};
assert.equal(isNewerEntitlementSnapshot(newerResponse, entitlementGeneration), true);
entitlementGeneration = newerResponse.generation;
assert.equal(isNewerEntitlementSnapshot({
  generation: 1,
  entitlementState: { status: 'unauthenticated' },
}, entitlementGeneration), false);
assert.equal(isNewerEntitlementSnapshot({
  generation: 2,
  entitlementState: { status: 'sessionOnly' },
}, entitlementGeneration), false);
const newerEvent = {
  generation: 3,
  entitlementState: { status: 'sessionOnly' },
};
assert.equal(isNewerEntitlementSnapshot(newerEvent, entitlementGeneration), true);
entitlementGeneration = newerEvent.generation;
assert.equal(isNewerEntitlementSnapshot(newerResponse, entitlementGeneration), false);
assert.equal(isNewerEntitlementSnapshot({
  generation: Number.MAX_SAFE_INTEGER + 1,
  entitlementState: { status: 'unauthenticated' },
}, entitlementGeneration), false);

assert.match(
  licenseRuntimeNotice({
    generation: 1,
    entitlementState: { status: 'activationPending' },
    reason: 'activation-resume',
    runtimeImpact: 'hardInactive',
    stoppedPrograms: 0,
    failedPrograms: 0,
    failedProgramIds: [],
  }).suggestion,
  /does not need to be entered again/,
);
assert.equal(hasRefreshableLicenseSession({ status: 'deviceDenied', state: 'revoked' }), false);
assert.equal(hasRefreshableLicenseSession({ status: 'licenseInactive', reason: 'license_past_due' }), true);
assert.equal(hasRefreshableLicenseSession({
  status: 'clientUpgradeRequired',
  policy: clientVersionPolicy,
  entitlement: null,
}), true);
assert.equal(licenseStateNotice({
  status: 'clientUpgradeRequired',
  policy: clientVersionPolicy,
  entitlement: null,
}).title, 'Camellia Nexus update required');
assert.equal(clientVersionAdvisory(currentActiveEntitlementState, '1.9.9').kind, 'requiredBefore');
assert.equal(clientVersionAdvisory(currentActiveEntitlementState, '1.9.9').kind, 'requiredBefore');
assert.equal(clientVersionAdvisory(currentActiveEntitlementState, '2.0.0').kind, 'recommended');
assert.deepEqual(signedLicenseStatusPresentation('active'), {
  label: 'In good standing',
  tone: 'success',
});
assert.deepEqual(signedLicenseStatusPresentation('past_due'), {
  label: 'Payment past due',
  tone: 'warning',
});
assert.deepEqual(signedLicenseStatusPresentation('canceled'), {
  label: 'Canceled',
  tone: 'danger',
});
const activeGraceNotice = licenseRuntimeNotice({
  generation: 1,
  entitlementState: {
    status: 'active',
    entitlement: {
      keyId: 'k',
      claims: { ...currentEntitlementClaims, licenseStatus: 'past_due', licenseExpiresAt: 100 },
    },
  },
  reason: 'online-refresh',
  runtimeImpact: 'active',
  stoppedPrograms: 0,
  failedPrograms: 0,
  failedProgramIds: [],
});
assert.equal(activeGraceNotice.title, 'License payment past due');
assert.match(activeGraceNotice.message, /commercial grace term/);
const offlineNotice = licenseRuntimeNotice({
  generation: 1,
  entitlementState: {
    status: 'restrictedOffline',
    entitlement: { keyId: 'k', claims: currentEntitlementClaims },
    safetyWindowEndsAt: 4,
  },
  reason: 'test',
  runtimeImpact: 'restrictedOffline',
  stoppedPrograms: 0,
  failedPrograms: 0,
  failedProgramIds: [],
});
assert.equal(offlineNotice.title, 'License offline grace period');
assert.match(offlineNotice.message, /24 hours/);
assert.equal(licenseNoticeRequiresPersistentAttention({
  generation: 1,
  entitlementState: { status: 'revalidationRequired', reason: 'obsolete_epoch' },
  reason: 'test',
  runtimeImpact: 'hardInactive',
  stoppedPrograms: 1,
  failedPrograms: 0,
  failedProgramIds: [],
}), false);
const revokedEvent = {
  generation: 1,
  entitlementState: { status: 'deviceDenied', state: 'suspicious' },
  reason: 'test',
  runtimeImpact: 'hardInactive',
  stoppedPrograms: 2,
  failedPrograms: 0,
  failedProgramIds: [],
};
const revokedNotice = licenseRuntimeNotice(revokedEvent);
assert.equal(revokedNotice.title, 'Device authorization revoked');
assert.match(revokedNotice.additionalMessages.join(' '), /Managed programs were stopped/);
assert.match(revokedNotice.suggestion, /review the device security/);
assert.match(licenseRuntimeNotice({
  ...revokedEvent,
  entitlementState: { status: 'deviceDenied', state: 'removed' },
  stoppedPrograms: 0,
}).suggestion, /activate this device again/);
assert.notEqual(
  licenseNoticeKey(revokedEvent),
  licenseNoticeKey({ ...revokedEvent, stoppedPrograms: 0, failedPrograms: 1 }),
);
const upgradeEvent = {
  ...revokedEvent,
  entitlementState: {
    status: 'clientUpgradeRequired',
    policy: clientVersionPolicy,
    entitlement: null,
  },
  stoppedPrograms: 0,
};
assert.notEqual(
  licenseNoticeKey(upgradeEvent),
  licenseNoticeKey({
    ...upgradeEvent,
    entitlementState: {
      ...upgradeEvent.entitlementState,
      policy: { ...clientVersionPolicy, minimumVersion: '2.0.1' },
    },
  }),
);
const stopFailureNotice = licenseRuntimeNotice({
  generation: 1,
  entitlementState: {
    status: 'expired',
    entitlement: { keyId: 'k', claims: currentEntitlementClaims },
  },
  reason: 'test',
  runtimeImpact: 'hardInactive',
  stoppedPrograms: 1,
  failedPrograms: 1,
  failedProgramIds: ['xray-main'],
});
assert.match(stopFailureNotice.additionalMessages.join(' '), /One managed program was stopped/);
assert.match(stopFailureNotice.additionalMessages.join(' '), /xray-main/);
assert.match(stopFailureNotice.suggestion, /manually/);
assert.equal(licenseNoticeRequiresPersistentAttention({
  generation: 1,
  entitlementState: {
    status: 'expired',
    entitlement: { keyId: 'k', claims: currentEntitlementClaims },
  },
  reason: 'test',
  runtimeImpact: 'hardInactive',
  stoppedPrograms: 1,
  failedPrograms: 1,
  failedProgramIds: ['xray-main'],
}), true);

const catalog = { version: 1, order: ['alpha', 'beta', 'gamma'] };
assert.deepEqual(moveCatalogItem(catalog, 'gamma', 'beta').order, ['alpha', 'gamma', 'beta']);
assert.deepEqual(moveCatalogItem(catalog, 'alpha').order, ['beta', 'gamma', 'alpha']);

console.log('Frontend utility tests passed.');

import { randomUUID } from 'node:crypto';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const directory = path.dirname(fileURLToPath(import.meta.url));
const application = requiredEnvironment('CAMELLIA_NEXUS_E2E_APP_BINARY');
const embeddedPort = Number(requiredEnvironment('CAMELLIA_NEXUS_E2E_WEBDRIVER_PORT'));
const outputDirectory = path.resolve(
  process.env.CAMELLIA_NEXUS_E2E_OUTPUT_DIR ?? path.join(directory, 'test-results', 'native')
);
const phase = requiredEnvironment('CAMELLIA_NEXUS_E2E_PHASE');
const resetIdentity = process.env.CAMELLIA_NEXUS_E2E_RESET_IDENTITY === 'true';
const phaseSpecs = {
  'free-activation-limits': './tests/native/00-free-activation-limits.e2e.mjs',
  'free-device-limit': './tests/native/07-free-device-limit.e2e.mjs',
  'free-primary-release': './tests/native/08-free-primary-release.e2e.mjs',
  'free-device-recovery': './tests/native/09-free-device-recovery.e2e.mjs',
  'smoke-activation': './tests/native/01-smoke-activation.e2e.mjs',
  'smoke-persistence': './tests/native/02-smoke-persistence.e2e.mjs',
  'full-offline': './tests/native/03-full-offline.e2e.mjs',
  'full-recovery-billing': './tests/native/04-full-recovery-billing.e2e.mjs',
  'full-terminal-denial': './tests/native/05-full-terminal-denial.e2e.mjs',
  'full-restoration': './tests/native/06-full-restoration.e2e.mjs',
  'team-owner-activation': './tests/native/10-team-owner-activation.e2e.mjs',
  'team-member-join': './tests/native/11-team-member-join.e2e.mjs',
  'team-owner-workspace': './tests/native/12-team-owner-workspace.e2e.mjs',
  'team-additional-device': './tests/native/13-team-additional-device.e2e.mjs',
  'team-former-owner-leave': './tests/native/14-team-former-owner-leave.e2e.mjs',
  'team-new-owner': './tests/native/15-team-new-owner.e2e.mjs',
  cleanup: './tests/native/99-cleanup.e2e.mjs'
};

if (!phaseSpecs[phase]) {
  throw new Error(`Unsupported CAMELLIA_NEXUS_E2E_PHASE: ${phase}`);
}

if (!Number.isSafeInteger(embeddedPort) || embeddedPort < 1 || embeddedPort > 65_535) {
  throw new Error('CAMELLIA_NEXUS_E2E_WEBDRIVER_PORT must be a valid TCP port');
}

// @wdio/tauri-service uses this standard variable for the embedded driver's
// direct-evaluation endpoint. Keep it aligned with the dynamically allocated
// port used by the launcher and every worker process.
process.env.TAURI_WEBDRIVER_PORT = String(embeddedPort);

function requiredEnvironment(name) {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required for native desktop tests`);
  return value;
}

export const config = {
  runner: 'local',
  specs: [phaseSpecs[phase]],
  maxInstances: 1,
  maxInstancesPerCapability: 1,
  capabilities: [
    {
      browserName: 'tauri',
      'tauri:options': { application }
    }
  ],
  services: [
    [
      '@wdio/tauri-service',
      {
        appBinaryPath: application,
        driverProvider: 'embedded',
        embeddedPort,
        startTimeout: 90_000,
        statusPollTimeout: 5_000,
        commandTimeout: 45_000,
        captureBackendLogs: true,
        captureFrontendLogs: true,
        backendLogLevel: 'info',
        frontendLogLevel: 'warn',
        logDir: outputDirectory
      }
    ]
  ],
  framework: 'mocha',
  reporters: ['spec'],
  outputDir: outputDirectory,
  logLevel: process.env.CI ? 'warn' : 'info',
  bail: 0,
  waitforTimeout: 20_000,
  connectionRetryTimeout: 120_000,
  connectionRetryCount: 2,
  mochaOpts: {
    ui: 'bdd',
    timeout: 300_000
  },
  afterTest: async function (_test, _context, result) {
    if (!result.passed) {
      await browser
        .execute(() => {
          const selectors = [
            '.team-secret code',
            '.webhook-secret code',
            '[data-e2e-sensitive]'
          ];
          for (const element of document.querySelectorAll(selectors.join(','))) {
            element.textContent = '[REDACTED]';
          }
          for (const input of document.querySelectorAll('input')) {
            if (
              input instanceof HTMLInputElement &&
              /(?:invitation|enrollment|webhook).*token|secret/iu.test(
                `${input.name} ${input.placeholder} ${input.getAttribute('aria-label') ?? ''}`
              )
            ) {
              input.value = '[REDACTED]';
            }
          }
        })
        .catch(() => undefined);
      const safeName = `${Date.now()}-failure.png`;
      await browser.saveScreenshot(path.join(outputDirectory, safeName)).catch(() => undefined);
    }
  },
  after: async function () {
    if (resetIdentity) {
      const operationId = randomUUID();
      await browser.tauri
        .execute(
          ({ core }, resetOperationId) =>
            core.invoke('reset_license_device_identity', { operationId: resetOperationId }),
          operationId
        )
        .catch(() => undefined);
    }
  }
};

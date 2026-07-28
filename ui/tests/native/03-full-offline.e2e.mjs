import { expect } from '@wdio/globals';
import {
  buttonEnabled,
  clickButton,
  closeSettings,
  domText,
  invoke,
  openLicenseSettings,
  requiredEnvironment,
  selectProgram,
  waitForProgramState
} from './support.mjs';

describe('Camellia Nexus native offline continuity', () => {
  it('uses the signed cache while keeping network failures actionable and redacted', async () => {
    await $('h1=Workspace').waitForDisplayed({ timeout: 30_000 });
    const before = await invoke('get_entitlement_state');
    expect(['active', 'restrictedOffline']).toContain(before.entitlementState.status);

    const id = 'native-e2e-enforcement';
    await selectProgram(id);
    await clickButton('Start', '.program-hero');
    await waitForProgramState(id, 'running');
    await clickButton('Stop', '.program-hero');
    await waitForProgramState(id, 'stopped');

    await openLicenseSettings();
    await clickButton('Refresh entitlement', '.license-panel');
    await $('.license-panel .error-notice').waitForDisplayed({ timeout: 75_000 });
    const errorText = await domText('.license-panel .error-notice');
    const actionableNetworkFailures = [
      ['Operation timed out', 'The operation did not finish in time', 'Retry the operation.'],
      [
        'Network error',
        'The network request could not be completed',
        'Check the network connection, proxy settings and source URL'
      ]
    ];
    expect(
      actionableNetworkFailures.some((messages) =>
        messages.every((message) => errorText.includes(message))
      )
    ).toBe(true);
    expect(await buttonEnabled('Refresh entitlement', '.license-panel')).toBe(true);
    const sensitiveValues = [
      requiredEnvironment('CAMELLIA_NEXUS_E2E_SERVER_BASE_URL'),
      requiredEnvironment('CAMELLIA_NEXUS_E2E_PRO_ACCOUNT_ID'),
      requiredEnvironment('CAMELLIA_NEXUS_E2E_PRO_PRIMARY_CODE')
    ];
    if (sensitiveValues.some((value) => errorText.includes(value)) || /postgres|backtrace/iu.test(errorText)) {
      throw new Error('the offline error exposed service internals or fixture secrets');
    }
    const cached = await invoke('get_entitlement_state');
    expect(['active', 'restrictedOffline']).toContain(cached.entitlementState.status);
    await closeSettings();
  });
});

import { expect } from '@wdio/globals';
import {
  clickButton,
  invoke,
  openLicenseSettings,
  requiredEnvironment,
  setLabeledValue,
  submitUiAuthorization,
  waitForDomVisibility,
  waitForText
} from './support.mjs';

describe('Camellia Nexus native Free device limit', () => {
  it('keeps a second installation unlicensed when the single-device quota is occupied', async () => {
    await $('h1=Workspace').waitForDisplayed({ timeout: 30_000 });
    await openLicenseSettings();
    await clickButton('Activate device', '.license-panel');
    await waitForDomVisibility('.license-flow');
    await setLabeledValue('.license-flow', 'Device name', 'Windows hosted Free secondary');
    await submitUiAuthorization(requiredEnvironment('CAMELLIA_NEXUS_E2E_FREE_SECOND_DEVICE_CODE'));
    await waitForText('.license-panel', 'License limit reached');
    const snapshot = await invoke('get_entitlement_state');
    expect(snapshot.entitlementState.status).not.toBe('active');
    expect(snapshot.entitlementState.entitlement).toBeUndefined();
  });
});

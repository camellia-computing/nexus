import { expect } from '@wdio/globals';
import {
  clickButton,
  completeUiAuthorization,
  invoke,
  isDomVisible,
  openLicenseSettings,
  requiredEnvironment,
  setLabeledValue,
  waitForDomVisibility
} from './support.mjs';

describe('Camellia Nexus native Free device recovery', () => {
  it('reuses the unconsumed activation code after capacity becomes available', async () => {
    await $('h1=Workspace').waitForDisplayed({ timeout: 30_000 });
    const dialog = await openLicenseSettings();
    if (!(await isDomVisible('.license-flow'))) {
      await clickButton('Activate device', '.license-panel');
      await waitForDomVisibility('.license-flow');
    }
    await setLabeledValue('.license-flow', 'Device name', 'Windows hosted Free recovered');
    const snapshot = await completeUiAuthorization(
      requiredEnvironment('CAMELLIA_NEXUS_E2E_FREE_SECOND_DEVICE_CODE')
    );
    expect(snapshot.entitlementState.entitlement.claims.plan).toBe('free');
    await expect(dialog.$('.license-status')).toHaveText('Licensed');

    const devices = await invoke('get_license_devices', { cursor: null, pageSize: 50 });
    expect(devices.devices).toHaveLength(1);
    expect(devices.devices[0].deviceId).toBe(
      snapshot.entitlementState.entitlement.claims.deviceId
    );
    expect(devices.devices[0].state).toBe('active');
  });
});

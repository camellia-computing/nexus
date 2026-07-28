import { expect } from '@wdio/globals';
import {
  clickButton,
  clickButtonInTextContainer,
  confirmAction,
  invoke,
  openLicenseSettings,
  waitForButtonState,
  waitForText
} from './support.mjs';

describe('Camellia Nexus native Free device release', () => {
  it('removes the occupied primary device so another installation can recover', async () => {
    await $('h1=Workspace').waitForDisplayed({ timeout: 30_000 });
    const before = await invoke('get_license_devices', { cursor: null, pageSize: 50 });
    expect(before.devices).toHaveLength(1);
    expect(before.devices[0].state).toBe('active');

    const dialog = await openLicenseSettings();
    await waitForButtonState('Refresh devices', true, '.license-devices');
    await clickButton('Refresh devices', '.license-devices');
    await waitForText('.license-device-list', 'This device');
    await clickButtonInTextContainer('.license-device-list article', 'This device', 'Remove');
    await confirmAction('Remove');
    await waitForText('.license-panel', 'Not activated');
    expect((await invoke('get_entitlement_state')).entitlementState.status).toBe(
      'unauthenticated'
    );
    await expect(dialog.$('.license-status')).toHaveText('Not activated');
  });
});

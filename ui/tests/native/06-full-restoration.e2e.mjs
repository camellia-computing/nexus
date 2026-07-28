import { browser, expect } from '@wdio/globals';
import { readHandoff } from './handoff.mjs';
import {
  clickButton,
  clickButtonInTextContainer,
  closeSettings,
  completeUiAuthorization,
  confirmAction,
  invoke,
  openLicenseSettings,
  requiredEnvironment,
  selectProgram,
  setLabeledValue,
  waitForButtonState,
  waitForDomVisibility,
  waitForProgramState,
  waitForText
} from './support.mjs';

describe('Camellia Nexus native license restoration', () => {
  it('restores, removes, and reactivates the same device before a clean sign-out', async () => {
    await $('h1=Workspace').waitForDisplayed({ timeout: 30_000 });
    const expected = JSON.parse(await readHandoff('pro-device.json'));
    await openLicenseSettings();
    await clickButton('Refresh entitlement', '.license-panel');
    await browser.waitUntil(
      async () => (await invoke('get_entitlement_state')).entitlementState.status === 'active',
      { timeoutMsg: 'the restored account did not reactivate the existing device session' }
    );
    const restored = await invoke('get_entitlement_state');
    expect(restored.entitlementState.entitlement.claims.deviceId).toBe(expected.deviceId);

    await closeSettings();
    const id = 'native-e2e-enforcement';
    await selectProgram(id);
    await clickButton('Start', '.program-hero');
    await waitForProgramState(id, 'running');

    const activeDialog = await openLicenseSettings();
    await waitForButtonState('Refresh devices', true, '.license-devices');
    await clickButton('Refresh devices', '.license-devices');
    await waitForText('.license-device-list', 'This device');
    await clickButtonInTextContainer('.license-device-list article', 'This device', 'Remove');
    await confirmAction('Remove');
    await waitForProgramState(id, 'stopped');
    const removed = await invoke('get_entitlement_state');
    expect(removed.entitlementState.status).toBe('unauthenticated');
    await expect(activeDialog.$('.license-status')).toHaveText('Not activated');

    await clickButton('Activate device', '.license-panel');
    await waitForDomVisibility('.license-flow');
    await setLabeledValue('.license-flow', 'Device name', 'Windows hosted native E2E reactivated');
    const reactivated = await completeUiAuthorization(
      requiredEnvironment('CAMELLIA_NEXUS_E2E_PRO_RECOVERY_CODE')
    );
    expect(reactivated.entitlementState.entitlement.claims.deviceId).toBe(expected.deviceId);
    await expect(activeDialog.$('.license-status')).toHaveText('Licensed');

    await clickButton('Sign out', '.license-panel');
    await confirmAction('Sign out');
    await waitForText('.license-panel', 'Not activated');
    expect((await invoke('get_entitlement_state')).entitlementState.status).not.toBe('active');
    await expect(activeDialog.$('.license-status')).toHaveText('Not activated');
  });
});

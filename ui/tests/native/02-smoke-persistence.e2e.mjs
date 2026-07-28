import { expect } from '@wdio/globals';
import { readHandoff } from './handoff.mjs';
import {
  clickButton,
  confirmAction,
  invoke,
  openLicenseSettings,
  requiredEnvironment,
  selectProgram,
  waitForProgramState
} from './support.mjs';

describe('Camellia Nexus native secure-state persistence', () => {
  it('restores Credential Manager state after relaunch and enforces process shutdown', async () => {
    await $('h1=Workspace').waitForDisplayed({ timeout: 30_000 });
    const expected = JSON.parse(await readHandoff('pro-device.json'));
    const restored = await invoke('get_entitlement_state');
    expect(restored.entitlementState.status).toBe('active');
    expect(restored.entitlementState.entitlement.claims.deviceId).toBe(expected.deviceId);

    const id = 'native-e2e-enforcement';
    await selectProgram(id);
    await clickButton('Start', '.program-hero');
    await waitForProgramState(id, 'running');

    if (requiredEnvironment('CAMELLIA_NEXUS_E2E_SUITE') === 'smoke') {
      const dialog = await openLicenseSettings();
      await clickButton('Sign out', '.license-panel');
      await confirmAction('Sign out');
      await waitForProgramState(id, 'stopped');
      await expect(dialog.$('.license-status')).toHaveText('Not activated');
      expect((await invoke('get_entitlement_state')).entitlementState.status).not.toBe('active');
    } else {
      await clickButton('Stop', '.program-hero');
      await waitForProgramState(id, 'stopped');
    }
  });
});

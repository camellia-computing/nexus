import { expect } from '@wdio/globals';
import { waitForHandoff, writeHandoff } from './handoff.mjs';
import {
  clickButton,
  invoke,
  openLicenseSettings,
  selectProgram,
  waitForProgramState,
  waitForText
} from './support.mjs';

describe('Camellia Nexus native terminal license enforcement', () => {
  it('stops the managed process tree when the server suspends the account', async () => {
    await $('h1=Workspace').waitForDisplayed({ timeout: 30_000 });
    const id = 'native-e2e-enforcement';
    expect((await invoke('get_entitlement_state')).entitlementState.status).toBe('active');
    await selectProgram(id);
    await clickButton('Start', '.program-hero');
    await waitForProgramState(id, 'running');

    await writeHandoff('terminal-denial-ready.json', JSON.stringify({ programId: id }));
    await waitForHandoff('terminal-denial-applied.json', true);
    await openLicenseSettings();
    await waitForProgramState(id, 'stopped');
    await waitForText('.license-panel', 'Account suspended');
    const denied = await invoke('get_entitlement_state');
    expect(denied.entitlementState.status).toBe('licenseInactive');
    expect(denied.entitlementState.reason).toBe('account_suspended');
  });
});

import { expect } from '@wdio/globals';
import {
  clickButton,
  confirmAction,
  invoke,
  openLicenseSettings,
  waitForText
} from './support.mjs';

describe('Camellia Nexus native former owner departure', () => {
  it('lets the previous owner leave without disturbing the transferred ownership', async () => {
    await $('h1=Workspace').waitForDisplayed({ timeout: 30_000 });
    const before = await invoke('get_license_team_profile');
    expect(before.member.role).toBe('admin');
    await openLicenseSettings();
    await waitForText('.team-summary', 'Administrator');
    await clickButton('Leave workspace', '.team-leave-workspace');
    await confirmAction('Leave workspace');
    await waitForText('.license-panel', 'Not activated');
    await waitForText('.license-panel', 'Registered locally');
    expect((await invoke('get_entitlement_state')).entitlementState.status).toBe('unauthenticated');
  });
});

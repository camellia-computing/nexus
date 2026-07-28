import { expect } from '@wdio/globals';
import { readHandoff } from './handoff.mjs';
import {
  clickButton,
  confirmAction,
  invoke,
  openLicenseSettings,
  waitForText
} from './support.mjs';

describe('Camellia Nexus native transferred Team ownership', () => {
  it('keeps the new owner authoritative after the former owner leaves and signs out cleanly', async () => {
    await $('h1=Workspace').waitForDisplayed({ timeout: 30_000 });
    const formerOwner = JSON.parse(await readHandoff('team-owner.json'));
    const profile = await invoke('get_license_team_profile');
    expect(profile.member.role).toBe('owner');
    const members = await invoke('get_license_team_members', {
      request: { cursor: null, limit: 100 }
    });
    expect(members.members.find((member) => member.id === formerOwner.memberId)?.status).toBe(
      'removed'
    );

    await openLicenseSettings();
    await waitForText('.team-summary', 'Owner');
    await waitForText('.team-summary', '1/3');
    await clickButton('Sign out', '.license-panel');
    await confirmAction('Sign out');
    await waitForText('.license-panel', 'Not activated');
    expect((await invoke('get_entitlement_state')).entitlementState.status).not.toBe('active');
  });
});

import { expect } from '@wdio/globals';
import { writeHandoff } from './handoff.mjs';
import {
  activateThroughLicenseSettings,
  clickButton,
  closeSettings,
  domText,
  invoke,
  requiredEnvironment,
  setLabeledValue,
  waitForText
} from './support.mjs';

describe('Camellia Nexus native Team owner activation', () => {
  it('activates the owner and creates a one-time administrator invitation in the UI', async () => {
    await $('h1=Workspace').waitForDisplayed({ timeout: 30_000 });
    const { snapshot } = await activateThroughLicenseSettings(
      requiredEnvironment('CAMELLIA_NEXUS_E2E_TEAM_OWNER_CODE'),
      'Windows hosted Team owner'
    );
    expect(snapshot.entitlementState.entitlement.claims.plan).toBe('team');
    await waitForText('.team-workspace-panel', 'Owner');

    const profile = await invoke('get_license_team_profile');
    expect(profile.enabled).toBe(true);
    expect(profile.member.role).toBe('owner');
    await writeHandoff(
      'team-owner.json',
      JSON.stringify({
        memberId: profile.member.id,
        deviceId: snapshot.entitlementState.entitlement.claims.deviceId
      })
    );

    await setLabeledValue('.team-invite-form', 'Member name', 'Native E2E Administrator');
    await setLabeledValue(
      '.team-invite-form',
      'Email address',
      'native-e2e-administrator@example.invalid'
    );
    await setLabeledValue('.team-invite-form', 'Workspace role', 'admin');
    await clickButton('Create invitation', '.team-invite-form');
    await waitForText('.team-invite-form .team-secret', 'Invitation token');
    const token = await domText('.team-invite-form .team-secret code');
    expect(token.length).toBeGreaterThanOrEqual(32);
    await writeHandoff('team-member-invitation.token', token);
    await clickButton('Dismiss', '.team-invite-form .team-secret');
    await closeSettings();
  });
});

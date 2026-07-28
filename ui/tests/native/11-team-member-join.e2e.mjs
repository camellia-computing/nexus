import { randomUUID } from 'node:crypto';
import { expect } from '@wdio/globals';
import { readHandoff, writeHandoff } from './handoff.mjs';
import {
  activateThroughLicenseSettings,
  clickButton,
  invoke,
  requiredEnvironment,
  setDomValue,
  waitForText
} from './support.mjs';

describe('Camellia Nexus native Team member join', () => {
  it('preserves the license on invalid input and consumes the real invitation once', async () => {
    await $('h1=Workspace').waitForDisplayed({ timeout: 30_000 });
    const { snapshot } = await activateThroughLicenseSettings(
      requiredEnvironment('CAMELLIA_NEXUS_E2E_TEAM_MEMBER_CODE'),
      'Windows hosted Team administrator'
    );
    expect(snapshot.entitlementState.entitlement.claims.plan).toBe('team');
    await waitForText('.team-workspace-panel', 'Not linked');

    const invalidToken = 'invalid-native-e2e-invitation-token-00000000';
    await setDomValue('.team-accept input[placeholder="Invitation token"]', invalidToken);
    await clickButton('Join workspace', '.team-accept');
    await waitForText('.team-workspace-panel', 'Invitation token not accepted');
    const stillLicensed = await invoke('get_entitlement_state');
    expect(stillLicensed.entitlementState.status).toBe('active');
    expect(stillLicensed.entitlementState.entitlement.claims.deviceId).toBe(
      snapshot.entitlementState.entitlement.claims.deviceId
    );

    const invitationToken = await readHandoff('team-member-invitation.token', true);
    await setDomValue('.team-accept input[placeholder="Invitation token"]', invitationToken);
    await clickButton('Join workspace', '.team-accept');
    await waitForText('.team-summary', 'Native E2E Administrator');
    await waitForText('.team-summary', 'Administrator');
    const profile = await invoke('get_license_team_profile');
    expect(profile.member.role).toBe('admin');
    const replay = await invoke('accept_license_team_invitation', {
      request: { invitationToken, operationId: randomUUID() }
    });
    expect(replay.member.id).toBe(profile.member.id);
    expect(replay.member.role).toBe('admin');
    await writeHandoff(
      'team-member.json',
      JSON.stringify({
        memberId: profile.member.id,
        deviceId: snapshot.entitlementState.entitlement.claims.deviceId
      })
    );
  });
});

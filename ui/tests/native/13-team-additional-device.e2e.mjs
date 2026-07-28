import { randomUUID } from 'node:crypto';
import { expect } from '@wdio/globals';
import { readHandoff } from './handoff.mjs';
import {
  activateThroughLicenseSettings,
  clickButton,
  invoke,
  requiredEnvironment,
  setDomValue,
  waitForText
} from './support.mjs';

describe('Camellia Nexus native Team additional device', () => {
  it('links a separately activated device with the one-use enrollment token', async () => {
    await $('h1=Workspace').waitForDisplayed({ timeout: 30_000 });
    const { snapshot } = await activateThroughLicenseSettings(
      requiredEnvironment('CAMELLIA_NEXUS_E2E_TEAM_ADDITIONAL_DEVICE_CODE'),
      'Windows hosted Team additional device'
    );
    await waitForText('.team-workspace-panel', 'Not linked');
    const owner = JSON.parse(await readHandoff('team-owner.json'));
    const enrollmentToken = await readHandoff('team-owner-device-enrollment.token', true);
    expect(snapshot.entitlementState.entitlement.claims.deviceId).not.toBe(owner.deviceId);

    await setDomValue(
      '.team-accept:not(.team-device-accept) input[placeholder="Invitation token"]',
      enrollmentToken
    );
    await clickButton('Join workspace', '.team-accept:not(.team-device-accept)');
    await waitForText('.team-workspace-panel', 'Invitation token not accepted');
    const stillLicensed = await invoke('get_entitlement_state');
    expect(stillLicensed.entitlementState.status).toBe('active');
    expect(stillLicensed.entitlementState.entitlement.claims.deviceId).toBe(
      snapshot.entitlementState.entitlement.claims.deviceId
    );

    await setDomValue(
      '.team-device-accept input[aria-label="Device enrollment token"]',
      enrollmentToken
    );
    await clickButton('Link device', '.team-device-accept');
    await waitForText('.team-summary', 'Administrator');
    const linked = await invoke('get_license_team_profile');
    expect(linked.member.id).toBe(owner.memberId);
    expect(linked.member.role).toBe('admin');

    const replay = await invoke('accept_license_team_device_enrollment', {
      request: { enrollmentToken, operationId: randomUUID() }
    });
    expect(replay.member.id).toBe(owner.memberId);
  });
});

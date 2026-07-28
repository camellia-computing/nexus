import { browser, expect } from '@wdio/globals';
import { readHandoff, writeHandoff } from './handoff.mjs';
import {
  buttonEnabled,
  clickButton,
  clickButtonInTextContainer,
  clickDomElement,
  confirmAction,
  domText,
  expectNoHorizontalOverflow,
  invoke,
  openLicenseSettings,
  setDomValue,
  setLabeledChecked,
  setLabeledValue,
  waitForButtonState,
  waitForButtonEnabled,
  waitForDomVisibility,
  waitForText
} from './support.mjs';

const memberName = 'Native E2E Administrator';
const sharedName = 'Native hosted shared configuration';

describe('Camellia Nexus native Team control plane', () => {
  it('exercises governance, cloud resources, audit, and secret lifecycle through the UI', async () => {
    await $('h1=Workspace').waitForDisplayed({ timeout: 30_000 });
    await browser.setWindowSize(1180, 820);
    const member = JSON.parse(await readHandoff('team-member.json'));
    const ownerProfile = await invoke('get_license_team_profile');
    expect(ownerProfile.member.role).toBe('owner');
    const dialog = await openLicenseSettings();
    await waitForText('.team-summary', 'Owner');
    await waitForText('.team-members', memberName);
    await expectNoHorizontalOverflow('.settings-dialog');

    await setLabeledValue('.team-invite-form', 'Member name', 'Native E2E Seat Capacity Probe');
    await setLabeledValue(
      '.team-invite-form',
      'Email address',
      'native-e2e-seat-probe@example.invalid'
    );
    await setLabeledValue('.team-invite-form', 'Workspace role', 'viewer');
    await clickButton('Create invitation', '.team-invite-form');
    await waitForText('.team-summary', '3/3');
    await waitForButtonState('Create invitation', false, '.team-invite-form');
    await clickButton('Dismiss', '.team-invite-form .team-secret');
    await clickButtonInTextContainer(
      '.team-members article',
      'Native E2E Seat Capacity Probe',
      'Revoke invitation'
    );
    await confirmAction('Revoke invitation');
    await waitForText('.team-summary', '2/3');
    expect(await buttonEnabled('Create invitation', '.team-invite-form')).toBe(true);

    await clickButtonInTextContainer('.team-members article', memberName, 'Suspend access');
    await waitForText('.team-members', 'Access suspended');
    await clickButtonInTextContainer('.team-members article', memberName, 'Restore access');
    await waitForText('.team-members', 'Active member');

    await clickButton('Create device token', '.team-device-enrollment');
    await waitForDomVisibility('.team-device-enrollment .team-secret code');
    const enrollmentToken = await domText('.team-device-enrollment .team-secret code');
    expect(enrollmentToken.length).toBeGreaterThanOrEqual(32);
    await writeHandoff('team-owner-device-enrollment.token', enrollmentToken);
    await clickButton('Dismiss', '.team-device-enrollment .team-secret');

    await clickDomElement('[data-workspace-view="alerts"]');
    await waitForDomVisibility('#team-workspace-alerts');
    await clickButton('New alert rule', '#team-workspace-alerts');
    await setLabeledValue('#team-workspace-alerts .cloud-form', 'Rule name', 'Shared revision monitor');
    await setLabeledValue(
      '#team-workspace-alerts .cloud-form',
      'Event kind',
      'configuration_revised'
    );
    await setLabeledValue('#team-workspace-alerts .cloud-form', 'Severity', 'critical');
    await clickButton('Create rule', '#team-workspace-alerts .cloud-form');
    await waitForText('.workspace-notice', 'Alert rule created');

    await clickDomElement('[data-workspace-view="shared"]');
    await waitForDomVisibility('#team-workspace-shared');
    await clickButton('New configuration', '#team-workspace-shared');
    await setLabeledValue('.shared-form', 'Name', sharedName);
    await setLabeledValue('.shared-form', 'Program type', 'generic');
    await setLabeledValue('.shared-form', 'Input arguments', '--mode hosted-e2e');
    await setLabeledValue(
      '.shared-form',
      'Configuration content',
      '{"revision":1,"source":"native-e2e"}'
    );
    await clickButton('Create configuration', '.shared-form');
    await waitForText('.workspace-notice', 'Shared configuration created');
    await waitForText('.shared-list', sharedName);

    await clickButtonInTextContainer('.shared-list article', sharedName, 'Revise');
    await waitForDomVisibility('.shared-form');
    await setLabeledValue(
      '.shared-form',
      'Configuration content',
      '{"revision":2,"source":"native-e2e"}'
    );
    await clickButton('Save revision', '.shared-form');
    await waitForText('.workspace-notice', 'Shared configuration revision saved');
    await waitForText('.shared-list', 'Draft revision 2');
    await clickButtonInTextContainer('.shared-list article', sharedName, 'Publish draft');
    await waitForText('.workspace-notice', 'Shared configuration published');
    await waitForText('.shared-list', 'Published revision 2');
    await clickButtonInTextContainer('.shared-list article', sharedName, 'Delete');
    await confirmAction('Delete configuration');
    await waitForText('.workspace-notice', 'Shared configuration deleted');
    await setLabeledChecked('#team-workspace-shared', 'Show deleted', true);
    await waitForText('.shared-list', sharedName);
    await clickButtonInTextContainer('.shared-list article', sharedName, 'Restore');
    await waitForText('.workspace-notice', 'Shared configuration restored');
    const sharedPage = await invoke('get_license_workspace_configurations', {
      request: { cursor: null, limit: 50, includeDeleted: true }
    });
    const shared = sharedPage.configurations.find((item) => item.name === sharedName);
    expect(shared).toBeTruthy();
    expect(shared.deletedAt ?? null).toBeNull();
    expect(shared.draftRevision).toBe(2);
    expect(shared.publishedRevision).toBe(2);
    const sharedContent = await invoke('get_license_workspace_configuration', {
      documentId: shared.id,
      request: { revision: 2 }
    });
    expect(sharedContent.input).toBe('--mode hosted-e2e');
    expect(sharedContent.content).toBe('{"revision":2,"source":"native-e2e"}');

    await clickDomElement('[data-workspace-view="sync"]');
    await waitForDomVisibility('#team-workspace-sync');
    await waitForText('.timeline-list', 'configuration_restored');
    await waitForButtonState('Advance checkpoint', true, '#team-workspace-sync');
    await clickButton('Advance checkpoint', '#team-workspace-sync');
    await waitForText('.workspace-notice', 'This device checkpoint was advanced');

    await clickDomElement('[data-workspace-view="alerts"]');
    await clickButton('Refresh', '#team-workspace-alerts');
    await waitForText('.incident-list', 'A shared configuration was revised.');
    await clickButtonInTextContainer(
      '.incident-list article',
      'A shared configuration was revised.',
      'Acknowledge'
    );
    await waitForText('.workspace-notice', 'Incident acknowledged');
    await clickButtonInTextContainer(
      '.incident-list article',
      'A shared configuration was revised.',
      'Resolve'
    );
    await confirmAction('Resolve incident');
    await waitForText('.workspace-notice', 'Incident resolved');
    await waitForButtonEnabled('Refresh', '#team-workspace-alerts');
    const resolvedIncidents = await invoke('get_license_workspace_alert_incidents', {
      request: {
        cursor: null,
        limit: 50,
        status: 'resolved',
        eventKind: 'configuration_revised',
        severity: 'critical'
      }
    });
    expect(resolvedIncidents.incidents.some((incident) =>
      incident.summary === 'A shared configuration was revised.' &&
      incident.status === 'resolved' &&
      incident.severity === 'critical'
    )).toBe(true);

    await clickDomElement('[data-workspace-view="audit"]');
    await waitForDomVisibility('#team-workspace-audit');
    await waitForText('.audit-list', 'Configuration revised');
    const auditEventTypes = await invoke('get_license_workspace_audit_event_types');
    expect(auditEventTypes.eventTypes).toContain('workspace_configuration_revised');
    await setLabeledValue(
      '#team-workspace-audit .audit-toolbar-actions',
      'Event type',
      'workspace_configuration_revised'
    );
    await clickButton('Apply filter', '#team-workspace-audit');
    await waitForText('.audit-list', 'Configuration revised');
    await waitForButtonEnabled('Export up to 5,000', '#team-workspace-audit');
    const audit = await invoke('export_license_workspace_audit_events', {
      request: {
        cursor: null,
        limit: 5_000,
        eventType: 'workspace_configuration_revised'
      }
    });
    expect(audit.events.length).toBeGreaterThan(0);
    expect(audit.events.length).toBeLessThanOrEqual(5_000);
    expect(audit.events.every((event) =>
      event.eventType === 'workspace_configuration_revised'
    )).toBe(true);

    await clickDomElement('[data-workspace-view="webhooks"]');
    await waitForDomVisibility('#team-workspace-webhooks');
    await clickButton('New endpoint', '#team-workspace-webhooks');
    await setLabeledValue('.webhook-form', 'Endpoint name', 'Native inactive receiver');
    await setLabeledValue(
      '.webhook-form',
      'HTTPS URL',
      'https://example.com/camellia-native-e2e'
    );
    await setLabeledChecked('.webhook-form', 'Configuration revised', true);
    await setLabeledChecked('.webhook-form', 'Endpoint active', false);
    await clickButton('Create endpoint', '.webhook-form');
    await waitForDomVisibility('.webhook-secret code');
    const firstSecret = await domText('.webhook-secret code');
    expect(firstSecret.length).toBeGreaterThanOrEqual(32);
    const createdEndpoints = await invoke('get_license_workspace_webhook_endpoints');
    const createdEndpoint = createdEndpoints.find((endpoint) =>
      endpoint.name === 'Native inactive receiver'
    );
    expect(createdEndpoint).toBeTruthy();
    expect(createdEndpoint.url).toBe('https://example.com/camellia-native-e2e');
    expect(createdEndpoint.active).toBe(false);
    expect(createdEndpoint.eventTypes).toEqual(['configuration.revised']);
    await clickButton('Dismiss secret', '.webhook-secret');
    await clickButtonInTextContainer(
      '.webhook-endpoints article',
      'Native inactive receiver',
      'Rotate secret'
    );
    await confirmAction('Rotate secret');
    await waitForDomVisibility('.webhook-secret code');
    const rotatedSecret = await domText('.webhook-secret code');
    expect(rotatedSecret.length).toBeGreaterThanOrEqual(32);
    expect(rotatedSecret).not.toBe(firstSecret);
    const rotatedEndpoints = await invoke('get_license_workspace_webhook_endpoints');
    const rotatedEndpoint = rotatedEndpoints.find((endpoint) => endpoint.id === createdEndpoint.id);
    expect(rotatedEndpoint.secretVersion).toBeGreaterThan(createdEndpoint.secretVersion);
    await clickButton('Dismiss secret', '.webhook-secret');
    await clickButtonInTextContainer(
      '.webhook-endpoints article',
      'Native inactive receiver',
      'Delete'
    );
    await confirmAction('Delete endpoint');
    await waitForText('.workspace-notice', 'Webhook endpoint deleted');
    expect((await invoke('get_license_workspace_webhook_endpoints')).some((endpoint) =>
      endpoint.id === createdEndpoint.id
    )).toBe(false);

    await clickDomElement('[data-workspace-view="members"]');
    await setDomValue('.team-governance select', member.memberId);
    await clickButton('Transfer ownership', '.team-governance');
    await confirmAction('Transfer ownership');
    await waitForText('.team-summary', 'Administrator');
    const transferredProfile = await invoke('get_license_team_profile');
    expect(transferredProfile.member.id).toBe(ownerProfile.member.id);
    expect(transferredProfile.member.role).toBe('admin');
    expect(transferredProfile.memberCount).toBe(2);
    const transferredMembers = await invoke('get_license_team_members', {
      request: { cursor: null, limit: 100 }
    });
    expect(transferredMembers.members.find((item) => item.id === member.memberId)?.role).toBe(
      'owner'
    );
  });
});

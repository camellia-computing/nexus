import { browser, expect } from '@wdio/globals';
import path from 'node:path';
import {
  clickButton,
  closeSettings,
  completeUiAuthorization,
  ensureNavigationOpen,
  genericFixtureRequest,
  invoke,
  openLicenseSettings,
  requiredEnvironment,
  selectProgram,
  waitForDomVisibility,
  waitForProgramState,
  waitForText
} from './support.mjs';

function errorText(error) {
  if (typeof error === 'string') return error;
  try {
    return JSON.stringify(error, Object.getOwnPropertyNames(error));
  } catch {
    return String(error);
  }
}

async function expectCommandRejected(command, args) {
  let failure;
  try {
    await invoke(command, args);
  } catch (error) {
    failure = error;
  }
  expect(failure).toBeTruthy();
  expect(errorText(failure)).toContain('License service operation failed');
}

async function expectLicenseValue(label, value) {
  await browser.waitUntil(
    () =>
      browser.execute(
        (labelText, expected) => {
          const row = [...document.querySelectorAll('.license-card dl > div')].find(
            (candidate) => candidate.querySelector('dt')?.textContent?.trim() === labelText
          );
          return row?.querySelector('dd')?.textContent?.trim() === expected;
        },
        label,
        value
      ),
    { timeoutMsg: `license field ${label} did not become ${value}` }
  );
}

describe('Camellia Nexus native Free plan', () => {
  it('runs local programs while enforcing every signed Free capability and quota boundary', async () => {
    await $('h1=Workspace').waitForDisplayed({ timeout: 30_000 });
    const dialog = await openLicenseSettings();
    await clickButton('Activate device', '.license-panel');
    await waitForDomVisibility('.license-flow');
    const deviceName = await dialog.$('input[placeholder="Windows workstation"]');
    await deviceName.setValue('Windows hosted Free primary');
    const snapshot = await completeUiAuthorization(
      requiredEnvironment('CAMELLIA_NEXUS_E2E_FREE_PRIMARY_CODE')
    );
    const claims = snapshot.entitlementState.entitlement.claims;
    expect(claims.plan).toBe('free');
    expect(claims.capabilities).toEqual([]);
    expect(claims.workspacePermissions).toEqual([]);
    expect(claims.deviceLimit).toBe(1);
    expect(claims.memberLimit).toBe(1);
    expect(claims.limits).toEqual({
      max_programs: 5,
      max_config_sources_per_program: 0,
      max_team_members: 1,
      max_remote_monitors: 0,
      max_shared_programs: 0,
      max_webhook_endpoints: 0,
      max_workspace_storage_bytes: 0,
      max_alert_rules: 0,
      max_audit_export_events: 0
    });
    await expect(dialog.$('.license-status')).toHaveText('Licensed');
    await expectLicenseValue('Plan', 'Free');
    await expectLicenseValue('Program limit', '5');
    await expectLicenseValue('Config sources', '0');
    await expectLicenseValue('Device limit', '1');

    const devices = await invoke('get_license_devices', { cursor: null, pageSize: 50 });
    expect(devices.devices).toHaveLength(1);
    expect(devices.devices[0].state).toBe('active');
    const billing = await invoke('get_license_billing_summary');
    expect(billing.invoices).toEqual([]);
    await closeSettings();

    const system32 = path.join(requiredEnvironment('SystemRoot'), 'System32');
    const executables = [
      requiredEnvironment('CAMELLIA_NEXUS_E2E_FIXTURE_EXECUTABLE'),
      path.join(system32, 'cmd.exe'),
      path.join(system32, 'ping.exe'),
      path.join(system32, 'where.exe'),
      path.join(system32, 'whoami.exe')
    ];
    const programIds = Array.from({ length: 5 }, (_, index) => `native-e2e-free-${index + 1}`);
    for (const [index, programId] of programIds.entries()) {
      await invoke(
        'create_program',
        genericFixtureRequest(
          programId,
          `Free local program ${index + 1}`,
          executables[index],
          path.dirname(executables[index])
        )
      );
    }
    expect(await invoke('list_programs')).toHaveLength(5);

    await browser.refresh();
    await $('h1=Workspace').waitForDisplayed({ timeout: 30_000 });
    await selectProgram(programIds[0]);
    await clickButton('Start', '.program-hero');
    await waitForProgramState(programIds[0], 'running');
    await clickButton('Stop', '.program-hero');
    await waitForProgramState(programIds[0], 'stopped');

    await expectCommandRejected(
      'create_program',
      genericFixtureRequest(
        'native-e2e-free-over-limit',
        'Must stay over limit',
        path.join(system32, 'hostname.exe'),
        system32
      )
    );
    expect(await invoke('list_programs')).toHaveLength(5);
    await ensureNavigationOpen();
    const addProgram = await $('button[aria-label="Add program"]');
    expect(await addProgram.getAttribute('title')).toBe(
      'The program limit for this license has been reached'
    );
    await addProgram.click();
    await waitForText('.license-prompt', 'Program limit reached');

    await expectCommandRejected('refresh_config_sources', { programId: programIds[0] });
    await expectCommandRejected(
      'validate_config',
      { programId: programIds[0], content: '{}', baseHash: '0'.repeat(64) }
    );
    await expectCommandRejected(
      'replace_package',
      {
        programId: programIds[0],
        packageSource: requiredEnvironment('CAMELLIA_NEXUS_E2E_FIXTURE_WORKING_DIRECTORY')
      }
    );
    await expectCommandRejected('open_sing_box_dashboard', {
      programId: programIds[0],
      dashboardKind: 'native'
    });
    await expectCommandRejected('get_license_team_profile', {});
    expect(await invoke('list_programs')).toHaveLength(5);
    const executableAfterRejectedReplacement = (
      await invoke('get_program', { programId: programIds[0] })
    ).spec.executable;
    expect(executableAfterRejectedReplacement.mode).toBe('external');
    expect(executableAfterRejectedReplacement.path).toBe(executables[0]);

    for (const programId of programIds) {
      await invoke('remove_program', { programId });
    }
    expect(await invoke('list_programs')).toHaveLength(0);
  });
});

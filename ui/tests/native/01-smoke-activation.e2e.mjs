import { browser, expect } from '@wdio/globals';
import { writeHandoff } from './handoff.mjs';
import {
  clickButton,
  closeSettings,
  completeUiAuthorization,
  confirmAction,
  ensureNavigationOpen,
  expectNoHorizontalOverflow,
  genericFixtureRequest,
  invoke,
  isDomVisible,
  openLicenseSettings,
  requiredEnvironment,
  selectProgram,
  waitForDomVisibility,
  waitForProgramState
} from './support.mjs';

describe('Camellia Nexus native activation and lifecycle', () => {
  it('uses the visible activation flow and real program controls', async () => {
    const heading = await $('h1=Workspace');
    await heading.waitForDisplayed({ timeout: 30_000 });
    expect(await browser.tauri.execute(() => window.location.href)).toContain('tauri');

    for (const [width, height] of [
      [1280, 800],
      [760, 620],
      [680, 480]
    ]) {
      await browser.setWindowSize(width, height);
      await expectNoHorizontalOverflow();
      const navigationOpened = await ensureNavigationOpen();
      expect(await isDomVisible('button[aria-label="Add program"]')).toBe(true);
      expect(await isDomVisible('button[aria-label="Settings"]')).toBe(true);
      if (navigationOpened) {
        await browser.keys(['Escape']);
        await waitForDomVisibility('button[aria-label="Settings"]', false);
      }
    }

    let guarded = false;
    try {
      await invoke(
        'create_program',
        genericFixtureRequest('native-e2e-guarded', 'Must stay guarded')
      );
    } catch {
      guarded = true;
    }
    expect(guarded).toBe(true);

    const dialog = await openLicenseSettings();
    const signOut = await dialog.$('button=Sign out');
    expect(await signOut.isEnabled()).toBe(false);
    const refreshDevices = await dialog.$('button=Refresh devices');
    expect(await refreshDevices.isEnabled()).toBe(false);

    await clickButton('Activate device', '.license-panel');
    await waitForDomVisibility('.license-flow');
    await clickButton('Cancel activation', '.license-flow');
    await waitForDomVisibility('.license-flow', false);

    await clickButton('Activate device', '.license-panel');
    await waitForDomVisibility('.license-flow');
    const deviceName = await dialog.$('input[placeholder="Windows workstation"]');
    await deviceName.setValue('Windows hosted native E2E');
    const snapshot = await completeUiAuthorization(
      requiredEnvironment('CAMELLIA_NEXUS_E2E_PRO_PRIMARY_CODE')
    );
    expect(snapshot.entitlementState.entitlement.claims.plan).toBe('pro');
    await expect(dialog.$('.license-status')).toHaveText('Licensed');
    await writeHandoff(
      'pro-device.json',
      JSON.stringify({ deviceId: snapshot.entitlementState.entitlement.claims.deviceId })
    );
    await closeSettings();

    const lifecycleId = 'native-e2e-generic';
    await invoke(
      'create_program',
      genericFixtureRequest(lifecycleId, 'Native E2E generic process')
    );
    await selectProgram(lifecycleId);
    await clickButton('Start', '.program-hero');
    await waitForProgramState(lifecycleId, 'running');
    await clickButton('Logs');
    await browser.waitUntil(
      async () => (await $('.log-pane.stdout pre').getText()).includes('native-e2e-ready'),
      { timeoutMsg: 'the real fixture stdout did not reach the Logs tab' }
    );
    await clickButton('Stop', '.program-hero');
    await waitForProgramState(lifecycleId, 'stopped');
    await clickButton('Details', '.program-tabs');
    await waitForDomVisibility('#program-panel-overview');
    await clickButton('Delete program', '#program-panel-overview');
    await confirmAction('Delete program');
    await waitForDomVisibility(`[data-program-id="${lifecycleId}"]`, false);

    const enforcementId = 'native-e2e-enforcement';
    await invoke(
      'create_program',
      genericFixtureRequest(enforcementId, 'Native E2E enforcement process')
    );
    await ensureNavigationOpen();
    await waitForDomVisibility(`[data-program-id="${enforcementId}"]`);
  });
});

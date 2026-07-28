import { expect } from '@wdio/globals';
import { invoke } from './support.mjs';

describe('Camellia Nexus native failure cleanup', () => {
  it('opens the isolated identity so the harness can reset its secure state', async () => {
    await $('h1=Workspace').waitForDisplayed({ timeout: 30_000 });
    const snapshot = await invoke('get_entitlement_state');
    expect(snapshot).toHaveProperty('entitlementState');
  });
});

import { browser, expect } from '@wdio/globals';
import {
  clickButton,
  invoke,
  openLicenseSettings,
  requiredEnvironment,
  setLabeledValue,
  waitForButtonEnabled,
  waitForDomVisibility,
  waitForText
} from './support.mjs';

describe('Camellia Nexus native online recovery and billing', () => {
  it('recovers the same session and submits a real payment claim through the UI', async () => {
    await $('h1=Workspace').waitForDisplayed({ timeout: 30_000 });
    const dialog = await openLicenseSettings();
    await clickButton('Refresh entitlement', '.license-panel');
    await browser.waitUntil(
      async () => (await invoke('get_entitlement_state')).entitlementState.status === 'active',
      { timeoutMsg: 'the cached Pro entitlement did not recover online' }
    );
    await clickButton('Refresh billing', '.license-billing');
    await waitForButtonEnabled('Refresh billing', '.license-billing');
    await waitForDomVisibility('.billing-payment-form');
    await setLabeledValue(
      '.billing-payment-form',
      'Transaction or receipt ID',
      'native-e2e-payment-receipt'
    );
    await setLabeledValue('.billing-payment-form', 'Payer name', 'Native E2E Customer');
    await setLabeledValue(
      '.billing-payment-form',
      'Note',
      'Synthetic hosted-runner payment evidence'
    );
    await clickButton('Submit for review', '.billing-payment-form');
    await waitForText('.billing-invoices', 'Evidence submitted');

    const billing = await invoke('get_license_billing_summary');
    expect(billing.invoices.some((invoice) =>
      invoice.id === requiredEnvironment('CAMELLIA_NEXUS_E2E_BILLING_INVOICE_ID')
    )).toBe(true);
    expect(billing.paymentMethods.some((method) =>
      method.id === requiredEnvironment('CAMELLIA_NEXUS_E2E_BILLING_PAYMENT_METHOD_ID')
    )).toBe(true);
    expect(billing.paymentClaims.some((claim) =>
      claim.externalTransactionId === 'native-e2e-payment-receipt' && claim.status === 'submitted'
    )).toBe(true);
    await expect(dialog.$('.license-status')).toHaveText('Licensed');
  });
});

import path from 'node:path';
import { browser, expect } from '@wdio/globals';
import { ensureNavigationOpen, expectNoHorizontalOverflow, requiredEnvironment } from './support.mjs';

const outputDirectory = requiredEnvironment('CAMELLIA_NEXUS_E2E_OUTPUT_DIR');

describe('Native Intent and Final configuration layout', () => {
  it('keeps structured settings and Final editing usable in the actual WebView2', async () => {
    await $('h1=Workspace').waitForDisplayed({ timeout: 30_000 });
    expect(await browser.tauri.execute(() => window.location.href)).toContain('tauri');
    await ensureNavigationOpen();
    await $('.program-item[data-program-id="xray-primary"]').click();
    await $('#program-tab-intent').click();
    const intent = await $('#program-panel-intent');
    await intent.waitForDisplayed();
    expect(await intent.getText()).toContain('Local access');
    expect(await intent.getText()).toContain('Logging');
    await intent.$('button=Add Local proxy').click();
    const form = await intent.$('.object-form');
    await form.waitForDisplayed();
    await form.$('input[type="number"]').setValue('18080');
    for (const width of [1280, 760, 680, 520, 400]) {
      await browser.setWindowSize(width, 820);
      await expectNoHorizontalOverflow();
      const contained = await browser.execute(() => [...document.querySelectorAll('.object-form input, .object-form select, .object-form button')].every((control) => {
        const rect = control.getBoundingClientRect();
        const parent = control.closest('.intent-group').getBoundingClientRect();
        return rect.left >= parent.left - 1 && rect.right <= parent.right + 1;
      }));
      expect(contained).toBe(true);
    }
    await form.$('button=Add').click();
    await form.waitForExist({ reverse: true });
    await $('#program-tab-configuration').click();
    const editor = await $('[aria-label="Configuration editor"]');
    await editor.waitForDisplayed();
    expect(await editor.getText()).toContain('18080');
    expect(await editor.getText()).toContain('127.0.0.1');
    await expectNoHorizontalOverflow();
    await browser.saveScreenshot(path.join(outputDirectory, 'intent-final-native-compact.png'));
    await $('#program-tab-compatibility').click();
    await $('#program-panel-compatibility').waitForDisplayed();
    await expectNoHorizontalOverflow();
  });
});

import path from 'node:path';
import { browser, expect } from '@wdio/globals';
import {
  ensureNavigationOpen,
  expectNoHorizontalOverflow,
  requiredEnvironment,
} from './support.mjs';

const outputDirectory = requiredEnvironment('CAMELLIA_NEXUS_E2E_OUTPUT_DIR');

async function waitForWorkspace() {
  await $('h1=Workspace').waitForDisplayed({ timeout: 30_000 });
  await $('.program-item[data-program-id="xray-primary"]').waitForExist();
}

async function reloadPreview(query = '') {
  await browser.url(`http://tauri.localhost/?__ui_preview${query}`);
  await waitForWorkspace();
}

async function openXrayDetails() {
  await ensureNavigationOpen();
  await $('[data-program-id="xray-primary"]').click();
  await $('#program-panel-overview').waitForDisplayed();
  const card = await $('.compatibility-card');
  await card.waitForDisplayed();
  return card;
}

async function focusScreenshotTarget(selector, offset = 16) {
  const found = await browser.execute((targetSelector, targetOffset) => {
    const target = document.querySelector(targetSelector);
    const scroller = document.querySelector('.shell > main');
    if (!(target instanceof HTMLElement) || !(scroller instanceof HTMLElement)) return false;
    const targetTop = target.getBoundingClientRect().top;
    const scrollerTop = scroller.getBoundingClientRect().top;
    scroller.scrollTop += targetTop - scrollerTop - targetOffset;
    return true;
  }, selector, offset);
  expect(found).toBe(true);
  await browser.waitUntil(async () => browser.execute((targetSelector) => {
    const target = document.querySelector(targetSelector);
    const scroller = document.querySelector('.shell > main');
    if (!(target instanceof HTMLElement) || !(scroller instanceof HTMLElement)) return false;
    const targetBounds = target.getBoundingClientRect();
    const scrollerBounds = scroller.getBoundingClientRect();
    return targetBounds.top >= scrollerBounds.top && targetBounds.top < scrollerBounds.bottom;
  }, selector), { timeout: 5_000, timeoutMsg: `${selector} did not enter the native viewport` });
}

describe('Camellia Nexus native configuration compatibility preview', () => {
  it('exercises real WebView2 layout, target evidence and source recovery states', async () => {
    await waitForWorkspace();
    expect(await browser.tauri.execute(() => window.location.href)).toContain('tauri');

    await browser.setWindowSize(1280, 820);
    const defaultCard = await openXrayDetails();
    expect(await defaultCard.getText()).toContain('Feature decisions');
    expect(await defaultCard.getText()).toContain('Accepted for this candidate');
    await focusScreenshotTarget('.compatibility-card', 92);
    await expectNoHorizontalOverflow();
    await browser.saveScreenshot(path.join(outputDirectory, 'configuration-default-wide.png'));

    await reloadPreview('&__ui_core_target=future&__ui_core_evidence=stale');
    await browser.setWindowSize(680, 720);
    const futureCard = await openXrayDetails();
    const futureText = await futureCard.getText();
    expect(futureText).toContain('Xray version 99.0.0');
    expect(futureText).toContain('Validation required');
    expect(futureText).toContain('newer than the local compatibility catalog');
    await focusScreenshotTarget('.compatibility-card', 172);
    await expectNoHorizontalOverflow();
    await browser.saveScreenshot(path.join(outputDirectory, 'configuration-future-compact.png'));
    await focusScreenshotTarget('.compatibility-fact.warning', 172);
    await expectNoHorizontalOverflow();
    await browser.saveScreenshot(path.join(outputDirectory, 'configuration-future-compact-evidence.png'));

    await reloadPreview('&__ui_config_source=invalid');
    await browser.setWindowSize(760, 760);
    await openXrayDetails();
    await $('#program-tab-configuration').click();
    await $('#program-panel-configuration').waitForDisplayed();
    await $('.guided-workspace').waitForDisplayed();
    const diagnosticPanel = await $('.guided-diagnostics');
    const diagnostics = await diagnosticPanel.getText();
    expect(diagnostics).toContain('SOURCE_INVALID');
    expect(diagnostics).toContain('Applied and Last Known Good were retained');
    await focusScreenshotTarget('.guided-diagnostics', 172);
    await expectNoHorizontalOverflow();
    await browser.saveScreenshot(path.join(outputDirectory, 'configuration-source-invalid.png'));
  });
});

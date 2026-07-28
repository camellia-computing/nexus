import { browser, expect } from '@wdio/globals';

export function requiredEnvironment(name) {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required for native desktop tests`);
  return value;
}

export async function invoke(command, args = {}) {
  return browser.tauri.execute(
    ({ core }, commandName, commandArguments) => core.invoke(commandName, commandArguments),
    command,
    args
  );
}

export function genericFixtureRequest(
  id,
  name,
  executablePath = requiredEnvironment('CAMELLIA_NEXUS_E2E_FIXTURE_EXECUTABLE'),
  workingDirectory = requiredEnvironment('CAMELLIA_NEXUS_E2E_FIXTURE_WORKING_DIRECTORY')
) {
  return {
    request: {
      spec: {
        schemaVersion: 3,
        id,
        name,
        executable: {
          mode: 'external',
          path: executablePath
        },
        type: {
          kind: 'generic',
          args: [
            '-NoLogo',
            '-NoProfile',
            '-NonInteractive',
            '-File',
            requiredEnvironment('CAMELLIA_NEXUS_E2E_FIXTURE_SCRIPT')
          ]
        },
        workingDirectory,
        environment: {},
        autoStart: false,
        restartPolicy: 'never',
        privilegePolicy: { mode: 'automatic' }
      }
    }
  };
}

export async function submitUiAuthorization(activationCode) {
  const flow = await $('.license-flow');
  await flow.waitForDisplayed();
  const value = await flow.getAttribute('data-e2e-authorization-url');
  if (!value) throw new Error('the visible activation flow has no E2E authorization URL');
  const authorizationUrl = new URL(value);
  expect(authorizationUrl.protocol).toBe('https:');
  const form = new URLSearchParams(authorizationUrl.searchParams);
  form.set('activation_code', activationCode);

  const response = await fetch(
    new URL('/oauth/authorize', requiredEnvironment('CAMELLIA_NEXUS_E2E_SERVER_BASE_URL')),
    {
      method: 'POST',
      headers: { 'content-type': 'application/x-www-form-urlencoded' },
      body: form,
      redirect: 'manual'
    }
  );
  expect(response.status).toBe(303);
  const location = response.headers.get('location');
  expect(location).toBeTruthy();

  const callback = await fetch(new URL(location, authorizationUrl), { redirect: 'manual' });
  return { authorizationUrl, callback };
}

export async function completeUiAuthorization(activationCode) {
  const { callback } = await submitUiAuthorization(activationCode);
  expect(callback.ok).toBe(true);
  await browser.waitUntil(
    async () => (await invoke('get_entitlement_state')).entitlementState.status === 'active',
    {
      timeout: 30_000,
      interval: 200,
      timeoutMsg: 'the application did not complete the native authorization callback'
    }
  );
  return invoke('get_entitlement_state');
}

export async function activateThroughLicenseSettings(activationCode, displayName) {
  const dialog = await openLicenseSettings();
  await clickButton('Activate device', '.license-panel');
  await waitForDomVisibility('.license-flow');
  const deviceName = await dialog.$('input[placeholder="Windows workstation"]');
  await deviceName.setValue(displayName);
  const snapshot = await completeUiAuthorization(activationCode);
  await waitForDomVisibility('.license-flow', false);
  return { dialog, snapshot };
}

export async function expectNoHorizontalOverflow(selector = 'body') {
  const overflow = await browser.execute((targetSelector) => {
    const target = document.querySelector(targetSelector);
    return {
      document: document.documentElement.scrollWidth - document.documentElement.clientWidth,
      body: document.body.scrollWidth - document.body.clientWidth,
      target: target ? target.scrollWidth - target.clientWidth : 0
    };
  }, selector);
  expect(overflow.document).toBeLessThanOrEqual(1);
  expect(overflow.body).toBeLessThanOrEqual(1);
  expect(overflow.target).toBeLessThanOrEqual(1);
}

export async function isDomVisible(selector) {
  return browser.execute((targetSelector) => {
    const element = document.querySelector(targetSelector);
    if (!(element instanceof HTMLElement) || element.closest('[inert]')) return false;
    const style = window.getComputedStyle(element);
    const bounds = element.getBoundingClientRect();
    return (
      style.display !== 'none' &&
      style.visibility !== 'hidden' &&
      Number(style.opacity) > 0 &&
      bounds.width > 0 &&
      bounds.height > 0
    );
  }, selector);
}

export async function waitForDomVisibility(selector, visible = true) {
  await browser.waitUntil(async () => (await isDomVisible(selector)) === visible, {
    timeoutMsg: `${selector} did not become ${visible ? 'visible' : 'hidden'}`
  });
}

export async function clickDomElement(selector) {
  const clicked = await browser.execute((targetSelector) => {
    const element = document.querySelector(targetSelector);
    if (!(element instanceof HTMLButtonElement) || element.disabled || element.closest('[inert]')) {
      return false;
    }
    element.click();
    return true;
  }, selector);
  expect(clicked).toBe(true);
}

export async function ensureNavigationOpen() {
  const settingsSelector = 'button[aria-label="Settings"]';
  if (await isDomVisible(settingsSelector)) return false;
  const navigationSelector = 'button[aria-label="Open navigation"]';
  await waitForDomVisibility(navigationSelector);
  await clickDomElement(navigationSelector);
  await waitForDomVisibility(settingsSelector);
  return true;
}

export async function openLicenseSettings() {
  await ensureNavigationOpen();
  await clickDomElement('button[aria-label="Settings"]');
  await waitForDomVisibility('.settings-dialog');
  const dialog = await $('.settings-dialog');
  await clickDomElement('[data-settings-section="license"]');
  await waitForDomVisibility('.license-panel');
  return dialog;
}

export async function closeSettings() {
  await browser.keys(['Escape']);
  await waitForDomVisibility('.settings-dialog', false);
}

export async function waitForProgramState(programId, status) {
  await browser.waitUntil(
    async () => (await invoke('get_program', { programId })).state.status === status,
    {
      timeout: 20_000,
      interval: 200,
      timeoutMsg: `${programId} did not reach ${status}`
    }
  );
}

export async function selectProgram(programId) {
  await ensureNavigationOpen();
  const selector = `[data-program-id="${programId}"]`;
  await waitForDomVisibility(selector);
  await clickDomElement(selector);
  await waitForDomVisibility('.program-hero');
}

export async function clickButton(label, rootSelector = 'body') {
  await browser.waitUntil(
    () =>
      browser.execute(
        (root, text) => {
          const scope = document.querySelector(root);
          const button = [...(scope?.querySelectorAll('button') ?? [])].find(
            (candidate) => candidate.textContent?.trim() === text
          );
          if (
            !(button instanceof HTMLButtonElement) ||
            button.disabled ||
            button.closest('[inert]')
          ) {
            return false;
          }
          button.click();
          return true;
        },
        rootSelector,
        label
      ),
    {
      timeout: 10_000,
      interval: 100,
      timeoutMsg: `button ${JSON.stringify(label)} did not become enabled in ${rootSelector}`
    }
  );
}

export async function waitForButtonEnabled(label, rootSelector = 'body') {
  await browser.waitUntil(
    () =>
      browser.execute(
        (root, text) => {
          const scope = document.querySelector(root);
          const button = [...(scope?.querySelectorAll('button') ?? [])].find(
            (candidate) => candidate.textContent?.trim() === text
          );
          return button instanceof HTMLButtonElement
            && !button.disabled
            && !button.closest('[inert]');
        },
        rootSelector,
        label
      ),
    {
      timeout: 20_000,
      interval: 100,
      timeoutMsg: `button ${JSON.stringify(label)} did not become enabled in ${rootSelector}`
    }
  );
}

export async function domText(selector) {
  return browser.execute((targetSelector) => {
    const element = document.querySelector(targetSelector);
    return element?.textContent?.replace(/\s+/gu, ' ').trim() ?? '';
  }, selector);
}

export async function waitForText(selector, text, present = true) {
  await browser.waitUntil(
    async () => (await domText(selector)).includes(text) === present,
    {
      timeoutMsg: `${selector} did not ${present ? 'contain' : 'remove'} ${text}`
    }
  );
}

export async function setDomValue(selector, value) {
  const changed = await browser.execute(
    (targetSelector, nextValue) => {
      const control = document.querySelector(targetSelector);
      if (
        !(
          control instanceof HTMLInputElement ||
          control instanceof HTMLTextAreaElement ||
          control instanceof HTMLSelectElement
        ) ||
        control.disabled ||
        control.closest('[inert]')
      ) {
        return false;
      }
      control.value = nextValue;
      control.dispatchEvent(new Event('input', { bubbles: true }));
      control.dispatchEvent(new Event('change', { bubbles: true }));
      return true;
    },
    selector,
    value
  );
  expect(changed).toBe(true);
}

export async function setLabeledValue(rootSelector, label, value) {
  const changed = await browser.execute(
    (root, labelText, nextValue) => {
      const scope = document.querySelector(root);
      const owner = [...(scope?.querySelectorAll('label') ?? [])].find((candidate) =>
        candidate.textContent?.replace(/\s+/gu, ' ').trim().startsWith(labelText)
      );
      const control = owner?.querySelector('input, textarea, select');
      if (
        !(
          control instanceof HTMLInputElement ||
          control instanceof HTMLTextAreaElement ||
          control instanceof HTMLSelectElement
        ) ||
        control.disabled ||
        control.closest('[inert]')
      ) {
        return false;
      }
      control.value = nextValue;
      control.dispatchEvent(new Event('input', { bubbles: true }));
      control.dispatchEvent(new Event('change', { bubbles: true }));
      return true;
    },
    rootSelector,
    label,
    value
  );
  expect(changed).toBe(true);
}

export async function setLabeledChecked(rootSelector, label, checked) {
  await browser.waitUntil(
    () =>
      browser.execute(
        (root, labelText, nextChecked) => {
          const scope = document.querySelector(root);
          const owner = [...(scope?.querySelectorAll('label') ?? [])].find((candidate) =>
            candidate.textContent?.replace(/\s+/gu, ' ').trim().startsWith(labelText)
          );
          const control = owner?.querySelector('input[type="checkbox"]');
          if (
            !(control instanceof HTMLInputElement) ||
            control.disabled ||
            control.closest('[inert]')
          ) {
            return false;
          }
          control.checked = nextChecked;
          control.dispatchEvent(new Event('input', { bubbles: true }));
          control.dispatchEvent(new Event('change', { bubbles: true }));
          return true;
        },
        rootSelector,
        label,
        checked
      ),
    {
      timeout: 20_000,
      interval: 100,
      timeoutMsg: `checkbox ${JSON.stringify(label)} did not become enabled in ${rootSelector}`
    }
  );
}

export async function clickButtonInTextContainer(containerSelector, containerText, label) {
  await browser.waitUntil(
    () =>
      browser.execute(
        (selector, expectedContainerText, buttonText) => {
          const container = [...document.querySelectorAll(selector)].find((candidate) =>
            candidate.textContent?.includes(expectedContainerText)
          );
          const button = [...(container?.querySelectorAll('button') ?? [])].find(
            (candidate) => candidate.textContent?.trim() === buttonText
          );
          if (
            !(button instanceof HTMLButtonElement) ||
            button.disabled ||
            button.closest('[inert]')
          ) {
            return false;
          }
          button.click();
          return true;
        },
        containerSelector,
        containerText,
        label
      ),
    {
      timeout: 10_000,
      interval: 100,
      timeoutMsg:
        `button ${JSON.stringify(label)} did not become enabled in ${containerSelector} ` +
        `containing ${JSON.stringify(containerText)}`
    }
  );
}

export async function buttonEnabled(label, rootSelector = 'body') {
  return browser.execute(
    (root, text) => {
      const scope = document.querySelector(root);
      const button = [...(scope?.querySelectorAll('button') ?? [])].find(
        (candidate) => candidate.textContent?.trim() === text
      );
      return button instanceof HTMLButtonElement && !button.disabled && !button.closest('[inert]');
    },
    rootSelector,
    label
  );
}

export async function waitForButtonState(label, enabled, rootSelector = 'body') {
  await browser.waitUntil(async () => (await buttonEnabled(label, rootSelector)) === enabled, {
    timeoutMsg: `${label} did not become ${enabled ? 'enabled' : 'disabled'}`
  });
}

export async function confirmAction(label) {
  await waitForDomVisibility('.confirm-dialog');
  await clickButton(label, '.confirm-dialog');
  await waitForDomVisibility('.confirm-dialog', false);
}

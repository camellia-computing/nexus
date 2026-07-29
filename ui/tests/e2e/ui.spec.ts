import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Locator, type Page } from '@playwright/test';

type Theme = 'cupertino' | 'material' | 'aurora';
type Mode = 'light' | 'dark';

const themeSignatures: Record<Theme, {
  radius: string;
  control: string;
  fontFragment: string;
  sidebarWidth: string;
  shellGap: string;
  supportColumns: number;
  canvas: Record<Mode, string>;
}> = {
  cupertino: {
    radius: '17px',
    control: '38px',
    fontFragment: 'SF Pro Display',
    sidebarWidth: '276px',
    shellGap: '12px',
    supportColumns: 3,
    canvas: { light: '#eaf0f8', dark: '#09101c' },
  },
  material: {
    radius: '22px',
    control: '40px',
    fontFragment: 'Roboto Flex',
    sidebarWidth: '238px',
    shellGap: '0px',
    supportColumns: 2,
    canvas: { light: '#fffbff', dark: '#141218' },
  },
  aurora: {
    radius: '18px',
    control: '38px',
    fontFragment: 'Segoe UI Variable Display',
    sidebarWidth: '294px',
    shellGap: '10px',
    supportColumns: 1,
    canvas: { light: '#e8f0fa', dark: '#070b17' },
  },
};

async function openPreview(
  page: Page,
  theme: Theme = 'cupertino',
  colorMode: Mode = 'light',
  scale = 1.05,
  query = '',
) {
  await page.addInitScript(({ theme, colorMode, scale }) => {
    localStorage.setItem('camellia-nexus.appearance.v3', JSON.stringify({
      version: 3,
      theme,
      colorMode,
      scale,
    }));
  }, { theme, colorMode, scale });
  await page.goto(`/?__ui_preview${query}`);
  await expect(page.getByRole('heading', { name: 'Workspace' })).toBeVisible();
  await expect(page.getByText('Primary Xray routing fabric', { exact: true }).first()).toBeAttached();
}

async function openProgramConfiguration(page: Page, programId: string) {
  const program = page.locator(`.program-item[data-program-id="${programId}"]`);
  if (!await program.isVisible()) {
    await page.getByRole('button', { name: 'Open navigation' }).click();
  }
  await program.click();
  await page.getByRole('tab', { name: 'Configuration' }).click();
  const editor = page.getByRole('textbox', { name: 'Configuration editor' });
  await expect(editor).toBeVisible();
  return editor;
}

async function replaceEditorContent(page: Page, editor: Locator, content: string) {
  await editor.focus();
  await page.keyboard.press('ControlOrMeta+A');
  await page.keyboard.insertText(content);
}

async function trackPreviewExternalActions(page: Page) {
  await page.addInitScript(() => {
    const counts: Record<string, number> = {};
    (window as typeof window & { __mockExternalActionCounts: Record<string, number> })
      .__mockExternalActionCounts = counts;
    window.addEventListener('camellia-ui-preview:external-action', (event) => {
      const command = (event as CustomEvent<string>).detail;
      counts[command] = (counts[command] ?? 0) + 1;
    });
  });
}

async function previewExternalActionCount(page: Page, command: string) {
  return page.evaluate((name) => (
    window as typeof window & { __mockExternalActionCounts: Record<string, number> }
  ).__mockExternalActionCounts[name] ?? 0, command);
}

async function trackProgramSelectionRequests(page: Page) {
  await page.addInitScript(() => {
    const requests: Array<{ command: string; programId: string }> = [];
    (window as typeof window & {
      __mockProgramSelectionRequests: typeof requests;
    }).__mockProgramSelectionRequests = requests;
    window.addEventListener('camellia-ui-preview:program-selection-request', (event) => {
      requests.push((event as CustomEvent<{ command: string; programId: string }>).detail);
    });
  });
}

async function programSelectionRequests(page: Page) {
  return page.evaluate(() => (
    window as typeof window & {
      __mockProgramSelectionRequests: Array<{ command: string; programId: string }>;
    }
  ).__mockProgramSelectionRequests);
}

async function releaseProgramSelectionRequests(page: Page, commands: string[]) {
  await page.evaluate((selectionCommands) => {
    window.dispatchEvent(new CustomEvent(
      'camellia-ui-preview:release-program-selection',
      { detail: selectionCommands },
    ));
  }, commands);
}

async function trackTeamWorkspaceRequests(page: Page) {
  await page.addInitScript(() => {
    const state = {
      memberRequests: 0,
      profileRequests: 0,
      billingRequests: 0,
      mutations: [] as Array<{
        command: string;
        operationId: string;
        rowIdentity?: Record<string, unknown>;
      }>,
      auditExportLimits: [] as number[],
    };
    (window as typeof window & { __teamWorkspaceRequests: typeof state }).__teamWorkspaceRequests = state;
    window.addEventListener('camellia-ui-preview:team-members-request', () => {
      state.memberRequests += 1;
    });
    window.addEventListener('camellia-ui-preview:team-profile-request', () => {
      state.profileRequests += 1;
    });
    window.addEventListener('camellia-ui-preview:billing-request', () => {
      state.billingRequests += 1;
    });
    window.addEventListener('camellia-ui-preview:workspace-mutation', (event) => {
      state.mutations.push((event as CustomEvent<{
        command: string;
        operationId: string;
        rowIdentity?: Record<string, unknown>;
      }>).detail);
    });
    window.addEventListener('camellia-ui-preview:audit-export-request', (event) => {
      state.auditExportLimits.push((event as CustomEvent<{ limit: number }>).detail.limit);
    });
  });
}

async function openLicenseSettings(page: Page) {
  await page.getByRole('button', { name: 'Settings' }).click();
  const dialog = page.locator('.settings-dialog');
  await dialog.getByRole('tab', { name: 'License' }).click();
  await expect(dialog.getByRole('heading', { name: 'Team workspace' })).toBeVisible();
  const versionTypography = await dialog.locator('.license-card .version-value').evaluateAll(
    (values) => values.map((value) => {
      const style = getComputedStyle(value);
      return { family: style.fontFamily, size: style.fontSize, weight: style.fontWeight };
    }),
  );
  expect(versionTypography).toHaveLength(3);
  expect(new Set(versionTypography.map(({ family }) => family)).size).toBe(1);
  expect(new Set(versionTypography.map(({ size }) => size)).size).toBe(1);
  expect(new Set(versionTypography.map(({ weight }) => weight)).size).toBe(1);
  return dialog;
}

async function expectNoViewportOverflow(page: Page) {
  const overflow = await page.evaluate(() => ({
    document: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    body: document.body.scrollWidth - document.body.clientWidth,
    main: (() => {
      const main = document.querySelector('main');
      return main ? main.scrollWidth - main.clientWidth : 0;
    })(),
  }));
  expect(overflow.document).toBeLessThanOrEqual(1);
  expect(overflow.body).toBeLessThanOrEqual(1);
  expect(overflow.main).toBeLessThanOrEqual(1);
}

async function expectAccessible(page: Page, include?: string) {
  const builder = new AxeBuilder({ page })
    .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']);
  if (include) builder.include(include);
  const result = await builder.analyze();
  expect(
    result.violations.map(({ id, impact, nodes }) => ({
      id,
      impact,
      targets: nodes.map((node) => node.target),
    })),
  ).toEqual([]);
}

async function expectThemeSignature(page: Page, theme: Theme, colorMode: Mode) {
  const signature = await page.locator('html').evaluate((element) => {
    const style = getComputedStyle(element);
    const supportGrid = document.querySelector<HTMLElement>('.status-support-grid');
    const activityPanel = document.querySelector<HTMLElement>('.activity-panel');
    const templatePanel = document.querySelector<HTMLElement>('.template-panel');
    const templateGrid = document.querySelector<HTMLElement>('.template-grid');
    return {
      radius: style.getPropertyValue('--ui-radius-md').trim(),
      control: style.getPropertyValue('--ui-control-md').trim(),
      font: style.getPropertyValue('--ui-font-display').trim(),
      sidebarWidth: style.getPropertyValue('--ui-sidebar-width').trim(),
      shellGap: style.getPropertyValue('--ui-shell-gap').trim(),
      canvas: style.getPropertyValue('--ui-canvas').trim(),
      text: style.getPropertyValue('--ui-text-primary').trim(),
      colorScheme: style.colorScheme,
      supportColumns: supportGrid
        ? getComputedStyle(supportGrid).gridTemplateColumns.split(/\s+/).filter(Boolean).length
        : 0,
      activityColumn: activityPanel ? getComputedStyle(activityPanel).gridColumnStart : '',
      templateColumn: templatePanel ? getComputedStyle(templatePanel).gridColumnStart : '',
      templateColumns: templateGrid
        ? getComputedStyle(templateGrid).gridTemplateColumns.split(/\s+/).filter(Boolean).length
        : 0,
    };
  });
  const expected = themeSignatures[theme];
  expect(signature.radius).toBe(expected.radius);
  expect(signature.control).toBe(expected.control);
  expect(signature.font).toContain(expected.fontFragment);
  expect(signature.sidebarWidth).toBe(expected.sidebarWidth);
  expect(signature.shellGap).toBe(expected.shellGap);
  expect(signature.canvas.toLowerCase()).toBe(expected.canvas[colorMode]);
  expect(signature.text).not.toBe('');
  expect(signature.colorScheme).toBe(colorMode);
  expect(signature.supportColumns).toBe(expected.supportColumns);
  if (theme === 'material') {
    expect(signature.templateColumn).toBe('1');
    expect(signature.activityColumn).toBe('2');
  } else {
    expect(signature.templateColumn).toBe('auto');
    expect(signature.activityColumn).toBe('auto');
  }
  expect(signature.templateColumns).toBe(theme === 'aurora' ? 2 : 1);
}

async function expectAccessibleThemeContrast(page: Page) {
  const ratios = await page.evaluate(() => {
    type Rgba = [number, number, number, number];
    const probe = document.createElement('span');
    document.body.append(probe);
    const resolve = (variable: string, property: 'color' | 'background-color') => {
      probe.style.setProperty(property, `var(${variable})`);
      const value = getComputedStyle(probe).getPropertyValue(property);
      probe.style.cssText = '';
      const parts = value.match(/[\d.]+/g)?.map(Number) ?? [];
      return [parts[0] ?? 0, parts[1] ?? 0, parts[2] ?? 0, parts[3] ?? 1] as Rgba;
    };
    const composite = (foreground: Rgba, background: Rgba): Rgba => {
      const alpha = foreground[3] + background[3] * (1 - foreground[3]);
      return [
        (foreground[0] * foreground[3] + background[0] * background[3] * (1 - foreground[3])) / alpha,
        (foreground[1] * foreground[3] + background[1] * background[3] * (1 - foreground[3])) / alpha,
        (foreground[2] * foreground[3] + background[2] * background[3] * (1 - foreground[3])) / alpha,
        alpha,
      ];
    };
    const luminance = (color: Rgba) => {
      const channels = color.slice(0, 3).map((channel) => {
        const normalized = channel / 255;
        return normalized <= 0.03928
          ? normalized / 12.92
          : ((normalized + 0.055) / 1.055) ** 2.4;
      });
      return 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2];
    };
    const contrast = (foreground: Rgba, background: Rgba) => {
      const [lighter, darker] = [luminance(foreground), luminance(background)].sort((left, right) => right - left);
      return (lighter + 0.05) / (darker + 0.05);
    };
    const canvas = resolve('--ui-canvas', 'background-color');
    const surfaces = ['--ui-surface-1', '--ui-surface-2', '--ui-surface-3']
      .map((variable) => composite(resolve(variable, 'background-color'), canvas));
    const surface = surfaces[0];
    const minimumSurfaceContrast = (variable: string) => {
      const foreground = resolve(variable, 'color');
      return Math.min(...surfaces.map((background) => contrast(composite(foreground, background), background)));
    };
    const brand = resolve('--ui-brand', 'color');
    const brandSoft = composite(resolve('--ui-brand-soft', 'background-color'), surface);
    const semanticContrast = (name: string) => {
      const background = composite(resolve(`--ui-${name}-soft`, 'background-color'), surface);
      return contrast(composite(resolve(`--ui-${name}`, 'color'), background), background);
    };
    const values = {
      primary: minimumSurfaceContrast('--ui-text-primary'),
      secondary: minimumSurfaceContrast('--ui-text-secondary'),
      tertiary: minimumSurfaceContrast('--ui-text-tertiary'),
      focus: minimumSurfaceContrast('--ui-focus-ring'),
      brand: contrast(composite(brand, surface), surface),
      brandSoft: contrast(composite(brand, brandSoft), brandSoft),
      onBrand: contrast(composite(resolve('--ui-on-brand', 'color'), brand), brand),
      success: semanticContrast('success'),
      warning: semanticContrast('warning'),
      danger: semanticContrast('danger'),
      info: semanticContrast('info'),
    };
    probe.remove();
    return values;
  });
  for (const [name, ratio] of Object.entries(ratios)) {
    expect(ratio, `${name} contrast`).toBeGreaterThanOrEqual(name === 'focus' ? 3 : 4.5);
  }
}

async function dispatchResize(page: Page, handleSelector: string, deltaY: number, pointerId: number) {
  const handle = page.locator(handleSelector);
  await handle.dispatchEvent('pointerdown', { button: 0, clientY: 500, pointerId });
  await page.evaluate(({ deltaY, pointerId }) => {
    window.dispatchEvent(new PointerEvent('pointermove', { clientY: 500 + deltaY, pointerId }));
    window.dispatchEvent(new PointerEvent('pointerup', { clientY: 500 + deltaY, pointerId }));
  }, { deltaY, pointerId });
}

async function waitForSettledRender(page: Page) {
  await page.evaluate(() => new Promise<void>((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
  }));
}

async function openXrayDashboard(page: Page) {
  const program = page.locator('.program-item[data-program-id="xray-primary"]');
  const navigationToggle = page.locator('.mobile-nav-toggle');
  if (await navigationToggle.isVisible()) {
    if ((await navigationToggle.getAttribute('aria-expanded')) !== 'true') {
      await navigationToggle.click();
    }
    await expect(navigationToggle).toHaveAttribute('aria-expanded', 'true');
    await expect(program).toBeVisible();
  }
  await program.click();
  await expect(page.getByRole('heading', { name: 'Primary Xray routing fabric' })).toBeVisible();
  await page.locator('#program-tab-dashboard').click();
  await expect(page.locator('.xray-dashboard-panel')).toBeVisible();
  await expect(page.locator('.xray-runtime-api-grid > article')).toHaveCount(4);
  await waitForSettledRender(page);
}

async function expectDenseXrayLayout(
  page: Page,
  widthRange: { min: number; max?: number },
) {
  const layout = await page.locator('.xray-dashboard-panel').evaluate((panel) => {
    const rect = (element: Element) => element.getBoundingClientRect();
    const visibleChildren = (element: Element) => [...element.children]
      .filter((child) => {
        const bounds = rect(child);
        return bounds.width > 0 && bounds.height > 0 && getComputedStyle(child).position !== 'absolute';
      });
    const overlaps = (left: DOMRect, right: DOMRect) =>
      left.left < right.right - 1 &&
      left.right > right.left + 1 &&
      left.top < right.bottom - 1 &&
      left.bottom > right.top + 1;
    const overlapFailures: string[] = [];
    const parents = panel.querySelectorAll([
      '.xray-observatory-list article',
      '.xray-balancer-row',
      '.xray-dashboard-card',
      '.xray-runtime-api-grid > article',
      '.xray-online-user-list article',
    ].join(','));
    parents.forEach((parent, parentIndex) => {
      const children = visibleChildren(parent);
      for (let left = 0; left < children.length; left += 1) {
        for (let right = left + 1; right < children.length; right += 1) {
          if (overlaps(rect(children[left]), rect(children[right]))) {
            overlapFailures.push(`${parent.className || parent.tagName}:${parentIndex}`);
          }
        }
      }
    });

    const containmentFailures = [...panel.querySelectorAll<HTMLElement>([
      '.xray-observatory-list article',
      '.xray-balancer-row',
      '.xray-dashboard-card',
      '.xray-runtime-api-grid > article',
      '.xray-online-user-list article',
    ].join(','))].filter((element) => element.scrollWidth > element.clientWidth + 1)
      .map((element) => element.className || element.tagName);

    const rowCount = (selector: string) => {
      const tops = [...panel.querySelectorAll(selector)].map((element) => rect(element).top);
      return tops.reduce<number[]>((rows, top) => {
        if (!rows.some((candidate) => Math.abs(candidate - top) < 2)) rows.push(top);
        return rows;
      }, []).length;
    };
    const sideBlocks = [...panel.querySelectorAll('.xray-side-stack > .xray-dashboard-block')]
      .map((element) => rect(element));
    const logger = panel.querySelector('.xray-logger-control');
    const loggerDescription = logger?.querySelector(':scope > small');
    const loggerButton = logger?.querySelector(':scope > button');
    const tableScroll = panel.querySelector<HTMLElement>('.xray-table-scroll');
    return {
      width: rect(panel).width,
      overviewRows: rowCount('.xray-dashboard-overview > article'),
      telemetryRows: rowCount('.xray-runtime-telemetry > div'),
      runtimeRows: rowCount('.xray-runtime-api-grid > article'),
      userRows: rowCount('.xray-online-user-list > article'),
      sideRows: sideBlocks.length === 2 && Math.abs(sideBlocks[0].top - sideBlocks[1].top) < 2 ? 1 : 2,
      pairHandleDisplay: getComputedStyle(
        panel.querySelector('.xray-side-stack > .resize-separator')!,
      ).display,
      overlapFailures,
      containmentFailures,
      loggerOrderSafe: !!loggerDescription && !!loggerButton &&
        rect(loggerDescription).bottom <= rect(loggerButton).top + 1,
      tableHasInternalOverflow: !!tableScroll && tableScroll.scrollWidth > tableScroll.clientWidth + 1,
    };
  });

  expect(layout.width).toBeGreaterThanOrEqual(widthRange.min);
  if (widthRange.max !== undefined) expect(layout.width).toBeLessThan(widthRange.max);
  expect(layout.overlapFailures).toEqual([]);
  expect(layout.containmentFailures).toEqual([]);
  expect(layout.loggerOrderSafe).toBe(true);

  if (layout.width >= 1_120) {
    expect(layout).toMatchObject({
      overviewRows: 1,
      telemetryRows: 1,
      runtimeRows: 1,
      userRows: 3,
      sideRows: 1,
      pairHandleDisplay: 'block',
    });
  } else if (layout.width >= 920) {
    expect(layout).toMatchObject({
      overviewRows: 2,
      telemetryRows: 2,
      runtimeRows: 2,
      userRows: 4,
      sideRows: 1,
      pairHandleDisplay: 'block',
    });
  } else if (layout.width >= 600) {
    expect(layout).toMatchObject({
      overviewRows: 2,
      telemetryRows: 2,
      runtimeRows: 2,
      userRows: 4,
      sideRows: 2,
      pairHandleDisplay: 'none',
    });
  } else {
    expect(layout).toMatchObject({
      overviewRows: 5,
      telemetryRows: 3,
      runtimeRows: 4,
      userRows: 7,
      sideRows: 2,
      pairHandleDisplay: 'none',
      tableHasInternalOverflow: true,
    });
  }
}

async function expectCollapsedFilterToggleAlignment(page: Page) {
  await page.getByRole('button', { name: 'Collapse sidebar' }).click();
  const sidebar = page.locator('#primary-sidebar');
  const toggle = sidebar.getByRole('button', { name: 'Search and filter' });
  const defaultOffset = await toggle.evaluate((button) => {
    const buttonBounds = button.getBoundingClientRect();
    const iconBounds = button.querySelector<HTMLElement>('.sidebar-command-icon')!.getBoundingClientRect();
    return Math.abs((buttonBounds.left + buttonBounds.width / 2) - (iconBounds.left + iconBounds.width / 2));
  });
  expect(defaultOffset).toBeLessThanOrEqual(1);

  await toggle.click();
  await sidebar.getByRole('combobox', { name: 'Filter programs' }).selectOption('running');
  const filteredLayout = await toggle.evaluate((button) => {
    const buttonBounds = button.getBoundingClientRect();
    const iconBounds = button.querySelector<HTMLElement>('.sidebar-command-icon')!.getBoundingClientRect();
    const indicator = button.querySelector<HTMLElement>('.sidebar-filter-indicator')!;
    const indicatorBounds = indicator.getBoundingClientRect();
    return {
      iconOffset: Math.abs((buttonBounds.left + buttonBounds.width / 2) - (iconBounds.left + iconBounds.width / 2)),
      indicatorPosition: getComputedStyle(indicator).position,
      indicatorContained: indicatorBounds.top >= buttonBounds.top
        && indicatorBounds.right <= buttonBounds.right
        && indicatorBounds.bottom <= buttonBounds.bottom,
    };
  });
  expect(filteredLayout.iconOffset).toBeLessThanOrEqual(1);
  expect(filteredLayout.indicatorPosition).toBe('absolute');
  expect(filteredLayout.indicatorContained).toBe(true);
}

for (const theme of ['cupertino', 'material', 'aurora'] as const) {
  for (const colorMode of ['light', 'dark'] as const) {
    test(`${theme} ${colorMode} renders the shared workspace`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width: 1440, height: 940 });
      await openPreview(page, theme, colorMode);
      await expect(page.locator('html')).toHaveAttribute('data-ui-theme', theme);
      await expect(page.locator('html')).toHaveAttribute('data-ui-color-scheme', colorMode);
      await expectThemeSignature(page, theme, colorMode);
      await expectAccessibleThemeContrast(page);
      await expectAccessible(page);
      await expectNoViewportOverflow(page);
      await page.screenshot({ path: testInfo.outputPath(`${theme}-${colorMode}.png`), fullPage: true });
      await expectCollapsedFilterToggleAlignment(page);
    });
  }
}

test('first program selection responds immediately and coalesces repeated clicks', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 860 });
  await trackProgramSelectionRequests(page);
  await openPreview(
    page,
    'cupertino',
    'light',
    1.05,
    '&__ui_controlled_program_selection',
  );

  const program = page.locator('.program-item[data-program-id="xray-primary"]');
  await program.click();
  await expect(program).toHaveClass(/selection-pending/);
  await expect(program).toHaveAttribute('aria-busy', 'true');
  await expect(page.locator('main')).toHaveAttribute('aria-busy', 'true');
  const loadingPanel = page.locator('.program-loading-panel');
  await expect(loadingPanel).toBeVisible();
  await expect(loadingPanel.getByRole('status')).toContainText('Loading program details');

  await program.click();
  await expect.poll(async () => (
    (await programSelectionRequests(page))
      .map(({ command }) => command)
      .sort()
  )).toEqual([
    'get_program',
    'list_actions',
  ]);
  await expectAccessible(page);
  await expectNoViewportOverflow(page);

  await releaseProgramSelectionRequests(page, ['get_program', 'list_actions']);
  await expect(page.locator('.program-hero h1')).toHaveText('Primary Xray routing fabric');
  await expect(program).toHaveClass(/active/);
  await expect(program).not.toHaveClass(/selection-pending/);
  await expect(program).not.toHaveAttribute('aria-busy', 'true');
  await expect(page.getByText('Checking administrator access requirements')).toBeVisible();
  await expect.poll(async () => (
    (await programSelectionRequests(page))
      .map(({ command }) => command)
      .sort()
  )).toEqual([
    'get_program',
    'get_program_privilege_assessment',
    'list_actions',
  ]);
  await releaseProgramSelectionRequests(page, ['get_program_privilege_assessment']);
  await expect(page.getByText('This configuration can use standard user access')).toBeVisible();
  expect(await programSelectionRequests(page)).toHaveLength(3);
});

for (const theme of ['material', 'aurora'] as const) {
  test(`${theme} keeps program loading feedback reachable at compact scale`, async ({ page }) => {
    await page.setViewportSize({ width: 960, height: 640 });
    await openPreview(
      page,
      theme,
      'dark',
      1.3,
      '&__ui_controlled_program_selection',
    );

    const program = page.locator('.program-item[data-program-id="sing-box-edge"]');
    await program.click();
    const loadingPanel = page.locator('.program-loading-panel');
    await expect(loadingPanel).toBeVisible();
    const cancel = loadingPanel.getByRole('button', { name: 'Cancel' });
    await expect(cancel).toBeVisible();
    await expectNoViewportOverflow(page);
    await cancel.click();
    await releaseProgramSelectionRequests(page, ['get_program', 'list_actions']);
    await waitForSettledRender(page);
    await expect(page.getByRole('heading', { name: 'Workspace' })).toBeVisible();
    await expect(program).not.toHaveClass(/selection-pending/);
    await expect(page.locator('.program-detail-loading')).not.toBeAttached();
  });
}

for (const theme of ['cupertino', 'material', 'aurora'] as const) {
  test(`${theme} reorganizes without horizontal overflow`, async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 860 });
    await openPreview(page, theme, 'light', 1.3);
    const expandedFilterLayout = await page.locator('.sidebar-filter-panel').evaluate((element) => {
      const fields = [...element.querySelectorAll<HTMLElement>('.sidebar-field')]
        .map((field) => field.getBoundingClientRect());
      const textClearance = [...element.querySelectorAll<HTMLInputElement | HTMLSelectElement>('input, select')]
        .map((control) => {
          const style = getComputedStyle(control);
          const canvas = document.createElement('canvas');
          const context = canvas.getContext('2d');
          if (!context) return Number.NEGATIVE_INFINITY;
          context.font = `${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
          const text = control instanceof HTMLInputElement
            ? control.placeholder
            : control.selectedOptions[0]?.textContent?.trim() ?? '';
          const availableWidth = control.clientWidth
            - Number.parseFloat(style.paddingLeft)
            - Number.parseFloat(style.paddingRight);
          return availableWidth - context.measureText(text).width;
        });
      return {
        columns: getComputedStyle(element).gridTemplateColumns.split(/\s+/).filter(Boolean).length,
        widthDelta: fields.length === 2 ? Math.abs(fields[0].width - fields[1].width) : 0,
        minimumTextClearance: Math.min(...textClearance),
      };
    });
    expect(expandedFilterLayout.columns).toBe(1);
    expect(expandedFilterLayout.widthDelta).toBeLessThanOrEqual(1);
    expect(expandedFilterLayout.minimumTextClearance).toBeGreaterThanOrEqual(0);

    await page.getByRole('button', { name: 'Collapse sidebar' }).click();
    const collapsedSidebar = page.locator('#primary-sidebar');
    await expect(collapsedSidebar.locator('.bulk-mode-toggle')).toBeHidden();
    const collapsedIconAlignment = await collapsedSidebar.evaluate((element) => {
      const brand = element.querySelector<HTMLElement>('.sidebar-brand-mark')!.getBoundingClientRect();
      const program = element.querySelector<HTMLElement>('.sidebar-program-icon')!.getBoundingClientRect();
      return Math.abs((brand.left + brand.width / 2) - (program.left + program.width / 2));
    });
    expect(collapsedIconAlignment).toBeLessThanOrEqual(1);
    await collapsedSidebar.getByRole('button', { name: 'Search and filter' }).click();
    const collapsedFilterLayout = await collapsedSidebar.locator('.sidebar-filter-panel').evaluate((element) => {
      const fields = [...element.querySelectorAll<HTMLElement>('.sidebar-field')]
        .map((field) => field.getBoundingClientRect());
      return {
        columns: getComputedStyle(element).gridTemplateColumns.split(/\s+/).filter(Boolean).length,
        panelWidth: element.getBoundingClientRect().width,
        widthDelta: Math.abs(fields[0].width - fields[1].width),
      };
    });
    expect(collapsedFilterLayout.columns).toBe(1);
    expect(collapsedFilterLayout.panelWidth).toBeGreaterThanOrEqual(275);
    expect(collapsedFilterLayout.widthDelta).toBeLessThanOrEqual(1);
    await collapsedSidebar.getByRole('button', { name: 'Search and filter' }).click();

    for (const width of [1440, 1024, 640, 390]) {
      await page.setViewportSize({ width, height: 860 });
      await expectNoViewportOverflow(page);
      const navigationButton = page.getByRole('button', { name: 'Open navigation' });
      if (width < 900) {
        await expect(navigationButton).toBeVisible();
        await navigationButton.click();
        await expect(page.locator('.shell')).toHaveClass(/sidebar-open/);
        const sidebar = page.locator('#primary-sidebar');
        await expect(sidebar).toHaveAttribute('role', 'dialog');
        await expect(sidebar).toHaveAttribute('aria-modal', 'true');
        await expect(page.locator('main')).toHaveAttribute('inert', '');
        const firstSidebarControl = sidebar.locator('button:not([disabled]), input:not([disabled]), select:not([disabled])').first();
        const lastSidebarControl = sidebar.locator('button:not([disabled]), input:not([disabled]), select:not([disabled])').last();
        await expect(firstSidebarControl).toBeFocused();
        await lastSidebarControl.focus();
        await page.keyboard.press('Tab');
        await expect(firstSidebarControl).toBeFocused();
        await expect(sidebar.locator('.program-grip').first()).toHaveCSS('display', 'grid');
        await expect.poll(() => sidebar.locator('.program-grip i').first().evaluate(
          (element) => element.getBoundingClientRect().width,
        )).toBeGreaterThan(0);
        await expectNoViewportOverflow(page);
        await page.keyboard.press('Escape');
        await expect(navigationButton).toBeFocused();
        await expect(page.locator('.shell')).not.toHaveClass(/sidebar-open/);
        await expect(page.locator('main')).not.toHaveAttribute('inert');
        await expect(sidebar).toHaveAttribute('inert', '');
      } else {
        await expect(navigationButton).toBeHidden();
      }
    }
  });
}

for (const theme of ['cupertino', 'material', 'aurora'] as const) {
  test(`${theme} centers settings content within its workspace`, async ({ page }) => {
    await page.setViewportSize({ width: 1180, height: 860 });
    await openPreview(page, theme, 'light');
    await page.getByRole('button', { name: 'Settings' }).click();
    const geometry = await page.locator('.settings-content').evaluate((content) => {
      const pane = content.querySelector<HTMLElement>('.settings-pane')!;
      const contentBox = content.getBoundingClientRect();
      const paneBox = pane.getBoundingClientRect();
      return {
        centerDelta: Math.abs(
          (contentBox.left + contentBox.width / 2) - (paneBox.left + paneBox.width / 2),
        ),
        leftInset: paneBox.left - contentBox.left,
        rightInset: contentBox.right - paneBox.right,
      };
    });
    // Native scrollbar gutters may offset the visual box by half a scrollbar.
    expect(geometry.centerDelta).toBeLessThanOrEqual(8.1);
    expect(Math.abs(geometry.leftInset - geometry.rightInset)).toBeLessThanOrEqual(16.2);
  });
}

for (const theme of ['cupertino', 'material', 'aurora'] as const) {
  for (const colorMode of ['light', 'dark'] as const) {
    test(`${theme} ${colorMode} external settings actions do not flash autostart`, async ({ page }) => {
      await page.setViewportSize({ width: 1024, height: 768 });
      await trackPreviewExternalActions(page);
      await openPreview(page, theme, colorMode, 1.05, '&__ui_slow_external');
      await page.getByRole('button', { name: 'Settings' }).click();
      const dialog = page.getByRole('dialog', { name: 'Settings' });
      await dialog.getByRole('tab', { name: 'General' }).click();

      const autostart = dialog.getByRole('button', { name: /Start at login/ });
      const visualState = () => autostart.evaluate((button) => ({
        disabled: (button as HTMLButtonElement).disabled,
        opacity: getComputedStyle(button).opacity,
        pressed: button.getAttribute('aria-pressed'),
        switchEnabled: button.querySelector('.switch')?.classList.contains('enabled') ?? false,
      }));
      const initialState = await visualState();

      const applicationData = dialog.getByRole('button', { name: 'Application data' });
      await applicationData.evaluate((button) => {
        (button as HTMLButtonElement).click();
        (button as HTMLButtonElement).click();
      });
      await dialog.getByRole('button', { name: /Application logs/ }).click();

      await expect(autostart).toBeEnabled();
      expect(await visualState()).toEqual(initialState);
      await expect.poll(() => previewExternalActionCount(page, 'open_data_directory')).toBe(1);
      await expect.poll(() => previewExternalActionCount(page, 'open_app_log_directory')).toBe(1);
    });
  }
}

test('failed external settings actions remain actionable and retryable', async ({ page }) => {
  await page.setViewportSize({ width: 1024, height: 768 });
  await trackPreviewExternalActions(page);
  await openPreview(page, 'cupertino', 'light', 1.05, '&__ui_fail_external');
  await page.getByRole('button', { name: 'Settings' }).click();
  const dialog = page.getByRole('dialog', { name: 'Settings' });
  await dialog.getByRole('tab', { name: 'General' }).click();

  const applicationLogs = dialog.getByRole('button', { name: /Application logs/ });
  await applicationLogs.click();
  await expect(dialog.getByRole('alert')).toContainText('The preview external action failed');
  await expect.poll(() => previewExternalActionCount(page, 'open_app_log_directory')).toBe(1);

  await applicationLogs.click();
  await expect(dialog.getByRole('alert')).toBeHidden();
  await expect.poll(() => previewExternalActionCount(page, 'open_app_log_directory')).toBe(2);
});

test('license-required notifications dismiss automatically', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 860 });
  await page.goto('/?__ui_preview&__ui_unlicensed&__ui_license_required_error');
  await expect(page.getByRole('heading', { name: 'Workspace' })).toBeVisible();
  const notice = page.locator('.notification').filter({ hasText: 'License required' });
  await expect(notice).toBeVisible();
  await expect(notice).toBeHidden({ timeout: 8_500 });
});

test('license revalidation notices dismiss automatically without concealing recovery', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 860 });
  await openPreview(page, 'cupertino', 'light', 1.05, '&__ui_revalidation_notice');
  const notice = page.locator('.notification').filter({ hasText: 'License revalidation required' });
  await expect(notice).toBeVisible();
  await expect(notice).toBeHidden({ timeout: 8_500 });
});

test('a recovered license immediately clears its obsolete revalidation notice', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 860 });
  await openPreview(
    page,
    'cupertino',
    'light',
    1.05,
    '&__ui_revalidation_notice&__ui_revalidation_recovery',
  );
  const notice = page.locator('.notification').filter({ hasText: 'License revalidation required' });
  await expect(notice).toBeVisible();
  await expect(notice).toBeHidden({ timeout: 3_000 });
});

test('unlicensed lifecycle actions stay disabled in every program menu', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 860 });
  await page.goto('/?__ui_preview&__ui_unlicensed');
  await expect(page.getByRole('heading', { name: 'Workspace' })).toBeVisible();

  const program = page.locator('.program-item[data-program-id="local-agent"]');
  await program.click({ button: 'right' });
  const contextStart = page.getByRole('menu').getByRole('menuitem', { name: 'Start' });
  await expect(contextStart).toBeDisabled();
  await expect(contextStart).toHaveAttribute('title', 'Activate device to continue');

  await page.keyboard.press('Escape');
  const runningProgram = page.locator('.program-item[data-program-id="xray-primary"]');
  await runningProgram.click({ button: 'right' });
  const runningMenu = page.getByRole('menu');
  await expect(runningMenu.getByRole('menuitem', { name: 'Restart' })).toBeDisabled();
  await expect(runningMenu.getByRole('menuitem', { name: 'Stop', exact: true })).toBeEnabled();

  await page.keyboard.press('Escape');
  await program.click();
  const detailStart = page.getByRole('button', { name: 'Start', exact: true });
  await expect(detailStart).toBeDisabled();
  await expect(detailStart).toHaveAttribute('title', 'Activate device to continue');

  const directResults = await page.evaluate(async () => {
    const invoke = (window as typeof window & {
      __TAURI_INTERNALS__: {
        invoke: (command: string, args: Record<string, string>) => Promise<unknown>;
      };
    }).__TAURI_INTERNALS__.invoke;
    return Promise.all(['start_program', 'restart_program'].map(async (command) => {
      try {
        await invoke(command, { programId: 'local-agent' });
        return 'allowed';
      } catch (error) {
        return (error as { code?: string }).code ?? 'unknown';
      }
    }));
  });
  expect(directResults).toEqual(['LICENSE_REQUIRED', 'LICENSE_REQUIRED']);
});

test('program context menu separates identity and signals hover selection', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 860 });
  await page.goto('/?__ui_preview');
  await expect(page.getByRole('heading', { name: 'Workspace' })).toBeVisible();

  await page.locator('.program-item[data-program-id="sing-box-edge"]').click({ button: 'right' });
  const menu = page.getByRole('menu');
  const identity = menu.locator('.context-menu-copy');
  await expect(identity.getByText('Singapore edge gateway', { exact: true })).toBeVisible();
  await expect(identity.getByText('sing-box', { exact: true })).toBeVisible();
  expect(await identity.evaluate((element) => getComputedStyle(element).display)).toBe('grid');
  const identitySpacing = await identity.evaluate((element) => {
    const name = element.querySelector('strong')?.getBoundingClientRect();
    const kind = element.querySelector('small')?.getBoundingClientRect();
    return name && kind ? kind.top - name.bottom : -1;
  });
  expect(identitySpacing).toBeGreaterThanOrEqual(2);

  const action = menu.getByRole('menuitem', { name: 'Open working folder' });
  const restingAppearance = await action.evaluate((element) => {
    const style = getComputedStyle(element);
    return { backgroundColor: style.backgroundColor, transform: style.transform };
  });
  await action.hover();
  await expect.poll(() => action.evaluate((element) => getComputedStyle(element).transform)).not.toBe(restingAppearance.transform);
  await expect.poll(() => action.evaluate((element) => getComputedStyle(element).backgroundColor)).not.toBe(restingAppearance.backgroundColor);
});

test('payment information requests are localized, prefilled and resubmitted without layout overflow', async ({ page }, testInfo) => {
  await page.clock.install();
  await page.addInitScript(() => {
    const state = {
      requests: 0,
      submissions: [] as Array<Record<string, unknown>>,
      teamProfileRequests: 0,
    };
    (window as typeof window & { __billingPreview: typeof state }).__billingPreview = state;
    window.addEventListener('camellia-ui-preview:billing-request', () => { state.requests += 1; });
    window.addEventListener('camellia-ui-preview:team-profile-request', () => {
      state.teamProfileRequests += 1;
    });
    window.addEventListener('camellia-ui-preview:billing-submission', (event) => {
      state.submissions.push((event as CustomEvent<Record<string, unknown>>).detail);
    });
  });
  await page.setViewportSize({ width: 1180, height: 900 });
  await openPreview(page, 'cupertino', 'light', 1.05, '&__ui_billing_needs_information');
  await page.getByRole('button', { name: 'Settings' }).click();
  const dialog = page.locator('.settings-dialog');
  await dialog.getByRole('tab', { name: 'General' }).click();
  await dialog.getByRole('button', { name: 'Chinese' }).click();
  await dialog.getByRole('tab', { name: '许可证' }).click();

  await expect(dialog.getByText('补充付款信息并重新提交', { exact: true })).toBeVisible();
  await expect(dialog.getByText('需要补充信息', { exact: true })).toBeVisible();
  await expect(dialog.getByText('最近同步', { exact: false }).first()).toBeVisible();
  await expect(dialog.getByText('套餐策略', { exact: true })).toBeVisible();
  await expect(dialog.getByText('设备数量上限', { exact: true })).toBeVisible();
  await expect(dialog.getByText('成员数量上限', { exact: true })).toBeVisible();
  await expect(dialog.getByText(/Plan policy|Device limit|Member limit/)).toHaveCount(0);
  await expect(dialog.getByText('USD 19.99 · 专业版', { exact: true })).toBeVisible();
  await expect(dialog.getByText(/19\.99000000/)).toHaveCount(0);
  await expect(dialog.getByText('审核说明', { exact: true })).toBeVisible();
  await expect.poll(() => page.evaluate(() => (
    window as typeof window & { __billingPreview: { requests: number } }
  ).__billingPreview.requests)).toBe(1);
  expect(await page.evaluate(() => (
    window as typeof window & { __billingPreview: { teamProfileRequests: number } }
  ).__billingPreview.teamProfileRequests)).toBe(0);

  const invoice = dialog.getByLabel('账单');
  await expect(invoice).toHaveValue('invoice_billing_preview');
  await expect(invoice.locator('option:checked')).toHaveText('USD 19.99 · CNX-PAY_F76…D4E3C190');
  const invoiceOverflow = await invoice.evaluate((element) => element.scrollWidth - element.clientWidth);
  expect(invoiceOverflow).toBeLessThanOrEqual(1);

  const transaction = dialog.getByLabel('交易号或回执编号');
  const paidAt = dialog.getByLabel('付款完成时间');
  const payer = dialog.getByLabel('付款人姓名');
  const note = dialog.getByLabel('备注');
  await expect(transaction).toHaveValue('PREVIEW-RECEIPT-001');
  await expect(payer).toHaveValue('Camellia Test');
  await expect(note).toHaveValue('receipt identifier pending confirmation');
  await expect(paidAt).not.toHaveValue('');
  const [formBounds, transactionBounds, paidAtBounds, payerBounds] = await Promise.all([
    dialog.locator('.billing-payment-form').boundingBox(),
    transaction.boundingBox(),
    paidAt.boundingBox(),
    payer.boundingBox(),
  ]);
  expect(formBounds && transactionBounds && transactionBounds.width / formBounds.width).toBeGreaterThan(0.9);
  expect(paidAtBounds && payerBounds && Math.abs(paidAtBounds.y - payerBounds.y)).toBeLessThanOrEqual(2);

  const device = dialog.locator('.license-device-list article').first();
  const [stateBounds, removeBounds] = await Promise.all([
    device.locator('.device-state-pill').boundingBox(),
    device.getByRole('button', { name: '移除' }).boundingBox(),
  ]);
  expect(stateBounds && removeBounds && Math.abs(stateBounds.height - removeBounds.height)).toBeLessThanOrEqual(4);
  const licenseIdentifier = dialog.locator('[title="license_preview_001"]');
  const identifierLines = await licenseIdentifier.evaluate((element) => {
    const style = getComputedStyle(element);
    return element.getBoundingClientRect().height / Number.parseFloat(style.lineHeight);
  });
  expect(identifierLines).toBeLessThan(1.5);
  await dialog.locator('.billing-payment-form').scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath('billing-needs-information-wide.png'), fullPage: true });

  await page.setViewportSize({ width: 560, height: 900 });
  const compactFormLayout = await dialog.locator('.billing-payment-form').evaluate((form) => {
    const bounds = form.getBoundingClientRect();
    const controls = [...form.querySelectorAll<HTMLElement>('input, select, textarea')].map((control) => {
      const controlBounds = control.getBoundingClientRect();
      return {
        left: controlBounds.left,
        right: controlBounds.right,
        overflow: control.scrollWidth - control.clientWidth,
      };
    });
    return {
      columns: getComputedStyle(form).gridTemplateColumns.split(' ').length,
      left: bounds.left,
      right: bounds.right,
      overflow: form.scrollWidth - form.clientWidth,
      controls,
    };
  });
  expect(compactFormLayout.columns).toBe(1);
  expect(compactFormLayout.overflow).toBeLessThanOrEqual(1);
  expect(compactFormLayout.controls.every((control) => (
    control.left >= compactFormLayout.left - 1
    && control.right <= compactFormLayout.right + 1
    && control.overflow <= 1
  ))).toBe(true);
  await expectNoViewportOverflow(page);
  await dialog.locator('.billing-payment-form').scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath('billing-needs-information-compact.png'), fullPage: true });

  await page.clock.fastForward(60_000);
  await expect.poll(() => page.evaluate(() => (
    window as typeof window & { __billingPreview: { requests: number } }
  ).__billingPreview.requests)).toBe(2);
  await dialog.getByRole('tab', { name: '常规' }).click();
  await page.clock.fastForward(60_000);
  await expect.poll(() => page.evaluate(() => (
    window as typeof window & { __billingPreview: { requests: number } }
  ).__billingPreview.requests)).toBe(2);
  await dialog.getByRole('tab', { name: '许可证' }).click();
  await expect.poll(() => page.evaluate(() => (
    window as typeof window & { __billingPreview: { requests: number } }
  ).__billingPreview.requests)).toBe(3);
  expect(await page.evaluate(() => (
    window as typeof window & { __billingPreview: { teamProfileRequests: number } }
  ).__billingPreview.teamProfileRequests)).toBe(0);

  await dialog.getByRole('button', { name: '刷新账单' }).click();
  await expect.poll(() => page.evaluate(() => (
    window as typeof window & { __billingPreview: { requests: number } }
  ).__billingPreview.requests)).toBe(4);

  await note.fill('receipt identifier confirmed');
  await dialog.getByRole('button', { name: '重新提交审核' }).click();
  await expect(dialog.getByText('付款信息已提交', { exact: true })).toBeVisible();
  await expect(dialog.locator('.billing-payment-form')).toHaveCount(0);
  const submissions = await page.evaluate(() => (
    window as typeof window & { __billingPreview: { submissions: Array<Record<string, unknown>> } }
  ).__billingPreview.submissions);
  expect(submissions).toHaveLength(1);
  expect(submissions[0]).toMatchObject({
    invoiceId: 'invoice_billing_preview',
    paymentMethodId: 'payment_method_billing_preview',
    externalTransactionId: 'PREVIEW-RECEIPT-001',
    paidAmount: '19.99000000',
    paidAsset: 'USD',
    payerName: 'Camellia Test',
    note: 'receipt identifier confirmed',
  });
  expect(submissions[0].operationId).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
});

test('license data refreshes on regained attention without duplicate requests or layout shift', async ({ page }) => {
  await page.setViewportSize({ width: 560, height: 860 });
  await trackTeamWorkspaceRequests(page);
  await openPreview(
    page,
    'material',
    'light',
    1.05,
    '&__ui_team_cloud&__ui_slow_team&__ui_slow_license_refresh',
  );
  await page.getByRole('button', { name: 'Open navigation' }).click();
  const dialog = await openLicenseSettings(page);
  await page.evaluate(() => {
    window.dispatchEvent(new Event('online'));
    window.dispatchEvent(new Event('online'));
  });
  await expect(dialog.locator('.team-members article')).not.toHaveCount(0);
  const syncState = dialog.locator('.license-sync-state');
  await expect(syncState).toContainText('Last synced');

  const requestCounts = () => page.evaluate(() => {
    const state = (window as typeof window & {
      __teamWorkspaceRequests: {
        memberRequests: number;
        profileRequests: number;
        billingRequests: number;
      };
    }).__teamWorkspaceRequests;
    return {
      members: state.memberRequests,
      profile: state.profileRequests,
      billing: state.billingRequests,
    };
  });
  const initial = await requestCounts();
  expect(initial).toEqual({ members: 1, profile: 1, billing: 1 });
  const panelBounds = await dialog.locator('.license-panel').boundingBox();

  await page.waitForTimeout(5_100);
  await page.evaluate(() => {
    window.dispatchEvent(new Event('online'));
    window.dispatchEvent(new Event('online'));
  });
  await expect(syncState).toContainText('Syncing license data');
  await expect.poll(requestCounts).toEqual({
    members: initial.members + 1,
    profile: initial.profile + 1,
    billing: initial.billing + 1,
  });
  await expect(syncState).toContainText('Last synced');
  await page.waitForTimeout(250);
  expect(await requestCounts()).toEqual({
    members: initial.members + 1,
    profile: initial.profile + 1,
    billing: initial.billing + 1,
  });

  const refreshedBounds = await dialog.locator('.license-panel').boundingBox();
  expect(panelBounds && refreshedBounds && Math.abs(panelBounds.width - refreshedBounds.width))
    .toBeLessThanOrEqual(1);
  const controlOverflow = await dialog.locator('.license-panel').evaluate((panel) => (
    [...panel.querySelectorAll<HTMLElement>('button, input, select, textarea')]
      .map((control) => {
        const bounds = control.getBoundingClientRect();
        const owner = panel.getBoundingClientRect();
        return {
          overflow: Math.max(owner.left - bounds.left, bounds.right - owner.right, 0),
          visible: control.offsetParent !== null,
          scrollNavigation: !!control.closest('.workspace-view-tabs'),
        };
      })
  ));
  expect(Math.max(0, ...controlOverflow.filter((control) => (
    control.visible && !control.scrollNavigation
  )).map((control) => control.overflow)))
    .toBeLessThanOrEqual(1);
  const tabNavigation = await dialog.locator('.workspace-view-tabs').evaluate((navigation) => ({
    overflowX: getComputedStyle(navigation).overflowX,
    activeVisible: (() => {
      const active = navigation.querySelector<HTMLElement>('[aria-selected="true"]');
      if (!active) return false;
      const activeBounds = active.getBoundingClientRect();
      const navigationBounds = navigation.getBoundingClientRect();
      return activeBounds.left >= navigationBounds.left - 1
        && activeBounds.right <= navigationBounds.right + 1;
    })(),
  }));
  expect(tabNavigation.overflowX).toBe('auto');
  expect(tabNavigation.activeVisible).toBe(true);
});

test('matching license and Team timeouts are redacted, coalesced and dismissible', async ({ page }) => {
  await page.clock.install({ time: new Date('2026-07-19T00:00:00Z') });
  await page.setViewportSize({ width: 1180, height: 900 });
  await openPreview(page, 'cupertino', 'light', 1.05, '&__ui_team');
  const dialog = await openLicenseSettings(page);
  await expect(dialog.locator('.team-members article')).not.toHaveCount(0);
  await page.evaluate(() => {
    window.dispatchEvent(new Event('camellia-ui-preview:license-timeout-errors'));
  });

  await dialog.getByRole('button', { name: 'Refresh entitlement' }).click();
  await dialog.getByRole('button', { name: 'Refresh team' }).click();
  const timeoutNotices = dialog.locator('.error-notice').filter({ hasText: 'Operation timed out' });
  await expect(timeoutNotices).toHaveCount(1);
  await expect(timeoutNotices.getByText('Technical details')).toHaveCount(0);
  await timeoutNotices.hover();
  await page.clock.fastForward(12_100);
  await expect(timeoutNotices).toHaveCount(1);
  await page.mouse.move(0, 0);
  await page.clock.fastForward(12_100);
  await expect(timeoutNotices).toHaveCount(0);

  await dialog.getByRole('button', { name: 'Refresh entitlement' }).click();
  await dialog.getByRole('button', { name: 'Refresh team' }).click();
  await expect(timeoutNotices).toHaveCount(1);
  await timeoutNotices.getByRole('button', { name: 'Dismiss error' }).click();
  await expect(timeoutNotices).toHaveCount(0);
});

test('team invitation copy feedback is accessible and the secret clears when settings close', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.setViewportSize({ width: 1180, height: 900 });
  await openPreview(page, 'cupertino', 'light', 1.05, '&__ui_team');
  await page.getByRole('button', { name: 'Settings' }).click();
  let dialog = page.getByRole('dialog', { name: 'Settings' });
  await dialog.getByRole('tab', { name: 'License' }).click();
  await expect(dialog.getByRole('heading', { name: 'Team workspace' })).toBeVisible();

  const auditorMember = dialog.locator('.team-members article').filter({ hasText: 'Preview auditor' });
  const memberRole = auditorMember.getByRole('combobox', { name: 'Workspace role' });
  await expect(memberRole).toHaveValue('auditor');
  await expect(memberRole).toHaveAttribute('data-team-select', 'enum');
  await expect(memberRole).toHaveAttribute('data-control-size', 'md');
  await expect(memberRole).toHaveCSS('text-align', 'center');
  await expect(memberRole).toHaveCSS('align-items', 'center');
  await expect(memberRole).toHaveCSS('justify-content', 'center');
  const removedMember = dialog.locator('.team-members article').filter({ hasText: 'Former operator' });
  await expect(removedMember).toContainText('Removed');
  await expect(removedMember.getByRole('combobox')).toHaveCount(0);
  await expect(removedMember.getByRole('button')).toHaveCount(0);

  await dialog.getByRole('button', { name: 'Create invitation' }).click();
  const formError = dialog.getByRole('alert').filter({ hasText: 'Enter the member name and email address' });
  await expect(formError).toBeVisible();
  await expect(dialog.getByLabel('Member name')).toHaveAttribute('aria-describedby', 'team-form-error');

  await dialog.getByLabel('Member name').fill('Preview operator');
  await dialog.getByLabel('Email address').fill('operator-preview@example.test');
  await dialog.locator('.team-invite-form').getByLabel('Workspace role').selectOption('auditor');
  await dialog.getByRole('button', { name: 'Create invitation' }).click();

  const invitation = 'preview-invitation-token-0123456789abcdef';
  await expect(dialog.getByText(invitation, { exact: true })).toBeVisible();
  await dialog.getByRole('button', { name: 'Copy invitation token' }).click();
  await expect(dialog.getByRole('status').filter({ hasText: 'Invitation token copied' })).toBeVisible();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(invitation);
  await dialog.getByRole('button', { name: 'Dismiss', exact: true }).click();
  await expect(dialog.getByText(invitation, { exact: true })).toHaveCount(0);

  const invitedMember = dialog.locator('.team-members article').filter({ hasText: 'Preview operator' });
  await expect(invitedMember).toContainText('Invitation pending');
  await invitedMember.getByRole('button', { name: 'Revoke invitation' }).click();
  const revoke = page.getByRole('alertdialog', { name: 'Revoke pending invitation?' });
  await expect(revoke).toContainText('reserved seat will be released');
  await revoke.getByRole('button', { name: 'Revoke invitation' }).click();
  await expect(invitedMember).toContainText('Removed');
  await expect(invitedMember.getByRole('button')).toHaveCount(0);
  await expect(dialog.getByText(invitation, { exact: true })).toHaveCount(0);

  await dialog.getByRole('button', { name: 'Close' }).click();
  await page.getByRole('button', { name: 'Settings' }).click();
  dialog = page.getByRole('dialog', { name: 'Settings' });
  await dialog.getByRole('tab', { name: 'License' }).click();
  await expect(dialog.getByText(invitation, { exact: true })).toHaveCount(0);
});

test('Team invitation lost responses retry the same operation and recover the original secret', async ({ page }) => {
  await page.clock.install({ time: new Date('2026-07-19T00:00:00Z') });
  await page.setViewportSize({ width: 1180, height: 900 });
  await trackTeamWorkspaceRequests(page);
  await openPreview(page, 'cupertino', 'light', 1.05, '&__ui_team&__ui_team_lost_response');
  const dialog = await openLicenseSettings(page);
  await dialog.getByLabel('Member name').fill('Lost response member');
  await dialog.getByLabel('Email address').fill('lost-response@example.test');
  await dialog.getByRole('button', { name: 'Create invitation' }).click();

  await expect(dialog.getByRole('alert')).toContainText('Operation timed out');
  const retry = dialog.getByRole('button', { name: 'Retry same request' });
  await expect(retry).toBeVisible();
  await page.clock.fastForward(12_100);
  await expect(retry).toBeVisible();
  await retry.click();

  const invitation = 'preview-invitation-token-0123456789abcdef';
  await expect(dialog.getByText(invitation, { exact: true })).toBeVisible();
  await expect(
    dialog.locator('.team-members article').filter({ hasText: 'Lost response member' }),
  ).toHaveCount(1);
  const operations = await page.evaluate(() => (
    window as typeof window & {
      __teamWorkspaceRequests: {
        mutations: Array<{
          command: string;
          operationId: string;
          rowIdentity?: Record<string, unknown>;
        }>;
      };
    }
  ).__teamWorkspaceRequests.mutations.filter((entry) => entry.command === 'create_invitation'));
  expect(operations).toHaveLength(2);
  expect(operations[0].operationId).toBe(operations[1].operationId);
});

test('ownership transfer lost responses retry the exact row-version request', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 900 });
  await trackTeamWorkspaceRequests(page);
  await openPreview(page, 'cupertino', 'light', 1.05, '&__ui_team&__ui_team_lost_response');
  const dialog = await openLicenseSettings(page);
  const target = dialog.getByLabel('New workspace owner');
  await target.selectOption('member_admin_preview');
  await dialog.getByRole('button', { name: 'Transfer ownership', exact: true }).click();
  let confirmation = page.getByRole('alertdialog', { name: 'Transfer workspace ownership?' });
  await confirmation.getByRole('button', { name: 'Transfer ownership', exact: true }).click();

  await expect(dialog.getByRole('alert')).toContainText('Operation timed out');
  const retry = dialog.getByRole('button', { name: 'Retry same request' });
  await expect(retry).toBeVisible();
  await retry.click();

  await expect(dialog.locator('.team-summary').getByText('Administrator', { exact: true })).toBeVisible();
  const operations = await page.evaluate(() => (
    window as typeof window & {
      __teamWorkspaceRequests: {
        mutations: Array<{
          command: string;
          operationId: string;
          rowIdentity?: Record<string, unknown>;
        }>;
      };
    }
  ).__teamWorkspaceRequests.mutations.filter((entry) => entry.command === 'transfer_ownership'));
  expect(operations).toHaveLength(2);
  expect(operations[0].operationId).toBe(operations[1].operationId);
  expect(operations[0].rowIdentity).toEqual(operations[1].rowIdentity);
  expect(operations[0].rowIdentity).toEqual({
    newOwnerMemberId: 'member_admin_preview',
    ownerRowVersion: 1,
    newOwnerRowVersion: 2,
  });
});

test('Team member pages append without unbounded initial rendering', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 900 });
  await openPreview(page, 'cupertino', 'light', 1.05, '&__ui_team&__ui_team_pages');
  const dialog = await openLicenseSettings(page);
  await expect(dialog.locator('.team-members article')).toHaveCount(2);
  await expect(dialog.getByRole('status').filter({ hasText: '2 member records loaded' })).toBeVisible();
  await dialog.getByRole('button', { name: 'Load more members' }).click();
  await expect(dialog.locator('.team-members article')).toHaveCount(4);
  await expect(dialog.getByRole('status').filter({ hasText: '4 member records loaded' })).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Load more members' })).toHaveCount(0);
});

test('member device enrollment tokens are shown once and link an unbound device', async ({ page, context }, testInfo) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.setViewportSize({ width: 1180, height: 900 });
  await openPreview(page, 'cupertino', 'light', 1.05, '&__ui_team&__ui_slow_team');
  await page.getByRole('button', { name: 'Settings' }).click();
  let dialog = page.getByRole('dialog', { name: 'Settings' });
  await dialog.getByRole('tab', { name: 'License' }).click();

  const createTokenButton = dialog.locator('.team-device-enrollment-heading button');
  await createTokenButton.click();
  await expect(createTokenButton).toHaveText('Creating token');
  await expect(createTokenButton).toBeDisabled();
  const token = 'preview-device-enrollment-token-0123456789abcdef';
  await expect(dialog.getByText(token, { exact: true })).toBeVisible();
  await expect(dialog.getByText(/This token is shown only once/)).toBeVisible();
  await dialog.getByRole('button', { name: 'Copy device enrollment token' }).click();
  await expect(dialog.getByRole('status').filter({ hasText: 'Device enrollment token copied' })).toBeVisible();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(token);
  await dialog.getByRole('button', { name: 'Dismiss', exact: true }).click();
  await expect(dialog.getByText(token, { exact: true })).toHaveCount(0);
  await expect(createTokenButton).toBeEnabled();

  const zeroDeviceMember = dialog.locator('.team-members article').filter({
    hasText: 'Preview auditor',
  });
  await expect(zeroDeviceMember).toContainText('0 linked devices');
  await zeroDeviceMember.getByRole('button', { name: 'Create recovery token' }).click();
  const recoveryToken = 'preview-recovery-enrollment-token-0123456789abcdef';
  await expect(dialog.getByText(recoveryToken, { exact: true })).toBeVisible();
  await dialog.getByRole('button', { name: 'Dismiss', exact: true }).click();
  await expect(dialog.getByText(recoveryToken, { exact: true })).toHaveCount(0);

  await dialog.getByRole('button', { name: 'Close settings' }).click();
  await page.getByRole('button', { name: 'Settings' }).click();
  dialog = page.getByRole('dialog', { name: 'Settings' });
  await dialog.getByRole('tab', { name: 'License' }).click();
  await expect(dialog.getByText(token, { exact: true })).toHaveCount(0);

  await openPreview(page, 'cupertino', 'light', 1.05, '&__ui_team_unlinked&__ui_slow_team');
  await page.getByRole('button', { name: 'Settings' }).click();
  dialog = page.getByRole('dialog', { name: 'Settings' });
  await dialog.getByRole('tab', { name: 'License' }).click();
  const unlinkedActions = dialog.locator('.team-accept');
  await expect(unlinkedActions).toHaveCount(2);
  const unlinkedAlignment = await unlinkedActions.evaluateAll((forms) => forms.map((form) => {
    const input = form.querySelector('input')?.getBoundingClientRect();
    const button = form.querySelector('button[type="submit"]')?.getBoundingClientRect();
    const bounds = form.getBoundingClientRect();
    return {
      height: bounds.height,
      inputLeft: input?.left ?? 0,
      inputWidth: input?.width ?? 0,
      buttonLeft: button?.left ?? 0,
      buttonWidth: button?.width ?? 0,
      buttonHeight: button?.height ?? 0,
    };
  }));
  expect(Math.abs(unlinkedAlignment[0].height - unlinkedAlignment[1].height)).toBeLessThanOrEqual(1);
  expect(Math.abs(unlinkedAlignment[0].inputLeft - unlinkedAlignment[1].inputLeft)).toBeLessThanOrEqual(1);
  expect(Math.abs(unlinkedAlignment[0].inputWidth - unlinkedAlignment[1].inputWidth)).toBeLessThanOrEqual(1);
  expect(Math.abs(unlinkedAlignment[0].buttonLeft - unlinkedAlignment[1].buttonLeft)).toBeLessThanOrEqual(1);
  expect(Math.abs(unlinkedAlignment[0].buttonWidth - unlinkedAlignment[1].buttonWidth)).toBeLessThanOrEqual(1);
  expect(Math.abs(unlinkedAlignment[0].buttonHeight - unlinkedAlignment[1].buttonHeight)).toBeLessThanOrEqual(1);
  await dialog.locator('.team-workspace-panel').screenshot({
    path: testInfo.outputPath('team-unlinked-actions.png'),
  });
  const invitationInput = dialog.getByPlaceholder('Invitation token');
  await invitationInput.fill(token);
  await dialog.getByRole('button', { name: 'Join workspace' }).click();
  const invitationError = dialog.getByRole('alert').filter({ hasText: 'Invitation token not accepted' });
  await expect(invitationError).toContainText('Device enrollment tokens belong in Link device');
  await expect(invitationError.getByText('Technical details')).toHaveCount(0);
  await expect(dialog.getByText('Licensed', { exact: true }).first()).toBeVisible();
  await expect(unlinkedActions).toHaveCount(2);
  await expect(invitationInput).toHaveValue(token);
  await invitationError.getByRole('button', { name: 'Dismiss error' }).click();
  await expect(invitationError).toHaveCount(0);

  const enrollmentInput = dialog.getByLabel('Device enrollment token');
  await dialog.getByRole('button', { name: 'Link device' }).click();
  await expect(dialog.getByRole('alert').filter({ hasText: 'Enter a valid device enrollment token' })).toBeVisible();
  await enrollmentInput.fill(token);
  const linkButton = dialog.locator('.team-device-accept button[type="submit"]');
  await linkButton.click();
  await expect(linkButton).toHaveText('Linking device');
  await expect(linkButton).toBeDisabled();
  await expect(dialog.getByText('Preview auditor', { exact: true })).toBeVisible();
  await expect(dialog.getByText('Auditor', { exact: true })).toBeVisible();
  await expect(enrollmentInput).toHaveCount(0);
});

test('ownership transfer refreshes row versions after conflict and requires confirmation', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 900 });
  await openPreview(page, 'cupertino', 'light', 1.05, '&__ui_team_conflict&__ui_slow_team');
  await page.getByRole('button', { name: 'Settings' }).click();
  const dialog = page.getByRole('dialog', { name: 'Settings' });
  await dialog.getByRole('tab', { name: 'License' }).click();

  const invitationRole = dialog.locator('.team-invite-form').getByLabel('Workspace role');
  await invitationRole.selectOption('admin');
  const editableMemberRole = dialog.locator('.team-member-controls select[data-team-select="enum"]').first();
  await expect(editableMemberRole).toBeVisible();
  const enrollmentToken = 'preview-device-enrollment-token-0123456789abcdef';
  await dialog.getByRole('button', { name: 'Create device token' }).click();
  await expect(dialog.getByText(enrollmentToken, { exact: true })).toBeVisible();
  const target = dialog.getByLabel('New workspace owner');
  await expect(invitationRole).toHaveAttribute('data-team-select', 'enum');
  await expect(invitationRole).toHaveCSS('justify-content', 'center');
  await expect(target).toHaveAttribute('data-team-select', 'entity');
  await expect(target).toHaveCSS('text-align', 'start');
  await expect(target).toHaveCSS('appearance', 'base-select');
  await expect(target).toHaveCSS('justify-content', 'flex-start');
  const teamSelectTypography = await Promise.all([editableMemberRole, invitationRole, target].map(
    (select) => select.evaluate((element) => {
      const style = getComputedStyle(element);
      return { fontSize: style.fontSize, fontWeight: style.fontWeight };
    }),
  ));
  expect(new Set(teamSelectTypography.map(({ fontSize }) => fontSize)).size).toBe(1);
  expect(new Set(teamSelectTypography.map(({ fontWeight }) => fontWeight)).size).toBe(1);
  await target.selectOption('member_admin_preview');
  await dialog.getByRole('button', { name: 'Transfer ownership', exact: true }).click();
  let confirmation = page.getByRole('alertdialog', { name: 'Transfer workspace ownership?' });
  await expect(confirmation).toContainText('Preview administrator');
  await confirmation.getByRole('button', { name: 'Transfer ownership', exact: true }).click();
  await expect(dialog.getByRole('alert')).toContainText('Workspace changed');
  await expect(dialog.getByText(enrollmentToken, { exact: true })).toHaveCount(0);
  await expect(dialog.locator('.team-summary').getByText('Owner', { exact: true })).toBeVisible();

  await target.selectOption('member_admin_preview');
  await dialog.getByRole('button', { name: 'Transfer ownership', exact: true }).click();
  confirmation = page.getByRole('alertdialog', { name: 'Transfer workspace ownership?' });
  await confirmation.getByRole('button', { name: 'Transfer ownership', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Transferring ownership', exact: true })).toBeDisabled();
  await expect(dialog.locator('.team-summary').getByText('Administrator', { exact: true })).toBeVisible();
  await expect(invitationRole).toHaveValue('operator');
  await expect(invitationRole.locator('option[value="admin"]')).toHaveCount(0);
  await expect(dialog.getByRole('button', { name: 'Transfer ownership', exact: true })).toHaveCount(0);
  await expect(dialog.getByRole('button', { name: 'Leave workspace', exact: true })).toBeVisible();
});

test('a non-owner can leave the workspace and clears one-time secrets', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 900 });
  await openPreview(page, 'cupertino', 'light', 1.05, '&__ui_team_member&__ui_slow_team');
  await page.getByRole('button', { name: 'Settings' }).click();
  const dialog = page.getByRole('dialog', { name: 'Settings' });
  await dialog.getByRole('tab', { name: 'License' }).click();
  await expect(dialog.locator('.team-summary').getByText('Operator', { exact: true })).toBeVisible();

  await dialog.getByRole('button', { name: 'Create device token' }).click();
  const token = 'preview-device-enrollment-token-0123456789abcdef';
  await expect(dialog.getByText(token, { exact: true })).toBeVisible();
  await dialog.getByRole('button', { name: 'Leave workspace', exact: true }).click();
  const confirmation = page.getByRole('alertdialog', { name: 'Leave this workspace?' });
  await confirmation.getByRole('button', { name: 'Leave workspace', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Leaving workspace', exact: true })).toBeDisabled();

  await expect(dialog.getByRole('heading', { name: 'Team workspace' })).toHaveCount(0);
  await expect(dialog.getByText(token, { exact: true })).toHaveCount(0);
  await expect(dialog.getByText('Not activated', { exact: true }).first()).toBeVisible();
});

for (const role of ['operator', 'viewer', 'auditor'] as const) {
  test(`Team ${role} permissions expose only their authorized workspace controls`, async ({ page }) => {
    await page.setViewportSize({ width: 1180, height: 900 });
    await trackTeamWorkspaceRequests(page);
    await openPreview(page, 'cupertino', 'light', 1.05, `&__ui_team_role=${role}`);
    const dialog = await openLicenseSettings(page);
    await expect(dialog.locator('.team-summary')).toContainText(`Preview ${role}`);
    await expect(dialog.locator('.team-members article')).not.toHaveCount(0);
    await expect(dialog.locator('.team-member-controls')).toHaveCount(0);
    await expect(dialog.locator('.team-members article').getByRole('button')).toHaveCount(0);
    const requests = await page.evaluate(() => (
      window as typeof window & { __teamWorkspaceRequests: { memberRequests: number } }
    ).__teamWorkspaceRequests.memberRequests);
    expect(requests).toBeGreaterThan(0);

    if (role === 'operator') {
      await expect(dialog.getByRole('tab', { name: 'Audit log' })).toHaveCount(0);
      await expect(dialog.getByRole('tab', { name: 'Webhooks' })).toHaveCount(0);
      await dialog.getByRole('tab', { name: 'Shared configurations' }).click();
      await expect(dialog.getByRole('button', { name: 'New configuration' })).toBeVisible();
      await expect(dialog.getByRole('button', { name: 'Publish draft' })).toHaveCount(0);
      await dialog.getByRole('tab', { name: 'Sync activity' }).click();
      await expect(dialog.getByRole('button', { name: 'Advance checkpoint' })).toBeVisible();
      await dialog.getByRole('tab', { name: 'Alerts' }).click();
      await expect(dialog.getByRole('button', { name: 'New alert rule' })).toHaveCount(0);
      await expect(dialog.getByRole('button', { name: 'Acknowledge' })).toBeVisible();
    } else if (role === 'viewer') {
      await dialog.getByRole('tab', { name: 'Shared configurations' }).click();
      await expect(dialog.getByRole('button', { name: 'New configuration' })).toHaveCount(0);
      await expect(dialog.getByRole('button', { name: /Revise|Delete|Publish draft/ })).toHaveCount(0);
      await dialog.getByRole('tab', { name: 'Sync activity' }).click();
      await expect(dialog.getByRole('button', { name: 'Advance checkpoint' })).toHaveCount(0);
      await dialog.getByRole('tab', { name: 'Alerts' }).click();
      await expect(dialog.getByRole('button', { name: /New alert rule|Acknowledge|Resolve/ })).toHaveCount(0);
    } else {
      await expect(dialog.getByRole('tab', { name: 'Shared configurations' })).toHaveCount(0);
      await expect(dialog.getByRole('tab', { name: 'Sync activity' })).toHaveCount(0);
      await dialog.getByRole('tab', { name: 'Alerts' }).click();
      await expect(dialog.getByText('An obsolete shared configuration was deleted.', { exact: true })).toBeVisible();
      await expect(dialog.getByRole('button', { name: /Acknowledge|Resolve|New alert rule/ })).toHaveCount(0);
      await dialog.getByRole('tab', { name: 'Audit log' }).click();
      await expect(dialog.getByRole('button', { name: 'Export up to 5,000' })).toBeVisible();
      await dialog.getByRole('tab', { name: 'Webhooks' }).click();
      await expect(dialog.getByText('Delivery metadata', { exact: true })).toBeVisible();
      await expect(dialog.getByRole('heading', { name: 'Endpoints' })).toHaveCount(0);
      await expect(dialog.getByRole('button', { name: 'New endpoint' })).toHaveCount(0);
      await expect(dialog.getByText('https://events.example.test/camellia', { exact: true })).toHaveCount(0);
    }
  });
}

test('Team administrators cannot mutate peers or exceed their delegated role', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 900 });
  await openPreview(page, 'cupertino', 'light', 1.05, '&__ui_team_role=admin');
  const adminDialog = await openLicenseSettings(page);
  await expect(adminDialog.locator('.team-member-controls')).not.toHaveCount(0);
  const peerAdmin = adminDialog.locator('.team-members article').filter({ hasText: 'Preview administrator' });
  await expect(peerAdmin.getByRole('combobox')).toHaveCount(0);
  await expect(peerAdmin.getByRole('button')).toHaveCount(0);
  const adminInvitationRole = adminDialog.locator('.team-invite-form').getByLabel('Workspace role');
  await expect(adminInvitationRole.locator('option[value="admin"]')).toHaveCount(0);
  await expect(adminInvitationRole).toHaveValue('operator');
  await adminDialog.getByRole('tab', { name: 'Shared configurations' }).click();
  await expect(adminDialog.getByRole('button', { name: 'New configuration' })).toBeVisible();
  await expect(adminDialog.getByRole('button', { name: 'Publish draft' })).toBeVisible();
  await adminDialog.getByLabel('Show deleted').check();
  const adminDeletedConfiguration = adminDialog.locator('.shared-list article').filter({ hasText: 'Retired proxy profile' });
  await expect(adminDeletedConfiguration).toBeVisible();
  await expect(adminDeletedConfiguration.getByRole('button', { name: 'Permanently remove' })).toHaveCount(0);
  await adminDialog.getByRole('tab', { name: 'Webhooks' }).click();
  await expect(adminDialog.getByRole('button', { name: 'New endpoint' })).toBeVisible();
});

test('Team billing roles do not request or render member identities', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 900 });
  await trackTeamWorkspaceRequests(page);
  await openPreview(page, 'cupertino', 'light', 1.05, '&__ui_team_role=billing');
  const billingDialog = await openLicenseSettings(page);
  await expect(billingDialog.locator('.team-summary')).toContainText('Preview billing');
  await expect(billingDialog.locator('.team-members')).toHaveCount(0);
  await expect(billingDialog.getByRole('tab', { name: 'Shared configurations' })).toHaveCount(0);
  const billingRequests = await page.evaluate(() => (
    window as typeof window & { __teamWorkspaceRequests: { memberRequests: number } }
  ).__teamWorkspaceRequests.memberRequests);
  expect(billingRequests).toBe(0);
});

test('a Team role downgrade clears privileged views, one-time secrets and stale responses', async ({
  page,
}) => {
  await page.setViewportSize({ width: 1180, height: 900 });
  await openPreview(page, 'cupertino', 'light', 1.05, '&__ui_team_role=admin&__ui_slow_team');
  const dialog = await openLicenseSettings(page);
  await dialog.getByRole('tab', { name: 'Webhooks' }).click();
  const initialEndpoint = dialog.locator('.webhook-endpoints article').filter({
    hasText: 'Operations receiver',
  });
  await expect(initialEndpoint).toBeVisible();

  await dialog.getByRole('button', { name: 'New endpoint' }).click();
  await dialog.getByLabel('Endpoint name').fill('Temporary privileged receiver');
  await dialog.getByLabel('HTTPS URL').fill('https://security.example.test/events');
  await dialog.getByLabel('Alert incident opened').check();
  await dialog.getByRole('button', { name: 'Create endpoint' }).click();
  const secret = 'preview-webhook-secret-0123456789abcdef';
  await expect(dialog.getByText(secret, { exact: true })).toBeVisible();

  await dialog.getByRole('button', { name: 'Refresh', exact: true }).click();
  await page.evaluate(() => {
    window.dispatchEvent(new Event('camellia-ui-preview:team-role-downgrade'));
  });
  await dialog.getByRole('button', { name: 'Refresh team' }).click();

  await expect(dialog.getByRole('tab', { name: 'Webhooks' })).toHaveCount(0);
  await expect(dialog.getByRole('tab', { name: 'Members' })).toHaveAttribute('aria-selected', 'true');
  await expect(dialog.getByText(secret, { exact: true })).toHaveCount(0);
  await expect(initialEndpoint).toHaveCount(0);
  await expect(dialog.getByRole('button', { name: 'New endpoint' })).toHaveCount(0);
});

test('Team identities, controls and audit records remain bounded at wide and compact widths', async ({
  page,
}, testInfo) => {
  await page.setViewportSize({ width: 1024, height: 800 });
  await openPreview(
    page,
    'cupertino',
    'light',
    1.05,
    '&__ui_team_role=owner&__ui_team_long',
  );
  const dialog = await openLicenseSettings(page);
  const longMember = dialog.locator('.team-members article').filter({
    hasText: 'Preview auditor with an exceptionally long production identity',
  });
  await expect(longMember).toBeVisible();

  const expectTeamLayoutBounded = async () => {
    const overflow = await dialog.evaluate((element) => ({
      dialog: element.scrollWidth - element.clientWidth,
      panel: (() => {
        const panel = element.querySelector<HTMLElement>('.team-workspace-panel');
        return panel ? panel.scrollWidth - panel.clientWidth : 0;
      })(),
      members: [...element.querySelectorAll<HTMLElement>('.team-members article')]
        .map((member) => member.scrollWidth - member.clientWidth),
    }));
    expect(overflow.dialog).toBeLessThanOrEqual(1);
    expect(overflow.panel).toBeLessThanOrEqual(1);
    expect(Math.max(0, ...overflow.members)).toBeLessThanOrEqual(1);
  };

  await expectTeamLayoutBounded();
  const summaryIdentityLayout = await dialog.locator('.team-summary-identity').evaluate((value) => {
    const valueBounds = value.getBoundingClientRect();
    const cellBounds = value.parentElement?.getBoundingClientRect();
    const style = getComputedStyle(value);
    return {
      bounded: !!cellBounds && valueBounds.left >= cellBounds.left && valueBounds.right <= cellBounds.right + 1,
      overflow: style.overflow,
      textOverflow: style.textOverflow,
      whiteSpace: style.whiteSpace,
    };
  });
  expect(summaryIdentityLayout).toEqual({
    bounded: true,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  });
  const memberPillWidths = await dialog.locator('.team-member-status, .team-member-role').evaluateAll(
    (values) => values.map((value) => value.getBoundingClientRect().width),
  );
  expect(Math.max(...memberPillWidths)).toBeLessThan(180);
  const wideIdentityWidth = await longMember.locator('.team-member-identity').evaluate(
    (element) => element.getBoundingClientRect().width,
  );
  expect(wideIdentityWidth).toBeGreaterThanOrEqual(160);
  await longMember.scrollIntoViewIfNeeded();
  await longMember.screenshot({ path: testInfo.outputPath('team-members-wide.png') });
  await dialog.locator('.team-members').screenshot({ path: testInfo.outputPath('team-members-overview.png') });
  await page.setViewportSize({ width: 390, height: 700 });
  await longMember.scrollIntoViewIfNeeded();
  await expectTeamLayoutBounded();
  await longMember.screenshot({ path: testInfo.outputPath('team-members-compact.png') });

  await page.setViewportSize({ width: 1024, height: 800 });
  await dialog.getByRole('tab', { name: 'Audit log' }).click();
  await expect(dialog.locator('.audit-list article')).not.toHaveCount(0);
  const auditToolbarAlignment = await dialog.locator('.audit-toolbar').evaluate((toolbar) => {
    const actions = toolbar.querySelector<HTMLElement>('.audit-toolbar-actions');
    if (!actions) return Number.POSITIVE_INFINITY;
    const toolbarBox = toolbar.getBoundingClientRect();
    const actionsBox = actions.getBoundingClientRect();
    return Math.abs(
      toolbarBox.left + toolbarBox.width / 2 - (actionsBox.left + actionsBox.width / 2),
    );
  });
  expect(auditToolbarAlignment).toBeLessThanOrEqual(1);
  const auditOverflow = await dialog.locator('.audit-list').evaluate(
    (element) => element.scrollWidth - element.clientWidth,
  );
  expect(auditOverflow).toBeLessThanOrEqual(1);
  await dialog.locator('.audit-toolbar').screenshot({ path: testInfo.outputPath('team-audit-toolbar.png') });
  await dialog.locator('.audit-list').scrollIntoViewIfNeeded();
  await dialog.locator('.audit-list').screenshot({ path: testInfo.outputPath('team-audit-wide.png') });

  await dialog.getByRole('tab', { name: 'General' }).click();
  await dialog.getByRole('button', { name: 'Chinese' }).click();
  await dialog.getByRole('tab', { name: '许可证' }).click();
  await dialog.getByRole('tab', { name: '审计日志' }).click();
  await expect(dialog.locator('.audit-list').getByText('设备验证质询已签发', { exact: true })).toBeVisible();
  await expect(dialog.getByText('challenge_issued', { exact: true })).toBeHidden();
  await expectTeamLayoutBounded();
});

test('shared configuration retries preserve operation IDs and conflicts require a fresh review', async ({ page }) => {
  await page.clock.install({ time: new Date('2026-07-19T00:00:00Z') });
  await page.setViewportSize({ width: 1180, height: 900 });
  await trackTeamWorkspaceRequests(page);
  await openPreview(page, 'cupertino', 'light', 1.05, '&__ui_team_cloud&__ui_workspace_retry');
  let dialog = await openLicenseSettings(page);
  await dialog.getByRole('tab', { name: 'Shared configurations' }).click();
  await expect(dialog.getByText('Production edge routing', { exact: true })).toBeVisible();
  await dialog.getByRole('button', { name: 'New configuration' }).click();
  await dialog.getByLabel('Name').fill('Retry-safe configuration');
  const sharedProgramType = dialog.getByLabel('Program type');
  await expect(sharedProgramType).toHaveAttribute('data-team-select', 'enum');
  await expect(sharedProgramType).toHaveCSS('text-align', 'center');
  await expect(sharedProgramType).toHaveCSS('appearance', 'base-select');
  await expect(sharedProgramType).toHaveCSS('justify-content', 'center');
  await sharedProgramType.selectOption('mihomo');
  await dialog.getByLabel('Configuration content').fill('mode: rule\nlog-level: warn');
  await dialog.getByRole('button', { name: 'Create configuration' }).click();
  await expect(dialog.getByRole('alert')).toContainText('Network error');
  const workspaceErrorChrome = await dialog.locator('.workspace-error').evaluate((element) => {
    const style = getComputedStyle(element);
    return { borderWidth: style.borderTopWidth, padding: style.paddingTop };
  });
  expect(workspaceErrorChrome).toEqual({ borderWidth: '0px', padding: '0px' });
  await expect(dialog.getByRole('button', { name: 'Retry same request' })).toBeVisible();
  await page.clock.fastForward(12_100);
  await expect(dialog.getByRole('alert')).toContainText('Network error');
  await expect(dialog.getByRole('button', { name: 'Retry same request' })).toBeVisible();
  await dialog.getByRole('button', { name: 'Retry same request' }).click();
  await expect(dialog.getByText('Retry-safe configuration', { exact: true })).toBeVisible();
  await expect(dialog.locator('.shared-list article').filter({ hasText: 'Retry-safe configuration' })).toContainText('Mihomo');
  const retryMutations = await page.evaluate(() => (
    window as typeof window & {
      __teamWorkspaceRequests: { mutations: Array<{ command: string; operationId: string }> };
    }
  ).__teamWorkspaceRequests.mutations.filter((entry) => entry.command === 'create_license_workspace_configuration'));
  expect(retryMutations).toHaveLength(2);
  expect(retryMutations[0].operationId).toBe(retryMutations[1].operationId);

  await openPreview(page, 'cupertino', 'light', 1.05, '&__ui_team_cloud&__ui_workspace_conflict');
  dialog = await openLicenseSettings(page);
  await dialog.getByRole('tab', { name: 'Shared configurations' }).click();
  const configuration = dialog.locator('.shared-list article').filter({ hasText: 'Production edge routing' });
  await configuration.getByRole('button', { name: 'Revise' }).click();
  await dialog.locator('.shared-form').getByLabel('Name').fill('Stale revision must not win');
  await dialog.getByRole('button', { name: 'Save revision' }).click();
  await expect(dialog.getByRole('alert')).toContainText('Workspace changed');
  await expect(dialog.locator('.shared-form')).toHaveCount(0);
  await expect(dialog.getByText('Stale revision must not win', { exact: true })).toHaveCount(0);

  await configuration.getByRole('button', { name: 'Revise' }).click();
  await dialog.locator('.shared-form').getByLabel('Name').fill('Reviewed fresh revision');
  await dialog.getByRole('button', { name: 'Save revision' }).click();
  await expect(dialog.getByText('Reviewed fresh revision', { exact: true })).toBeVisible();
});

test('shared configuration lifecycle, trusted retention and sync checkpoint stay service-authoritative', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1180, height: 920 });
  await openPreview(page, 'cupertino', 'light', 1.05, '&__ui_team_cloud');
  const dialog = await openLicenseSettings(page);
  await dialog.getByRole('tab', { name: 'Shared configurations' }).click();
  const sharedToolbar = dialog.locator('#team-workspace-shared .cloud-toolbar');
  const sharedToolbarLayout = await sharedToolbar.evaluate((element) => {
    const heading = element.querySelector<HTMLElement>('.shared-toolbar-heading')!.getBoundingClientRect();
    const copy = element.querySelector<HTMLElement>('.shared-toolbar-copy')!.getBoundingClientRect();
    const actions = [...element.querySelectorAll<HTMLElement>('.shared-toolbar-actions > button')]
      .map((control) => control.getBoundingClientRect());
    const context = element.querySelector<HTMLElement>('.shared-toolbar-context')!.getBoundingClientRect();
    const filter = element.querySelector<HTMLElement>('.shared-filter-toggle')!.getBoundingClientRect();
    const usage = element.querySelector<HTMLElement>('.shared-usage-summary')!.getBoundingClientRect();
    return {
      overflow: element.scrollWidth - element.clientWidth,
      headingAligned: Math.abs(copy.top + copy.height / 2 - actions[0].top - actions[0].height / 2),
      actionHeightDelta: Math.max(...actions.map(({ height }) => height))
        - Math.min(...actions.map(({ height }) => height)),
      actionsNaturalWidth: Math.abs(actions[0].width - actions[1].width),
      contextBelowHeading: context.top >= heading.bottom,
      filterBeforeUsage: filter.left < usage.left,
      filterHasButtonChrome: getComputedStyle(element.querySelector<HTMLElement>('.shared-filter-toggle')!).borderTopStyle !== 'none',
    };
  });
  expect(sharedToolbarLayout.overflow).toBeLessThanOrEqual(1);
  expect(sharedToolbarLayout.headingAligned).toBeLessThanOrEqual(1);
  expect(sharedToolbarLayout.actionHeightDelta).toBeLessThanOrEqual(1);
  expect(sharedToolbarLayout.actionsNaturalWidth).toBeGreaterThan(1);
  expect(sharedToolbarLayout.contextBelowHeading).toBe(true);
  expect(sharedToolbarLayout.filterBeforeUsage).toBe(true);
  expect(sharedToolbarLayout.filterHasButtonChrome).toBe(false);
  await sharedToolbar.screenshot({
    path: testInfo.outputPath('team-shared-toolbar.png'),
  });

  let configuration = dialog.locator('.shared-list article').filter({ hasText: 'Production edge routing' });
  await configuration.getByRole('button', { name: 'View' }).click();
  const content = dialog.getByLabel('Shared configuration content');
  await expect(content).toContainText('proxy-sg');
  await content.getByRole('button', { name: 'Close' }).click();

  const exportPromise = page.waitForEvent('download');
  await configuration.getByRole('button', { name: 'Export' }).click();
  expect((await exportPromise).suggestedFilename()).toBe('Production-edge-routing-r2.json');

  await configuration.getByRole('button', { name: 'Publish draft' }).click();
  await expect(configuration).toContainText('Published revision 2');

  await dialog.getByLabel('Show deleted').check();
  await configuration.getByRole('button', { name: 'Delete' }).click();
  await page.getByRole('alertdialog', { name: 'Delete shared configuration?' })
    .getByRole('button', { name: 'Delete configuration' }).click();
  configuration = dialog.locator('.shared-list article').filter({ hasText: 'Production edge routing' });
  await expect(configuration).toContainText('Deleted');
  await expect(configuration.getByRole('button', { name: 'Permanently remove' })).toBeVisible();

  await configuration.getByRole('button', { name: 'Permanently remove' }).click();
  let confirmation = page.getByRole('alertdialog', { name: 'Permanently remove shared configuration?' });
  await expect(confirmation).toContainText('trusted 30-day recovery period');
  await confirmation.getByRole('button', { name: 'Permanently remove', exact: true }).click();
  await expect(dialog.getByRole('alert')).toContainText('Recovery period still active');
  await expect(dialog.getByRole('button', { name: 'Retry same request' })).toHaveCount(0);
  await expect(configuration).toBeVisible();

  await configuration.getByRole('button', { name: 'Restore' }).click();
  await expect(configuration.getByText('Deleted', { exact: true })).toHaveCount(0);

  const eligible = dialog.locator('.shared-list article').filter({ hasText: 'Retired proxy profile' });
  await eligible.getByRole('button', { name: 'Permanently remove' }).click();
  confirmation = page.getByRole('alertdialog', { name: 'Permanently remove shared configuration?' });
  await confirmation.getByRole('button', { name: 'Permanently remove', exact: true }).click();
  await expect(eligible).toHaveCount(0);

  await dialog.getByRole('tab', { name: 'Sync activity' }).click();
  const checkpointValues = dialog.locator('.checkpoint-summary > div strong');
  await expect(checkpointValues.nth(0)).toHaveText('10');
  await expect(checkpointValues.nth(1)).toHaveText('12');
  const advance = dialog.getByRole('button', { name: 'Advance checkpoint' });
  await advance.click();
  await expect(checkpointValues.nth(0)).toHaveText('12');
  await expect(advance).toBeDisabled();
  await expect(dialog.getByText('This device is caught up. No changes follow its checkpoint')).toBeVisible();
});

test('alerts, bounded audit export and webhook secret lifecycle remain complete', async ({ page, context }, testInfo) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.setViewportSize({ width: 1180, height: 920 });
  await trackTeamWorkspaceRequests(page);
  await openPreview(page, 'cupertino', 'dark', 1.05, '&__ui_team_cloud');
  const dialog = await openLicenseSettings(page);

  await dialog.getByRole('tab', { name: 'Alerts' }).click();
  await expect(dialog.getByText('Critical sync conflicts', { exact: true })).toBeVisible();
  const incidentFilter = dialog.locator('.incidents-section .compact-filter select');
  await expect(incidentFilter).toBeVisible();
  await expect(incidentFilter).toHaveAttribute('data-team-select', 'enum');
  await expect(incidentFilter).toHaveCSS('text-align', 'center');
  await expect(incidentFilter).toHaveCSS('justify-content', 'center');
  await dialog.locator('.incidents-section > header').screenshot({
    path: testInfo.outputPath('team-incident-filter.png'),
  });
  await dialog.getByRole('button', { name: 'New alert rule' }).click();
  await expect(dialog.getByLabel('Event kind')).toHaveAttribute('data-team-select', 'enum');
  await expect(dialog.getByLabel('Severity')).toHaveAttribute('data-team-select', 'enum');
  await dialog.getByLabel('Rule name').fill('Quota review');
  await dialog.getByLabel('Event kind').selectOption('quota_warning');
  await dialog.getByLabel('Severity').selectOption('warning');
  await dialog.getByRole('button', { name: 'Create rule' }).click();
  let rule = dialog.locator('.resource-list article').filter({ hasText: 'Quota review' });
  await expect(rule).toBeVisible();
  await rule.getByRole('button', { name: 'Edit' }).click();
  await dialog.getByLabel('Rule name').fill('Quota review updated');
  await dialog.getByRole('button', { name: 'Save rule' }).click();
  rule = dialog.locator('.resource-list article').filter({ hasText: 'Quota review updated' });
  await expect(rule).toBeVisible();
  await rule.getByRole('button', { name: 'Delete' }).click();
  await page.getByRole('alertdialog', { name: 'Delete alert rule?' }).getByRole('button', { name: 'Delete alert rule' }).click();
  await expect(rule).toHaveCount(0);

  const openIncident = dialog.locator('.incident-list article').filter({ hasText: 'concurrent revision' });
  await openIncident.getByRole('button', { name: 'Acknowledge' }).click();
  await expect(openIncident).toContainText('Acknowledged');
  const acknowledgedIncident = dialog.locator('.incident-list article').filter({ hasText: 'storage is above' });
  await acknowledgedIncident.getByRole('button', { name: 'Resolve' }).click();
  await page.getByRole('alertdialog', { name: 'Resolve this incident?' }).getByRole('button', { name: 'Resolve incident' }).click();
  await expect(acknowledgedIncident).toHaveCount(0);

  await dialog.getByRole('tab', { name: 'Audit log' }).click();
  const auditEventFilter = dialog.getByLabel('Event type');
  await expect(auditEventFilter).toHaveAttribute('data-team-select', 'entity');
  await expect(auditEventFilter.locator('option')).toHaveCount(3);
  await auditEventFilter.selectOption('workspace_configuration_revised');
  await dialog.getByRole('button', { name: 'Apply filter' }).click();
  const revisedAuditEvent = dialog.locator('.audit-list article').filter({
    hasText: 'Configuration revised',
  });
  await expect(revisedAuditEvent).toBeVisible();
  await expect(dialog.locator('.audit-list article')).toHaveCount(1);
  await expect(revisedAuditEvent.getByText('workspace_configuration_revised', { exact: true }))
    .toBeHidden();
  await revisedAuditEvent.getByText('Details', { exact: true }).click();
  await expect(revisedAuditEvent.getByText('workspace_configuration_revised', { exact: true }))
    .toBeVisible();
  await auditEventFilter.selectOption('');
  await dialog.getByRole('button', { name: 'Apply filter' }).click();
  const challengeAuditEvent = dialog.locator('.audit-list article').filter({
    hasText: 'Device verification challenge issued',
  });
  await expect(challengeAuditEvent).toBeVisible();
  await expect(challengeAuditEvent.getByText('challenge_issued', { exact: true })).toBeHidden();
  await challengeAuditEvent.getByText('Details', { exact: true }).click();
  await expect(challengeAuditEvent.getByText('challenge_issued', { exact: true })).toBeVisible();
  const downloadPromise = page.waitForEvent('download');
  await dialog.getByRole('button', { name: 'Export up to 5,000' }).click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toMatch(/^camellia-nexus-audit-\d{4}-\d{2}-\d{2}\.json$/);
  await expect(dialog.getByRole('status').filter({ hasText: 'audit events' })).toContainText('2');
  expect(await page.evaluate(() => (
    window as typeof window & { __teamWorkspaceRequests: { auditExportLimits: number[] } }
  ).__teamWorkspaceRequests.auditExportLimits)).toEqual([5_000]);

  await dialog.getByRole('tab', { name: 'Webhooks' }).click();
  await expect(dialog.locator('.webhook-endpoints article').getByText('Operations receiver', { exact: true })).toBeVisible();
  await expect(dialog.getByText('Delivery metadata', { exact: true })).toBeVisible();
  // Native select names include their rendered option text in some engines,
  // so locate the explicitly classed filter instead of matching a translated
  // accessible name exactly.
  const endpointFilter = dialog.locator('.delivery-filter select');
  await expect(endpointFilter).toBeVisible();
  await expect(endpointFilter).toHaveAttribute('data-team-select', 'entity');
  await expect(endpointFilter).toHaveCSS('text-align', 'start');
  await expect(endpointFilter).toHaveCSS('justify-content', 'flex-start');
  await dialog.locator('.deliveries-section > header').screenshot({
    path: testInfo.outputPath('team-webhook-delivery-filter.png'),
  });
  await dialog.getByRole('button', { name: 'New endpoint' }).click();
  await dialog.getByLabel('Endpoint name').fill('Security receiver');
  await dialog.getByLabel('HTTPS URL').fill('https://security.example.test/events');
  await dialog.getByLabel('Alert incident opened').check();
  await dialog.getByRole('button', { name: 'Create endpoint' }).click();
  const firstSecret = 'preview-webhook-secret-0123456789abcdef';
  await expect(dialog.getByText(firstSecret, { exact: true })).toBeVisible();
  await dialog.getByRole('button', { name: 'Copy webhook secret' }).click();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(firstSecret);
  await dialog.getByRole('button', { name: 'Dismiss secret' }).click();
  await expect(dialog.getByText(firstSecret, { exact: true })).toHaveCount(0);

  let endpoint = dialog.locator('.webhook-endpoints article').filter({ hasText: 'Security receiver' });
  await endpoint.getByRole('button', { name: 'Edit' }).click();
  await dialog.getByLabel('Endpoint name').fill('Security receiver updated');
  await dialog.getByRole('button', { name: 'Save endpoint' }).click();
  endpoint = dialog.locator('.webhook-endpoints article').filter({ hasText: 'Security receiver updated' });
  await endpoint.getByRole('button', { name: 'Rotate secret' }).click();
  await page.getByRole('alertdialog', { name: 'Rotate webhook secret?' }).getByRole('button', { name: 'Rotate secret' }).click();
  const rotatedSecret = 'preview-webhook-rotated-secret-0123456789abcdef';
  await expect(dialog.getByText(rotatedSecret, { exact: true })).toBeVisible();
  await dialog.getByRole('button', { name: 'Dismiss secret' }).click();
  await endpoint.getByRole('button', { name: 'Delete' }).click();
  await page.getByRole('alertdialog', { name: 'Delete webhook endpoint?' }).getByRole('button', { name: 'Delete endpoint' }).click();
  await expect(endpoint).toHaveCount(0);
  await endpointFilter.selectOption('webhook_endpoint_preview');
  const defaultEndpoint = dialog.locator('.webhook-endpoints article').filter({ hasText: 'Operations receiver' });
  await defaultEndpoint.getByRole('button', { name: 'Delete' }).click();
  await page.getByRole('alertdialog', { name: 'Delete webhook endpoint?' }).getByRole('button', { name: 'Delete endpoint' }).click();
  await expect(defaultEndpoint).toHaveCount(0);
  await expect(endpointFilter).toHaveCount(0);
  await expect(dialog.getByText('No webhook endpoints have been configured', { exact: true })).toBeVisible();
});

for (const theme of ['cupertino', 'material', 'aurora'] as const) {
  for (const colorMode of ['light', 'dark'] as const) {
    test(`${theme} ${colorMode} keeps Team cloud controls inside the settings workspace`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width: 1024, height: 800 });
      await openPreview(page, theme, colorMode, 1.05, '&__ui_team_cloud');
      const dialog = await openLicenseSettings(page);
      const licenseEmblem = dialog.locator('.license-orb');
      await expect(licenseEmblem.locator('svg')).toBeVisible();
      await expect(licenseEmblem.locator('b')).toHaveCount(0);
      await licenseEmblem.screenshot({ path: testInfo.outputPath('license-emblem.png') });
      await dialog.getByRole('tab', { name: 'Shared configurations' }).click();
      await expect(dialog.getByText('Production edge routing', { exact: true })).toBeVisible();
      await expect(page.locator('html')).toHaveAttribute('data-ui-theme', theme);
      await expect(page.locator('html')).toHaveAttribute('data-ui-color-scheme', colorMode);
      await expectAccessibleThemeContrast(page);
      await expectAccessible(page, '.settings-dialog');
      const overflow = await dialog.evaluate((element) => {
        const panel = element.querySelector<HTMLElement>('.team-workspace-panel');
        const cloud = element.querySelector<HTMLElement>('.cloud-panel');
        return {
          dialog: element.scrollWidth - element.clientWidth,
          panel: panel ? panel.scrollWidth - panel.clientWidth : 0,
          cloud: cloud ? cloud.scrollWidth - cloud.clientWidth : 0,
        };
      });
      expect(overflow.dialog).toBeLessThanOrEqual(1);
      expect(overflow.panel).toBeLessThanOrEqual(1);
      expect(overflow.cloud).toBeLessThanOrEqual(1);
    });
  }
}

test('Team cloud navigation remains keyboard reachable at compact width and reduced motion', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.setViewportSize({ width: 390, height: 680 });
  await openPreview(page, 'material', 'dark', 1.05, '&__ui_team_cloud&__ui_slow_team');
  await page.getByRole('button', { name: 'Open navigation' }).click();
  await page.getByRole('button', { name: 'Settings' }).click();
  const dialog = page.getByRole('dialog', { name: 'Settings' });
  await dialog.getByRole('tab', { name: 'License' }).click();
  const membersTab = dialog.getByRole('tab', { name: 'Members' });
  await membersTab.focus();
  await page.keyboard.press('ArrowRight');
  await expect(dialog.getByRole('tab', { name: 'Shared configurations' })).toHaveAttribute('aria-selected', 'true');
  await expect(dialog.getByText('Loading shared configurations…', { exact: true })).toBeVisible();
  await expect(dialog.getByText('Production edge routing', { exact: true })).toBeVisible();
  await dialog.getByRole('tab', { name: 'Webhooks' }).click();
  await expect(dialog.getByText('Loading webhooks…', { exact: true })).toBeVisible();
  await expect(dialog.locator('.webhook-endpoints article').getByText('Operations receiver', { exact: true })).toBeVisible();
  const overflow = await dialog.evaluate((element) => {
    const panel = element.querySelector<HTMLElement>('.team-workspace-panel');
    const cloud = element.querySelector<HTMLElement>('.cloud-panel');
    return {
      dialog: element.scrollWidth - element.clientWidth,
      panel: panel ? panel.scrollWidth - panel.clientWidth : 0,
      cloud: cloud ? cloud.scrollWidth - cloud.clientWidth : 0,
    };
  });
  expect(overflow.dialog).toBeLessThanOrEqual(1);
  expect(overflow.panel).toBeLessThanOrEqual(1);
  expect(overflow.cloud).toBeLessThanOrEqual(1);
  await expectNoViewportOverflow(page);
});

test('license fallback values retain the settings typeface', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 860 });
  await page.goto('/?__ui_preview&__ui_unlicensed');
  await expect(page.getByRole('heading', { name: 'Workspace' })).toBeVisible();
  await page.getByRole('button', { name: 'Settings' }).click();
  const dialog = page.getByRole('dialog', { name: 'Settings' });
  await dialog.getByRole('tab', { name: 'License' }).click();
  const licenseCard = dialog.locator('.license-card').first();
  const planFallback = licenseCard.locator('dl > div').filter({ hasText: 'Plan' }).locator('dd');
  const idFallback = licenseCard.locator('dl > div').filter({ hasText: 'License ID' }).locator('dd');
  await expect(idFallback).toHaveText('Not available');
  const fonts = await Promise.all([
    planFallback.evaluate((element) => getComputedStyle(element).fontFamily),
    idFallback.evaluate((element) => getComputedStyle(element).fontFamily),
  ]);
  expect(fonts[1]).toBe(fonts[0]);
});

test('a removed device can reactivate its existing license or explicitly replace its identity', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 900 });
  await openPreview(page, 'cupertino', 'light', 1.05, '&__ui_removed_license');
  await page.getByRole('button', { name: 'Settings' }).click();
  const dialog = page.getByRole('dialog', { name: 'Settings' });
  await dialog.getByRole('tab', { name: 'License' }).click();
  await expect(dialog.getByText(/Reactivate this removed device/)).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Activate device' })).toBeEnabled();
  await expect(dialog.getByRole('button', { name: 'Reconnect registered device' })).toHaveCount(0);
  await expect(dialog.getByRole('button', { name: 'Use another license' })).toBeEnabled();
});

test('system appearance follows operating-system changes and reduced motion', async ({ page }) => {
  await page.emulateMedia({ colorScheme: 'dark', reducedMotion: 'reduce' });
  await page.addInitScript(() => {
    localStorage.setItem('camellia-nexus.appearance.v3', JSON.stringify({
      version: 3,
      theme: 'aurora',
      colorMode: 'system',
      scale: 1.05,
    }));
  });
  await page.goto('/?__ui_preview');
  await expect(page.getByRole('heading', { name: 'Workspace' })).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-ui-theme', 'aurora');
  await expect(page.locator('html')).toHaveAttribute('data-ui-color-mode', 'system');
  await expect(page.locator('html')).toHaveAttribute('data-ui-color-scheme', 'dark');
  await expect(page.locator('.home-dashboard')).toHaveCSS('animation-duration', '0.001s');

  await page.emulateMedia({ colorScheme: 'light', reducedMotion: 'reduce' });
  await expect(page.locator('html')).toHaveAttribute('data-ui-color-scheme', 'light');
});

test('Xray, configuration, resize and log history interactions remain functional', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.addInitScript(() => {
    localStorage.setItem('camellia-nexus.xray-dashboard.layout.v1', JSON.stringify({
      pairHeight: 100,
      trafficHeight: 100,
    }));
  });
  await openPreview(page, 'aurora', 'dark');
  await page.locator('.program-item[data-program-id="xray-primary"]').click();
  await expect(page.getByRole('heading', { name: 'Primary Xray routing fabric' })).toBeVisible();

  await page.getByRole('tab', { name: 'Dashboard' }).click();
  await expect(page.getByRole('heading', { name: 'Outbound observatory' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Routing control' })).toBeVisible();
  const blocks = await page.locator('.xray-side-stack > .xray-dashboard-block').evaluateAll((elements) =>
    elements.map((element) => element.getBoundingClientRect()).map(({ x, y, width, height }) => ({ x, y, width, height })),
  );
  expect(blocks).toHaveLength(2);
  expect(Math.abs(blocks[0].y - blocks[1].y)).toBeLessThan(2);
  expect(blocks[1].x).toBeGreaterThan(blocks[0].x + blocks[0].width - 2);

  const dashboardStack = page.locator('.xray-side-stack');
  const resizeHandle = dashboardStack.locator('.resize-separator');
  const beforeResize = await dashboardStack.boundingBox();
  expect(beforeResize).not.toBeNull();
  expect(beforeResize!.height).toBe(400);
  await expect(resizeHandle).toHaveAttribute('aria-valuenow', '400');
  await expect.poll(() => page.locator('.traffic-block').evaluate(
    (element) => element.getBoundingClientRect().height,
  )).toBe(300);
  await expect(page.locator('.traffic-block > .resize-separator')).toHaveAttribute(
    'aria-valuenow',
    '300',
  );
  await resizeHandle.dispatchEvent('pointerdown', { button: 0, clientY: 500, pointerId: 40 });
  await expect(dashboardStack).toHaveClass(/panel-resizing/);
  await page.evaluate(() => {
    window.dispatchEvent(new PointerEvent('pointermove', { clientY: 720, pointerId: 99 }));
    window.dispatchEvent(new PointerEvent('pointerup', { clientY: 720, pointerId: 99 }));
  });
  await expect(dashboardStack).toHaveClass(/panel-resizing/);
  await expect.poll(() => dashboardStack.evaluate((element) => element.getBoundingClientRect().height)).toBe(beforeResize!.height);
  await page.evaluate(() => {
    window.dispatchEvent(new PointerEvent('pointermove', { clientY: 580, pointerId: 40 }));
    window.dispatchEvent(new PointerEvent('pointerup', { clientY: 580, pointerId: 40 }));
  });
  const afterPointerResize = await dashboardStack.boundingBox();
  expect(afterPointerResize!.height).toBeGreaterThan(beforeResize!.height + 55);
  await resizeHandle.focus();
  await page.keyboard.press('End');
  await expect.poll(() => dashboardStack.evaluate((element) => element.getBoundingClientRect().height)).toBe(1_200);
  await expect(resizeHandle).toHaveAttribute('aria-valuenow', '1200');
  await page.keyboard.press('ArrowUp');
  const afterKeyboardResize = await dashboardStack.boundingBox();
  expect(afterKeyboardResize!.height).toBe(1_184);
  await expect(resizeHandle).toHaveAttribute('aria-valuenow', '1184');

  const main = page.locator('main');
  const mainTopBeforeFirstConfiguration = await main.evaluate((element) => {
    element.scrollTop = Math.min(160, element.scrollHeight - element.clientHeight);
    return element.scrollTop;
  });
  expect(mainTopBeforeFirstConfiguration).toBeGreaterThan(0);
  await page.evaluate(() => {
    const main = document.querySelector('main')!;
    const samples: number[] = [];
    (window as typeof window & { __configurationScrollSamples?: number[] }).__configurationScrollSamples = samples;
    let frames = 120;
    const sample = () => {
      samples.push(main.scrollTop);
      frames -= 1;
      if (frames > 0) requestAnimationFrame(sample);
    };
    requestAnimationFrame(sample);
  });
  await page.getByRole('tab', { name: 'Configuration' }).click();
  await expect(page.locator('.cm-editor')).toBeVisible();
  await expect.poll(() => main.evaluate((element) => element.scrollTop)).toBe(mainTopBeforeFirstConfiguration);
  await page.waitForTimeout(250);
  const configurationScrollSamples = await page.evaluate(
    () => (window as typeof window & { __configurationScrollSamples?: number[] }).__configurationScrollSamples ?? [],
  );
  expect(Math.max(...configurationScrollSamples.map((value) => Math.abs(value - configurationScrollSamples[0])))).toBeLessThanOrEqual(1);
  await expect(page.getByRole('button', { name: 'Validate', exact: true })).toHaveCount(1);
  await expect(page.getByRole('button', { name: 'Dump parsed configuration' })).toBeVisible();
  const configToolbarLayout = await page.locator('.config-toolbar').evaluate((element) => {
    const toolbar = element.getBoundingClientRect();
    const managedNotice = element.previousElementSibling?.getBoundingClientRect();
    const tools = element.querySelector<HTMLElement>('.config-toolbar-tools')!.getBoundingClientRect();
    const commit = element.querySelector<HTMLElement>('.config-toolbar-commit')!.getBoundingClientRect();
    const commitButtons = [...element.querySelectorAll<HTMLElement>('.config-toolbar-commit button')]
      .map((button) => button.getBoundingClientRect());
    return {
      managedNoticeGap: managedNotice ? toolbar.top - managedNotice.bottom : -1,
      groupsOverlap: tools.right > commit.left && tools.left < commit.right
        && tools.bottom > commit.top && tools.top < commit.bottom,
      commitButtonTopDelta: Math.abs(commitButtons[0].top - commitButtons[1].top),
    };
  });
  expect(configToolbarLayout.managedNoticeGap).toBeGreaterThanOrEqual(12);
  expect(configToolbarLayout.groupsOverlap).toBe(false);
  expect(configToolbarLayout.commitButtonTopDelta).toBeLessThan(2);
  const configContainer = page.locator('.config-editor-resize');
  const configHandleSelector = '.config-editor-resize > .resize-separator';
  await dispatchResize(page, configHandleSelector, -4_000, 41);
  await expect.poll(() => configContainer.evaluate((element) => element.getBoundingClientRect().height)).toBe(260);
  await expect.poll(() => configContainer.evaluate((element) => {
    const editorShell = element.querySelector('.code-editor-shell');
    return editorShell
      ? element.getBoundingClientRect().height - editorShell.getBoundingClientRect().height
      : Number.POSITIVE_INFINITY;
  })).toBeLessThanOrEqual(12);
  await expect.poll(() => page.locator('.cm-editor').evaluate(
    (element) => element.getBoundingClientRect().height,
  )).toBeGreaterThan(140);
  await dispatchResize(page, configHandleSelector, 4_000, 42);
  await expect.poll(() => configContainer.evaluate((element) => element.getBoundingClientRect().height)).toBe(2_400);
  await expect.poll(() => configContainer.evaluate((element) => {
    const editorShell = element.querySelector('.code-editor-shell');
    return editorShell
      ? element.getBoundingClientRect().height - editorShell.getBoundingClientRect().height
      : Number.POSITIVE_INFINITY;
  })).toBeLessThanOrEqual(12);
  await expect.poll(() => page.evaluate(() => {
    const saved = JSON.parse(localStorage.getItem('camellia-nexus.program-detail.layout.v1') || '{}');
    return saved.configHeight;
  })).toBe(2_400);

  await page.reload();
  await expect(page.getByRole('heading', { name: 'Workspace' })).toBeVisible();
  await page.locator('.program-item[data-program-id="xray-primary"]').click();
  await page.getByRole('tab', { name: 'Configuration' }).click();
  await expect(page.locator('.cm-editor')).toBeVisible();
  await expect.poll(() => page.locator('.config-editor-resize').evaluate(
    (element) => element.getBoundingClientRect().height,
  )).toBe(2_400);

  await main.evaluate((element) => { element.scrollTop = 0; });
  await page.getByRole('tab', { name: 'Logs' }).click();
  await expect(page.getByRole('button', { name: 'Both' })).toHaveAttribute('aria-pressed', 'true');
  await expect.poll(() => main.evaluate((element) => element.scrollTop)).toBe(0);
  const output = page.locator('.log-pane.stdout pre');
  await expect(output).toBeVisible();
  await expect(output).toContainText('request 240 completed');
  await expect.poll(() => output.evaluate((element) => element.scrollHeight > element.clientHeight)).toBe(true);
  await waitForSettledRender(page);
  const historicalTop = await output.evaluate((element) => {
    element.scrollTop = Math.floor((element.scrollHeight - element.clientHeight) / 3);
    element.dispatchEvent(new Event('scroll'));
    return element.scrollTop;
  });
  expect(historicalTop).toBeGreaterThan(48);
  await waitForSettledRender(page);
  expect(await output.evaluate((element) => element.scrollTop)).toBe(historicalTop);
  await expect(output).toContainText('LIVE stdout sample 8', { timeout: 4_500 });
  await expect.poll(() => main.evaluate((element) => element.scrollTop)).toBe(0);
  await expect.poll(() => output.evaluate((element) => element.scrollTop)).toBe(historicalTop);

  const bottomBeforeGrowth = await output.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
    element.dispatchEvent(new Event('scroll'));
    return element.scrollTop;
  });
  await expect(output).toContainText('LIVE stdout sample 16', { timeout: 4_500 });
  await expect.poll(() => output.evaluate(
    (element) => element.scrollHeight - element.clientHeight - element.scrollTop,
  )).toBeLessThan(48);
  await expect.poll(() => output.evaluate((element) => element.scrollTop)).toBeGreaterThan(bottomBeforeGrowth);
});

for (const scenario of [
  {
    name: 'wide Cupertino light',
    viewport: { width: 1680, height: 980 },
    theme: 'cupertino' as const,
    mode: 'light' as const,
    scale: 0.95,
    widthRange: { min: 1_120 },
    chinese: false,
  },
  {
    name: 'medium Material dark',
    viewport: { width: 1420, height: 940 },
    theme: 'material' as const,
    mode: 'dark' as const,
    scale: 1.15,
    widthRange: { min: 920, max: 1_120 },
    chinese: false,
  },
  {
    name: 'compact Aurora light',
    viewport: { width: 1080, height: 900 },
    theme: 'aurora' as const,
    mode: 'light' as const,
    scale: 1.3,
    widthRange: { min: 600, max: 920 },
    chinese: false,
  },
  {
    name: 'narrow Cupertino dark in Chinese',
    viewport: { width: 560, height: 820 },
    theme: 'cupertino' as const,
    mode: 'dark' as const,
    scale: 1.3,
    widthRange: { min: 0, max: 600 },
    chinese: true,
  },
]) {
  test(`Xray dense dashboard remains responsive and accessible in ${scenario.name}`, async ({ page }) => {
    await page.setViewportSize(scenario.viewport);
    await openPreview(
      page,
      scenario.theme,
      scenario.mode,
      scenario.scale,
      '&__ui_xray_dense',
    );

    if (scenario.chinese) {
      const navigationToggle = page.locator('.mobile-nav-toggle');
      if (await navigationToggle.isVisible()) await navigationToggle.click();
      await page.getByRole('button', { name: 'Settings' }).click();
      const settings = page.getByRole('dialog', { name: 'Settings' });
      await settings.getByRole('tab', { name: 'General' }).click();
      await settings.getByRole('button', { name: 'Chinese' }).click();
      await page.keyboard.press('Escape');
    }

    await openXrayDashboard(page);
    await expect(page.locator('.xray-observatory-list article')).toHaveCount(9);
    await expect(page.locator('.xray-online-user-list article')).toHaveCount(7);
    await expect(page.locator('.xray-tag-preview').nth(1)).toContainText('+8');
    await expectDenseXrayLayout(page, scenario.widthRange);
    await expectNoViewportOverflow(page);
    await expectAccessible(page, '.xray-dashboard-panel');
  });
}

test('configuration mode choices remain equal and responsive for every supported program type', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1024, height: 900 });
  await openPreview(page, 'aurora', 'dark', 1.3);
  await page.getByRole('button', { name: 'Add program' }).first().click();
  const dialog = page.getByRole('dialog', { name: 'Add to Camellia Nexus' });

  for (const programName of ['sing-box', 'Xray', 'Mihomo']) {
    await dialog.getByRole('button', { name: programName, exact: true }).click();
    const layout = await dialog.locator('.configuration-mode-picker').evaluate((picker) => {
      const pickerBounds = picker.getBoundingClientRect();
      const buttons = [...picker.querySelectorAll<HTMLElement>(':scope > button')].map((button) => {
        const bounds = button.getBoundingClientRect();
        const title = button.querySelector('strong')!.getBoundingClientRect();
        const description = button.querySelector('small')!.getBoundingClientRect();
        return {
          x: bounds.x,
          y: bounds.y,
          width: bounds.width,
          height: bounds.height,
          right: bounds.right,
          titleBottom: title.bottom,
          descriptionTop: description.top,
          overflow: button.scrollWidth - button.clientWidth,
        };
      });
      return { width: pickerBounds.width, right: pickerBounds.right, overflow: picker.scrollWidth - picker.clientWidth, buttons };
    });
    expect(layout.buttons).toHaveLength(2);
    expect(Math.abs(layout.buttons[0].width - layout.buttons[1].width)).toBeLessThanOrEqual(1);
    expect(Math.abs(layout.buttons[0].height - layout.buttons[1].height)).toBeLessThanOrEqual(1);
    expect(Math.abs(layout.buttons[0].y - layout.buttons[1].y)).toBeLessThanOrEqual(1);
    expect(layout.buttons.every((button) => button.descriptionTop >= button.titleBottom)).toBe(true);
    expect(layout.buttons.every((button) => button.right <= layout.right + 1 && button.overflow <= 1)).toBe(true);
    expect(layout.overflow).toBeLessThanOrEqual(1);
  }
  await page.screenshot({ path: testInfo.outputPath('configuration-modes-wide.png'), fullPage: true });

  await page.setViewportSize({ width: 560, height: 900 });
  const compactLayout = await dialog.locator('.configuration-mode-picker').evaluate((picker) => {
    const buttons = [...picker.querySelectorAll<HTMLElement>(':scope > button')]
      .map((button) => button.getBoundingClientRect())
      .map(({ x, y, width, height, bottom }) => ({ x, y, width, height, bottom }));
    return { overflow: picker.scrollWidth - picker.clientWidth, buttons };
  });
  expect(compactLayout.buttons).toHaveLength(2);
  expect(Math.abs(compactLayout.buttons[0].width - compactLayout.buttons[1].width)).toBeLessThanOrEqual(1);
  expect(compactLayout.buttons[1].y).toBeGreaterThanOrEqual(compactLayout.buttons[0].bottom + 3);
  expect(compactLayout.overflow).toBeLessThanOrEqual(1);
  await expectNoViewportOverflow(page);
  await dialog.locator('.configuration-mode-picker').scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath('configuration-modes-compact.png'), fullPage: true });
});

test('advanced create options stay open while editing and reset with the dialog', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1100, height: 900 });
  await openPreview(page, 'material', 'light');
  await page.getByRole('button', { name: 'Add program' }).first().click();

  let dialog = page.getByRole('dialog', { name: 'Add to Camellia Nexus' });
  await dialog.getByRole('button', { name: 'sing-box', exact: true }).click();
  const advanced = dialog.locator('details.advanced-section');
  await advanced.locator('summary').click();
  await expect(advanced).toHaveAttribute('open', '');

  await dialog.getByLabel('Program ID').fill('edge-stable');
  const restartWidthBefore = (await dialog.getByLabel('Restart policy').boundingBox())!.width;
  await dialog.getByLabel('Restart policy').selectOption('always');
  await dialog.getByLabel('Administrator access').selectOption('elevated');
  await dialog.getByText('Start with Camellia Nexus', { exact: true }).click();
  await dialog.getByRole('button', { name: 'Add variable' }).click();
  await dialog.getByLabel('Environment key 1').fill('LOG_LEVEL');
  await dialog.getByLabel('Environment value 1').fill('debug');
  await expect(advanced).toHaveAttribute('open', '');
  const restartWidthAfter = (await dialog.getByLabel('Restart policy').boundingBox())!.width;
  expect(Math.abs(restartWidthAfter - restartWidthBefore)).toBeLessThanOrEqual(1);
  await expect(dialog.getByLabel('Restart policy')).toHaveCSS('text-align', 'center');
  await expect(dialog.getByLabel('Administrator access')).toHaveCSS('text-align', 'start');
  const centeredPicker = await dialog.getByLabel('Restart policy').evaluate((element) => {
    const select = element as HTMLSelectElement;
    const option = select.options[0];
    const selectStyle = getComputedStyle(select);
    const optionStyle = getComputedStyle(option);
    const checkmarkStyle = getComputedStyle(option, '::checkmark');
    const mirrorStyle = getComputedStyle(option, '::after');
    return {
      supportsBaseSelect: CSS.supports('appearance', 'base-select'),
      usesDesktopPointer: matchMedia('(hover: hover) and (pointer: fine)').matches,
      appearance: selectStyle.appearance,
      alignItems: selectStyle.alignItems,
      height: selectStyle.height,
      paddingTop: selectStyle.paddingTop,
      paddingBottom: selectStyle.paddingBottom,
      pickerAppearance: getComputedStyle(select, '::picker(select)').appearance,
      optionDisplay: optionStyle.display,
      checkmarkDisplay: checkmarkStyle.display,
      mirrorContent: mirrorStyle.content,
    };
  });
  expect(centeredPicker.supportsBaseSelect).toBe(true);
  expect(centeredPicker.usesDesktopPointer).toBe(true);
  expect(centeredPicker.appearance).toBe('base-select');
  expect(centeredPicker.alignItems).toBe('center');
  expect(centeredPicker.height).toBe('40px');
  expect(centeredPicker.paddingTop).toBe('0px');
  expect(centeredPicker.paddingBottom).toBe('0px');
  expect(centeredPicker.pickerAppearance).toBe('base-select');
  expect(centeredPicker.optionDisplay).toBe('flex');
  expect(centeredPicker.checkmarkDisplay).toBe('none');
  expect(centeredPicker.mirrorContent).toBe('none');
  await expect(dialog.getByLabel('Administrator access')).toHaveCSS('appearance', 'base-select');
  await expect(dialog.getByLabel('Administrator access')).toHaveCSS('align-items', 'center');
  await dialog.getByLabel('Restart policy').click();
  await expect.poll(() => dialog.getByLabel('Restart policy').evaluate((element) => element.matches(':open'))).toBe(true);
  await page.screenshot({ path: testInfo.outputPath('centered-option-picker.png'), fullPage: true });
  await dialog.getByText('Environment variables', { exact: true }).click();
  await expect.poll(() => dialog.getByLabel('Restart policy').evaluate((element) => element.matches(':open'))).toBe(false);

  const wideCards = await advanced.locator('.create-advanced-options').evaluate((options) => {
    const cards = [...options.querySelectorAll<HTMLElement>('.create-advanced-card')]
      .map((card) => card.getBoundingClientRect())
      .map(({ y, width, height, bottom }) => ({ y, width, height, bottom }));
    return { overflow: options.scrollWidth - options.clientWidth, cards };
  });
  expect(wideCards.cards).toHaveLength(3);
  expect(Math.abs(wideCards.cards[0].y - wideCards.cards[1].y)).toBeLessThanOrEqual(1);
  expect(Math.abs(wideCards.cards[0].width - wideCards.cards[1].width)).toBeLessThanOrEqual(1);
  expect(Math.abs(wideCards.cards[0].height - wideCards.cards[1].height)).toBeLessThanOrEqual(1);
  expect(wideCards.cards[2].y).toBeGreaterThanOrEqual(wideCards.cards[0].bottom + 8);
  expect(wideCards.cards[2].width).toBeGreaterThan(wideCards.cards[0].width * 1.9);
  expect(wideCards.overflow).toBeLessThanOrEqual(1);
  await advanced.scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath('advanced-options-stable.png'), fullPage: true });

  await page.setViewportSize({ width: 560, height: 900 });
  const compactCards = await advanced.locator('.create-advanced-options').evaluate((options) => {
    const cards = [...options.querySelectorAll<HTMLElement>('.create-advanced-card')]
      .map((card) => card.getBoundingClientRect())
      .map(({ y, width, bottom }) => ({ y, width, bottom }));
    return { overflow: options.scrollWidth - options.clientWidth, cards };
  });
  expect(compactCards.cards).toHaveLength(3);
  expect(compactCards.cards[1].y).toBeGreaterThanOrEqual(compactCards.cards[0].bottom + 8);
  expect(compactCards.cards[2].y).toBeGreaterThanOrEqual(compactCards.cards[1].bottom + 8);
  expect(Math.max(...compactCards.cards.map((card) => card.width)) - Math.min(...compactCards.cards.map((card) => card.width))).toBeLessThanOrEqual(1);
  expect(compactCards.overflow).toBeLessThanOrEqual(1);
  await expectNoViewportOverflow(page);

  await dialog.getByRole('button', { name: 'Cancel' }).click();
  await page.setViewportSize({ width: 1100, height: 900 });
  await page.getByRole('button', { name: 'Add program' }).first().click();
  dialog = page.getByRole('dialog', { name: 'Add to Camellia Nexus' });
  await dialog.getByRole('button', { name: 'sing-box', exact: true }).click();
  await expect(dialog.locator('details.advanced-section')).not.toHaveAttribute('open', '');
});

test('managed configuration sources stay dense, stable and responsive', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  await openPreview(page, 'cupertino', 'dark');
  await page.locator('.program-item[data-program-id="sing-box-edge"]').click();

  const tabAlignment = await page.locator('.program-tabs button').evaluateAll((buttons) => (
    buttons.map((button) => {
      const icon = button.querySelector<SVGGraphicsElement>('svg')!;
      const label = button.querySelector<HTMLElement>('.tab-label')!;
      const iconBounds = icon.getBoundingClientRect();
      const labelBounds = label.getBoundingClientRect();
      const art = icon.getBBox();
      return {
        centerDelta: Math.abs(
          iconBounds.top + iconBounds.height / 2 - (labelBounds.top + labelBounds.height / 2),
        ),
        artX: art.x,
        artY: art.y,
        artWidth: art.width,
        artHeight: art.height,
      };
    })
  ));
  expect(tabAlignment.every(({ centerDelta }) => centerDelta <= 1)).toBe(true);
  expect(tabAlignment.every(({ artX, artY, artWidth, artHeight }) => (
    Math.abs(artX - 3) <= 0.1
      && Math.abs(artY - 3) <= 0.1
      && Math.abs(artWidth - 14) <= 0.1
      && Math.abs(artHeight - 14) <= 0.1
  ))).toBe(true);

  const editor = page.locator('.config-source-editor');
  const automaticUpdates = editor.getByLabel('Automatic updates');
  const interval = editor.getByLabel('Update interval');
  const policy = editor.locator('.remote-update-policy');
  const initialPolicy = await editor.evaluate((element) => {
    const policyBounds = element.querySelector<HTMLElement>('.remote-update-policy')!.getBoundingClientRect();
    const sourceBounds = element.querySelector<HTMLElement>('.config-source-row')!.getBoundingClientRect();
    return { height: policyBounds.height, sourceGap: sourceBounds.top - policyBounds.bottom };
  });
  await expect(automaticUpdates).toBeChecked();
  await expect(interval).toBeEnabled();
  await interval.selectOption('360');
  await expect(interval).toHaveValue('360');
  await policy.locator('.compact-switch').click();
  await expect(interval).toBeDisabled();
  await expect(interval).toHaveValue('360');
  await policy.locator('.compact-switch').click();
  await expect(interval).toBeEnabled();
  await expect(interval).toHaveValue('360');
  const disabledPolicy = await editor.evaluate((element) => {
    const policyBounds = element.querySelector<HTMLElement>('.remote-update-policy')!.getBoundingClientRect();
    const sourceBounds = element.querySelector<HTMLElement>('.config-source-row')!.getBoundingClientRect();
    return { height: policyBounds.height, sourceGap: sourceBounds.top - policyBounds.bottom };
  });
  expect(Math.abs(disabledPolicy.height - initialPolicy.height)).toBeLessThanOrEqual(1);
  expect(Math.abs(disabledPolicy.sourceGap - initialPolicy.sourceGap)).toBeLessThanOrEqual(1);

  const remote = editor.locator('.config-source-row.remote');
  const typeSelect = remote.getByLabel('Source type');
  await expect(typeSelect).toHaveCSS('text-align', 'center');
  const compactSelectGeometry = await editor.locator('select[data-control-size="md"]').evaluateAll(
    (selects) => selects.map((select) => {
      const style = getComputedStyle(select);
      return {
        height: select.getBoundingClientRect().height,
        paddingTop: style.paddingTop,
        paddingBottom: style.paddingBottom,
        alignItems: style.alignItems,
      };
    }),
  );
  expect(compactSelectGeometry.length).toBeGreaterThanOrEqual(2);
  expect(Math.max(...compactSelectGeometry.map(({ height }) => height))
    - Math.min(...compactSelectGeometry.map(({ height }) => height))).toBeLessThanOrEqual(1);
  expect(compactSelectGeometry.every(({ paddingTop, paddingBottom }) => (
    paddingTop === '0px' && paddingBottom === '0px'
  ))).toBe(true);
  expect(compactSelectGeometry.every(({ alignItems }) => alignItems === 'center')).toBe(true);
  const primaryLayout = await remote.locator('.source-primary').evaluate((primary) => {
    const bounds = primary.getBoundingClientRect();
    const visibleControls = [...primary.children]
      .filter((element) => getComputedStyle(element).visibility !== 'hidden')
      .map((element) => (element as HTMLElement).getBoundingClientRect());
    return {
      overflow: primary.scrollWidth - primary.clientWidth,
      centerDelta: Math.max(...visibleControls.map((control) => control.top + control.height / 2))
        - Math.min(...visibleControls.map((control) => control.top + control.height / 2)),
      inside: visibleControls.every((control) => control.left >= bounds.left - 1 && control.right <= bounds.right + 1),
    };
  });
  expect(primaryLayout.overflow).toBeLessThanOrEqual(1);
  expect(primaryLayout.centerDelta).toBeLessThanOrEqual(1);
  expect(primaryLayout.inside).toBe(true);

  const authentication = remote.getByRole('button', { name: 'Basic authentication' });
  await expect(authentication).toHaveAttribute('aria-pressed', 'false');
  await authentication.click();
  await expect(authentication).toHaveAttribute('aria-pressed', 'true');
  await expect(remote.getByLabel('Username')).toBeVisible();
  await expect(remote.getByLabel('Password')).toBeVisible();
  const authenticationAlignment = await remote.locator('.source-authentication').evaluate((element) => {
    const label = element.querySelector<HTMLElement>('.source-auth-label')!.getBoundingClientRect();
    const inputs = [...element.querySelectorAll<HTMLInputElement>('input')]
      .map((input) => input.getBoundingClientRect());
    return Math.max(...inputs.map((input) => Math.abs(
      label.top + label.height / 2 - (input.top + input.height / 2),
    )));
  });
  expect(authenticationAlignment).toBeLessThanOrEqual(1);

  await editor.getByRole('button', { name: 'Local file' }).click();
  const localPath = editor.getByLabel('Local configuration path').last();
  await expect(localPath).toHaveAttribute('placeholder', 'config.json · /etc/proxy/config.json');
  const longRemoteUrl = `https://example.com/${'nested-configuration-segment/'.repeat(18)}config.json`;
  const remoteUrl = remote.getByLabel('Remote configuration URL');
  await remoteUrl.fill(longRemoteUrl);
  const remoteAddressField = remote.locator('.source-address-field');
  await expect(remoteAddressField).toHaveAttribute('data-overflowing', 'true');
  await expect(remote.getByRole('tooltip')).toHaveText(longRemoteUrl);
  const describedBy = await remoteUrl.getAttribute('aria-describedby');
  expect(describedBy).toBeTruthy();
  await page.keyboard.press('Escape');
  await expect(remote.getByRole('tooltip')).toHaveCount(0);
  const optionalMetadata = page.locator('.field-caption').filter({ hasText: 'Download URL' }).first();
  const optionalMetadataStyle = await optionalMetadata.evaluate((element) => {
    const metadata = element.querySelector('em')!;
    return {
      gap: getComputedStyle(element).gap,
      fontStyle: getComputedStyle(metadata).fontStyle,
      text: metadata.textContent,
    };
  });
  expect(optionalMetadataStyle).toEqual({ gap: '6px', fontStyle: 'normal', text: '(Optional)' });
  await localPath.focus();
  await remoteUrl.hover();
  await expect(remote.getByRole('tooltip')).toHaveText(longRemoteUrl);
  await page.mouse.move(0, 0);
  await expect(remote.getByRole('tooltip')).toHaveCount(0);
  const sourceColumnAlignment = await editor.locator('.source-stack').evaluate((element) => {
    const selectors = ['.source-name', '.source-type-select', '.source-address', '.source-enabled', '.source-actions'];
    const rows = [...element.querySelectorAll<HTMLElement>('.source-primary')];
    const columns = rows.map((row) => selectors.map((selector) => {
      const bounds = row.querySelector<HTMLElement>(selector)!.getBoundingClientRect();
      return { left: bounds.left, right: bounds.right, center: bounds.top + bounds.height / 2 };
    }));
    return {
      horizontalDelta: Math.max(...selectors.map((_, index) => (
        Math.max(...columns.map((column) => column[index].left))
          - Math.min(...columns.map((column) => column[index].left))
          + Math.max(...columns.map((column) => column[index].right))
          - Math.min(...columns.map((column) => column[index].right))
      ))),
      verticalDelta: Math.max(...columns.map((column) => (
        Math.max(...column.map((item) => item.center))
          - Math.min(...column.map((item) => item.center))
      ))),
    };
  });
  expect(sourceColumnAlignment.horizontalDelta).toBeLessThanOrEqual(2);
  expect(sourceColumnAlignment.verticalDelta).toBeLessThanOrEqual(1);
  await page.screenshot({ path: testInfo.outputPath('managed-sources-wide.png'), fullPage: true });

  await page.setViewportSize({ width: 680, height: 760 });
  const compactLayout = await editor.evaluate((element) => ({
    overflow: element.scrollWidth - element.clientWidth,
    sources: [...element.querySelectorAll<HTMLElement>('.config-source-row')].map((source) => ({
      overflow: source.scrollWidth - source.clientWidth,
      width: source.getBoundingClientRect().width,
      primaryWidth: source.querySelector<HTMLElement>('.source-primary')!.getBoundingClientRect().width,
    })),
  }));
  expect(compactLayout.overflow).toBeLessThanOrEqual(1);
  expect(compactLayout.sources.every((source) => source.overflow <= 1 && source.primaryWidth <= source.width)).toBe(true);
  await expectNoViewportOverflow(page);
  await editor.scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath('managed-sources-compact.png'), fullPage: true });
});

test('create program keeps its content scrollable and actions reachable at the minimum window size', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 680, height: 480 });
  await openPreview(page, 'aurora', 'dark', 1.3);

  const addProgram = page.getByRole('button', { name: 'Add program' });
  if (!await addProgram.isVisible()) {
    await page.getByRole('button', { name: 'Open navigation' }).click();
  }
  await addProgram.click();

  const dialog = page.getByRole('dialog', { name: 'Add to Camellia Nexus' });
  await dialog.getByRole('button', { name: 'sing-box', exact: true }).click();
  const body = dialog.locator('.modal-body');
  const createProgram = dialog.getByRole('button', { name: 'Create program' });
  await expectAccessible(page, '[role="dialog"]');

  const layout = await dialog.evaluate((element) => {
    const bodyElement = element.querySelector<HTMLElement>('.modal-body')!;
    const title = element.querySelector<HTMLElement>('.modal-title')!.getBoundingClientRect();
    const bodyBounds = bodyElement.getBoundingClientRect();
    const actions = element.querySelector<HTMLElement>('.modal-actions')!.getBoundingClientRect();
    const dialogBounds = element.getBoundingClientRect();
    return {
      bodyClientHeight: bodyElement.clientHeight,
      bodyScrollHeight: bodyElement.scrollHeight,
      bodyOverflow: getComputedStyle(bodyElement).overflowY,
      titleBottom: title.bottom,
      bodyTop: bodyBounds.top,
      bodyBottom: bodyBounds.bottom,
      actionsTop: actions.top,
      dialogTop: dialogBounds.top,
      dialogBottom: dialogBounds.bottom,
      viewportHeight: innerHeight,
    };
  });
  expect(layout.bodyScrollHeight).toBeGreaterThan(layout.bodyClientHeight);
  expect(layout.bodyOverflow).toBe('auto');
  expect(layout.bodyTop).toBeGreaterThanOrEqual(layout.titleBottom - 1);
  expect(layout.actionsTop).toBeGreaterThanOrEqual(layout.bodyBottom - 1);
  expect(layout.dialogTop).toBeGreaterThanOrEqual(0);
  expect(layout.dialogBottom).toBeLessThanOrEqual(layout.viewportHeight);
  await expect(createProgram).toBeInViewport();

  await body.hover();
  await page.mouse.wheel(0, 1_000);
  await expect.poll(() => body.evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
  await expect(createProgram).toBeInViewport();
  await expectNoViewportOverflow(page);
  await page.screenshot({ path: testInfo.outputPath('create-program-minimum-window.png') });
});

test('Mihomo keeps YAML configuration, managed Dashboard and compact layout integrated', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1024, height: 800 });
  await trackPreviewExternalActions(page);
  await openPreview(page, 'material', 'dark', 1.05, '&__ui_slow_external');
  await page.locator('.program-item[data-program-id="mihomo-alpha"]').click();
  await expect(page.getByRole('heading', { name: 'Mihomo Alpha gateway' })).toBeVisible();
  await expect(page.locator('.detail-program-icon.mihomo .program-glyph.mihomo')).toBeVisible();
  await expect(page.getByText('Combine ordered native YAML sources into the active configuration')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Mihomo Dashboard' })).toBeVisible();
  await expect(page.getByLabel('Enable Mihomo Dashboard')).toBeChecked();

  const openDashboard = page.getByRole('button', { name: 'Mihomo Dashboard' });
  await openDashboard.evaluate((button) => {
    (button as HTMLButtonElement).click();
    (button as HTMLButtonElement).click();
  });
  await expect.poll(() => previewExternalActionCount(page, 'open_mihomo_dashboard')).toBe(1);

  await page.getByRole('tab', { name: 'Configuration' }).click();
  const editor = page.getByRole('textbox', { name: 'Configuration editor' });
  await expect(editor).toBeVisible();
  await expect(editor).toContainText('mode: rule');
  await expect(editor).toContainText('external-controller');
  await expect(page.getByRole('button', { name: 'Validate', exact: true })).toBeVisible();
  await expectNoViewportOverflow(page);
  await page.screenshot({ path: testInfo.outputPath('mihomo-material-dark-compact.png'), fullPage: true });

  await page.getByRole('main').getByRole('button', { name: 'Dashboard', exact: true }).click();
  await page.getByRole('button', { name: 'Add program' }).first().click();
  const dialog = page.getByRole('dialog', { name: 'Add to Camellia Nexus' });
  await dialog.getByRole('button', { name: 'Mihomo', exact: true }).click();
  await expect(dialog.getByLabel('Executable')).toHaveValue('mihomo');
  await expect(dialog.getByText('Use arguments or an optional stored configuration')).toBeVisible();
  await expect(dialog.getByRole('heading', { name: 'Stored configuration' })).toBeVisible();
  await dialog.getByRole('button', { name: 'Managed configuration' }).click();
  await expect(dialog.getByText('Merge local and remote YAML configuration sources')).toBeVisible();
  await expect(dialog.getByLabel('Enable Mihomo Dashboard')).not.toBeChecked();
});

test('the configuration editor has a localized name, description and visible focus', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 860 });
  await openPreview(page);
  const editor = await openProgramConfiguration(page, 'xray-primary');
  const describedBy = await editor.getAttribute('aria-describedby');
  expect(describedBy).toMatch(
    /^program-configuration-editor-description-\d+ program-configuration-editor-status-\d+$/,
  );
  const [descriptionId, statusId] = describedBy!.split(' ');
  await expect(page.locator(`#${descriptionId}`)).toContainText(
    'Edit the program configuration',
  );
  await expect(page.locator(`#${statusId}`)).toHaveAttribute('role', 'group');
  await editor.focus();
  await expect.poll(() => page.locator('.code-editor-shell').evaluate((element) => ({
    style: getComputedStyle(element).outlineStyle,
    width: getComputedStyle(element).outlineWidth,
  }))).toEqual({ style: 'solid', width: '3px' });
  await page.keyboard.press('Tab');
  await expect(page.getByRole('button', { name: /^Problems:/ })).toBeFocused();
  await expectAccessible(page, '.code-editor-shell');

  await page.getByRole('button', { name: 'Settings' }).click();
  const settings = page.getByRole('dialog', { name: 'Settings' });
  await settings.getByRole('tab', { name: 'General' }).click();
  await settings.getByRole('button', { name: 'Chinese' }).click();
  await page.keyboard.press('Escape');
  const localizedEditor = page.getByRole('textbox', { name: '配置编辑器' });
  await expect(localizedEditor).toBeVisible();
  const localizedDescriptionId = (await localizedEditor.getAttribute('aria-describedby'))!.split(' ')[0];
  await expect(page.locator(`#${localizedDescriptionId}`)).toContainText(
    '编辑程序配置',
  );
});

test('the JSON configuration editor supports diagnostics, formatting and command shortcuts', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await openPreview(page, 'cupertino', 'light', 1.05);
  const editor = await openProgramConfiguration(page, 'xray-primary');
  const shell = page.locator('.code-editor-shell');
  const problems = page.getByRole('button', { name: /^Problems:/ });
  const formatDocument = page.getByRole('button', { name: 'Format document', exact: true });

  await expect(formatDocument).toBeVisible();
  await expect(page.getByRole('toolbar', { name: 'Configuration editor commands' })).toBeVisible();
  await expect(problems).toHaveAccessibleName('Problems: No problems');
  await expect(shell.locator('.editor-schema-status')).toHaveCount(0);
  await expect(shell.getByRole('button', { name: 'Show suggestions', exact: true })).toHaveCount(0);

  await replaceEditorContent(page, editor, '{"route": }');
  await expect(problems).toHaveAccessibleName(/Problems: [1-9]\d* errors?/);
  await problems.click();
  const lintPanel = shell.locator('.cm-panel-lint');
  await expect(lintPanel).toBeVisible();
  await expect(lintPanel).toContainText('A JSON value is expected');
  await expectAccessible(page, '.code-editor-shell');

  await formatDocument.click();
  await expect(shell.getByText('Fix syntax errors before formatting', { exact: true })).toBeVisible();
  await expect(editor.locator('.cm-line')).toHaveText(['{"route": }']);
  await lintPanel.locator('button[name="close"]').click();

  await replaceEditorContent(page, editor, '{"route":{"final":"direct"}}');
  await expect(problems).toHaveAccessibleName('Problems: No problems');
  await formatDocument.click();
  await expect(
    shell.getByText('Formatting complete. Save to keep these changes', { exact: true }),
  ).toBeVisible();
  await expect(editor.locator('.cm-line')).toHaveText([
    '{',
    '  "route": {',
    '    "final": "direct"',
    '  }',
    '}',
    '',
  ]);

  const undo = page.getByRole('button', { name: 'Undo', exact: true });
  const redo = page.getByRole('button', { name: 'Redo', exact: true });
  await expect(undo).toBeEnabled();
  await undo.click();
  await expect(editor.locator('.cm-line')).toHaveText(['{"route":{"final":"direct"}}']);
  await expect(redo).toBeEnabled();
  await redo.click();
  await expect(editor.locator('.cm-line')).toHaveText([
    '{',
    '  "route": {',
    '    "final": "direct"',
    '  }',
    '}',
    '',
  ]);

  await page.getByRole('button', { name: 'Find', exact: true }).click();
  const searchPanel = shell.locator('.cm-panels-top .cm-search');
  const searchInput = searchPanel.locator('input[name="search"]');
  await expect(searchPanel).toBeVisible();
  await expect(searchInput).toBeFocused();
  await expect(shell.locator('.cm-panels-bottom .cm-search')).toHaveCount(0);
  const searchLayout = await searchPanel.evaluate((element) => {
    const controls = Array.from(element.querySelectorAll<HTMLElement>(
      'input[type="text"], button:not([name="close"]), label',
    ));
    const bounds = element.getBoundingClientRect();
    return {
      overflow: element.scrollWidth - element.clientWidth,
      outside: controls.some((control) => {
        const controlBounds = control.getBoundingClientRect();
        return controlBounds.left < bounds.left - 1 || controlBounds.right > bounds.right + 1;
      }),
      heights: controls.map((control) => Math.round(control.getBoundingClientRect().height)),
      fontSizes: controls.map((control) => getComputedStyle(control).fontSize),
    };
  });
  expect(searchLayout.overflow).toBeLessThanOrEqual(1);
  expect(searchLayout.outside).toBe(false);
  expect(Math.max(...searchLayout.heights) - Math.min(...searchLayout.heights)).toBeLessThanOrEqual(1);
  expect(new Set(searchLayout.fontSizes).size).toBe(1);
  await searchPanel.locator('button[name="close"]').click();

  await page.getByRole('button', { name: 'Replace', exact: true }).click();
  const replaceInput = shell.locator('.cm-search input[name="replace"]');
  await expect(replaceInput).toBeVisible();
  await expect(replaceInput).toBeFocused();
  await shell.locator('.cm-search button[name="close"]').click();

  const wrap = page.getByRole('button', { name: 'Wrap', exact: true });
  await expect(wrap).toHaveAttribute('aria-pressed', 'true');
  await wrap.click();
  await expect(page.getByRole('button', { name: 'No wrap', exact: true })).toHaveAttribute(
    'aria-pressed',
    'false',
  );

  await editor.focus();
  await page.keyboard.press('Control+Enter');
  await expect(page.locator('.result').getByText('Valid configuration', { exact: true })).toBeVisible();

  await editor.focus();
  await page.keyboard.press('Control+s');
  await expect(page.locator('.result pre')).toContainText('Configuration saved');
  await expectNoViewportOverflow(page);
  await shell.screenshot({ path: testInfo.outputPath('configuration-editor-json-commands.png') });
});

test('sing-box schema completion and structural diagnostics remain program-specific', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1180, height: 860 });
  await openPreview(page, 'aurora', 'dark', 1.05);
  const editor = await openProgramConfiguration(page, 'sing-box-edge');
  const shell = page.locator('.code-editor-shell');
  const options = shell.locator('.cm-tooltip-autocomplete li');
  const problems = shell.getByRole('button', { name: /^Problems:/ });
  const showSuggestions = shell.getByRole('button', { name: 'Show suggestions', exact: true });

  await expect(shell.locator('.editor-schema-status.schema-ready')).toContainText(
    'Schema suggestions ready',
  );
  await expect(showSuggestions).toBeEnabled();
  await expect(showSuggestions).toHaveAttribute('title', 'Show suggestions · Ctrl Space');
  const editorDescriptionId = (await editor.getAttribute('aria-describedby'))!.split(' ')[0];
  await expect(page.locator(`#${editorDescriptionId}`)).toContainText(
    'Schema suggestions appear as you type',
  );
  await expect(page.getByRole('button', { name: 'Format with sing-box', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Format document', exact: true })).toBeVisible();
  await shell.screenshot({
    path: testInfo.outputPath('configuration-editor-schema-suggestions.png'),
  });
  await replaceEditorContent(page, editor, '{\n  "');
  await expect(options.filter({ hasText: 'outbounds' })).toBeVisible();
  await expect(options.filter({ hasText: 'log' })).toBeVisible();
  await page.keyboard.press('Escape');
  await replaceEditorContent(page, editor, '{\n  "out');
  await showSuggestions.click();
  await expect(options.first()).toBeVisible();
  await expect(options.first()).toContainText('outbounds');
  await options.first().click();
  await expect(editor).toContainText('"outbounds": []');
  await page.keyboard.press('Escape');

  await replaceEditorContent(page, editor, '{\n  "lo');
  await editor.focus();
  await page.keyboard.press('Control+Space');
  await expect(options.filter({ hasText: 'log' })).toBeVisible();
  await page.keyboard.press('Escape');

  await replaceEditorContent(page, editor, [
    '{',
    '  "outbounds": [',
    '    {"type": "direct", "tag": "direct"},',
    '    {"type": "socks", "tag": "edge", "server": "127.0.0.1", "server_port": 1080, "detour": "',
  ].join('\n'));
  const directReference = options.filter({ hasText: 'direct' });
  await expect(directReference).toBeVisible();
  await expect(options.filter({ hasText: 'edge' })).toHaveCount(0);
  await directReference.click();
  await expect(editor).toContainText('"detour": "direct"');

  await replaceEditorContent(page, editor, '{"outbounds": [], "unsupported": true}');
  await expect(problems).toHaveAccessibleName('Problems: 1 error');
  await problems.click();
  const diagnostic = shell.locator('.cm-diagnostic').filter({ hasText: 'Unknown property' });
  await expect(diagnostic).toContainText('unsupported');
  await expect(diagnostic).toContainText('JSON Schema');
  await expectAccessible(page, '.code-editor-shell');
});

test('schema failure keeps editing available and retry restores enhancement', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 680 });
  await openPreview(page, 'material', 'light', 1.3, '&__ui_schema_error');
  const editor = await openProgramConfiguration(page, 'sing-box-edge');
  const shell = page.locator('.code-editor-shell');

  await expect(editor).toBeEditable();
  await expect(shell.locator('.editor-schema-status.schema-unavailable')).toContainText(
    'Program schema unavailable',
  );
  const showSuggestions = shell.getByRole('button', { name: 'Show suggestions', exact: true });
  await expect(showSuggestions).toBeDisabled();
  await expect(showSuggestions).toHaveAttribute('title', 'Program schema unavailable');
  const retry = shell.getByRole('button', { name: 'Retry', exact: true });
  await retry.scrollIntoViewIfNeeded();
  await expect(retry).toBeInViewport();
  await expectNoViewportOverflow(page);
  await retry.click();
  await expect(shell.locator('.editor-schema-status.schema-ready')).toContainText(
    'Schema suggestions ready',
  );
  await expect(showSuggestions).toBeEnabled();
  await expect(shell.locator('.editor-schema-status.schema-unavailable')).toHaveCount(0);
  await page.getByRole('button', { name: 'Find', exact: true }).click();
  const compactSearch = shell.locator('.cm-panels-top .cm-search');
  await expect(compactSearch).toBeVisible();
  await expect.poll(() => compactSearch.evaluate((element) => ({
    panel: element.scrollWidth - element.clientWidth,
    shell: element.closest('.code-editor-shell')!.scrollWidth
      - element.closest('.code-editor-shell')!.clientWidth,
  }))).toEqual({ panel: 0, shell: 0 });
  await expect(shell.locator('.cm-scroller')).toHaveAttribute('tabindex', '0');
  await expect(shell.locator('.cm-scroller')).toHaveAttribute(
    'aria-label',
    'Configuration document',
  );
  await expectAccessible(page, '.code-editor-shell');
});

test('the YAML configuration editor preserves comments, anchors and aliases while formatting', async ({ page }) => {
  await page.setViewportSize({ width: 1080, height: 820 });
  await openPreview(page, 'material', 'dark', 1.15);
  const editor = await openProgramConfiguration(page, 'mihomo-alpha');
  const shell = page.locator('.code-editor-shell');
  const problems = page.getByRole('button', { name: /^Problems:/ });
  const formatDocument = page.getByRole('button', { name: 'Format document', exact: true });

  await replaceEditorContent(page, editor, 'mode: rule\nmode: direct\n');
  await expect(problems).toHaveAccessibleName(/Problems: [1-9]\d* errors?/);
  await formatDocument.click();
  await expect(shell.getByText('Fix syntax errors before formatting', { exact: true })).toBeVisible();

  await replaceEditorContent(
    page,
    editor,
    '# outbound pool\nproxies:\n - &edge { name: edge, server: example.test }\nselected: *edge\n',
  );
  await expect(problems).toHaveAccessibleName('Problems: No problems');
  await formatDocument.click();
  await expect(
    shell.getByText('Formatting complete. Save to keep these changes', { exact: true }),
  ).toBeVisible();
  const formatted = (await editor.locator('.cm-line').allTextContents()).join('\n');
  expect(formatted).toContain('# outbound pool');
  expect(formatted).toContain('&edge');
  expect(formatted).toContain('*edge');
  await expectAccessible(page, '.code-editor-shell');
});

for (const scenario of [
  {
    name: 'wide Cupertino light at compact scale',
    viewport: { width: 1440, height: 900 },
    theme: 'cupertino' as const,
    mode: 'light' as const,
    scale: 0.95,
    programId: 'xray-primary',
    language: 'JSON',
    chinese: false,
  },
  {
    name: 'medium Material dark at standard scale',
    viewport: { width: 1080, height: 820 },
    theme: 'material' as const,
    mode: 'dark' as const,
    scale: 1.05,
    programId: 'mihomo-alpha',
    language: 'YAML',
    chinese: false,
  },
  {
    name: 'compact Aurora light at large scale',
    viewport: { width: 820, height: 760 },
    theme: 'aurora' as const,
    mode: 'light' as const,
    scale: 1.15,
    programId: 'xray-primary',
    language: 'JSON',
    chinese: false,
  },
  {
    name: 'schema-assisted Material light near the toolbar breakpoint',
    viewport: { width: 760, height: 760 },
    theme: 'material' as const,
    mode: 'light' as const,
    scale: 1.3,
    programId: 'sing-box-edge',
    language: 'JSON',
    chinese: false,
  },
  {
    name: 'narrow Cupertino dark in Chinese at XL scale',
    viewport: { width: 560, height: 720 },
    theme: 'cupertino' as const,
    mode: 'dark' as const,
    scale: 1.3,
    programId: 'mihomo-alpha',
    language: 'YAML',
    chinese: true,
  },
]) {
  test(`the configuration editor remains responsive and accessible in ${scenario.name}`, async ({ page }, testInfo) => {
    await page.setViewportSize(scenario.viewport);
    await openPreview(page, scenario.theme, scenario.mode, scenario.scale);
    await openProgramConfiguration(page, scenario.programId);

    if (scenario.chinese) {
      const navigationToggle = page.getByRole('button', { name: 'Open navigation' });
      if (await navigationToggle.isVisible()) await navigationToggle.click();
      await page.getByRole('button', { name: 'Settings' }).click();
      const settings = page.getByRole('dialog', { name: 'Settings' });
      await settings.getByRole('tab', { name: 'General' }).click();
      await settings.getByRole('button', { name: 'Chinese' }).click();
      await page.keyboard.press('Escape');
    }

    const editorName = scenario.chinese ? '配置编辑器' : 'Configuration editor';
    const editor = page.getByRole('textbox', { name: editorName });
    const shell = page.locator('.code-editor-shell');
    await shell.scrollIntoViewIfNeeded();
    await expect(editor).toBeVisible();
    await expect(shell.getByText(scenario.language, { exact: true })).toBeVisible();
    await expect(
      page.getByRole('button', {
        name: scenario.chinese ? '格式化文档' : 'Format document',
        exact: true,
      }),
    ).toBeInViewport();
    await expect(
      page.getByRole('button', { name: scenario.chinese ? /^问题:/ : /^Problems:/ }),
    ).toBeInViewport();

    const layout = await shell.evaluate((element) => {
      const shellBounds = element.getBoundingClientRect();
      const command = element.querySelector<HTMLElement>('.editor-command-bar')!;
      const editorBody = element.querySelector<HTMLElement>('.editor')!;
      const status = element.querySelector<HTMLElement>('.editor-status-bar')!;
      const commandBounds = command.getBoundingClientRect();
      const editorBounds = editorBody.getBoundingClientRect();
      const statusBounds = status.getBoundingClientRect();
      return {
        shellOverflow: element.scrollWidth - element.clientWidth,
        commandOverflow: command.scrollWidth - command.clientWidth,
        statusOverflow: status.scrollWidth - status.clientWidth,
        commandOverflowStyle: getComputedStyle(command).overflowX,
        statusOverflowStyle: getComputedStyle(status).overflowX,
        commandTop: commandBounds.top,
        commandBottom: commandBounds.bottom,
        editorTop: editorBounds.top,
        editorBottom: editorBounds.bottom,
        statusTop: statusBounds.top,
        statusBottom: statusBounds.bottom,
        shellTop: shellBounds.top,
        shellBottom: shellBounds.bottom,
        commandLeft: commandBounds.left,
        commandRight: commandBounds.right,
        statusLeft: statusBounds.left,
        statusRight: statusBounds.right,
        shellLeft: shellBounds.left,
        shellRight: shellBounds.right,
      };
    });
    expect(layout.shellOverflow).toBeLessThanOrEqual(1);
    expect(layout.commandOverflow).toBeLessThanOrEqual(1);
    expect(layout.statusOverflow).toBeLessThanOrEqual(1);
    expect(layout.commandOverflowStyle).toBe('hidden');
    expect(layout.statusOverflowStyle).toBe('hidden');
    expect(Math.abs(layout.commandTop - layout.shellTop)).toBeLessThanOrEqual(1);
    expect(Math.abs(layout.commandBottom - layout.editorTop)).toBeLessThanOrEqual(1);
    expect(Math.abs(layout.editorBottom - layout.statusTop)).toBeLessThanOrEqual(1);
    expect(Math.abs(layout.statusBottom - layout.shellBottom)).toBeLessThanOrEqual(1);
    expect(layout.commandLeft).toBeGreaterThanOrEqual(layout.shellLeft - 1);
    expect(layout.commandRight).toBeLessThanOrEqual(layout.shellRight + 1);
    expect(layout.statusLeft).toBeGreaterThanOrEqual(layout.shellLeft - 1);
    expect(layout.statusRight).toBeLessThanOrEqual(layout.shellRight + 1);
    await page.getByRole('button', {
      name: scenario.chinese ? '查找' : 'Find',
      exact: true,
    }).click();
    const searchPanel = shell.locator('.cm-panels-top .cm-search');
    await expect(searchPanel).toBeVisible();
    await expect.poll(() => searchPanel.evaluate((element) => {
      const panelBounds = element.getBoundingClientRect();
      const controls = Array.from(element.querySelectorAll<HTMLElement>(
        'input, button, label',
      ));
      return {
        overflow: element.scrollWidth - element.clientWidth,
        outside: controls.some((control) => {
          const bounds = control.getBoundingClientRect();
          return bounds.left < panelBounds.left - 1 || bounds.right > panelBounds.right + 1;
        }),
      };
    })).toEqual({ overflow: 0, outside: false });
    await expectNoViewportOverflow(page);
    await expectAccessible(page, '.code-editor-shell');
    await shell.screenshot({
      path: testInfo.outputPath(
        `configuration-editor-${scenario.theme}-${scenario.mode}-${scenario.scale}.png`,
      ),
    });
  });
}

test('successful exits are not presented as active programs', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 860 });
  await openPreview(page, 'cupertino', 'light', 1.05, '&__ui_exited_program');

  const activePanel = page.locator('.activity-panel');
  await expect(activePanel).toContainText('Primary Xray routing fabric');
  await expect(activePanel).toContainText('Mihomo Alpha gateway');
  await expect(activePanel).not.toContainText('Singapore edge gateway');
  await expect(activePanel.locator('.section-count')).toHaveText('2');
  await expect(page.locator('.program-item[data-program-id="sing-box-edge"]')).toContainText('Exited');
});

test('a stale log failure cannot block or notify a newly selected program', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 860 });
  await openPreview(page, 'cupertino', 'light', 1.05, '&__ui_stale_log_error');
  await page.locator('.program-item[data-program-id="xray-primary"]').click();
  await page.getByRole('tab', { name: 'Logs' }).click();

  await page.locator('.program-item[data-program-id="sing-box-edge"]').click();
  await page.getByRole('tab', { name: 'Logs' }).click();
  await expect(page.locator('.log-pane.stdout pre')).toContainText('request 240 completed');
  await page.waitForTimeout(700);
  await expect(page.locator('.notification').filter({ hasText: 'Stale preview log failure.' })).toHaveCount(0);
});

test('an active tab that becomes disabled returns to Details without a keyboard dead end', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 860 });
  await openPreview(page, 'cupertino', 'light');
  await page.locator('.program-item[data-program-id="xray-primary"]').click();

  const detailsTab = page.getByRole('tab', { name: 'Details' });
  const dashboardTab = page.getByRole('tab', { name: 'Dashboard' });
  await dashboardTab.click();
  await expect(dashboardTab).toHaveAttribute('aria-selected', 'true');
  await dashboardTab.focus();
  await expect(dashboardTab).toBeFocused();

  await page.getByRole('button', { name: 'Stop', exact: true }).evaluate((element) => {
    (element as HTMLButtonElement).click();
  });

  await expect(dashboardTab).toBeDisabled();
  await expect(dashboardTab).not.toHaveAttribute('aria-controls');
  await expect(dashboardTab).toHaveAttribute('aria-selected', 'false');
  await expect(dashboardTab).toHaveAttribute('tabindex', '-1');
  await expect(detailsTab).toHaveAttribute('aria-selected', 'true');
  await expect(detailsTab).toHaveAttribute('aria-controls', 'program-panel-overview');
  await expect(detailsTab).toHaveAttribute('tabindex', '0');
  await expect(detailsTab).toBeFocused();
  await expect(page.locator('#program-panel-overview')).toBeVisible();
});

test('changing language updates visible derived program state without remounting the workspace', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 900 });
  await openPreview(page, 'cupertino', 'light');

  const program = page.locator('.program-item[data-program-id="xray-primary"]');
  await expect(program.locator('.sidebar-program-copy small')).toHaveText('Running');
  await program.click();
  await expect(page.locator('.state-line')).toContainText('Running · PID 31415');
  await expect(page.locator('.summary-item').first().locator('strong')).toHaveText('Managed configuration');

  await page.getByRole('tab', { name: 'Dashboard' }).click();
  await expect(page.locator('.xray-observatory-list article').first()).toContainText('successful probes');

  await page.getByRole('button', { name: 'Settings' }).click();
  const settingsDialog = page.locator('.settings-dialog');
  await settingsDialog.getByRole('tab', { name: 'General' }).click();
  await settingsDialog.getByRole('button', { name: 'Chinese' }).click();
  await expect(settingsDialog.getByRole('heading', { name: '常规' })).toBeVisible();
  await page.keyboard.press('Escape');

  await expect(page.getByRole('heading', { name: 'Primary Xray routing fabric' })).toBeVisible();
  await expect(program.locator('.sidebar-program-copy small')).toHaveText('运行中');
  await expect(page.locator('.state-line')).toContainText('运行中 · PID 31415');
  await expect(page.locator('.summary-item').first().locator('strong')).toHaveText('托管配置');
  await expect(page.locator('.xray-observatory-list article').first()).toContainText('次探测成功');
  await expect(page.getByRole('tab', { name: '主界面' })).toHaveAttribute('aria-selected', 'true');
});

test('settings reorganize into complete controls at an ultra-narrow width', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 680 });
  await openPreview(page, 'material', 'dark');
  await page.getByRole('button', { name: 'Open navigation' }).click();
  await page.getByRole('button', { name: 'Settings' }).click();

  const dialog = page.getByRole('dialog', { name: 'Settings' });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('tab', { name: 'Appearance' })).toBeVisible();
  await expect(dialog.getByRole('tab', { name: 'Program behavior' })).toBeVisible();
  await expect(dialog.getByRole('tab', { name: 'Appearance' })).toHaveAttribute('aria-controls', 'settings-panel-appearance');
  await expect(dialog.getByRole('tab', { name: 'General' })).not.toHaveAttribute('aria-controls');
  await dialog.getByRole('tab', { name: 'Program behavior' }).click();
  await expect(dialog.getByRole('button', { name: 'Warn' })).toHaveAttribute('aria-pressed', 'true');
  await dialog.getByRole('tab', { name: 'Appearance' }).click();
  const layout = await dialog.evaluate((element) => {
    const navigation = element.querySelector<HTMLElement>('.settings-nav-list');
    const appearance = element.querySelector<HTMLElement>('.appearance-grid');
    return {
      navigationColumns: navigation?.getBoundingClientRect().width
        ? getComputedStyle(navigation).gridTemplateColumns.split(/\s+/).filter(Boolean).length
        : 0,
      appearanceColumns: appearance?.getBoundingClientRect().width
        ? getComputedStyle(appearance).gridTemplateColumns.split(/\s+/).filter(Boolean).length
        : 0,
      navigationOverflow: navigation ? navigation.scrollWidth - navigation.clientWidth : 0,
      dialogOverflow: element.scrollWidth - element.clientWidth,
    };
  });
  expect(layout).toEqual({
    navigationColumns: 2,
    appearanceColumns: 1,
    navigationOverflow: 0,
    dialogOverflow: 0,
  });
  await expectNoViewportOverflow(page);
});

test('dialogs trap focus, close with Escape and restore the trigger', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 860 });
  await openPreview(page);
  const settings = page.getByRole('button', { name: 'Settings' });
  await settings.click();
  const settingsDialog = page.getByRole('dialog', { name: 'Settings' });
  await expect(settingsDialog).toBeVisible();
  const settingsFocusables = settingsDialog.locator(
    'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
  );
  const firstFocusable = settingsFocusables.first();
  const lastFocusable = settingsFocusables.last();
  await firstFocusable.focus();
  await page.keyboard.press('Shift+Tab');
  await expect(lastFocusable).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(firstFocusable).toBeFocused();
  await settingsDialog.getByRole('tab', { name: 'General' }).click();
  const aboutTrigger = settingsDialog.getByRole('button', { name: /About Camellia Nexus/ });
  await aboutTrigger.click();
  const aboutDialog = page.locator('.about-dialog');
  await expect(aboutDialog).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(aboutDialog).toBeHidden();
  await expect(settingsDialog.getByRole('tab', { name: 'General' })).toHaveAttribute('aria-selected', 'true');
  await expect(aboutTrigger).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(settingsDialog).toBeHidden();
  await expect(settings).toBeFocused();

  const create = page.getByRole('button', { name: 'Add program' }).first();
  await create.click();
  const createDialog = page.getByRole('dialog', { name: 'Add to Camellia Nexus' });
  await expect(createDialog).toBeVisible();
  await expect(createDialog.locator('input').first()).toBeFocused();
  await expect(createDialog.getByRole('button', { name: 'Generic' })).toHaveAttribute('aria-pressed', 'true');
  const programId = createDialog.getByLabel('Program ID');
  await programId.fill('');
  await createDialog.getByRole('button', { name: 'Create program' }).click();
  await expect(createDialog.getByRole('alert')).toContainText('Review the highlighted values and try again');
  await expect(programId).toHaveAttribute('aria-describedby', 'create-error-id');
  await expect(programId).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(createDialog).toBeHidden();
  await expect(create).toBeFocused();
});

test('a dialog keeps its keyboard trap when the focused control is removed', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 860 });
  await openPreview(page);
  const settings = page.getByRole('button', { name: 'Settings' });
  await settings.click();
  const dialog = page.getByRole('dialog', { name: 'Settings' });
  const focusedControl = dialog.getByRole('tab', { name: 'General' });
  await focusedControl.focus();
  await focusedControl.evaluate((element) => element.remove());
  await expect.poll(() => page.evaluate(() => document.activeElement === document.body)).toBe(true);
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(settings).toBeFocused();
});

test('a confirmation suspends its parent settings dialog and restores its action', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 860 });
  await openPreview(page);
  await page.getByRole('button', { name: 'Settings' }).click();
  const settingsDialog = page.getByRole('dialog', { name: 'Settings' });
  await settingsDialog.getByRole('tab', { name: 'License' }).click();
  await expect(settingsDialog).toContainText('This device is licensed and ready to use');
  await expect(settingsDialog.getByRole('button', { name: 'Activate device' })).toHaveCount(0);
  const signOut = settingsDialog.getByRole('button', { name: 'Sign out' });
  await signOut.click();

  const confirmation = page.getByRole('alertdialog', { name: 'Sign out' });
  await expect(confirmation).toBeVisible();
  await expect(page.locator('.settings-modal-layer')).toHaveAttribute('inert', '');
  await expect(page.locator('.settings-modal-layer')).toHaveAttribute('aria-hidden', 'true');
  await page.keyboard.press('Escape');
  await expect(confirmation).toBeHidden();
  await expect(page.locator('.settings-modal-layer')).not.toHaveAttribute('inert');
  await expect(signOut).toBeFocused();
});

/** Opens a real workbench view through the same contextual chooser a person uses. */
import { expect, type Locator, type Page } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };

/** Each view's own title in both languages (views.<id>.title), so a case may name a view the way its locale shows it. */
const TITLED: Readonly<Record<string, string>> = Object.fromEntries([english, chinese].flatMap((catalogue) =>
  Object.entries((catalogue as { views?: Record<string, { title?: unknown } | null> }).views ?? {})
    .flatMap(([id, view]) => typeof view?.title === 'string' ? [[view.title, id] as const] : [])));

const VIEWS: Readonly<Record<string, string>> = {
  ...TITLED,
  Outline: 'outline', History: 'history', Chat: 'chat', Documents: 'documents', People: 'people', Schedules: 'schedules',
  Changes: 'changes', Files: 'files', Git: 'git', Environment: 'environment', Processes: 'processes', Resources: 'resources',
  Ports: 'ports', Browsers: 'browsers', Displays: 'displays', Terminal: 'terminal', Parameters: 'parameters', Usage: 'usage',
  Knowledge: 'knowledge', 'Given to the agent': 'given', Given: 'given', Diagnostics: 'diagnostics', Browser: 'browser', VNC: 'vnc', Help: 'help', Comments: 'comments', Links: 'links',
};

export async function showWorkbenchView(page: Page, name: string | RegExp): Promise<void> {
  const pattern = typeof name === 'string' ? null : new RegExp(name.source, name.flags.replace('g', ''));
  const id = typeof name === 'string' ? VIEWS[name] : Object.entries(VIEWS).find(([title]) => pattern?.test(title))?.[1];
  if (id === undefined) throw new Error(`No registered test view matches ${String(name)}`);
  const add = page.getByRole('button', { name: /^(Add a view|添加视图)$/ }).first();
  const narrow = page.locator('[data-narrow-containers] button').first();
  // Admission/route loading is not a phone layout. Wait for an actual frame control before choosing its workflow.
  await expect.poll(async () => await add.isVisible() || await narrow.isVisible()).toBe(true);
  if (!await add.isVisible()) await narrow.click();
  await page.getByRole('button', { name: /^(Add a view|添加视图)$/ }).first().click();
  const chooser = page.getByRole('dialog', { name: /^(Views|视图)$/ });
  await expect(chooser).toBeVisible();
  await chooser.locator(`[role="option"][data-value="commands:view.open.${id}"]`).click();
  await expect(chooser).toHaveCount(0);
}

/** Turns on, in Settings, Interface, that a hidden bar peeks when the pointer rests at its edge; the person's choice is kept for the person like the others. */
export async function showHiddenBarsOnHover(page: Page): Promise<void> {
  await page.goto('/settings/interface');
  const hover = page.getByRole('switch', { name: /^Show hidden bars on hover: (On|Off)$/ });
  await expect(hover).toBeVisible();
  if (await hover.getAttribute('aria-checked') !== 'true') {
    const saved = page.waitForResponse((response) => response.request().method() === 'PUT' && new URL(response.url()).pathname.endsWith('/me/state/sidebar'));
    await hover.click();
    expect((await saved).ok()).toBe(true);
  }
  await expect(page.getByRole('switch', { name: 'Show hidden bars on hover: On' })).toBeChecked();
}

/** The pointer comes to the strip at the edge of a hidden bar and rests there: the left bar's is beside the rail, the right bar's is the last pixel of the window. */
export async function restAtEdge(page: Page, bar: 'left' | 'right'): Promise<void> {
  const width = page.viewportSize()?.width ?? 1440;
  const rail = await page.locator('[data-navigation-rail]').boundingBox();
  const x = bar === 'right' ? width - 1 : (rail?.x ?? 0) + (rail?.width ?? 0) + 4;
  await page.mouse.move(Math.max(x - 240, 1), 520);
  await page.mouse.move(x, 520, { steps: 6 });
}

/** Read the retained identity after the normal workbench save completes. */
export async function waitForSavedTabId(page: Page, tab: Locator): Promise<string> {
  const elementId = await tab.getAttribute('id');
  if (elementId === null) throw new Error('The workbench tab has no id');
  const id = elementId.replace(/^(?:mobile-)?tab-/, '');
  await expect.poll(() => page.evaluate((tabId) => Object.keys(localStorage).some((key) =>
    key.endsWith('.workbench.workspace') && localStorage.getItem(key)?.includes(JSON.stringify(tabId))), id)).toBe(true);
  return id;
}

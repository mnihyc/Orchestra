/** Device layouts from the tab-bar release retain every chosen location and target after upgrading. */
import { expect, type Page, test } from '@playwright/test';
import type { Layout } from '../../src/features/workbench/layout';
import { id, RUNS_ON } from '../doubles/fixtures';
import { begin } from './inspector-support';
import { ACME, signIn } from './support';

const KEY = `orch.${id(1, 1)}.workbench.thread`;
const file = (name: string, path: string) => ({ id: name, view: 'file', of: { env: RUNS_ON.workspace, thread: ACME.mainThread, path }, keep: true });
function legacy() {
  return {
    version: 1, maximized: null,
    left: { open: true, size: 333, group: { kind: 'group', id: 'left', active: 'files', tabs: [{ id: 'files', view: 'files' }, { id: 'nav', view: 'navigation' }] } },
    right: { open: true, size: 367, group: { kind: 'group', id: 'right', active: 'changes', tabs: [{ id: 'outline', view: 'outline' }, { id: 'changes', view: 'changes' }] } },
    bottom: { open: true, size: 211, group: { kind: 'group', id: 'bottom', active: 'terminal', tabs: [{ id: 'terminal', view: 'terminal' }] } },
    main: { kind: 'split', id: 'main-split', direction: 'row', sizes: [35, 65], children: [
      { kind: 'group', id: 'route', active: 'screen', tabs: [{ id: 'screen', view: 'screen' }] },
      { kind: 'group', id: 'editor', active: 'app', tabs: [file('app', 'web/src/app.ts')] },
    ] },
  };
}
async function seed(page: Page, layout: unknown) {
  await page.evaluate(({ key, layout }) => localStorage.setItem(key, JSON.stringify({ arrangement: 'conversation', layout: JSON.stringify(layout) })), { key: KEY, layout });
}
async function saved(page: Page): Promise<Layout | null> {
  return page.evaluate((key) => {
    const text = localStorage.getItem(key);
    if (text === null) return null;
    const envelope = JSON.parse(text) as { layout: string };
    return JSON.parse(envelope.layout) as Layout;
  }, KEY);
}

test.beforeEach(async () => { await begin(); });

test('a rearranged v1 layout keeps its side-bar choices, main split, bottom, sizes and kept file', async ({ page }) => {
  await page.setViewportSize({ width: 1920, height: 1000 });
  await signIn(page);
  const old = legacy();
  await seed(page, old);
  await page.goto(`/thread/${ACME.mainThread}`);
  await expect(page.locator('[data-workbench-bar="left"]')).toHaveAttribute('data-active-container', 'code');
  await expect(page.locator('[data-workbench-bar="left"]').getByRole('button', { name: 'Files', exact: true })).toBeVisible();
  await expect(page.locator('[data-workbench-bar="right"]')).toHaveAttribute('data-active-container', 'code');
  await expect(page.getByRole('tablist', { name: 'Bottom', exact: true }).getByRole('tab', { name: 'Terminal', exact: true })).toBeVisible();
  await expect(page.getByRole('tab', { name: 'app.ts', exact: true })).toBeVisible();
  // An ordinary choice writes the migrated value; loading alone does not discard or replace the old value.
  await page.getByRole('button', { name: 'Left bar', exact: true }).click();
  await page.getByRole('button', { name: 'Left bar', exact: true }).click();
  await expect.poll(async () => (await saved(page))?.version).toBe(2);
  const layout = await saved(page);
  expect(layout?.main).toEqual(old.main);
  expect(layout?.bottom).toEqual(old.bottom);
  expect(layout?.left.size).toBe(333);
  expect(layout?.right.size).toBe(367);
  const sectionIds = [...(layout?.left.containers ?? []), ...(layout?.right.containers ?? [])].flatMap((container) => container.sections.map((section) => section.tab.id));
  expect(sectionIds.sort()).toEqual(['changes', 'files', 'nav', 'outline']);
  await page.reload();
  await expect(page.getByRole('tab', { name: 'app.ts', exact: true })).toBeVisible();
  await expect(page.locator('[data-workbench-bar="left"]')).toHaveAttribute('data-active-container', 'code');
});

test('the secondary main tab list shows every visible title, the dirty dot and Close', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await signIn(page);
  const old = legacy();
  await seed(page, { ...old, bottom: { ...old.bottom, open: false }, main: { kind: 'group', id: 'editors', active: 'app', tabs: [{ id: 'screen', view: 'screen' }, file('app', 'web/src/app.ts'), file('util', 'web/src/util.ts')] } });
  await page.goto(`/thread/${ACME.mainThread}`);
  await expect(page.getByRole('tablist', { name: 'Main area', exact: true }).getByRole('tab')).toHaveCount(3);
  await expect(page.getByRole('tab', { name: 'util.ts', exact: true })).toBeVisible();
  await expect(page.locator('[data-file-tab="web/src/app.ts"] .cm-content')).toBeVisible();
  await page.locator('[data-file-tab="web/src/app.ts"] .cm-content').fill('export const changed = true;');
  const all = page.getByRole('button', { name: 'Find tab in Main area', exact: true });
  await all.click();
  const menu = page.getByRole('menu', { name: 'Find tab in Main area', exact: true });
  await expect(menu.getByRole('menuitem', { name: 'Main thread', exact: true })).toBeVisible();
  await expect(menu.getByRole('menuitem', { name: 'app.ts, unsaved', exact: true }).locator('[data-unsaved]')).toBeVisible();
  await expect(menu.getByRole('menuitem', { name: 'Close app.ts', exact: true })).toBeVisible();
  await menu.getByRole('menuitem', { name: 'util.ts', exact: true }).click();
  const selected = page.getByRole('tab', { name: 'util.ts', exact: true });
  await expect(selected).toHaveAttribute('aria-selected', 'true');
  await expect(selected).toBeInViewport();
  await all.click();
  await menu.getByRole('menuitem', { name: 'Close util.ts', exact: true }).click();
  await expect(page.getByRole('tab', { name: 'util.ts', exact: true })).toHaveCount(0);
});

test('main and bottom groups wrap all their tabs and retain pointer and keyboard access', async ({ page }) => {
  await page.setViewportSize({ width: 1000, height: 1000 });
  await signIn(page);
  const old = legacy();
  const main = ['screen', 'outline', 'history', 'chat', 'documents', 'people', 'changes', 'files', 'git'];
  const bottom = ['environment', 'processes', 'resources', 'ports', 'parameters', 'usage', 'knowledge', 'given', 'diagnostics'];
  const group = (id: string, views: string[]) => ({ kind: 'group', id, active: views[0], tabs: views.map((view) => ({ id: `wrap-${view}`, view })) });
  const mainGroup = group('wrap-main', main);
  const bottomGroup = group('wrap-bottom', bottom);
  await seed(page, {
    ...old,
    left: { ...old.left, group: { ...old.left.group, active: 'nav', tabs: [{ id: 'nav', view: 'navigation' }] } },
    right: { ...old.right, open: false, group: { kind: 'group', id: 'wrap-right', active: null, tabs: [] } },
    main: { ...mainGroup, active: 'wrap-screen' },
    bottom: { ...old.bottom, group: { ...bottomGroup, active: 'wrap-environment' } },
  });
  await page.goto(`/thread/${ACME.mainThread}`);
  for (const [name, count] of [['Main area', main.length], ['Bottom', bottom.length]] as const) {
    const tabs = page.getByRole('tablist', { name, exact: true }).getByRole('tab');
    const strip = page.getByRole('tablist', { name, exact: true });
    await expect(tabs).toHaveCount(count);
    const box = await strip.boundingBox();
    expect(box).not.toBeNull();
    if (box === null) continue;
    for (const tab of await tabs.all()) {
      await expect(tab).toBeVisible();
      const control = await tab.boundingBox();
      expect(control).not.toBeNull();
      if (control === null) continue;
      expect(control.x).toBeGreaterThanOrEqual(box.x - 1);
      expect(control.x + control.width).toBeLessThanOrEqual(box.x + box.width + 1);
      expect(control.y).toBeGreaterThanOrEqual(box.y - 1);
      expect(control.y + control.height).toBeLessThanOrEqual(box.y + box.height + 1);
    }
    const first = await tabs.first().boundingBox();
    const last = await tabs.last().boundingBox();
    expect(first).not.toBeNull();
    expect(last).not.toBeNull();
    if (first !== null && last !== null) expect(last.y).toBeGreaterThan(first.y);
    await tabs.last().click();
    await expect(tabs.last()).toHaveAttribute('aria-selected', 'true');
    await tabs.last().press('Home');
    await expect(tabs.first()).toBeFocused();
    await tabs.first().press('End');
    await expect(tabs.last()).toBeFocused();
    await expect(tabs.last()).toHaveAttribute('aria-selected', 'true');
  }
});

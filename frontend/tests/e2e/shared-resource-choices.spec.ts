/** Resource links and bounded choices keep their native actions at both screen widths and in both themes. */
import { expect, type Locator, type Page, type Request, type TestInfo, test } from '@playwright/test';
import { writeFile } from 'node:fs/promises';
import AxeBuilder from '@axe-core/playwright';
import english from '../../src/i18n/en.json' with { type: 'json' };
import type { AuthResult, EverythingRowPage, Mount } from '../../src/generated/api-types.ts';
import { inventoryRow } from '../doubles/everything-row.ts';
import { GRACE } from '../doubles/thread-scenes.ts';
import { ACME, bodySent, PEOPLE, scenario, setPresence, signIn } from './support.ts';

interface Call { method: string; path: string }

test.use({ hasTouch: true });

async function recordRequests(info: TestInfo, name: string, calls: readonly Call[]) {
  const path = info.outputPath(`${name}.json`);
  await writeFile(path, JSON.stringify(calls, null, 2));
  await info.attach(name, { path, contentType: 'application/json' });
}

async function capture(page: Page, info: TestInfo, name: string) {
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(page.viewportSize()!.width + 1);
  const path = info.outputPath(`${name}.png`);
  await page.screenshot({ path, animations: 'disabled', caret: 'hide' });
  await info.attach(name, { path, contentType: 'image/png' });
}

async function bounded(page: Page, list: Locator) {
  const geometry = await list.evaluate((element) => {
    const frame = element.closest('[data-radix-popper-content-wrapper]')?.firstElementChild;
    if (!(frame instanceof HTMLElement)) throw new Error('The popup has no positioned frame');
    const box = frame.getBoundingClientRect();
    const scrollers = [frame, ...Array.from(frame.querySelectorAll<HTMLElement>('*'))].filter((node) => /auto|scroll/.test(getComputedStyle(node).overflowY) && node.scrollHeight > node.clientHeight + 1);
    const active = element.querySelector<HTMLElement>('[aria-selected="true"]');
    const viewport = frame.querySelector('[data-combobox-footer]') === null ? frame : element;
    const row = active?.getBoundingClientRect(), view = viewport.getBoundingClientRect();
    // The shared card limit is 20 rem; keep the assertion correct at the browser's text size.
    return { left: box.left, right: box.right, top: box.top, bottom: box.bottom, width: box.width, maxWidth: 20 * Number.parseFloat(getComputedStyle(document.documentElement).fontSize), scrollers: scrollers.length,
      row: row === undefined ? null : { top: row.top, bottom: row.bottom }, view: { top: view.top, bottom: view.bottom } };
  });
  expect(geometry.left).toBeGreaterThanOrEqual(-1);
  expect(geometry.right).toBeLessThanOrEqual(page.viewportSize()!.width + 1);
  expect(geometry.top).toBeGreaterThanOrEqual(-1);
  expect(geometry.bottom).toBeLessThanOrEqual(page.viewportSize()!.height + 1);
  expect(geometry.width).toBeLessThanOrEqual(geometry.maxWidth + 1);
  expect(geometry.scrollers).toBeLessThanOrEqual(1);
  if (geometry.row !== null) {
    expect(geometry.row.top).toBeGreaterThanOrEqual(geometry.view.top - 1);
    expect(geometry.row.bottom).toBeLessThanOrEqual(geometry.view.bottom + 1);
  }
  if (await list.getAttribute('role') === 'listbox') {
    const accessibility = await new AxeBuilder({ page }).include('[role="listbox"]').withRules(['aria-required-children', 'nested-interactive']).analyze();
    expect(accessibility.violations).toEqual([]);
    for (const card of await page.locator('[data-inspection-card]').all()) {
      if (await card.isVisible()) await bounded(page, card);
    }
  }
}

for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  test(`shared resource names and choices work at ${width} in ${theme}`, async ({ page, request }, info) => {
    await scenario();
    const double = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
    expect((await request.post(`${double}/__double/request-feeds/fixtures`)).status()).toBe(204);
    const api = `${double}/api/v1`;
    const login = await request.post(`${api}/auth/login`, { data: { login: PEOPLE.ada.login, password: PEOPLE.ada.password } });
    expect(login.status()).toBe(200);
    const auth = await login.json() as AuthResult;
    const mountReply = await request.get(`${api}/mounts`, { headers: { Authorization: `Bearer ${auth.access_token}` } });
    expect(mountReply.status()).toBe(200);
    const mounts = await mountReply.json() as Mount[];
    expect(mounts.length).toBeGreaterThan(0);
    // This fake aggregate omits mounts; retain its native authorization and project the fixture's real rows.
    await page.route('**/api/v1/everything?*', async (route) => {
      const query = new URL(route.request().url()).searchParams;
      if (query.get('kinds') !== 'mount' || query.get('resource_details') !== 'true') { await route.continue(); return; }
      const response = await route.fetch();
      expect(response.status()).toBe(200);
      const items = mounts.map((mount) => inventoryRow({ id: mount.id, kind: 'mount', name: mount.name, owner: mount.owner,
        my_access: mount.access.permissions.includes('mount.manage') ? 'manage' : 'view', resource: { kind: 'mount', value: mount } }));
      await route.fulfill({ response, json: { items, next_cursor: null } });
    });
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    const calls: Call[] = [];
    const observe = (request: Request) => { const path = new URL(request.url()).pathname; if (path.startsWith('/api/v1/')) calls.push({ method: request.method(), path }); };
    page.on('request', observe);
    try {
      for (const [kind, title] of [['mounts', english.screens.mounts.title], ['agents', english.agents.list.label]] as const) {
        await page.mouse.move(0, 0);
        const before = calls.length;
        await page.goto(`/resources/${kind}`);
        const table = page.getByRole('table', { name: title });
        const link = table.locator('a.resource-chip').first();
        await expect(link).toBeVisible();
        const href = await link.getAttribute('href');
        if (href === null) throw new Error('A resource name has no destination');
        const nativePath = new URL(href, page.url()).pathname.replace(/^\/resources(?=\/)/, '/api/v1');
        expect(calls.slice(before).filter((call) => call.path === nativePath)).toEqual([]);
        await capture(page, info, `${kind}-${width}-${theme}`);
        await link.focus();
        const card = page.locator(`[data-inspection-card]:has([data-resource-card="${kind === 'mounts' ? 'mount' : 'agent'}"])`);
        await expect(card.getByRole('heading')).toContainText(await link.innerText());
        await bounded(page, card);
        await capture(page, info, `${kind}-card-${width}-${theme}`);
        if (width === 400) await link.tap(); else await link.press('Enter');
        await expect(page).toHaveURL(new URL(href, page.url()).href);
        await recordRequests(info, `${kind}-requests`, calls.slice(before));
      }

      const providerBefore = calls.length;
      const setting = 'agent.default_provider_id';
      await page.goto(`/project/${ACME.web}/settings?section=policies&setting=${setting}`);
      const field = page.locator(`[data-setting="${setting}"]`).getByRole('combobox', { name: english.settingRegistry.agent.default_provider_id.title, exact: true });
      await expect(field).toBeVisible();
      const current = await field.inputValue();
      const read = page.waitForRequest((request) => { const url = new URL(request.url()); return url.pathname === '/api/v1/everything' && url.searchParams.get('resource_view') === 'usable'; });
      if (width === 400) await field.tap(); else await field.click();
      // Read the same native fixture through its API client; navigation may retire Chrome's response body.
      const choices = await request.get((await read).url(), { headers: { Authorization: `Bearer ${auth.access_token}` } });
      expect(choices.status()).toBe(200);
      const inventory = await choices.json() as EverythingRowPage;
      const next = inventory.items.find((row) => row.resource?.kind === 'provider' && row.name !== current && row.resource.value.status !== 'disabled' && row.resource.value.access.permissions.includes('provider.use'));
      if (next?.name === null || next?.name === undefined) throw new Error('The authorized fixture has no second provider choice');
      const list = page.getByRole('listbox', { name: english.settingRegistry.agent.default_provider_id.title, exact: true });
      await bounded(page, list);
      await capture(page, info, `provider-choice-${width}-${theme}`);
      const inspect = page.getByRole('button', { name: english.ui.detailCard.inspect.replace('{{name}}', next.name), exact: true });
      if (width === 400) await inspect.tap(); else {
        const ids = await list.getByRole('option').evaluateAll((options) => options.map((option) => option.id));
        const target = ids.indexOf(await list.getByRole('option', { name: next.name, exact: true }).getAttribute('id') ?? '');
        expect(target).toBeGreaterThanOrEqual(0);
        await field.press('Home');
        for (let step = 0; step < target; step += 1) await field.press('ArrowDown');
        await field.press('ArrowRight');
      }
      const providerCard = page.getByRole('dialog', { name: next.name, exact: true });
      await expect(providerCard.getByRole('heading')).toHaveText(next.name);
      await bounded(page, providerCard);
      await capture(page, info, `provider-card-${width}-${theme}`);
      await page.keyboard.press('Escape');
      await expect(providerCard).not.toBeVisible();
      await expect(list).toBeVisible();
      expect(calls.filter((call) => call.method === 'PUT' && call.path === `/api/v1/nodes/${ACME.web}/settings/${setting}`)).toEqual([]);
      const saved = page.waitForResponse((response) => response.request().method() === 'PUT' && new URL(response.url()).pathname === `/api/v1/nodes/${ACME.web}/settings/${setting}`);
      const hint = list.getByRole('option', { name: next.name, exact: true }).locator('[data-resource-hint]');
      if (width === 400) await hint.tap(); else await hint.click();
      const response = await saved;
      expect(response.status()).toBe(200);
      expect(await bodySent(response.request())).toMatchObject({ value: next.id });
      expect(calls.filter((call) => call.method === 'PUT' && call.path === `/api/v1/nodes/${ACME.web}/settings/${setting}`)).toHaveLength(1);
      await expect(field).toHaveValue(next.name);
      await expect(field).toHaveAttribute('aria-expanded', 'false');
      await recordRequests(info, 'provider-requests', calls.slice(providerBefore));

      const usageBefore = calls.length;
      await page.goto('/usage?scope=me&view=requests');
      const outcome = page.getByRole('combobox', { name: english.usagePerf.outcome, exact: true });
      await outcome.click();
      const outcomes = page.getByRole('listbox', { name: english.usagePerf.outcome, exact: true });
      expect(await outcomes.getByRole('option').count()).toBeGreaterThan(10);
      await outcome.press('End');
      await bounded(page, outcomes);
      await capture(page, info, `long-choice-end-${width}-${theme}`);
      await outcome.press('Home');
      await bounded(page, outcomes);
      await capture(page, info, `long-choice-home-${width}-${theme}`);
      await outcome.press('Escape');
      await expect(outcome).toBeFocused();
      await expect(outcome).toHaveAttribute('aria-expanded', 'false');
      await recordRequests(info, 'long-list-requests', calls.slice(usageBefore));

      const peopleBefore = calls.length;
      // Exercise the shared people list with enough presence rows to require scrolling.
      const viewers = Array.from({ length: 12 }, (_, index) => ({ since: new Date().toISOString(), user: {
        ...GRACE, id: `00000000-0000-4000-8000-${String(index + 200).padStart(12, '0')}`, username: '', display_name: `Viewer ${index + 1}`,
      } }));
      await setPresence(ACME.mainThread, viewers);
      await page.goto(`/thread/${ACME.mainThread}`);
      const actions = page.locator('[data-thread-header]').getByRole('button', { name: /^Actions/ });
      if (width === 400) await actions.tap(); else await actions.click();
      const view = page.getByRole('menuitem', { name: english.thread.header.view, exact: true });
      if (width === 400) await view.tap(); else await view.click();
      const presence = page.getByRole('menuitem', { name: /^Looking now:/ });
      if (width === 400) await presence.tap(); else await presence.click();
      // The previous mouse position must not hover an unrelated row while using touch or keyboard.
      await page.mouse.move(0, 0);
      const looking = page.getByRole('dialog', { name: english.composer.row.looking, exact: true });
      const stack = looking.getByRole('button', { name: /^People:/ });
      if (width === 400) await stack.tap(); else { await stack.focus(); await stack.press('Enter'); }
      const people = page.getByRole('dialog', { name: english.ui.avatarStack.list, exact: true });
      await expect(people.getByRole('listitem')).toHaveCount(viewers.length);
      await bounded(page, people);
      await capture(page, info, `people-list-${width}-${theme}`);
      const person = people.getByRole('button', { name: 'Viewer 1', exact: true });
      if (width === 400) await person.tap(); else { await person.focus(); await person.press('Enter'); }
      const identity = page.getByRole('dialog', { name: 'Viewer 1', exact: true });
      await expect(identity).toBeVisible();
      await expect(identity).toContainText('Viewer 1');
      await bounded(page, identity);
      await capture(page, info, `people-card-${width}-${theme}`);
      await expect(page.locator('[data-inspection-card]:visible')).toHaveCount(1);
      await page.keyboard.press('Escape');
      await expect(identity).not.toBeVisible();
      await expect(people).toBeVisible();
      expect(calls.filter((call) => call.path === '/api/v1/profiles/' || call.path === '/api/v1/profiles')).toEqual([]);
      await recordRequests(info, 'people-requests', calls.slice(peopleBefore));
    } finally {
      page.off('request', observe);
      await recordRequests(info, 'all-requests', calls);
    }
  });
}

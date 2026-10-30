/** Environment access groups and access-program names in the shipped frontend. */
import AxeBuilder from '@axe-core/playwright';
import { expect, test } from '@playwright/test';
import type { BindingEnvironment } from '../../src/generated/api-types';
import { ACME, PEOPLE, scenario, signIn } from './support';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

async function control(path: string, body: unknown) {
  const response = await fetch(`${DOUBLE}/__double${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`${path}: ${response.status}`);
}

function vendorIds(value: unknown): unknown {
  if (value === 'Daybreak Blue') return 'daybreakBlue';
  if (value === 'Daybreak Red') return 'daybreakRed';
  if (Array.isArray(value)) return value.map(vendorIds);
  if (typeof value === 'object' && value !== null) return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, vendorIds(item)]));
  return value;
}

for (const width of [1280, 400]) for (const theme of ['light', 'dark'] as const) {
  test(`the environment picker groups access and keeps its list accessible at ${width}px in ${theme}`, async ({ page }) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    const starts: string[] = [];
    page.on('request', (request) => { if (request.method() === 'POST' && /\/topics\/[^/]+\/threads$/.test(new URL(request.url()).pathname)) starts.push(request.url()); });
    await page.route('**/api/v1/nodes/*/binding/options*', async (route) => {
      const response = await route.fetch();
      const data = await response.json() as { environments: BindingEnvironment[] };
      const usable = data.environments.find((row) => row.usable);
      if (usable === undefined) throw new Error('The scenario has no usable environment');
      data.environments.push({ ...usable, environment: { ...usable.environment, id: '00000000-0000-4000-8024-000000009999', name: 'Partner workspace' }, usable: false, reason: 'environment_use_required', is_default: false });
      await route.fulfill({ response, json: data });
    });
    await signIn(page);
    await page.goto(`/topic/${ACME.fixLogin}`);
    await page.getByRole('region', { name: 'Start a thread' }).getByRole('button', { name: 'Thread setup' }).click();
    const dialog = page.getByRole('dialog', { name: 'New thread in Fix login' });
    await dialog.getByRole('combobox', { name: 'Where it runs' }).click();
    const list = page.getByRole('listbox');
    await expect(list.getByRole('group', { name: 'Can use' }).getByRole('option', { name: 'Workspace' })).toBeVisible();
    const ask = list.getByRole('group', { name: 'Ask for access' });
    await expect(ask.getByRole('option', { name: 'Partner workspace' })).toBeVisible();
    await expect(list.getByRole('option', { name: 'New', exact: true })).toBeVisible();
    const audit = await new AxeBuilder({ page }).include('[role="listbox"]').withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
    expect(audit.violations.filter((violation) => violation.impact === 'serious' || violation.impact === 'critical').map((violation) => violation.id)).toEqual([]);
    await ask.getByRole('option', { name: 'Partner workspace' }).click();
    await expect(dialog.getByRole('textbox', { name: 'Why do you need it?' })).toBeVisible();
    expect(starts).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
  });
}

test('the provider editor shows the vendor program name and saves the original id', async ({ page }) => {
  await scenario();
  await page.route('**/api/v1/providers/**', async (route) => {
    const response = await route.fetch();
    if (!(response.headers()['content-type'] ?? '').includes('json')) return route.fulfill({ response });
    await route.fulfill({ response, json: vendorIds(await response.json()) });
  });
  await signIn(page);
  await page.goto('/resources/providers/00000000-0000-4000-8022-000000000002?tab=models');
  await page.getByRole('table', { name: 'Models' }).waitFor();
  await page.getByRole('button', { name: 'Actions for GPT-5.5 Codex', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Edit options…' }).click();
  const dialog = page.getByRole('dialog', { name: 'What this model can be run with here' });
  const programs = dialog.getByRole('group', { name: 'Access programs' });
  await programs.getByRole('switch', { name: 'Daybreak Blue' }).click();
  const saved = page.waitForRequest((request) => request.method() === 'PATCH' && /\/providers\/[^/]+\/models\/[^/]+/.test(new URL(request.url()).pathname));
  await dialog.getByRole('button', { name: 'Save', exact: true }).click();
  const body = (await saved).postDataJSON() as { parameters: { access_programs: string[] } };
  expect(body.parameters.access_programs).toContain('daybreakBlue');
  await expect(dialog).not.toBeVisible();
});

test('a thread and the usage log name access programs while retaining the program id', async ({ page, request }) => {
  await scenario();
  await control('/model/entry', { model_name: 'gpt-sol', display_name: 'GPT Sol (A)', parameters: { efforts: ['medium'], access_programs: ['daybreakBlue', 'daybreakRed', 'standard', 'daybreakGreen'] } });
  await control('/thread/emit', { thread: ACME.mainThread, rows: { thread: { state: 'idle', model: 'gpt-sol', model_display: 'GPT Sol (A)', effort: 'medium' } } });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await page.getByRole('button', { name: /^Model and effort: GPT Sol \(A\)/ }).click();
  const access = page.getByRole('dialog', { name: 'Model and effort' }).getByRole('region', { name: 'Access' });
  await access.getByRole('combobox').click();
  await expect(page.getByRole('option', { name: 'Daybreak Blue', exact: true })).toBeVisible();
  await expect(page.getByRole('option', { name: 'daybreakGreen', exact: true })).toBeVisible();
  await page.getByRole('option', { name: 'Standard', exact: true }).click();
  await expect(access.getByRole('combobox')).toContainText('Standard');
  const login = await request.post(`${DOUBLE}/api/v1/auth/login`, { data: { login: PEOPLE.ada.login, password: PEOPLE.ada.password } });
  const token = (await login.json() as { access_token: string }).access_token;
  await expect.poll(async () => {
    const response = await request.get(`${DOUBLE}/api/v1/nodes/${ACME.mainThread}/settings`, { headers: { Authorization: `Bearer ${token}` } });
    const settings = await response.json() as { key: string; own: { value: unknown } | null }[];
    return settings.find((setting) => setting.key === 'agent.params')?.own?.value;
  }).toEqual({ cyber_access_program: 'standard' });
  await control('/usage/use', { pattern: 'A', program: 'daybreakBlue', performance: true });
  await page.goto('/usage?view=requests&range=7d');
  const log = page.getByRole('table', { name: 'Requests', exact: true });
  await expect(log).toContainText('Daybreak Blue');
  const filters = page.getByRole('button', { name: 'Filter requests', exact: true });
  if (await filters.isVisible()) await filters.click();
  await page.getByRole('combobox', { name: 'Access programme' }).click();
  await page.getByRole('option', { name: 'Daybreak Blue', exact: true }).click();
  await expect(page).toHaveURL(/program=daybreakBlue/);
});

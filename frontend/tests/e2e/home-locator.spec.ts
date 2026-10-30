/** Real creation fields search owned native-shaped homes without reads or implicit writes. */
import { createInstance } from 'i18next';
import { expect, test } from '@playwright/test';
import type { Org } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { PEOPLE, scenario, signIn } from './support';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
// Match the component proof's24 owned roots and choose the final one, beyond a short dropdown's first ten.
const HOMES = 24;

for (const [locale, theme, width] of [['en', 'light', 1440], ['zh-CN', 'dark', 400]] as const) test(`home locators preserve hierarchy, help and explicit creation in ${locale}/${theme}/${width}`, async ({ page, request }, info) => {
  await scenario();
  const login = await request.post(`${DOUBLE}/api/v1/auth/login`, { data: { login: PEOPLE.ada.login, password: PEOPLE.ada.password } }); expect(login.ok()).toBe(true);
  const { access_token: token } = await login.json() as { access_token: string };
  const headers = { Authorization: `Bearer ${token}` };
  const homes: Org[] = [];
  for (let index = 0; index < HOMES; index += 1) {
    const made = await request.post(`${DOUBLE}/api/v1/orgs`, { headers, data: { name: `Locator home ${String(index + 1).padStart(2, '0')}`, slug: `locator-home-${index + 1}` } });
    expect(made.ok()).toBe(true); homes.push(await made.json() as Org);
  }
  const late = homes.at(-1); if (late === undefined) throw new Error('The real fixture did not create a late owned home');
  const preference = await request.patch(`${DOUBLE}/api/v1/me`, { headers, data: { locale, theme } }); expect(preference.ok()).toBe(true);
  const catalogue = createInstance(); await catalogue.init({ lng: locale, resources: { en: { translation: english }, 'zh-CN': { translation: chinese } } });
  const t = catalogue.getFixedT(locale);
  await page.setViewportSize({ width, height: 900 }); await signIn(page); await page.goto('/resources/agents');
  await expect(page.locator('html')).toHaveAttribute('lang', locale); await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
  const reads: string[] = [], writes: string[] = [];
  page.on('request', (call) => {
    const path = new URL(call.url()).pathname.replace('/api/v1', '');
    if (call.method() === 'GET' && (path === '/orgs' || /^\/orgs\/[^/]+\/tree$/.test(path) || /^\/nodes\/[^/]+$/.test(path))) reads.push(path);
    if (call.method() === 'POST' && /^\/orgs\/[^/]+\/agents$/.test(path)) writes.push(path);
  });
  await page.locator('[data-resource-kind="agents"]').getByRole('button').click();
  const dialog = page.getByRole('dialog', { name: t('agents.add.title'), exact: true }); await expect(dialog).toBeVisible();
  await dialog.getByRole('combobox', { name: t('agents.add.harness'), exact: true }).click();
  await page.getByRole('option', { name: 'Codex', exact: true }).click();
  const madeName = `Locator profile ${locale}`; await dialog.getByLabel(t('agents.add.name'), { exact: true }).fill(madeName);
  const field = dialog.getByRole('combobox', { name: t('resources.home.label'), exact: true });
  await expect(field).toHaveValue('Acme');
  await field.focus();
  for (const home of homes) await expect(page.getByRole('option', { name: home.name, exact: true })).toBeAttached();
  const department = ['Acme', 'Research', 'ML'].join(t('resources.home.pathJoin'));
  await expect(page.getByRole('option', { name: department, exact: true })).toBeAttached();
  const initial = [...reads];
  await field.fill('__no_such_home__'); await expect(page.getByText(t('ui.combobox.empty'), { exact: true })).toBeVisible();
  await field.press('Enter'); expect(writes).toEqual([]); expect(reads).toEqual(initial); await expect(dialog).toBeVisible();
  await field.press('Escape'); await expect(field).toHaveValue('Acme'); await expect(page.getByRole('listbox')).toHaveCount(0);
  await field.press('ArrowDown');
  for (const home of homes) await expect(page.getByRole('option', { name: home.name, exact: true })).toBeAttached();
  await field.fill(late.name); await expect(page.getByRole('option')).toHaveCount(1);
  const option = page.getByRole('option', { name: late.name, exact: true });
  await expect(option).toHaveAccessibleDescription(t('choiceCatalog.resourceHome.org.description', { name: late.name }));
  expect(reads).toEqual(initial); expect(writes).toEqual([]);
  await page.screenshot({ path: info.outputPath(`home-search-${locale}.png`), animations: 'disabled' });
  await field.press('Enter'); await expect(field).toHaveValue(late.name);
  await expect.poll(() => reads.slice(initial.length)).toEqual([`/orgs/${late.id}/tree`]);
  const afterChoice = [...reads];
  await dialog.getByRole('button', { name: t('help.hint.label', { term: t('help.hints.home.title') }), exact: true }).click();
  await expect(page.getByText(t('help.hints.home.what'), { exact: true })).toBeVisible();
  await expect(page.getByText(t('help.hints.home.which'), { exact: true })).toBeVisible();
  await page.screenshot({ path: info.outputPath(`home-help-${locale}.png`), animations: 'disabled' });
  await page.keyboard.press('Escape'); await expect(dialog).toBeVisible(); expect(reads).toEqual(afterChoice); expect(writes).toEqual([]);
  const created = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname === `/api/v1/orgs/${late.id}/agents`);
  await dialog.getByRole('button', { name: t('agents.add.submit'), exact: true }).click(); expect((await created).ok()).toBe(true);
  await expect(page.getByRole('heading', { name: madeName, exact: true })).toBeVisible();
  expect(writes).toEqual([`/orgs/${late.id}/agents`]);
  await page.goto('/resources/providers'); await page.locator('[data-resource-kind="providers"]').getByRole('button').click();
  const provider = page.getByRole('dialog', { name: t('providers.add.title'), exact: true }); await expect(provider).toBeVisible();
  const compact = provider.getByRole('combobox', { name: t('resources.home.label'), exact: true }); await compact.focus();
  await expect(page.getByRole('option', { name: late.name, exact: true })).toBeAttached();
  for (const row of await page.getByRole('option').all()) await expect(row).toHaveAccessibleDescription('');
  await compact.press('Escape');
  // This form shares guide destinations; keep its named task guide,
  // rather than require another copy of the home hint beside the compact field.
  await expect(provider.getByRole('button', { name: t('taskGuides.open', { title: t('taskGuides.addProvider') }), exact: true })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: info.outputPath(`home-compact-${locale}.png`), animations: 'disabled' });
});

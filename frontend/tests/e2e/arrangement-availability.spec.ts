/** Walk the shipping creation and switch controls without hiding native arrangement refusals. */
import { expect, type Locator, type Page, type Request, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import type { ArrangementConsent, ProviderListedPage, ProviderSetup } from '../../src/generated/api-types';
import { MAIN, openHistory } from './change-support';
import { scenario, signIn } from './support';

async function language(page: Page, locale: 'en' | 'zh-CN') {
  if (locale === 'zh-CN') {
    await page.goto('/settings/interface');
    await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
    await page.getByRole('option', { name: '简体中文', exact: true }).click();
  }
  await expect(page.locator('html')).toHaveAttribute('lang', locale);
}

async function fits(control: Locator, width: number) {
  const bounds = await control.boundingBox();
  if (bounds === null) throw new Error('the actual arrangement control is not visible');
  expect(bounds.x).toBeGreaterThanOrEqual(0);
  expect(bounds.x + bounds.width).toBeLessThanOrEqual(width + 1);
}

const switchConsents: ArrangementConsent[] = [
  { type: 'separated', allowed: true, available: true, puts: '', form: null },
  { type: 'key_separated', allowed: false, available: true, puts: '', form: null },
  { type: 'credential_shared', allowed: true, available: false, puts: '', form: null },
  { type: 'handed_over', allowed: false, available: false, puts: '', form: null },
];

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`switch arrangement reasons ${locale}/${theme}`, async ({ page }, info) => {
    await scenario();
    await page.emulateMedia({ colorScheme: theme });
    await page.route(/\/api\/v1\/providers(?:\?|$)/, async (route) => {
      if (route.request().method() !== 'GET') { await route.continue(); return; }
      const response = await route.fetch();
      const data = await response.json() as ProviderListedPage;
      await route.fulfill({ response, json: { ...data, items: data.items.map((row) =>
        'arrangements' in row ? { ...row, arrangements: switchConsents } : row) } });
    });
    await openHistory(page);
    await language(page, locale);
    const words = locale === 'en' ? english : chinese;
    for (const width of [1280, 400]) {
      await page.setViewportSize({ width, height: 1000 });
      await page.goto(`/thread/${MAIN}`);
      await page.getByRole('button', { name: words.change.menu.switch, exact: true }).click();
      const form = page.getByRole('dialog').filter({ has: page.locator('[data-thread-change-pickers]') });
      const provider = form.getByRole('combobox', { name: words.switchChoice.picker, exact: true });
      const fromOpenAI = (await provider.textContent())?.includes('OpenAI key') === true;
      await provider.click();
      await page.getByRole('option', { name: fromOpenAI ? /Team Anthropic/ : /OpenAI key/ }).first().click();
      await form.locator('[data-change-runs] > details > summary').click();
      const group = form.getByRole('radiogroup', { name: words.start.arrangement.label, exact: true });
      const ownerRefused = group.getByRole('radio', { name: words.choiceCatalog.arrangement.key_separated.name, exact: true });
      const machineRefused = group.getByRole('radio', { name: words.choiceCatalog.arrangement.credential_shared.name, exact: true });
      await expect(group.getByRole('radio')).toHaveCount(5);
      await expect(ownerRefused).toBeDisabled();
      await expect(ownerRefused).toHaveAttribute('aria-description', words.arrangementAvailability.owner);
      await expect(machineRefused).toBeDisabled();
      await expect(machineRefused).toHaveAttribute('aria-description', words.arrangementAvailability.machine);
      await expect(machineRefused.getByText(words.arrangementAvailability.machine, { exact: true })).toBeVisible();
      await expect(machineRefused.locator('svg')).toHaveCount(1);
      const allowed = group.getByRole('radio', { name: words.choiceCatalog.arrangement.separated.name, exact: true });
      await allowed.click();
      await expect(allowed).toBeChecked();
      await expect(page.getByRole('alertdialog')).toHaveCount(0);
      await fits(group, width);
      await info.attach(`switch-arrangements-${locale}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
      await form.getByRole('button', { name: words.common.cancel, exact: true }).click();
      await expect(form).toHaveCount(0);
    }
  });

  test(`creation arrangement reasons ${locale}/${theme}`, async ({ page }, info) => {
    await scenario();
    await page.emulateMedia({ colorScheme: theme });
    let keeperName: string | undefined;
    await page.route('**/api/v1/provider-setup?*', async (route) => {
      const response = await route.fetch();
      const data = await response.json() as ProviderSetup;
      const keeper = data.machines?.find((choice) => choice.roles.includes('provider'));
      if (keeper === undefined) throw new Error('the native creation fixture needs a provider keeper');
      keeperName = keeper.machine.name;
      await route.fulfill({ response, json: { ...data,
        api_arrangements: data.api_arrangements.map((consent) => ({ ...consent, allowed: consent.type === 'separated' })),
        machines: data.machines?.map((choice) => ({ ...choice, arrangements: ['separated', 'key_separated'] })),
      } });
    });
    await signIn(page);
    await language(page, locale);
    const words = locale === 'en' ? english : chinese;
    for (const width of [1280, 400]) {
      await page.setViewportSize({ width, height: 1000 });
      await page.goto('/resources/providers');
      await page.locator('[data-resource-kind="providers"]').getByRole('button', { name: words.providers.list.add, exact: true }).click();
      const form = page.getByRole('dialog', { name: words.providers.add.title, exact: true });
      await form.getByRole('combobox', { name: words.resources.home.label, exact: true }).click();
      await page.getByRole('option', { name: 'Acme', exact: true }).click();
      await form.getByRole('combobox', { name: words.providerCreation.examples, exact: true }).click();
      await page.getByRole('option', { name: 'OpenAI', exact: true }).click();
      await form.getByRole('button', { name: words.common.next, exact: true }).click();
      await form.locator('details > summary').filter({ hasText: words.providers.add.arrangements }).click();
      const proposed = form.getByRole('checkbox', { name: words.providerCreation.arrangements.separated, exact: true });
      const optional = form.getByRole('checkbox', { name: words.providerCreation.arrangements.key_separated, exact: true });
      await expect(proposed).toBeChecked();
      await expect(optional).toBeEnabled();
      await expect(optional).not.toBeChecked();
      await optional.click();
      await expect(optional).toBeChecked();
      const keeper = keeperName;
      if (keeper === undefined) throw new Error('the setup read must name its keeper');
      await form.getByRole('combobox', { name: words.providerCreation.keyMachine, exact: true }).click();
      await page.getByRole('option', { name: keeper, exact: true }).click();
      const refused = form.getByRole('checkbox', { name: words.providerCreation.arrangements.credential_shared, exact: true });
      await expect(refused).toBeDisabled();
      await expect(refused).not.toBeChecked();
      await expect(refused).toHaveAccessibleDescription(words.arrangementAvailability.machine);
      await expect(optional).toBeChecked();
      await expect(page.getByRole('alertdialog')).toHaveCount(0);
      await fits(form.locator('details').filter({ has: refused }), width);
      await info.attach(`creation-arrangements-${locale}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
      const writes: Request[] = [];
      const record = (request: Request) => { if (request.method() === 'POST' && new URL(request.url()).pathname === '/api/v1/providers') writes.push(request); };
      page.on('request', record);
      const name = `Explicit setup ${width}`;
      await form.getByLabel(words.providers.add.name, { exact: true }).fill(name);
      expect(writes).toHaveLength(0);
      const sent = page.waitForRequest((request) => request.method() === 'POST' && new URL(request.url()).pathname === '/api/v1/providers');
      await form.getByRole('button', { name: words.providerCreation.create, exact: true }).click();
      expect((await sent).postDataJSON()).toMatchObject({ name, arrangements: ['separated', 'key_separated'] });
      await expect(form).toHaveCount(0);
      expect(writes).toHaveLength(1);
      page.off('request', record);
    }
  });
}

/** The human check's card in Management → Sign-in: only the choice of service where none is chosen, the controls of the service where one is, the mode only for hCaptcha, and a secret that is written and never shown again.
 *
 * With `ORCH_HUMAN_CHECK_SHOTS` set to a directory, each state is also written there as a picture.
 */
import { mkdirSync } from 'node:fs';
import { join } from 'node:path';

import { expect, type Locator, type Page, test } from '@playwright/test';

import { signInForManagement } from './admin-mode-support.ts';
import { PEOPLE, scenario } from './support.ts';


test.beforeEach(({ page }) => {
  page.on('console', (msg) => { if (msg.type() === 'error') process.stdout.write('Browser error: ' + msg.text().split('\n')[0]?.slice(0, 400) + '\n'); });
  page.on('pageerror', (err) => process.stdout.write('Page error: ' + err.message.slice(0, 400) + '\n'));
});
test.afterEach(async ({ page }, info) => {
  if (info.status === info.expectedStatus || page.isClosed()) return;
  const facts = await page.evaluate(() => ({
    path: location.pathname, lang: document.documentElement.lang,
    headings: [...document.querySelectorAll('h1,h2,h3')].map((el) => el.textContent?.trim()).slice(-35),
    status: [...document.querySelectorAll('[role="alert"]')].map((el) => el.textContent?.trim().slice(0, 220)).slice(-5),
    admin: document.querySelector('[data-as-admin]')?.getAttribute('data-as-admin'),
    lastTab: document.getElementById('tab-grid-content-159')?.outerHTML.split('>')[0]?.slice(0, 560),
    finder: document.querySelector('[role="dialog"] [role="combobox"]')?.getAttribute('value'),
    options: [...document.querySelectorAll('[role="dialog"] [role="option"]')].map((el) => [el.getAttribute('data-value'),el.getAttribute('aria-label')]).slice(0, 10),
    humanCard: document.querySelector('#admin-human-check')?.parentElement?.parentElement?.textContent?.trim().slice(0, 280),
    provider: !!document.querySelector('[data-setting="human_check.provider"]'),
  }));
  process.stdout.write('Page at failed assertion: ' + JSON.stringify(facts) + '\n');
});
const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

/** What each state of the card is, by the settings the fake system holds. */
const STATES = {
  off: {},
  hcaptcha: { 'human_check.provider': 'hcaptcha', 'human_check.site_key': '10000000-ffff-ffff-ffff-000000000001', 'human_check.mode': 'checkbox' },
  recaptcha: { 'human_check.provider': 'recaptcha', 'human_check.site_key': '6Lc-the-site-key' },
} as const;

const WORDS = {
  en: { card: 'Human check', service: 'Service', siteKey: 'Site key', widget: 'Widget', unseen: 'Runs unseen: it scores people without a box.', secret: { hcaptcha: 'Secret', recaptcha: 'Secret key' }, none: 'None is kept.', set: 'Save', test: 'Test', ok: 'The service answered and accepts the keys.' },
  'zh-CN': { card: '人机验证', service: '服务', siteKey: '网站密钥', widget: '组件', unseen: '在后台运行：不显示复选框，直接为人打分。', secret: { hcaptcha: '私钥', recaptcha: '私钥' }, none: '尚未保存。', set: '保存', test: '测试', ok: '服务已响应，并接受这些密钥。' },
} as const;

async function put(key: string, value: unknown): Promise<void> {
  // The administrator's own writes go through the page; the states are put in place by the fake system directly.
  const response = await fetch(`${DOUBLE}/__double/admin/setting`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ key, value }) });
  if (!response.ok) throw new Error(`the fake API refused ${key}: ${response.status}`);
}

/** The card as the page shows it: scrolled to the top of the window the page scrolls in, the window tall enough to hold it. */
async function shot(page: Page, card: Locator, name: string): Promise<void> {
  const where = process.env.ORCH_HUMAN_CHECK_SHOTS;
  if (where === undefined || where === '') return;
  mkdirSync(where, { recursive: true });
  await card.evaluate((element) => element.scrollIntoView({ block: 'start' }));
  await page.screenshot({ path: join(where, `${name}.png`) });
}

for (const locale of ['en', 'zh-CN'] as const) for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) for (const state of ['off', 'hcaptcha', 'recaptcha'] as const) {
  test(`the card with ${state} chosen at ${width}px in ${locale}/${theme}`, async ({ page }) => {
    const w = WORDS[locale];
    await scenario({ scenario: 'team' });
    await page.setViewportSize({ width, height: 1300 });
    await page.emulateMedia({ colorScheme: theme });
    await page.addInitScript(({ theme }) => localStorage.setItem('orch.appearance', JSON.stringify({ locale: 'en', theme, density: 'comfortable' })), { theme });
    // The sign-in is made in English, as the shared helper reads it, before a check is asked of it; the states are then put in
    // place, and the card is opened in the language asked for, chosen where a person chooses it.
    await signInForManagement(page, PEOPLE.ada);
    for (const [key, value] of Object.entries(STATES[state])) await put(key, value);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    await page.goto('/admin/sign-in');
    const card = page.getByRole('region', { name: w.card, exact: true });
    await expect(card.getByRole('heading', { name: w.card })).toBeVisible();
    await expect(card.getByLabel(w.service, { exact: true })).toBeVisible();
    if (state === 'off') {
      await expect(card.getByLabel(w.siteKey, { exact: true })).toHaveCount(0);
    } else {
      await expect(card.getByLabel(w.siteKey, { exact: true })).toBeVisible();
      // The mode is hCaptcha's alone; reCAPTCHA says in one line that it runs unseen.
      await expect(card.getByLabel(w.widget, { exact: true })).toHaveCount(state === 'hcaptcha' ? 1 : 0);
      await expect(card.getByText(w.unseen)).toHaveCount(state === 'recaptcha' ? 1 : 0);
      await expect(card.getByLabel(w.secret[state], { exact: true })).toHaveAttribute('type', 'password');
    }
    await shot(page, card, `management-card-${state}-${locale}-${width}-${theme}`);
  });
}

test('the secret is set, kept without being shown, tested and taken away', async ({ page }) => {
  await scenario({ scenario: 'team' });
  await signInForManagement(page, PEOPLE.ada);
  for (const [key, value] of Object.entries(STATES.hcaptcha)) await put(key, value);
  await page.goto('/admin/sign-in');
  const card = page.getByRole('region', { name: 'Human check', exact: true });
  await expect(card.getByText('None is kept.')).toBeVisible();
  await expect(card.getByRole('button', { name: 'Test', exact: true })).toBeDisabled();
  await card.getByLabel('Secret', { exact: true }).fill('the-secret-of-the-test');
  await card.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(card.getByText(/^Kept, set /)).toBeVisible();
  await expect(card.getByLabel('Secret', { exact: true })).toHaveValue('');
  await expect(page.locator('body')).not.toContainText('the-secret-of-the-test');
  await card.getByRole('button', { name: 'Test', exact: true }).click();
  await expect(card.getByText('The service answered and accepts the keys.')).toBeVisible();
  await card.getByRole('button', { name: 'Remove', exact: true }).click();
  await expect(card.getByText('None is kept.')).toBeVisible();
});

/** What the web edge of a release before the human check leaves out for hCaptcha, as the server reports it. */
const BLOCKS = { state: 'blocks', via: 'stack', lacking: ['https://hcaptcha.com in script-src', 'https://*.hcaptcha.com in script-src', 'https://hcaptcha.com in frame-src'] };

async function edge(state: Record<string, unknown>): Promise<void> {
  const response = await fetch(`${DOUBLE}/__double/admin/edge`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(state) });
  if (!response.ok) throw new Error(`the fake API refused the edge: ${response.status}`);
}

test('the service cannot be chosen while the web edge has no room for it, and can once it has', async ({ page }) => {
  await scenario({ scenario: 'team' });
  await signInForManagement(page, PEOPLE.ada);
  await edge(BLOCKS);
  await page.goto('/admin/sign-in');
  const card = page.getByRole('region', { name: 'Human check', exact: true });
  const service = card.getByLabel('Service', { exact: true });
  await service.click();
  await page.getByRole('option', { name: 'hCaptcha', exact: true }).click();
  // Refused, with words and the way on, and nothing of the service is switched on: no key is asked for.
  await expect(card.getByText(/does not let pages reach this service yet/)).toBeVisible();
  await expect(card.getByText(/operator guide/)).toBeVisible();
  await expect(card.getByLabel('Site key', { exact: true })).toHaveCount(0);
  // The edge is set; the same choice, tried again, is let through.
  await edge({ state: 'allows', via: 'stack', lacking: [] });
  await card.getByRole('button', { name: 'Try again', exact: true }).click();
  await expect(card.getByLabel('Site key', { exact: true })).toBeVisible();
  await expect(card.getByText(/does not let pages reach this service yet/)).toHaveCount(0);
  await expect(card.getByText(/web edge/)).toHaveCount(0);
});

for (const locale of ['en', 'zh-CN'] as const) for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) {
  test(`the card names the hosts the web edge leaves out at ${width}px in ${locale}/${theme}`, async ({ page }) => {
    await scenario({ scenario: 'team' });
    await page.setViewportSize({ width, height: 1300 });
    await page.emulateMedia({ colorScheme: theme });
    await page.addInitScript(({ theme }) => localStorage.setItem('orch.appearance', JSON.stringify({ locale: 'en', theme, density: 'comfortable' })), { theme });
    await signInForManagement(page, PEOPLE.ada);
    for (const [key, value] of Object.entries(STATES.hcaptcha)) await put(key, value);
    await edge(BLOCKS);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    await page.goto('/admin/sign-in');
    const card = page.getByRole('region', { name: WORDS[locale].card, exact: true });
    // Each host once, however many directives it is left out of.
    const note = card.getByText(/https:\/\/hcaptcha\.com, https:\/\/\*\.hcaptcha\.com/);
    await expect(note).toBeVisible();
    await expect(card.getByText(/frame-src|script-src/)).toHaveCount(0);
    await shot(page, card, `management-card-edge-blocked-${locale}-${width}-${theme}`);
    await edge({ state: 'allows', via: 'stack', lacking: [] });
  });
}

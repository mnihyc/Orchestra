/** The human check in a real browser: the box above the button, a token for each try, the sentence under the button, and a page that is as it was where none is asked.
 *
 * The vendors' scripts are stood in by routes, so that a run needs no outside network; the one case that loads hCaptcha's real script
 * with its public test key carries the tag `@online` and is skipped, with its reason, where the network is not there. With
 * `ORCH_HUMAN_CHECK_SHOTS` set to a directory, the screens are also written there as pictures.
 */
import { randomBytes } from 'node:crypto';
import { mkdirSync } from 'node:fs';
import { join } from 'node:path';

import { expect, type Page, test } from '@playwright/test';

import type { Meta } from '../../src/generated/api-types.ts';
import { PEOPLE, scenario } from './support.ts';

const HCAPTCHA_TEST_SITE_KEY = '10000000-ffff-ffff-ffff-000000000001';
const HCAPTCHA_TEST_TOKEN = '10000000-aaaa-bbbb-cccc-000000000001';
const ACTS: NonNullable<Meta['human_check']>['acts'] = ['sign_in', 'email_code', 'register', 'recovery', 'resend'];

/** What stands in for hCaptcha's script: a box that is ticked by a click, reset by the page, and an unseen run that gives a token. */
const HCAPTCHA_STAND_IN = `(function () {
  var count = 0, boxes = {};
  window.hcaptcha = {
    render: function (container, params) {
      count += 1; var id = 'widget-' + count;
      var box = document.createElement('button'); box.type = 'button';
      box.setAttribute('role', 'checkbox'); box.setAttribute('aria-checked', 'false'); box.setAttribute('aria-label', 'I am human');
      var dark = params.theme === 'dark';
      box.textContent = 'I am human';
      box.style.cssText = 'width:' + (params.size === 'compact' ? 164 : 303) + 'px;height:78px;text-align:left;padding:0 16px;border:1px solid #888;border-radius:4px;'
        + 'background:' + (dark ? '#262626' : '#fafafa') + ';color:' + (dark ? '#eee' : '#222') + ';font:14px sans-serif';
      box.onclick = function () { box.setAttribute('aria-checked', 'true'); box.textContent = 'You are human'; params.callback('stand-in-token-' + id); };
      container.appendChild(box); boxes[id] = box; return id;
    },
    reset: function (id) { var box = boxes[id]; if (box) { box.setAttribute('aria-checked', 'false'); box.textContent = 'I am human'; } },
    execute: function () { count += 1; return Promise.resolve({ response: 'unseen-token-' + count, key: 'k' }); },
  };
  if (window.orchHcaptchaLoaded) window.orchHcaptchaLoaded();
})();`;

/** What stands in for Google's script, in the form of `api.js` and of `enterprise.js`: an unseen run that gives a token for the action. */
const GOOGLE_STAND_IN = `(function () {
  var api = { ready: function (callback) { callback(); }, execute: function (key, options) { return Promise.resolve('stand-in-' + options.action); } };
  window.grecaptcha = { ready: api.ready, execute: api.execute, enterprise: api };
})();`;

async function standIn(page: Page): Promise<void> {
  await page.route('https://js.hcaptcha.com/**', (route) => route.fulfill({ contentType: 'application/javascript', body: HCAPTCHA_STAND_IN }));
  await page.route(/https:\/\/www\.(google|recaptcha)\.(com|net)\/recaptcha\/.*/, (route) => route.fulfill({ contentType: 'application/javascript', body: GOOGLE_STAND_IN }));
}

/** The edge policy when the operator has not enabled a human-check provider. */
const POLICY_WITHOUT_VENDOR = "default-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self'; script-src 'self' 'nonce-__ORCH_CSP_NONCE__' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' blob: https:; font-src 'self'; connect-src 'self'; worker-src 'self'; manifest-src 'self'";

/** Apply the same per-response nonce to the application scripts and policy that the deployed edge sends. */
async function edgeWithoutVendor(page: Page): Promise<void> {
  await page.route('**/*', async (route) => {
    if (route.request().resourceType() !== 'document') return route.fallback();
    const response = await route.fetch();
    const nonce = randomBytes(16).toString('hex');
    const body = (await response.text()).replaceAll('__ORCH_CSP_NONCE__', nonce);
    const headers = response.headers();
    delete headers['content-encoding'];
    delete headers['content-length'];
    headers['content-security-policy'] = POLICY_WITHOUT_VENDOR.replace('__ORCH_CSP_NONCE__', nonce);
    return route.fulfill({ response, headers, body });
  });
}

async function look(page: Page, locale: 'en' | 'zh-CN', theme: 'light' | 'dark', width: number): Promise<void> {
  await page.addInitScript(({ locale, theme }) => {
    localStorage.setItem('orch.appearance', JSON.stringify({ locale, theme, density: 'comfortable' }));
  }, { locale, theme });
  await page.setViewportSize({ width, height: 900 });
  await page.emulateMedia({ colorScheme: theme });
}

async function shot(page: Page, name: string): Promise<void> {
  const where = process.env.ORCH_HUMAN_CHECK_SHOTS;
  if (where === undefined || where === '') return;
  mkdirSync(where, { recursive: true });
  await page.screenshot({ path: join(where, `${name}.png`), fullPage: true });
}

const asking = (over: Partial<NonNullable<Meta['human_check']>> = {}): NonNullable<Meta['human_check']> => ({
  provider: 'hcaptcha', site_key: HCAPTCHA_TEST_SITE_KEY, mode: 'checkbox', acts: ACTS, alternate_host: false, wait_secs: 20, ...over,
});

const WORDS = {
  en: { signIn: 'Sign in', create: 'Create account', username: 'Username', login: 'Username or email address', password: 'Password', spent: 'Each try uses up the check. Confirm it again.', recaptcha: 'This site is protected by reCAPTCHA.' },
  'zh-CN': { signIn: '登录', create: '创建账号', username: '用户名', login: '用户名或邮箱地址', password: '密码', spent: '每次尝试都会用掉验证，请重新验证。', recaptcha: '本网站受 reCAPTCHA 保护。' },
} as const;

for (const locale of ['en', 'zh-CN'] as const) for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) {
  const w = WORDS[locale];
  const tag = `${locale}-${width}-${theme}`;

  test(`sign-in asks for the box, sends its token once and says why it asks again at ${width}px in ${locale}/${theme}`, async ({ page }) => {
    await scenario({ scenario: 'team', meta: { registration_mode: 'open', human_check: asking() } });
    await standIn(page);
    await look(page, locale, theme, width);
    const sent: (string | undefined)[] = [];
    page.on('request', (request) => {
      if (request.method() === 'POST' && new URL(request.url()).pathname === '/api/v1/auth/login') sent.push(request.headers()['x-human-check']);
    });
    await page.goto('/login');
    const box = page.getByRole('checkbox', { name: 'I am human' });
    await expect(box).toBeVisible();
    await page.getByLabel(w.login, { exact: true }).fill(PEOPLE.ada.login);
    await page.getByLabel(w.password, { exact: true }).fill('not the password at all');
    const button = page.getByRole('button', { name: w.signIn, exact: true });
    await expect(button).toBeDisabled();
    await shot(page, `sign-in-box-${tag}`);
    await box.click();
    await expect(button).toBeEnabled();
    await button.click();
    await expect.poll(() => sent.length).toBe(1);
    expect(sent[0]).toMatch(/^stand-in-token-/);
    // The try used the token up: the box is fresh, drawn, and one line says why; the button waits for it.
    await expect(page.getByText(w.spent)).toBeVisible();
    await expect(box).toHaveAttribute('aria-checked', 'false');
    await expect(button).toBeDisabled();
    await shot(page, `sign-in-spent-${tag}`);
  });

  test(`registration asks for the box above its button at ${width}px in ${locale}/${theme}`, async ({ page }) => {
    await scenario({ scenario: 'team', meta: { registration_mode: 'open', human_check: asking() } });
    await standIn(page);
    await look(page, locale, theme, width);
    await page.goto('/register');
    await expect(page.getByRole('checkbox', { name: 'I am human' })).toBeVisible();
    await expect(page.getByRole('button', { name: w.create, exact: true })).toBeDisabled();
    await shot(page, `register-box-${tag}`);
  });

  test(`an unseen check draws no box and says what the service asks under the button at ${width}px in ${locale}/${theme}`, async ({ page }) => {
    await scenario({ scenario: 'team', meta: { registration_mode: 'open', human_check: asking({ provider: 'recaptcha', mode: 'invisible', verification: 'siteverify' }) } });
    await standIn(page);
    await look(page, locale, theme, width);
    const sent: (string | undefined)[] = [];
    page.on('request', (request) => {
      if (request.method() === 'POST' && new URL(request.url()).pathname === '/api/v1/auth/login') sent.push(request.headers()['x-human-check']);
    });
    await page.goto('/login');
    await expect(page.getByText(w.recaptcha)).toBeVisible();
    await expect(page.getByRole('checkbox')).toHaveCount(0);
    await page.getByLabel(w.login, { exact: true }).fill(PEOPLE.ada.login);
    // A refused password leaves the page signed out, as the sentence on the registration page below needs.
    await page.getByLabel(w.password, { exact: true }).fill('not the password at all');
    await shot(page, `sign-in-unseen-${tag}`);
    await page.getByRole('button', { name: w.signIn, exact: true }).click();
    await expect.poll(() => sent.length).toBe(1);
    expect(sent[0]).toBe('stand-in-orch_sign_in');
    await page.goto('/register');
    await expect(page.getByText(w.recaptcha)).toBeVisible();
    await shot(page, `register-unseen-${tag}`);
  });
}

test('a page where no human check is asked makes the requests it always made, and none to a service', async ({ page }) => {
  const home = new URL(String(test.info().project.use.baseURL)).origin;
  const requests = async (meta: Record<string, unknown>): Promise<string[]> => {
    await scenario({ scenario: 'team', meta });
    await standIn(page);
    const seen = new Set<string>();
    const note = (request: { method(): string; url(): string }) => {
      const url = new URL(request.url());
      seen.add(`${request.method()} ${url.origin === home ? '' : url.origin}${url.pathname.replace(/\/assets\/.*/, '/assets/*')}`);
    };
    page.on('request', note);
    await page.goto('/login');
    await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
    await page.goto('/register');
    await page.waitForLoadState('networkidle');
    page.off('request', note);
    return [...seen].sort();
  };
  const without = await requests({ registration_mode: 'open' });
  const none = await requests({ registration_mode: 'open', human_check: null });
  expect(none).toEqual(without);
  // Nothing was asked of a vendor, and nothing left the application's own address.
  expect(without.filter((line) => /hcaptcha|google|recaptcha|gstatic/.test(line))).toEqual([]);
  expect(without.filter((line) => /https?:\/\//.test(line))).toEqual([]);
});

test('a score-only check on a screen whose act is not asked adds no script @human-check', async ({ page }) => {
  await scenario({ scenario: 'team', meta: { registration_mode: 'open', mail: true, human_check: asking({ acts: ['register'] }) } });
  const vendor: string[] = [];
  await page.route('https://js.hcaptcha.com/**', (route) => { vendor.push(route.request().url()); return route.abort(); });
  await page.goto('/recover');
  await page.waitForLoadState('networkidle');
  expect(vendor).toEqual([]);
});

test('hCaptcha with its public test key passes in the page and its token is sent @online', async ({ page, request }) => {
  const reached = await request.get('https://js.hcaptcha.com/1/api.js', { timeout: 8_000 }).then((reply) => reply.ok(), () => false);
  test.skip(!reached, 'hCaptcha cannot be reached from here: this case loads its real script, with its public test key');
  await scenario({ scenario: 'team', meta: { registration_mode: 'open', human_check: asking() } });
  const sent: (string | undefined)[] = [];
  page.on('request', (call) => {
    if (call.method() === 'POST' && new URL(call.url()).pathname === '/api/v1/auth/login') sent.push(call.headers()['x-human-check']);
  });
  await page.goto('/login');
  await page.getByLabel('Username or email address', { exact: true }).fill(PEOPLE.ada.login);
  await page.getByLabel('Password', { exact: true }).fill(PEOPLE.ada.password);
  // The key never challenges: ticking its box in its own frame is all a person does.
  const frame = page.locator('iframe[src*="hcaptcha.com"][src*="checkbox"]').first().contentFrame();
  await frame.locator('#checkbox').click({ timeout: 30_000 });
  const button = page.getByRole('button', { name: 'Sign in', exact: true });
  await expect(button).toBeEnabled({ timeout: 30_000 });
  await button.click();
  await expect.poll(() => sent.length).toBe(1);
  expect(sent[0]).toBe(HCAPTCHA_TEST_TOKEN);
});

for (const mode of ['box', 'unseen'] as const) {
  test(`behind an edge whose policy has no room for the service the browser blocks the script, and the try is made with the word that says so, ${mode}`, async ({ page }) => {
    await scenario({ scenario: 'team', meta: { registration_mode: 'open', human_check: mode === 'box' ? asking() : asking({ provider: 'recaptcha', mode: 'invisible', verification: 'siteverify' }) } });
    const reached: string[] = [];
    await page.route(/https:\/\/(js\.hcaptcha\.com|www\.google\.com)\/.*/, (route) => { reached.push(route.request().url()); return route.abort(); });
    await edgeWithoutVendor(page);
    const blocked: string[] = [];
    await page.addInitScript(() => document.addEventListener('securitypolicyviolation', (event) => {
      (window as unknown as { __blocked: string[] }).__blocked = [...((window as unknown as { __blocked?: string[] }).__blocked ?? []), `${event.violatedDirective} ${event.blockedURI}`];
    }));
    const sent: (string | undefined)[] = [];
    page.on('request', (request) => {
      if (request.method() === 'POST' && new URL(request.url()).pathname === '/api/v1/auth/login') sent.push(request.headers()['x-human-check']);
    });
    await page.goto('/login');
    await page.getByLabel('Username or email address', { exact: true }).fill(PEOPLE.ada.login);
    await page.getByLabel('Password', { exact: true }).fill('not the password at all');
    // The browser itself refused the service's script: the policy of the page names no service, and the request for the
    // script never left the page.
    await expect.poll(() => page.evaluate(() => (window as unknown as { __blocked?: string[] }).__blocked ?? [])).toEqual(expect.arrayContaining([expect.stringMatching(/^script-src.*(hcaptcha|google)/)]));
    blocked.push(...await page.evaluate(() => (window as unknown as { __blocked?: string[] }).__blocked ?? []));
    expect(reached).toEqual([]);
    await expect(page.getByText('The check could not be loaded.')).toBeVisible();
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    // Nothing could give a token, and the person is not left at a button that does nothing: the try is made with the
    // word that says the widget did not load, for the server to look into.
    await expect.poll(() => sent.length).toBe(1);
    expect(sent[0]).toBe('unloaded');
  });
}

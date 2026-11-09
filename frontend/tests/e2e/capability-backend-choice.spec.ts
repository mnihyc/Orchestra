/** EN: Settings → image backend → provider → eligible delegated route → Undo.
 * 中文：设置 → 图像后端 → 提供方 → 可用的委派路径 → 撤销。
 * These are screen cases; provider Use and native execution have independent engine/API cases. */
import { expect, test } from '@playwright/test';
import type { AuthResult, BackendRef, CapabilityBackendCandidate, Provider, ResolvedSetting } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { RUNS_ON } from '../doubles/fixtures';
import { ACME, DOUBLE, PEOPLE, scenario, signIn } from './support';

const KEY = 'agents.image_generation_backend';
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`a complete delegated backend is one explicit guarded choice ${locale}/${theme}/${width}`, async ({ page, request }, info) => {
    await scenario();
    const login = await request.post(`${DOUBLE}/api/v1/auth/login`, { data: PEOPLE.ada });
    expect(login.ok()).toBe(true);
    const auth = await login.json() as AuthResult;
    const headers = { Authorization: `Bearer ${auth.access_token}` };
    const read = await request.get(`${DOUBLE}/api/v1/providers/${RUNS_ON.codexSubscription}`, { headers });
    expect(read.ok()).toBe(true);
    const provider = await read.json() as Provider;
    // Start from an explicitly empty own value using the native settings writer.
    const empty = await request.put(`${DOUBLE}/api/v1/nodes/${ACME.web}/settings/${KEY}`, { headers, data: { value: null } });
    expect(empty.ok()).toBe(true);
    const before = await empty.json() as ResolvedSetting;
    expect((await request.post(`${DOUBLE}/__double/first-steps/appearance`, { data: { locale, theme } })).ok()).toBe(true);
    const backend: BackendRef = { route: 'image_generation.delegation', config: null, offering: { provider: provider.id, offering: 'image-model' } };
    const eligible = { backend, offering_name: 'Image model', eligible: true,
      route_text: { en: 'Delegated session', 'zh-CN': '委派会话' }, program_text: { en: 'Codex on ChatGPT', 'zh-CN': 'Codex（ChatGPT 登录）' }, reason: null, reason_text: {} } satisfies CapabilityBackendCandidate;
    const refused = { ...eligible, backend: { ...backend, offering: { provider: provider.id, offering: 'free-model' } }, offering_name: 'Free model', eligible: false,
      reason: 'image_generation.free_plan', reason_text: { en: 'Image generation is not offered on Free', 'zh-CN': 'Free 套餐不提供图像生成' } } satisfies CapabilityBackendCandidate;
    await page.route(`**/api/v1/providers/${provider.id}/capability-backends?**`, async (route) => {
      const query = new URL(route.request().url()).searchParams;
      expect(query.get('place')).toBe(ACME.web);
      expect(query.get('capability')).toBe('image_generation');
      await route.fulfill({ json: { items: query.has('offering') ? [eligible] : [refused, eligible], next_cursor: null } });
    });
    const writes: unknown[] = [];
    const settingPath = `/api/v1/nodes/${ACME.web}/settings/${KEY}`;
    page.on('request', (request) => { if (request.method() === 'PUT' && new URL(request.url()).pathname === settingPath) writes.push(request.postDataJSON()); });
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await signIn(page);
    await page.goto(`/project/${ACME.web}/settings?section=policies&setting=${KEY}`);
    const words = locale === 'en' ? english : chinese;
    const row = page.locator(`[data-setting="${KEY}"]`);
    const providerField = row.getByRole('combobox', { name: words.capabilityBackend.provider, exact: true });
    const routeField = row.getByRole('combobox', { name: words.capabilityBackend.route, exact: true });
    await expect(routeField).toBeDisabled();
    await providerField.click();
    await providerField.fill(provider.name);
    await page.getByRole('option', { name: provider.name, exact: true }).click();
    await expect(providerField).toHaveAttribute('aria-expanded', 'false');
    expect(writes).toEqual([]);
    await routeField.click();
    const list = page.getByRole('listbox', { name: words.capabilityBackend.route, exact: true });
    const free = list.getByRole('option').filter({ hasText: 'Free model' });
    await expect(free).toHaveAttribute('aria-disabled', 'true');
    await expect(free).toContainText(refused.reason_text[locale]);
    const choice = list.getByRole('option').filter({ hasText: 'Image model' });
    const program = eligible.program_text[locale];
    await expect(choice).toContainText(words.capabilityBackend.delegated.replace('{{program}}', program));
    await choice.click();
    await expect.poll(() => writes.length).toBe(1);
    expect(writes[0]).toEqual({ value: backend, expected_revision: before.own?.revision ?? 0 });
    await expect(routeField).toHaveValue(`${words.capabilityBackend.delegated.replace('{{program}}', program)} · Image model`);
    const undo = row.getByRole('button', { name: words.settings.compact.undoNamed.replace('{{name}}', words.settingRegistry.agents.image_generation_backend.title), exact: true });
    await undo.click();
    await expect.poll(() => writes.length).toBe(2);
    await expect(routeField).toBeDisabled();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    const screenshot = info.outputPath(`capability-backend-${locale}-${theme}-${width}.png`);
    await page.screenshot({ path: screenshot, fullPage: true, animations: 'disabled' });
    await info.attach('capability-backend', { path: screenshot, contentType: 'image/png' });
  });
}

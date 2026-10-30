/** The proposed end is visible and preselected on the shipping approval card at both widths and themes. */
import { expect, test } from '@playwright/test';
import { approvalsMade, askAccess } from './inbox-support.ts';
import { ACME, PEOPLE, bodySent, scenario, signIn } from './support.ts';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
const DAY_MS = 24 * 60 * 60 * 1000;

for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`an approver sees and accepts the requested end at ${width}px ${locale}/${theme}`, async ({ page, request }) => {
    await scenario();
    const end = new Date(Date.now() + 7 * DAY_MS).toISOString();
    const approval = await askAccess({ requester: 'dora', title: 'Review requested access', object: { kind: 'thread', id: ACME.mainThread, name: 'Main thread' }, permissions: ['thread.view'], expires_at: end });
    await page.setViewportSize({ width, height: 1000 });
    await signIn(page);
    const login = await request.post(`${DOUBLE}/api/v1/auth/login`, { data: { login: PEOPLE.ada.login, password: PEOPLE.ada.password } });
    expect(login.ok()).toBe(true);
    const { access_token: token } = await login.json() as { access_token: string };
    expect((await request.patch(`${DOUBLE}/api/v1/me`, { headers: { Authorization: `Bearer ${token}` }, data: { locale, theme } })).ok()).toBe(true);
    let requests = 0;
    page.on('request', (entry) => { if (new URL(entry.url()).pathname.startsWith('/api/')) requests += 1; });
    const words = locale === 'en' ? english : chinese;
    await page.goto('/approvals');
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    const card = page.locator(`[data-access-row="${approval.id}"]`);
    await expect(card).toBeVisible();
    await expect(card.getByRole('combobox', { name: words.approvals.access.ends, exact: true })).toContainText(words.access.request.asRequested);
    await expect(card.locator('[data-requested-end] time')).toHaveAttribute('datetime', end);
    const bounds = await card.boundingBox();
    expect(bounds).not.toBeNull();
    if (bounds === null) throw new Error('The approval has no bounds');
    expect(bounds.x).toBeGreaterThanOrEqual(0);
    expect(bounds.x + bounds.width).toBeLessThanOrEqual(width);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    await test.info().attach('API requests', { body: String(requests), contentType: 'text/plain' });
    const path = test.info().outputPath(`requested-end-${width}-${locale}-${theme}.png`);
    await page.screenshot({ path, animations: 'disabled' });
    await test.info().attach('Approval', { path, contentType: 'image/png' });
    const sent = page.waitForResponse((entry) => entry.request().method() === 'POST' && new URL(entry.url()).pathname.endsWith(`/approvals/${approval.id}/decision`));
    await card.getByRole('button', { name: words.actions.approve.named.replace('{{name}}', approval.title), exact: true }).click();
    const response = await sent;
    expect(response.ok()).toBe(true);
    expect(await bodySent(response.request())).toEqual({ approve: true, expires_at: end });
    await expect.poll(async () => (await approvalsMade()).find((item) => item.id === approval.id)?.decision).toEqual({ approve: true, expires_at: end });
  });
}

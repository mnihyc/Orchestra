/** A person follows a declared alias, checks its source and copies the public link in either language. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { chats } from './chat-support';
import { DOCS } from './documents-support';
import { ACME, DOUBLE, PEOPLE, scenario, signIn } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  test(`declared links open here and copy publicly (${locale}, ${width}, ${theme})`, async ({ page, request, context }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    const words = locale === 'en' ? english : chinese;
    const alias = `https://inside.orch.test/doc/${DOCS.notes}`;
    const foreign = `https://inside.orch.test.evil.test/doc/${DOCS.private}`;
    const item = await chats('/item', { thread: ACME.mainThread, kind: 'agent_message', text: `[Plan](${alias}) · [External](${foreign})` });
    await page.route((url) => ['/api/v1/meta', '/api/v1/me/bootstrap', '/api/v1/me/authority'].includes(url.pathname), async (route) => {
      const reply = await route.fetch();
      const body = await reply.json() as Record<string, unknown>;
      const meta = typeof body.meta === 'object' && body.meta !== null ? body.meta as Record<string, unknown> : body;
      meta.public_url = 'https://public.orch.test';
      meta.alternate_urls = ['https://inside.orch.test'];
      await route.fulfill({ response: reply, json: body });
    });
    await signIn(page);
    const login = await request.post(`${DOUBLE}/api/v1/auth/login`, { data: { login: PEOPLE.ada.login, password: PEOPLE.ada.password } });
    expect(login.ok()).toBe(true);
    const { access_token: token } = await login.json() as { access_token: string };
    expect((await request.patch(`${DOUBLE}/api/v1/me`, { headers: { Authorization: `Bearer ${token}` }, data: { locale, theme } })).ok()).toBe(true);
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    const foreignRequests: string[] = [];
    page.on('request', (one) => { if (new URL(one.url()).hostname.endsWith('.evil.test')) foreignRequests.push(one.url()); });
    await page.goto(`/thread/${ACME.mainThread}`);
    const row = page.locator(`[data-item="${String(item.id)}"]`).first();
    const chip = row.locator(`a[href="/doc/${DOCS.notes}"]`);
    await expect(chip).toBeVisible();
    await expect(row.getByRole('link', { name: 'External', exact: true })).toHaveAttribute('href', foreign);
    await chip.click();
    const card = page.locator('[data-reference-card]').filter({ has: page.getByRole('button', { name: words.people.reference.open, exact: true }) }).last();
    await expect(card).toBeVisible();
    await card.getByRole('button', { name: words.actions.copyLink.label, exact: true }).click();
    await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(`https://public.orch.test/doc/${DOCS.notes}`);
    await card.getByRole('button', { name: words.people.reference.open, exact: true }).click();
    await expect(page.getByRole('textbox', { name: words.documents.page.body, exact: true })).toBeVisible();
    expect(new URL(page.url()).pathname).toBe(`/doc/${DOCS.notes}`);
    expect(foreignRequests).toEqual([]);
    await page.screenshot({ path: info.outputPath(`orch-links-${locale}-${width}-${theme}.png`), animations: 'disabled' });
  });
}

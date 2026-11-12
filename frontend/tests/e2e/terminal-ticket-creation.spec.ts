/** Native ticket identities stay repeatable while first capabilities stay once-only. */
import { expect, test } from '@playwright/test';
import en from '../../src/i18n/en.json' with { type: 'json' };
import zh from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { RUNS_ON } from '../doubles/fixtures';
import { begin, MAIN, workspace } from './inspector-support';
import { signIn } from './support';
import { showWorkbenchView } from './workbench-support';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

for (const locale of ['en', 'zh-CN'] as const) for (const width of [400, 1440])
for (const theme of ['light', 'dark'] as const) for (const kind of ['shell', 'agent'] as const) {
  test(`terminal ticket renewal and safe acknowledgement in ${kind}/${locale}/${width}/${theme}`, async ({ page, request }, info) => {
    await begin();
    if (kind === 'shell') await workspace('/terminal-seed', { person: 'ada', count: 1, thread: MAIN });
    else {
      // The same advertised profile feeds both the initial detail and the live resource rows.
      const seeded = await request.post(`${DOUBLE}/__double/resources/agent-terminal`, { data: { agent_id: RUNS_ON.codex } });
      expect(seeded.status()).toBe(204);
    }
    await page.addInitScript(({ theme }) => {
      localStorage.setItem('orch.appearance', JSON.stringify({ locale: 'en', theme, density: 'comfortable' }));
    }, { theme });
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    const words = locale === 'en' ? en : zh;
    const attempts: { key: string | undefined; body: string | null }[] = [];
    const path = kind === 'shell' ? '**/api/v1/terminals/*/ticket' : '**/api/v1/threads/*/agent-terminal/ticket';
    await page.route(path, async (route) => {
      attempts.push({ key: route.request().headers()['idempotency-key'], body: route.request().postData() });
      if (attempts.length === 1) {
        // The invalid-token problem takes the client's session renewal and exact replay path.
        await route.fulfill({ status: 401, json: { type: 'orch:error:auth.invalid_token', code: 'auth.invalid_token', status: 401, title: 'Sign in', detail: '', request_id: 'renew-terminal-ticket' } });
      } else if (kind === 'shell') await route.continue();
      else await route.fulfill({ json: { projection: 'receipt', ticket_id: `00000000-0000-4000-8000-${String(attempts.length).padStart(12, '0')}`, ticket_expires_at: '2035-01-01T00:00:00Z' } });
    });
    const sockets: string[] = [];
    page.on('websocket', (socket) => { if (socket.url().includes('/ws/agent-terminals/')) sockets.push(socket.url()); });
    await page.goto(`/thread/${MAIN}`);
    await expect(page.getByRole('heading', { name: 'Main thread' })).toBeVisible();
    await showWorkbenchView(page, kind === 'shell' ? 'Terminal' : 'Diagnostics');
    if (kind === 'shell') {
      await expect(page.getByTestId('terminal-panel')).toBeVisible();
      await page.getByTestId('terminal-panel').getByRole('tab').first().click();
      await expect(page.locator('.xterm-rows')).toContainText('$');
    } else {
      await page.getByRole('tab', { name: words.threadViews.diagnostics.terminal, exact: true }).click();
      await expect(page.getByText(words.ui.receipt.accepted, { exact: true })).toBeVisible();
    }
    await expect.poll(() => attempts.length).toBe(2);
    expect(attempts[0]?.key).toMatch(/^[0-9a-f-]{36}$/i);
    expect(attempts[1]).toEqual(attempts[0]);
    if (kind === 'agent') {
      expect(sockets).toEqual([]);
      await page.getByRole('button', { name: words.threadViews.diagnostics.take, exact: true }).click();
      const confirm = page.getByRole('dialog', { name: words.threadViews.diagnostics.take, exact: true });
      await confirm.getByRole('button', { name: words.threadViews.diagnostics.take, exact: true }).click();
      await expect.poll(() => attempts.length).toBe(3);
      expect(attempts[2]?.key).not.toBe(attempts[0]?.key);
      expect(attempts[2]?.body).not.toBe(attempts[0]?.body);
      await expect(page.getByText(words.ui.receipt.accepted, { exact: true })).toBeVisible();
      expect(sockets).toEqual([]);
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await info.attach('terminal-ticket-surface', { body: await page.screenshot(), contentType: 'image/png' });
  });
}

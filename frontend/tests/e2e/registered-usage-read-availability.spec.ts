/** The registered Usage view retries provider observations without claiming their View permission is absent. */
import { expect, test } from '@playwright/test';
import type { ThreadOpening, ThreadParameters } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { MAIN } from './change-support';
import { scenario, signIn } from './support';
import { showWorkbenchView } from './workbench-support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1280, 400]) {
  test(`provider observation read ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme }); await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    // The joined opening can already supply a readable provider. Start cold and
    // omit only that optional member so this case exercises the independent read.
    await page.goto('about:blank');
    let failing = true, providerId: string | undefined;
    const failed = { http: 0, native: 0 };
    await page.route(new RegExp(`/api/v1/threads/${MAIN}/opening(?:\\?|$)`), async (route) => {
      const response = await route.fetch(), opening = await response.json() as ThreadOpening;
      providerId = opening.sync.thread.binding?.provider.id;
      if (providerId === undefined) throw new Error('The fixture thread has its bound provider');
      await route.fulfill({ response, json: { ...opening, parameters: opening.parameters === null ? null : { ...opening.parameters, provider: null } } });
    });
    await page.route(new RegExp(`/api/v1/threads/${MAIN}/parameters(?:\\?|$)`), async (route) => {
      const response = await route.fetch(), parameters = await response.json() as ThreadParameters;
      await route.fulfill({ response, json: { ...parameters, provider: null } });
    });
    await page.route(/\/api\/v1\/providers\/[^/?]+(?:\?.*)?$/, async (route) => {
      if (route.request().method() !== 'GET' || !failing) { await route.continue(); return; }
      failed.http += 1; await route.abort('failed');
    });
    // Its native source and HTTP fallback fail together; every other feed stays real.
    await page.routeWebSocket(/\/api\/v1\/ws(?:\?|$)/u, (socket) => {
      const server = socket.connectToServer();
      socket.onMessage((message) => server.send(message));
      server.onMessage((message) => {
        let frame: { op?: string; id?: number; feed?: string };
        try { frame = JSON.parse(String(message)) as typeof frame; }
        catch { socket.send(message); return; }
        if (failing && providerId !== undefined && frame.feed === `resource:provider:${providerId}`
          && (frame.op === 'event' || frame.op === 'sync')) {
          failed.native += 1;
          socket.send(JSON.stringify({ op: 'error', id: frame.id, feed: frame.feed, code: 'read_unavailable', detail: frame.feed }));
        } else socket.send(message);
      });
      socket.onClose((code, reason) => { void server.close({ ...(code === undefined ? {} : { code }), ...(reason === undefined ? {} : { reason }) }); });
    });
    await page.goto(`/thread/${MAIN}`);
    await showWorkbenchView(page, 'Usage');
    const view = page.locator('[data-thread-usage]');
    await expect(view.getByText(words.implicitAvailability.usageFailed, { exact: true })).toBeVisible();
    await expect(view.getByText(words.providerPlan.viewMissing, { exact: true })).toHaveCount(0);
    await expect.poll(() => failed.http).toBeGreaterThan(0);
    await expect.poll(() => failed.native).toBeGreaterThan(0);
    await info.attach(`provider-observation-failure-${locale}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
    failing = false;
    const recovered = page.waitForResponse((response) => response.request().method() === 'GET'
      && new URL(response.url()).pathname === `/api/v1/providers/${providerId}` && response.ok());
    await view.getByRole('button', { name: words.common.tryAgain, exact: true }).click();
    expect((await recovered).ok()).toBe(true);
    await expect(view.getByText(words.implicitAvailability.usageFailed, { exact: true })).toHaveCount(0);
    await expect(view.getByText(words.providerPlan.viewMissing, { exact: true })).toHaveCount(0);
  });
}

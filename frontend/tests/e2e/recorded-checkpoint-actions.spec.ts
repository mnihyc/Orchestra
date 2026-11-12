/** Cold recorded checkpoints stay on the person's request and can be reached without a metadata read. */
import { expect, test } from '@playwright/test';
import { caption } from '../../e2e/capture/open.ts';
import { recordedConversation } from '../doubles/recorded-conversation.ts';
import { ACME, configureThread, emit, replySent, scenario, signIn } from './support.ts';

for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`recorded requests keep keyboard-reachable checkpoints ${width}/${theme}/${locale}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await scenario(); await configureThread(ACME.mainThread, { reset: true }); await emit(ACME.mainThread, recordedConversation());
    const login = page.waitForResponse((reply) => reply.request().method() === 'POST' && new URL(reply.url()).pathname.endsWith('/auth/login')).then(replySent);
    await signIn(page); const auth: unknown = await login;
    if (typeof auth !== 'object' || auth === null || !('access_token' in auth) || typeof auth.access_token !== 'string') throw new Error('The native reader needs its fake sign-in response.');
    expect((await page.request.patch('/api/v1/me', { headers: { Authorization: `Bearer ${auth.access_token}` }, data: { theme, locale } })).ok()).toBe(true);
    const metadata: string[] = []; page.on('request', (request) => { if (new URL(request.url()).pathname.startsWith('/api/v1/snapshots/')) metadata.push(request.url()); });
    await page.goto(`/thread/${ACME.mainThread}`);
    await expect(page.locator('html')).toHaveAttribute('lang', locale); await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await expect(page.locator('[data-restore-turn]')).toHaveCount(2);
    for (const [id, turn] of [['request-one', 'recorded-one'], ['request-two', 'recorded-two']] as const) {
      const row = page.locator(`[data-item="${id}"]`), acts = row.locator('[data-hover-actions="person"]');
      const restore = acts.getByRole('button', { name: caption('@thread.turnActions.restoreBefore', locale), exact: true });
      await row.hover(); await expect(acts).toHaveCSS('opacity', '1');
      await acts.getByRole('button', { name: caption('@timeline.actions.copy', locale), exact: true }).focus();
      await page.keyboard.press('Tab'); await expect(restore).toBeFocused(); await expect(restore).toBeEnabled();
      await expect(restore).toHaveAttribute('data-restore-turn', turn); await expect(acts).toHaveCSS('opacity', '1');
    }
    expect(metadata).toEqual([]);
  });
}

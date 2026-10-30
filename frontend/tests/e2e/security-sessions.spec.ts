/** Security's bulk action is one confirmed request and keeps the current browser, without a second list read. */
import { expect, test } from '@playwright/test';
import { chats } from './chat-support.ts';
import { scenario, signIn } from './support.ts';

const WORDS = {
  en: { action: 'Sign out other sessions', confirm: 'Sign out other sessions', cancel: 'Cancel', current: 'This browser', title: 'Where you are signed in' },
  'zh-CN': { action: '退出其他会话', confirm: '退出其他会话', cancel: '取消', current: '当前浏览器', title: '你已登录的位置' },
} as const;

for (const width of [1440, 400]) for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`other-session sign-out at ${width}px in ${locale}/${theme}`, async ({ page }, info) => {
    await scenario();
    await signIn(page);
    await chats('/locale', { as: 'ada', locale });
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    const requests: string[] = [];
    page.on('request', (request) => {
      if (new URL(request.url()).pathname === '/api/v1/me/sessions') requests.push(request.method());
    });
    await page.goto('/settings/security');
    const w = WORDS[locale];
    const sessions = page.getByRole('region', { name: w.title, exact: true });
    const action = sessions.getByRole('button', { name: w.action, exact: true });
    await action.focus();
    await page.keyboard.press('Enter');
    let confirm = page.getByRole('alertdialog');
    await expect(confirm).toBeVisible();
    await confirm.getByRole('button', { name: w.cancel, exact: true }).click();
    expect(requests.filter((method) => method === 'DELETE')).toHaveLength(0);
    await action.click();
    confirm = page.getByRole('alertdialog');
    await info.attach(`sessions-${width}-${locale}-${theme}`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
    await confirm.getByRole('button', { name: w.confirm, exact: true }).click();
    await expect(confirm).not.toBeVisible();
    await expect(sessions.getByText(w.current, { exact: true })).toBeVisible();
    await expect(sessions.getByRole('listitem')).toHaveCount(1);
    await expect(action).toHaveCount(0);
    expect(requests).toEqual(['GET', 'DELETE']);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.reload();
    await expect(page.getByRole('region', { name: w.title }).getByRole('listitem')).toHaveCount(1);
  });
}

/** Screen-only recovery-code setup, replacement and sign-in, at both widths, languages and themes. */
import { expect, test } from '@playwright/test';
import { chats } from './chat-support.ts';
import { bodySent, PEOPLE, scenario, signIn } from './support.ts';

const WORDS = {
  en: { app: 'Authenticator app', setup: 'Set up an authenticator app', password: 'Your password', next: 'Continue', code: 'Code from the app', on: 'Switch on', saved: 'Save your recovery codes', list: 'One-use recovery codes', close: 'Close', replace: 'Generate new codes…', confirm: 'Generate new codes', replacing: 'Replace recovery codes?', use: 'Use a recovery code', recovery: 'Recovery code', account: 'Your account', out: 'Sign out', login: 'Username or email address', loginPassword: 'Password', enter: 'Sign in' },
  'zh-CN': { app: '验证器应用', setup: '设置验证器应用', password: '你的密码', next: '继续', code: '应用中的验证码', on: '开启', saved: '保存你的恢复码', list: '一次性恢复码', close: '关闭', replace: '生成新恢复码…', confirm: '生成新恢复码', replacing: '替换恢复码？', use: '使用恢复码', recovery: '恢复码', account: '你的账号', out: '退出登录', login: '用户名或邮箱地址', loginPassword: '密码', enter: '登录' },
} as const;

for (const width of [1440, 400]) for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`save and replace recovery codes at ${width}px in ${locale}/${theme}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    await chats('/locale', { as: 'ada', locale });
    const w = WORDS[locale];
    const reads: string[] = [];
    const writes: Promise<unknown>[] = [];
    page.on('request', (request) => {
      if (new URL(request.url()).pathname !== '/api/v1/me/recovery-codes') return;
      if (request.method() === 'GET') reads.push(request.url());
      if (request.method() === 'POST') writes.push(bodySent(request));
    });
    await page.goto('/settings/security');
    await page.getByRole('switch', { name: w.app, exact: true }).click();
    const setup = page.getByRole('dialog', { name: w.setup, exact: true });
    await setup.getByLabel(w.password, { exact: true }).fill(PEOPLE.ada.password);
    await setup.getByRole('button', { name: w.next, exact: true }).click();
    await setup.getByLabel(w.code, { exact: true }).fill('135790');
    await setup.getByRole('button', { name: w.on, exact: true }).click();
    let saved = page.getByRole('dialog', { name: w.saved });
    await expect(saved).toBeVisible();
    const old = await saved.getByRole('list', { name: w.list }).getByRole('listitem').allTextContents();
    expect(old).toHaveLength(10);
    expect(reads).toHaveLength(0);
    const box = await saved.boundingBox();
    if (box === null) throw new Error('the recovery sheet is not drawn');
    expect(box.x).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width).toBeLessThanOrEqual(width);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await info.attach(`recovery-${width}-${locale}-${theme}`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
    await saved.getByRole('button', { name: w.close, exact: true }).click();
    await page.reload();
    await expect(page.getByText(old[0] ?? '', { exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: w.replace, exact: true }).click();
    const replacing = page.getByRole('dialog', { name: w.replacing, exact: true });
    await replacing.getByLabel(w.password, { exact: true }).fill(PEOPLE.ada.password);
    await replacing.getByRole('button', { name: w.use, exact: true }).click();
    await replacing.getByLabel(w.recovery, { exact: true }).fill(old[0] ?? '');
    await replacing.getByRole('button', { name: w.confirm, exact: true }).click();
    saved = page.getByRole('dialog', { name: w.saved });
    await expect(saved).toBeVisible();
    const fresh = await saved.getByRole('list', { name: w.list }).getByRole('listitem').allTextContents();
    expect(fresh).toHaveLength(10);
    expect(fresh).not.toEqual(old);
    expect(reads).toHaveLength(1);
    expect(await Promise.all(writes)).toEqual([{ proof: { password: PEOPLE.ada.password, recovery_code: old[0] } }]);
    await saved.getByRole('button', { name: w.close, exact: true }).click();
    await page.getByRole('button', { name: w.account, exact: true }).click();
    await page.getByRole('menuitem', { name: w.out, exact: true }).click();
    await page.getByLabel(w.login).fill(PEOPLE.ada.login);
    await page.getByLabel(w.loginPassword, { exact: true }).fill(PEOPLE.ada.password);
    await page.getByRole('button', { name: w.enter, exact: true }).click();
    await page.getByRole('button', { name: w.use, exact: true }).focus();
    await page.keyboard.press('Enter');
    await page.getByLabel(w.recovery, { exact: true }).fill(old[1] ?? '');
    await page.getByRole('button', { name: w.enter, exact: true }).click();
    await expect(page.getByLabel(w.recovery, { exact: true })).toHaveAttribute('aria-invalid', 'true');
    await page.getByLabel(w.recovery, { exact: true }).fill(fresh[0] ?? '');
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(/\/org\/acme$/);
  });
}

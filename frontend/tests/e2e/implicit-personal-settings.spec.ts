/** Native view fixtures pin the existing controls; actual sign-in and skill authority remain server checks. */
import { expect, test } from '@playwright/test';
import type { Me, MeAuthorityReply, MeBootstrapReply } from '../../src/generated/api-types';
import type { PersonalSkillsPolicyView, ThreadKnowledgeView } from '../../src/api/share';
import { ACME } from '../doubles/fixtures';
import { showWorkbenchView } from './workbench-support';
import { scenario, signIn } from './support';

const words = {
  en: { mail: 'Code by mail', policy: 'Not allowed by sign-in policy', remove: 'Switch off the code by mail' },
  'zh-CN': { mail: '邮件验证码', policy: '登录策略不允许', remove: '关闭邮件验证码' },
} as const;

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`Email second-step availability stays visible ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    await signIn(page);
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    let enabled = false;
    const person = (value: Me): Me => ({ ...value, locale, theme,
      email: 'ada@example.org', email_verified: true, email_code: [], email_code_second_step: enabled });
    await page.route('**/api/v1/me', async (route) => {
      if (route.request().method() !== 'GET') { await route.continue(); return; }
      const response = await route.fetch();
      const value = await response.json() as Me;
      await route.fulfill({ response, json: person(value) });
    });
    for (const path of ['/me/bootstrap', '/me/authority']) {
      await page.route(`**/api/v1${path}`, async (route) => {
        const response = await route.fetch();
        const value = await response.json() as MeBootstrapReply | MeAuthorityReply;
        await route.fulfill({ response, json: { ...value, person: person(value.person) } });
      });
    }
    const writes: string[] = [];
    page.on('request', (request) => {
      if (request.method() === 'POST' && new URL(request.url()).pathname.startsWith('/api/v1/me/email-code/')) writes.push(request.url());
    });
    await page.goto('/settings/security');
    await expect(page.locator('html')).toHaveAttribute('lang', locale);
    const mail = page.getByRole('switch', { name: words[locale].mail, exact: true });
    await expect(mail).toBeVisible();
    await expect(mail).toBeDisabled();
    await expect(mail).toHaveAccessibleDescription(words[locale].policy);
    await expect(page.getByText(words[locale].policy, { exact: true })).toBeVisible();
    expect(writes).toEqual([]);
    await info.attach('email-policy', { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
    enabled = true;
    await page.reload();
    await expect(mail).toBeChecked();
    await expect(mail).toBeEnabled();
    await mail.focus();
    await page.keyboard.press('Enter');
    const dialog = page.getByRole('dialog', { name: words[locale].remove, exact: true });
    await expect(dialog).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(dialog).toHaveCount(0);
    expect(writes).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    // EN: Security → Second step → Code by mail; read the policy reason, reload an enabled method, open and cancel its existing removal dialog.
    // 中文：安全 → 第二步验证 → 邮件验证码；查看策略原因，重新加载已启用的方法，打开并取消现有的关闭对话框。
  });
}


const skillsWords = {
  en: { off: 'Personal skills off here', source: 'Inherited from Fix login', hidden: 'Set above',
    failed: 'Personal skills could not be checked', retry: 'Try again', allowed: 'Personal skills allowed' },
  'zh-CN': { off: '此处已关闭个人技能', source: '继承自Fix login', hidden: '由上级设置',
    failed: '无法检查个人技能', retry: '重试', allowed: '允许个人技能' },
} as const;

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`inherited personal skills stay factual in Knowledge ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario(); await signIn(page);
    await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    const person = (value: Me): Me => ({ ...value, locale, theme });
    await page.route('**/api/v1/me', async (route) => {
      if (route.request().method() !== 'GET') { await route.continue(); return; }
      const response = await route.fetch(); const value = await response.json() as Me;
      await route.fulfill({ response, json: person(value) });
    });
    for (const path of ['/me/bootstrap', '/me/authority']) {
      await page.route(`**/api/v1${path}`, async (route) => {
        const response = await route.fetch(); const value = await response.json() as MeBootstrapReply | MeAuthorityReply;
        await route.fulfill({ response, json: { ...value, person: person(value.person) } });
      });
    }
    let phase: 'visible' | 'private' | 'unavailable' | 'allowed' = 'visible';
    let reads = 0;
    const policies = (): PersonalSkillsPolicyView => phase === 'visible'
      ? { state: 'off', reason: 'policy_off', sources: [{ scope: 'topic', node: { id: ACME.fixLogin, kind: 'topic', name: 'Fix login', redacted: false } }] }
      : phase === 'private' ? { state: 'off', reason: 'policy_off', sources: [{ scope: 'org', node: null }] }
        : { state: phase };
    await page.route(`**/api/v1/threads/${ACME.mainThread}/shared`, async (route) => {
      const response = await route.fetch(); const value = await response.json() as ThreadKnowledgeView;
      reads += 1;
      await route.fulfill({ response, json: { ...value, personal_skills: policies() } });
    });
    const writes: string[] = [];
    page.on('request', (request) => {
      const path = new URL(request.url()).pathname;
      if (!['GET', 'HEAD', 'OPTIONS'].includes(request.method())
        && (path.includes('/settings/agents.personal_skills') || path.startsWith('/api/v1/skills') || path === '/api/v1/share/agents')) writes.push(path);
    });
    const w = skillsWords[locale];
    await page.goto(`/thread/${ACME.mainThread}`); await showWorkbenchView(page, 'Knowledge');
    await expect(page.locator('html')).toHaveAttribute('lang', locale);
    const panel = page.getByTestId('thread-knowledge');
    await expect(panel.getByText(w.off, { exact: true })).toBeVisible();
    const source = panel.getByRole('link', { name: w.source, exact: true });
    await expect(source).toHaveAttribute('href', `/topic/${ACME.fixLogin}?tab=settings&section=defaults&setting=agents.personal_skills`);
    await source.focus(); await page.keyboard.press('Enter');
    await expect(page).toHaveURL(new RegExp(`/topic/${ACME.fixLogin}.*setting=agents.personal_skills`));
    phase = 'private';
    await page.goto(`/thread/${ACME.mainThread}`); await showWorkbenchView(page, 'Knowledge'); await page.reload();
    const privateLine = panel.getByRole('status').filter({ hasText: w.off });
    await expect(privateLine).toContainText(w.hidden);
    await expect(privateLine.getByRole('link')).toHaveCount(0);
    await info.attach('personal-skills-private-source', { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
    phase = 'unavailable'; await page.reload();
    await expect(panel.getByText(w.failed, { exact: true })).toBeVisible();
    const before = reads; phase = 'allowed';
    await panel.getByRole('button', { name: w.retry, exact: true }).click();
    await expect(panel.getByText(w.allowed, { exact: true })).toBeVisible();
    expect(reads).toBeGreaterThan(before); expect(writes).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    // EN: Thread → Knowledge → Personal skills off here → inherited source; return, inspect the private source, Retry unavailable metadata, then read Allowed.
    // 中文：线程 → 知识 → 此处已关闭个人技能 → 上级来源；返回后查看隐藏来源，重试无法读取的策略，再核对允许状态。
  });
}

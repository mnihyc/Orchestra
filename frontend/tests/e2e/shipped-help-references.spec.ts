/** The shipped service references and administrator-message Help are reachable in both editions. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { signInScreenRoute } from '../../e2e/layout-fixtures';
import { me, userRef } from '../doubles/fixtures';
import { ACME, DOUBLE, scenario } from './support';

const ID = '00000000-0000-4000-8000-000000000392';
const TITLE = 'Read this service notice';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const)
for (const width of [400, 1440] as const) {
  test(`shipped help references ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario(); await signInScreenRoute(page, { width, theme, locale });
    const words = locale === 'en' ? english : chinese;
    const message = {
      id: ID, title: TITLE, body: '## Service notice\n\nRead the current **service policy**.',
      sender: userRef(me(1, 'ada', 'Ada Lovelace')),
      org: { id: ACME.org, kind: 'org', name: 'Acme', redacted: false },
      created_at: '2026-11-08T12:00:00Z', recipient_count: 1, delivery_job_id: null,
    };
    await page.route(`**/api/v1/messages/${ID}`, (route) => route.fulfill({ json: message }));
    const notice = await page.request.post(`${DOUBLE}/__double/notifications/row`, { data: {
      kind: 'admin_message', title: TITLE, body: message.body, actor: 'ada', node: ACME.org,
      url: `/messages/${ID}`, parameters: { message_id: ID, org_id: ACME.org, org_name: 'Acme' },
    } });
    expect(notice.ok()).toBe(true);
    const sends: string[] = [];
    page.on('request', (request) => {
      if (request.method() !== 'GET' && /\/api\/v1\/(?:admin\/|orgs\/[^/]+\/)?messages(?:\/|$)/u.test(new URL(request.url()).pathname)) sends.push(request.method());
    });

    await page.goto('/help?part=articles');
    await page.getByRole('searchbox', { name: words.help.search.label, exact: true }).fill('services.allowed');
    await page.getByRole('button', { name: new RegExp(`^${words.settingRegistry.services.allowed.title}`) }).click();
    await expect(page.getByRole('article', { name: words.settingRegistry.services.allowed.title, exact: true }).getByRole('heading', { name: words.settingRegistry.services.allowed.title, level: 2, exact: true })).toBeVisible();
    await expect(page.getByText(words.settingRegistry.services.allowed.description, { exact: true })).toBeVisible();
    await info.attach('service-setting-help', { body: await page.screenshot(), contentType: 'image/png' });

    await page.goto('/help?part=articles');
    await page.getByRole('searchbox', { name: words.help.search.label, exact: true }).fill('sharing.allow_copy_out');
    await page.getByRole('button', { name: new RegExp(`^${words.settingRegistry.sharing.allow_copy_out.title}`) }).click();
    await expect(page.getByRole('article', { name: words.settingRegistry.sharing.allow_copy_out.title, exact: true }).getByRole('heading', { name: words.settingRegistry.sharing.allow_copy_out.title, level: 2, exact: true })).toBeVisible();
    await expect(page.getByText(words.settingRegistry.sharing.allow_copy_out.description, { exact: true })).toBeVisible();

    const referenceTitle = locale === 'en' ? 'Option field reference' : '选项字段参考';
    await page.goto('/help?part=articles');
    await page.getByRole('searchbox', { name: words.help.search.label, exact: true }).fill(referenceTitle);
    await page.locator('[data-help-result="option-reference"]').getByRole('button', { name: referenceTitle, exact: true }).click();
    const reference = page.getByRole('region', { name: referenceTitle, exact: true });
    const row = reference.getByText('option:AccessOptions.services', { exact: true });
    await row.scrollIntoViewIfNeeded();
    await expect(row).toBeVisible();
    await expect(row.locator('xpath=ancestor::tr')).toContainText(locale === 'en' ? 'independent of generation models' : '与生成模型列表相互独立');
    await info.attach('service-option-reference', { body: await page.screenshot(), contentType: 'image/png' });
    for (const field of ['model', 'provider_id']) {
      const modelOption = reference.getByText(`option:ask.models.*.${field}`, { exact: true });
      await modelOption.scrollIntoViewIfNeeded(); await expect(modelOption).toBeVisible();
      await expect(modelOption.locator('xpath=ancestor::tr')).toContainText(locale === 'en' ? (field === 'model' ? 'routing name' : 'API provider') : (field === 'model' ? '路由名称' : 'API 提供方'));
    }

    const duplicateTitle = locale === 'en' ? 'Duplicate a resource or conversation' : '创建资源或对话的副本';
    await page.goto('/help?part=articles');
    await page.getByRole('searchbox', { name: words.help.search.label, exact: true }).fill(duplicateTitle);
    await page.locator('[data-help-result="duplicate-resources"]').getByRole('button', { name: duplicateTitle, exact: true }).click();
    for (const heading of (locale === 'en' ? ['What follows', 'Credentials and notices', 'When a destination is refused', 'Move a home within its org'] : ['哪些内容会跟随', '凭据和通知', '目标位置被拒绝时', '在组织内移动所属位置'])) {
      const section = page.getByRole('heading', { level: 2, name: heading, exact: true });
      await section.scrollIntoViewIfNeeded(); await expect(section).toBeVisible();
    }
    await info.attach('copy-context-help', { body: await page.screenshot(), contentType: 'image/png' });


    await page.goto('/inbox');
    const received = page.locator('li[data-kind="admin_message"]').getByRole('link', { name: TITLE, exact: true });
    await received.focus(); await page.keyboard.press('Enter');
    await expect(page.getByRole('heading', { name: TITLE, exact: true })).toBeVisible();
    await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
    await page.keyboard.press('F1');
    const help = page.getByRole('region', { name: words.views.help.title, exact: true });
    if (width === 400) {
      // Narrow screens navigate to the Help page, whose article catalogue has no contextual screen tab.
      await expect(page).toHaveURL(/\/help(?:\?|$)/);
      await expect(help.getByRole('tab', { name: words.help.parts.screen, exact: true })).toHaveCount(0);
    } else {
      await help.getByRole('tab', { name: words.help.parts.screen, exact: true }).click();
      await expect(help.getByText(words.screens.administratorMessage.about, { exact: true })).toBeVisible();
    }
    const articleTitle = locale === 'en' ? 'Read an administrator message' : '阅读管理员消息';
    await help.getByRole('button', { name: articleTitle, exact: true }).click();
    await expect(help.getByRole('heading', { name: articleTitle, level: 1, exact: true })).toBeVisible();
    await expect(help.getByRole('heading', { name: locale === 'en' ? 'Read the message' : '阅读消息', level: 2, exact: true })).toBeVisible();
    expect(sends).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await info.attach('message-screen-help', { body: await page.screenshot(), contentType: 'image/png' });
  });
}

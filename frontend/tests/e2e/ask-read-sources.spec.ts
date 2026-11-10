/** Source controls for real read results and unavailable sources, in both languages. */
import { expect, test } from '@playwright/test';
import { askMessage, askSnapshot } from '../unit/ask-fixtures';
import { fixture, language, openAsk } from './ask-support';
import { DOCS } from './documents-support';
import { ACME, scenario } from './support';
import type { ReferencePreview } from '../../src/generated/api-types';
import { id } from '../doubles/fixtures';

for (const locale of ['en', 'zh-CN'] as const) test(`Ask opens a readable source and keeps an unavailable source inactive (${locale})`, async ({ page, request }) => {
  // EN: topic > Ask > Sources > Rollout plan > Open. The unavailable source
  // has no link. Native API cases decide named variants and current authority;
  // this contract fixture decides the person's source-opening click path.
  // 中文：话题 > 问一问 > 来源 > Rollout plan > 打开；无法读取的来源没有链接。
  // 原生 API 用例验证命名变体和当前权限；此契约夹具验证来源控件的点击路径。
  await scenario();
  const answer = locale === 'en' ? 'The current readable plan is cited below.' : '下方列出当前可读取的计划来源。';
  await fixture(page, [askSnapshot({ messages: [askMessage({
    content: [{ type: 'text', text: answer }],
    citations: [
      { reference: 'readable-plan', url: `/doc/${DOCS.plan}`, title: 'Rollout plan' },
      { reference: 'unavailable-plan', url: `/doc/${DOCS.private}`, title: null },
    ],
  })] })]);
  const t = await language(page, request, locale, 'light');
  await page.goto(`/topic/${ACME.fixLogin}`);
  const panel = await openAsk(page, t);
  await expect(panel.getByText(answer, { exact: true })).toBeVisible();
  const sources = panel.getByRole('list', { name: t('ask.sources') });
  await expect(sources.getByText(t('ask.sourceUnavailable'), { exact: true })).toBeVisible();
  await expect(sources.locator(`a[href="/doc/${DOCS.private}"]`)).toHaveCount(0);
  await sources.getByRole('link', { name: 'Rollout plan', exact: true }).click();
  const card = page.locator('[data-reference-card]').filter({ has: page.getByRole('button', { name: t('people.reference.open'), exact: true }) }).last();
  await expect(card).toBeVisible();
  await card.getByRole('button', { name: t('people.reference.open'), exact: true }).click();
  await expect.poll(() => new URL(page.url()).pathname).toBe(`/doc/${DOCS.plan}`);
});

for (const locale of ['en', 'zh-CN'] as const) test(`Ask keeps the copied chat message in its source card (${locale})`, async ({ page, request }) => {
  // EN: topic > Ask > Sources > Team chat > inspect message 7 in the card.
  // 中文：话题 > 问一问 > 来源 > Team chat > 在卡片中查看第 7 条消息。
  await scenario();
  const chat = id(0x77, 1), queries: string[] = [];
  await fixture(page, [askSnapshot({ messages: [askMessage({ citations: [{ reference: 'message-seven', url: `/chat/${chat}?message=7`, title: 'Team chat' }] })] })]);
  await page.route('**/api/v1/references/preview*', async (route) => {
    const url = new URL(route.request().url());
    if (url.searchParams.get('kind') !== 'chat' || url.searchParams.get('id') !== chat) { await route.continue(); return; }
    const message = url.searchParams.get('message'); queries.push(message ?? '');
    const preview: ReferencePreview = { kind: 'chat', id: chat, node_kind: null, title: 'Team chat', place: null, description: '', updated_at: null, updated_by: null,
      part: message === '7' ? { part: { message: 7 }, number: 7, anchor: id(0x77, 7), text: 'Selected message seven', by: null } : null, file: null };
    await route.fulfill({ json: preview });
  });
  const t = await language(page, request, locale, 'light');
  await page.goto(`/topic/${ACME.fixLogin}`);
  const panel = await openAsk(page, t);
  const source = panel.getByRole('list', { name: t('ask.sources') }).getByRole('link', { name: 'Team chat', exact: true });
  await expect(source).toBeVisible(); await source.click();
  await expect(page.locator('[data-reference-card]').getByText('Selected message seven', { exact: true })).toBeVisible();
  expect(queries).toContain('7');
});

for (const locale of ['en', 'zh-CN'] as const) test(`Ask follows organization, project, topic and thread sources (${locale})`, async ({ page, request }) => {
  // EN: topic > Ask > Sources > Acme / Web / Fix login / Main thread > Open;
  // each source opens its canonical page (the organization uses its readable address). Native tests resolve the actual
  // API operation names; this fixture decides the source-opening click paths.
  // 中文：话题 > 问一问 > 来源 > Acme / Web / Fix login / Main thread > 打开；
  // 每个来源都打开对应的规范页面，组织使用可读地址。原生测试验证实际 API 操作名；夹具验证点击路径。
  await scenario();
  const references = [
    { kind: 'org', id: ACME.org, title: 'Acme', path: '/org/acme' },
    { kind: 'project', id: ACME.web, title: 'Web', path: `/project/${ACME.web}` },
    { kind: 'topic', id: ACME.fixLogin, title: 'Fix login', path: `/topic/${ACME.fixLogin}` },
    { kind: 'thread', id: ACME.mainThread, title: 'Main thread', path: `/thread/${ACME.mainThread}` },
  ];
  const answer = locale === 'en' ? 'These readable places and thread are the sources.' : '来源为这些可读取的地方和会话。';
  await fixture(page, [askSnapshot({ messages: [askMessage({
    content: [{ type: 'text', text: answer }],
    citations: references.map((reference) => ({ reference: `${reference.kind}:${reference.id}`, url: `/${reference.kind}/${reference.id}`, title: reference.title })),
  })] })]);
  const t = await language(page, request, locale, 'light');
  for (const reference of references) {
    await page.goto(`/topic/${ACME.fixLogin}`);
    const panel = await openAsk(page, t);
    await expect(panel.getByText(answer, { exact: true })).toBeVisible();
    await panel.getByRole('list', { name: t('ask.sources') }).getByRole('link', { name: reference.title, exact: true }).click();
    const card = page.locator('[data-reference-card]').filter({ has: page.getByRole('button', { name: t('people.reference.open'), exact: true }) }).last();
    await expect(card).toBeVisible();
    await card.getByRole('button', { name: t('people.reference.open'), exact: true }).click();
    await expect.poll(() => new URL(page.url()).pathname).toBe(reference.path);
  }
});

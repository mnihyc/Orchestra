/** Actual page headers, explicit receipts, retained failures and same-panel root-pinned drafts. */
import { expect, test } from '@playwright/test';
import { ACME, DOUBLE, scenario } from './support';
import { DOCS, makePrivateDocument } from './documents-support';
import { chats } from './chat-support';
import { fixture, language } from './ask-support';
import { showView } from './inspector-support';
import { versions } from './prompt-version-support';
import type { SearchHit, SearchStreamEvent, SummaryBatchBody, SummaryReceipt, SummaryRefreshBody } from '../../src/generated/api-types';
import { id, MADE_AT } from '../doubles/fixtures';
import { openLayoutRoute, prepareLayoutRoute } from '../../e2e/layout-fixtures';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`co-mounted thread and catch-up actions name their subjects (${locale}/${theme}/${width})`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 });
    const ask = await fixture(page); const t = await language(page, request, locale, theme);
    const row = { pattern: '/thread/$threadId', url: `/thread/${ACME.mainThread}`, fixture: 'background-running' } as const;
    await prepareLayoutRoute(page, row);
    let paid = 0;
    page.on('request', (sent) => { if (sent.method() === 'POST' && new URL(sent.url()).pathname.startsWith('/api/v1/summaries/') && new URL(sent.url()).pathname.endsWith('/refresh')) paid += 1; });
    await page.goto(row.url); await openLayoutRoute(page, row);
    const thread = page.locator('[data-summary-target="thread"]').first();
    const chat = page.locator('[data-summary-target="chat"]').first();
    await expect(thread).toBeVisible(); await expect(chat).toBeVisible();
    await expect(thread.getByText(t('summaryBox.state.no_summarizer'), { exact: true })).toBeVisible();
    await expect(chat.getByText(t('summaryBox.subjectState', { subject: t('summaryBox.catchUp'), state: t('summaryBox.state.no_summarizer') }), { exact: true })).toBeVisible();
    // EN: Background thread -> distinguish Summary and Catch up -> open each subject's details -> Ask from each into an unsent draft.
    // 中文：后台会话 -> 区分摘要与补看 -> 分别打开对应详情 -> 分别就该内容提问，只填入草稿而不发送。
    for (const name of [t('summaryBox.details'), t('summaryBox.catchUpActions.details')]) {
      await expect(page.getByRole('button', { name, exact: true })).toHaveCount(1);
      await page.getByRole('button', { name, exact: true }).click();
      await expect(page.getByText(t('summaryBox.baseRates'), { exact: true })).toBeVisible();
      await page.keyboard.press('Escape');
    }
    await info.attach('summary and catch-up have distinct compact subjects', { body: await page.screenshot(), contentType: 'image/png' });
    for (const [box, name] of [[thread, t('summaryBox.ask')], [chat, t('summaryBox.catchUpActions.ask')]] as const) {
      await expect(page.getByRole('button', { name, exact: true })).toHaveCount(1);
      const reference = await box.getAttribute('data-summary-reference');
      if (reference === null) throw new Error('The summary must retain its actual source reference');
      await box.getByRole('button', { name, exact: true }).click();
      await expect(page.locator('[data-view-window="ask"]').getByRole('textbox', { name: t('ask.composer'), exact: true })).toHaveValue(new RegExp(reference));
    }
    expect(ask.sends).toEqual([]); expect(paid).toBe(0);
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`one page summary beside Parameters explains the historical boundary (${locale}/${theme}/${width})`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 });
    const ask = await fixture(page); const t = await language(page, request, locale, theme);
    await versions(page);
    const text = locale === 'en' ? 'Only the current continuation has this paragraph.' : '这段摘要只描述当前版本。';
    await request.post(`${DOUBLE}/__double/summaries/set`, { data: { kind: 'thread', id: ACME.mainThread, root: ACME.org, state: 'fresh', text } });
    let reads = 0, paid = 0;
    page.on('request', (sent) => {
      const path = new URL(sent.url()).pathname;
      if (sent.method() === 'GET' && path === `/api/v1/summaries/thread/${ACME.mainThread}`) reads += 1;
      if (sent.method() === 'POST' && path.startsWith('/api/v1/summaries/') && path.endsWith('/refresh')) paid += 1;
    });
    await page.goto(`/thread/${ACME.mainThread}`);
    await showView(page, t('views.parameters.title'));
    const inspector = page.locator('[data-screen="thread"] [data-summary-inspector]');
    await expect(page.locator('[data-summary-inspector]')).toHaveCount(1);
    await expect(page.getByRole('button', { name: t('summaryBox.details'), exact: true })).toHaveCount(1);
    await expect(page.getByRole('button', { name: t('summaryBox.ask'), exact: true })).toHaveCount(1);
    await expect(inspector.locator('[data-summary-text]')).toHaveText(text);
    const currentReads = reads;
    // EN: Current Parameters → Previous version → visible historical reason, with no live paragraph or new summary read.
    // 中文：当前参数 → 上一版本 → 显示历史版本说明，不显示当前摘要，也不读取新的实时摘要。
    await page.getByRole('button', { name: t('messageVersions.previous'), exact: true }).click();
    await expect(inspector.getByText(t('summaryBox.historical'), { exact: true })).toBeVisible();
    await expect(inspector.locator('[data-summary-target]')).toHaveCount(0);
    expect(reads).toBe(currentReads); expect(paid).toBe(0); expect(ask.sends).toEqual([]);
    await info.attach('retained summary inspector boundary', { body: await inspector.screenshot(), contentType: 'image/png' });
    await page.getByRole('button', { name: t('messageVersions.backToActive'), exact: true }).click();
    await expect(inspector.locator('[data-summary-text]')).toHaveText(text);
    expect(paid).toBe(0);
  });
  test(`search explains an unavailable summary and retries its free read (${locale}/${theme}/${width})`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 });
    const ask = await fixture(page); const t = await language(page, request, locale, theme);
    const doc = await makePrivateDocument('ada', ACME.fixLogin, 'SummaryReadRetry');
    const text = locale === 'en' ? 'The stored paragraph is readable after retry.' : '重试后可读取已有摘要。';
    await request.post(`${DOUBLE}/__double/summaries/set`, { data: { kind: 'document', id: doc, root: ACME.org, state: 'fresh', text } });
    await page.route('**/api/v1/search/stream?**', async (route) => { const reply = await route.fetch(); await route.fulfill({ response: reply, body: await reply.text() }); });
    let refuse = true, reads = 0, paid = 0;
    await page.route('**/api/v1/summaries/batch', async (route) => {
      reads += 1;
      if (refuse) await route.fulfill({ status: 503, json: { status: 503, code: 'provider.unavailable', title: 'Hidden cache canary', detail: 'Hidden source, grant and audience canary' } });
      else await route.continue();
    });
    page.on('request', (sent) => { if (sent.method() === 'POST' && new URL(sent.url()).pathname.startsWith('/api/v1/summaries/') && new URL(sent.url()).pathname.endsWith('/refresh')) paid += 1; });
    await page.goto(`/search?q=SummaryReadRetry&org=${ACME.org}`);
    const unavailable = page.locator('[data-search-summary-state="unavailable"]');
    // EN: Search → unavailable summary line → free Try again → existing paragraph, with no hidden reason or paid act.
    // 中文：搜索 → 摘要不可用说明 → 免费重试读取 → 已有段落，不显示私密原因，也不请求付费操作。
    await expect(unavailable.getByText(t('summaryBox.viewUnavailable'), { exact: true })).toBeVisible();
    await expect(page.getByText('Hidden cache canary', { exact: true })).toHaveCount(0);
    await expect(page.getByText('Hidden source, grant and audience canary', { exact: true })).toHaveCount(0);
    await expect(page.locator('[data-search-summary]')).toHaveCount(0);
    const failedReads = reads; refuse = false;
    await info.attach('generic summary read failure', { body: await unavailable.screenshot(), contentType: 'image/png' });
    await unavailable.getByRole('button', { name: t('common.tryAgain'), exact: true }).click();
    await expect(page.locator('[data-search-summary]')).toHaveText(text);
    expect(reads).toBe(failedReads + 1); expect(paid).toBe(0); expect(ask.sends).toEqual([]);
    await info.attach('stored summary after free read retry', { body: await page.locator('[data-search-summary]').screenshot(), contentType: 'image/png' });
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`the summary input budget keeps its help address in both languages (${locale}/${theme}/${width})`, async ({ page, request }) => {
    await scenario(); await page.setViewportSize({ width, height: 900 });
    const t = await language(page, request, locale, theme);
    const title = locale === 'en' ? 'Read and refresh summaries' : '阅读和刷新摘要';
    const heading = locale === 'en' ? 'Choose the input budget' : '选择输入预算';
    await page.goto('/help');
    await page.getByRole('tab', { name: new RegExp(t('help.parts.articles')) }).click();
    await page.getByRole('button', { name: title, exact: true }).click();
    const article = page.getByRole('region', { name: title, exact: true });
    await expect(article.getByRole('heading', { name: title, level: 1 })).toBeVisible();
    // EN: Open the summaries guide → follow its saved input-budget address → read the section → reload the same address.
    // 中文：打开摘要指南 → 进入已保存的输入预算地址 → 阅读该节 → 重新加载同一地址。
    await page.goto('/help/summaries#choose-the-input-budget');
    const section = article.getByRole('heading', { name: heading, level: 2 });
    await expect(section).toHaveAttribute('id', 'choose-the-input-budget');
    await expect(section).toBeInViewport();
    await page.reload();
    await expect(page).toHaveURL(/\/help\/summaries#choose-the-input-budget$/);
    await expect(section).toHaveAttribute('id', 'choose-the-input-budget');
    await expect(section).toBeInViewport();
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`search batches twenty stored source paragraphs and opens only an unsent draft (${locale}/${theme}/${width})`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 });
    const ask = await fixture(page); const t = await language(page, request, locale, theme);
    const paragraphs: string[] = [];
    for (let index = 1; index <= 20; index += 1) {
      const doc = await makePrivateDocument('ada', ACME.fixLogin, `BatchProjection ${index}`);
      const text = locale === 'en' ? `Stored projection ${index}.` : `已有的摘要 ${index}。`;
      paragraphs.push(text);
      await request.post(`${DOUBLE}/__double/summaries/set`, { data: { kind: 'document', id: doc, root: ACME.org, state: 'fresh', text } });
    }
    // Buffer this transport fixture's complete native search frame, keeping all source ids and Read branches real in the double.
    await page.route('**/api/v1/search/stream?**', async (route) => { const reply = await route.fetch(); await route.fulfill({ response: reply, body: await reply.text() }); });
    const batches: number[] = []; let singles = 0, paid = 0;
    page.on('request', (sent) => {
      const path = new URL(sent.url()).pathname;
      if (sent.method() === 'POST' && path === '/api/v1/summaries/batch') { const body = sent.postDataJSON() as SummaryBatchBody; batches.push(body.targets.length); }
      if (sent.method() === 'GET' && path.startsWith('/api/v1/summaries/')) singles += 1;
      if (sent.method() === 'POST' && path.startsWith('/api/v1/summaries/') && path.endsWith('/refresh')) paid += 1;
    });
    await page.goto(`/search?q=BatchProjection&org=${ACME.org}`);
    await expect(page.locator('[data-search-summary]').first()).toBeVisible();
    // EN: Search twenty readable sources → one stored-read batch → Ask about results → editable draft, no Send.
    // 中文：搜索二十个可读来源 → 一次已有摘要批量读取 → 就结果提问 → 可编辑草稿，不发送。
    expect(batches).toEqual([20]); expect(singles).toBe(0); expect(paid).toBe(0);
    await info.attach('twenty source projections share one read batch', { body: await page.screenshot(), contentType: 'image/png' });
    await page.getByRole('button', { name: t('summaryBox.askResults'), exact: true }).click();
    const input = page.locator('[data-view-window="ask"]').getByRole('textbox', { name: t('ask.composer'), exact: true });
    await expect(input).toHaveValue(new RegExp(paragraphs[0] ?? 'Stored'));
    const text = await input.inputValue();
    for (const paragraph of paragraphs) expect(text).toContain(paragraph);
    expect(ask.sends).toEqual([]); expect(paid).toBe(0); expect(singles).toBe(0);
    await info.attach('batch projections quoted in an unsent same-root draft', { body: await page.screenshot(), contentType: 'image/png' });
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`scheduled summaries show their actual wait and preserve explicit refresh (${locale}/${theme}/${width})`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 });
    await fixture(page); const t = await language(page, request, locale, theme);
    const until = new Date(Date.now() + 900_000).toISOString();
    await request.post(`${DOUBLE}/__double/summaries/set`, { data: { kind: 'document', id: DOCS.plan, root: ACME.org, state: 'scheduled', text: 'The previous paragraph stays readable.', wait: { reason: 'interval', until }, poll_after_ms: 900_000 } });
    let reads = 0, paid = 0;
    page.on('request', (sent) => { const path = new URL(sent.url()).pathname; if (path === `/api/v1/summaries/document/${DOCS.plan}`) reads += 1; if (sent.method() === 'POST' && path === `/api/v1/summaries/document/${DOCS.plan}/refresh`) paid += 1; });
    await page.goto(`/doc/${DOCS.plan}`);
    const box = page.locator('[data-summary-target="document"]').first();
    // EN: Scheduled → minimum-interval reason and actual time → previous paragraph → deliberate Refresh.
    // 中文：已安排 → 最短间隔原因与实际时间 → 阅读原摘要 → 主动刷新。
    await expect(box).toHaveAttribute('data-summary-state', 'scheduled');
    await expect(box.getByText(t('summaryBox.wait.interval'), { exact: true })).toBeVisible();
    await expect(box.locator(`time[datetime="${until}"]`)).toBeVisible();
    await expect(box.locator('[data-summary-text]')).toHaveText('The previous paragraph stays readable.');
    expect(reads).toBe(1); expect(paid).toBe(0);
    await info.attach('future eligibility and retained paragraph', { body: await box.screenshot(), contentType: 'image/png' });
    await box.getByRole('button', { name: t('summaryBox.refresh'), exact: true }).click();
    await expect.poll(() => paid).toBe(1);
    await expect(box.locator('[data-summary-text]')).toHaveText('Completed summary for document.');
    await info.attach('explicit refresh remains available', { body: await box.screenshot(), contentType: 'image/png' });
    // EN: Help → Articles → State reference → SummaryState → read the meaning of the scheduled wait.
    // 中文：帮助 → 文章 → 状态参考 → SummaryState → 阅读已安排等待状态的含义。
    const referenceTitle = locale === 'en' ? 'State reference' : '状态参考';
    const meaning = locale === 'en'
      ? 'An update waits until its minimum interval or recovery delay ends; previous text remains readable.'
      : '更新正在等待最短间隔或恢复延迟结束，原有文字仍可阅读。';
    await page.goto('/help');
    await page.getByRole('tab', { name: new RegExp(t('help.parts.articles')) }).click();
    await page.getByRole('button', { name: referenceTitle, exact: true }).click();
    await page.goto('/help/state-reference#summarystate');
    const reference = page.getByRole('region', { name: referenceTitle, exact: true });
    await expect(reference.getByRole('heading', { name: 'SummaryState', exact: true })).toBeInViewport();
    const row = reference.getByRole('row').filter({ hasText: 'state:SummaryState.scheduled' });
    await expect(row).toBeVisible();
    await expect(row.getByRole('cell').nth(1)).toHaveText(meaning);
    expect(paid).toBe(1);
    await info.attach('scheduled state reference in the current language', { body: await row.screenshot(), contentType: 'image/png' });
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`a personal default refresh needs no automatic configuration and explains unavailable models (${locale}/${theme}/${width})`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 });
    await fixture(page); const t = await language(page, request, locale, theme);
    const target = { kind: 'document', id: DOCS.plan, root: ACME.org };
    await request.post(`${DOUBLE}/__double/summaries/set`, { data: { ...target, state: 'empty', text: null } });
    const settingsWrites: string[] = [];
    page.on('request', (sent) => { if (sent.method() !== 'GET' && new URL(sent.url()).pathname.startsWith('/api/v1/settings')) settingsWrites.push(sent.url()); });
    await page.goto(`/doc/${DOCS.plan}`);
    const box = page.locator('[data-summary-target="document"]').first();
    // EN: Unconfigured automatic summaries → personal Refresh → same receipt/readback, with no setting write.
    // 中文：未配置自动摘要 → 主动刷新 → 正常回执与重新读取，不写入自动设置。
    await expect(box.getByRole('button', { name: t('summaryBox.refresh'), exact: true })).toBeVisible();
    await expect(box.getByRole('button', { name: t('summaryBox.choose'), exact: true })).toHaveCount(0);
    const accepted = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname === `/api/v1/summaries/document/${DOCS.plan}/refresh`);
    await box.getByRole('button', { name: t('summaryBox.refresh'), exact: true }).click();
    const reply = await accepted;
    const body = reply.request().postDataJSON() as SummaryRefreshBody;
    expect(reply.status()).toBe(202); expect(body.model).toBeUndefined();
    await expect(box.locator('[data-summary-text]')).toHaveText('Completed summary for document.');
    expect(settingsWrites).toEqual([]);
    await info.attach('one-click personal refresh without automatic setup', { body: await box.screenshot(), contentType: 'image/png' });
    for (const reason of ['no_model', 'cap_reached'] as const) {
      await request.post(`${DOUBLE}/__double/summaries/set`, { data: { ...target, state: 'no_summarizer', text: null, manual_problem: reason } });
      await page.reload();
      await expect(box.getByText(t(reason === 'cap_reached' ? 'summaryBox.manualCap' : 'summaryBox.manualModel'), { exact: true })).toBeVisible();
      await expect(box.getByRole('button', { name: t('summaryBox.refresh'), exact: true })).toHaveCount(0);
      await info.attach(`manual refresh ${reason}`, { body: await box.screenshot(), contentType: 'image/png' });
    }
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`an acknowledged refresh retries a failed read without another paid act (${locale}/${theme}/${width})`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 });
    await fixture(page); const t = await language(page, request, locale, theme);
    await request.post(`${DOUBLE}/__double/summaries/set`, { data: { kind: 'document', id: DOCS.plan, root: ACME.org, state: 'fresh', text: 'The previous stored paragraph.' } });
    const path = `/api/v1/summaries/document/${DOCS.plan}`;
    const paid: string[] = [];
    let allowRead = false;
    page.on('request', (sent) => { if (sent.method() === 'POST' && new URL(sent.url()).pathname === `${path}/refresh`) { const body = sent.postDataJSON() as SummaryRefreshBody; paid.push(body.client_id); } });
    await page.route((url) => url.pathname === path, async (route) => {
      if (paid.length > 0 && !allowRead) await route.fulfill({ status: 503, json: { status: 503, code: 'provider.unavailable', detail: 'The free summary read is temporarily unavailable' } });
      else await route.continue();
    });
    await page.goto(`/doc/${DOCS.plan}`);
    const box = () => page.locator('[data-summary-target="document"]').first();
    // EN: Confirmed Refresh → failed ordinary read → Try again reads only → later deliberate Refresh is new.
    // 中文：刷新回执已确认 → 普通读取失败 → 重试只读取 → 之后主动刷新才产生新请求。
    await box().getByRole('button', { name: t('summaryBox.refresh'), exact: true }).click();
    await expect(page.getByRole('button', { name: t('common.tryAgain'), exact: true })).toBeVisible();
    expect(paid).toHaveLength(1);
    await info.attach('confirmed write with a failed readback', { body: await page.screenshot(), contentType: 'image/png' });
    allowRead = true;
    await page.getByRole('button', { name: t('common.tryAgain'), exact: true }).click();
    await expect(box().locator('[data-summary-text]')).toHaveText('Completed summary for document.');
    expect(paid).toHaveLength(1);
    await box().getByRole('button', { name: t('summaryBox.refresh'), exact: true }).click();
    await expect.poll(() => paid.length).toBe(2);
    expect(paid[1]).not.toBe(paid[0]);
    await info.attach('free read retry followed by an explicit new act', { body: await page.screenshot(), contentType: 'image/png' });
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`a lost refresh acknowledgement retries one act then permits a deliberate new refresh (${locale}/${theme}/${width})`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 });
    await fixture(page); const t = await language(page, request, locale, theme);
    await request.post(`${DOUBLE}/__double/summaries/set`, { data: { kind: 'document', id: DOCS.plan, root: ACME.org, state: 'fresh', text: 'A stored paragraph before the explicit refresh.' } });
    const attempts: string[] = [], receipts: string[] = [];
    const path = `/api/v1/summaries/document/${DOCS.plan}/refresh`;
    // Transport-only lost acknowledgement after the double accepted the actual request.
    await page.route((url) => url.pathname === path, async (route) => {
      const body = route.request().postDataJSON() as SummaryRefreshBody;
      attempts.push(body.client_id);
      const reply = await route.fetch();
      const receipt = await reply.json() as SummaryReceipt;
      receipts.push(receipt.id);
      if (attempts.length === 1) await route.abort('connectionreset');
      else await route.fulfill({ response: reply });
    });
    await page.goto(`/doc/${DOCS.plan}`);
    const box = page.locator('[data-summary-target="document"]').first();
    const refresh = () => box.getByRole('button', { name: t('summaryBox.refresh'), exact: true });
    // EN: Refresh → lost acknowledgement → retry → same native request identity → new deliberate Refresh.
    // 中文：刷新 → 回执丢失 → 重试 → 复用同一请求标识 → 再次主动刷新产生新标识。
    await refresh().click(); await expect(box.getByRole('alert')).toBeVisible();
    await info.attach('lost acknowledgement keeps the paragraph and retry', { body: await box.screenshot(), contentType: 'image/png' });
    await refresh().click();
    await expect(box).toHaveAttribute('data-summary-state', 'fresh');
    await expect(box.locator('[data-summary-text]')).toHaveText('Completed summary for document.');
    expect(attempts).toHaveLength(2); expect(attempts[1]).toBe(attempts[0]);
    expect(receipts[1]).toBe(receipts[0]);
    await refresh().click();
    await expect.poll(() => attempts.length).toBe(3);
    expect(attempts[2]).not.toBe(attempts[0]);
    await expect(box).toHaveAttribute('data-summary-state', 'fresh');
    await info.attach('acknowledged refresh allows a later distinct act', { body: await box.screenshot(), contentType: 'image/png' });
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`a ready result root stays askable when another root source is unavailable (${locale}/${theme}/${width})`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 });
    const ask = await fixture(page); const t = await language(page, request, locale, theme);
    const otherRoot = id(0x73, 1), otherTopic = id(0x73, 2);
    const otherName = locale === 'en' ? 'Unavailable work' : '暂不可用的工作';
    const readyText = locale === 'en' ? 'The ready root remains readable.' : '可用组织的摘要仍可阅读。';
    const hits: SearchHit[] = [
      { kind: 'name', node: { id: ACME.fixLogin, kind: 'topic', name: 'Ready source', redacted: false }, path: [{ id: ACME.org, kind: 'org', name: 'Acme', redacted: false }], href: `/topic/${ACME.fixLogin}`, at: MADE_AT, snippet: 'Ready source snippet', matches: [] },
      { kind: 'name', node: { id: otherTopic, kind: 'topic', name: 'Unavailable source', redacted: false }, path: [{ id: otherRoot, kind: 'org', name: otherName, redacted: false }], href: `/topic/${otherTopic}`, at: MADE_AT, snippet: 'Other root snippet', matches: [] },
    ];
    const events: SearchStreamEvent[] = [
      { type: 'options', meaning_pause_ms: 0, page_size: 20, reorder_window_secs: 0 },
      ...hits.map((hit, index): SearchStreamEvent => ({ type: 'hit', id: `root-hit-${index}`, hit, found_by: 'words' })),
      { type: 'order', received: 2, items: hits.map((_, index) => ({ id: `root-hit-${index}`, found_by: 'words' as const, rank: index + 1 })) },
      { type: 'source', source: 'words', state: 'done', received: 2, reason: null },
      { type: 'source', source: 'meaning', state: 'done', received: 0, reason: null }, { type: 'cursor', cursor: null },
    ];
    await request.post(`${DOUBLE}/__double/summaries/set`, { data: { kind: 'topic', id: ACME.fixLogin, root: ACME.org, state: 'fresh', text: readyText } });
    // The UI-only second root is unavailable in the batch; native current-Read proof remains in Rust.
    await page.route('**/api/v1/search/stream?**', (route) => route.fulfill({ contentType: 'application/x-ndjson', body: events.map((event) => JSON.stringify(event)).join('\n') + '\n' }));
    let refreshes = 0;
    page.on('request', (sent) => { if (sent.method() === 'POST' && new URL(sent.url()).pathname.endsWith('/refresh') && new URL(sent.url()).pathname.startsWith('/api/v1/summaries/')) refreshes += 1; });
    await page.goto('/search?q=ready&org=all');
    await expect(page.locator('[data-search-summary]').filter({ hasText: readyText })).toBeVisible();
    // EN: All-org results → Ask about these results → choose ready Acme, with the other root disabled.
    // 中文：所有组织的结果 → 就这些结果提问 → 选择可用的 Acme，另一组织保持禁用。
    await page.getByRole('button', { name: t('summaryBox.askResults'), exact: true }).click();
    const ready = page.getByRole('menuitem', { name: t('summaryBox.askIn', { name: 'Acme' }), exact: true });
    await expect(ready).not.toHaveAttribute('aria-disabled', 'true');
    await expect(page.getByRole('menuitem', { name: t('summaryBox.askIn', { name: otherName }), exact: true })).toHaveAttribute('aria-disabled', 'true');
    await info.attach('independent result root choices', { body: await page.screenshot(), contentType: 'image/png' });
    await ready.click();
    const input = page.locator('[data-view-window="ask"]').getByRole('textbox', { name: t('ask.composer'), exact: true });
    await expect(input).toHaveValue(new RegExp(readyText));
    expect(await input.inputValue()).not.toContain('Other root snippet');
    expect(ask.sends).toEqual([]); expect(refreshes).toBe(0);
    expect(ask.creates.every((body) => body.root_org_id === ACME.org)).toBe(true);
    await info.attach('only the ready root in an editable Ask draft', { body: await page.screenshot(), contentType: 'image/png' });
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`passive chat inspection reads stored summaries without model actions (${locale}/${theme}/${width})`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 });
    const ask = await fixture(page); const t = await language(page, request, locale, theme);
    const made = await chats('/chat', { org: ACME.org, members: ['dora', 'linus'], name: 'Separate group' });
    if (typeof made.chat !== 'string') throw new Error('The ordinary separate group fixture is required');
    const chat = made.chat;
    await chats('/say', { chat, as: 'dora', text: 'A message for passive inspection.' });
    const text = locale === 'en' ? 'A stored inspected paragraph.' : '查看已有的摘要段落。';
    for (const kind of ['chat', 'chat_title']) await request.post(`${DOUBLE}/__double/summaries/set`, { data: { kind, id: chat, root: ACME.org, state: 'fresh', text } });
    const writes: string[] = [];
    page.on('request', (sent) => {
      const path = new URL(sent.url()).pathname;
      if (sent.method() !== 'GET' && (path.startsWith('/api/v1/summaries/') || path === `/api/v1/chats/${chat}/read` || path === `/api/v1/chats/${chat}/state`)) writes.push(path);
    });
    // EN: Open a group one has not joined → read its stored catch-up → inspect its independent title.
    // 中文：打开未加入的群聊 → 查看已有补看摘要 → 展开独立的聊天标题摘要。
    await page.goto(`/chat/${chat}`);
    const catchUp = page.locator('[data-summary-target="chat"]');
    await expect(catchUp.locator('[data-summary-text]')).toHaveText(text);
    await info.attach('passive stored catch-up', { body: await catchUp.screenshot(), contentType: 'image/png' });
    const askAction = catchUp.getByRole('button', { name: t('summaryBox.catchUpActions.ask'), exact: true });
    const refreshAction = catchUp.getByRole('button', { name: t('summaryBox.catchUpActions.refresh'), exact: true });
    await expect(askAction).toHaveAttribute('aria-disabled', 'true');
    await expect(refreshAction).toHaveAttribute('aria-disabled', 'true');
    await askAction.click(); await refreshAction.click();
    await page.getByRole('button', { name: t('summaryOptions.fields.chat_title'), exact: true }).click();
    const title = page.locator('[data-summary-target="chat_title"]');
    await expect(title.locator('[data-summary-text]')).toHaveText(text);
    await expect(title.getByRole('button', { name: t('summaryBox.chatTitleActions.ask'), exact: true })).toHaveAttribute('aria-disabled', 'true');
    expect(writes).toEqual([]); expect(ask.sends).toEqual([]); expect(ask.creates).toEqual([]);
    await info.attach('independent title remains passive', { body: await page.screenshot(), contentType: 'image/png' });
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`native summary headers and Ask on four pages (${locale}/${theme}/${width})`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 });
    const ask = await fixture(page);
    const t = await language(page, request, locale, theme);
    const seeded = await chats('/seed');
    if (typeof seeded.chat !== 'string') throw new Error('The ordinary chat fixture is required');
    const targets = [
      { kind: 'thread', id: ACME.mainThread, path: `/thread/${ACME.mainThread}` },
      { kind: 'topic', id: ACME.fixLogin, path: `/topic/${ACME.fixLogin}` },
      { kind: 'document', id: DOCS.plan, path: `/doc/${DOCS.plan}` },
      { kind: 'chat', id: seeded.chat, path: `/chat/${seeded.chat}` },
    ] as const;
    for (const target of targets) {
      const labels = target.kind === 'chat' ? 'summaryBox.catchUpActions' : 'summaryBox';
      const text = locale === 'en' ? `A readable ${target.kind} summary.` : `可读的${target.kind}摘要。`;
      const configured = await request.post(`${DOUBLE}/__double/summaries/set`, { data: { kind: target.kind, id: target.id, root: ACME.org, state: 'fresh', text, input: { included_items: 5, shortened_items: 3, omitted_items: 12 } } });
      expect(configured.ok()).toBe(true);
      await page.goto(target.path);
      const box = page.locator(`[data-summary-target="${target.kind}"][data-summary-reference="${target.path}"]`).first();
      await expect(box.locator('[data-summary-text]')).toHaveText(text);
      await info.attach(`${target.kind} summary before refresh`, { body: await box.screenshot(), contentType: 'image/png' });
      // EN: Summary details → actual included/shortened/omitted source counts, without a paid refresh.
      // 中文：摘要详情 → 查看实际纳入、缩短和未纳入的条目数，不触发付费刷新。
      await box.getByRole('button', { name: t(`${labels}.details`), exact: true }).click();
      await expect(page.getByText(t('summaryBox.inputFacts', { included: 5, shortened: 3, omitted: 12 }), { exact: true })).toBeVisible();
      await info.attach(`${target.kind} source coverage`, { body: await page.screenshot(), contentType: 'image/png' });
      await page.keyboard.press('Escape');
      const accepted = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname === `/api/v1/summaries/${target.kind}/${target.id}/refresh`);
      await box.getByRole('button', { name: t(`${labels}.refresh`), exact: true }).click();
      expect((await accepted).status()).toBe(202);
      await expect(box).toHaveAttribute('data-summary-state', 'updating');
      await expect(box.locator('[data-summary-text]')).toHaveText(text);
      await expect(box).toHaveAttribute('data-summary-state', 'fresh');
      await expect(box.locator('[data-summary-text]')).toHaveText(`Completed summary for ${target.kind}.`);
      // EN: A committed receipt is acknowledged and read back, without a false refresh failure.
      // 中文：确认已提交的回执并重新读取摘要，不把成功刷新显示为失败。
      await expect(box.getByRole('alert')).toHaveCount(0);
      await box.getByRole('button', { name: t(`${labels}.ask`), exact: true }).click();
      const input = page.locator('[data-view-window="ask"]').getByRole('textbox', { name: t('ask.composer'), exact: true });
      await expect(input).toHaveValue(new RegExp(target.path));
      expect(await input.inputValue()).toContain(`> Completed summary for ${target.kind}.`);
      expect(ask.sends).toEqual([]);
      expect(ask.creates.every((body) => body.root_org_id === ACME.org)).toBe(true);
      await info.attach(`${target.kind} summary after Ask`, { body: await page.screenshot(), contentType: 'image/png' });
    }
    await request.post(`${DOUBLE}/__double/summaries/set`, { data: { kind: 'document', id: DOCS.plan, root: ACME.org, state: 'failed', text: 'Previous readable result stays.' } });
    await page.goto(`/doc/${DOCS.plan}`);
    const failed = page.locator('[data-summary-target="document"]');
    await expect(failed).toHaveAttribute('data-summary-state', 'failed');
    await expect(failed.locator('[data-summary-text]')).toHaveText('Previous readable result stays.');
    await info.attach('failed update retains readable paragraph', { body: await failed.screenshot(), contentType: 'image/png' });
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`current thread inspector shares summary state, supplier and details (${locale}/${theme}/${width})`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 });
    const ask = await fixture(page); const t = await language(page, request, locale, theme);
    const text = locale === 'en' ? 'The same native paragraph in the inspector.' : '检查器中的同一段原生摘要。';
    await request.post(`${DOUBLE}/__double/summaries/set`, { data: { kind: 'thread', id: ACME.mainThread, root: ACME.org, state: 'fresh', text } });
    await page.goto(`/thread/${ACME.mainThread}`);
    await showView(page, t('views.parameters.title'));
    const inspector = page.locator('[data-summary-inspector]');
    const box = inspector.locator('[data-summary-target="thread"]');
    await expect(box.locator('[data-summary-text]')).toHaveText(text);
    await expect(box).toHaveAttribute('data-summary-state', 'fresh');
    await expect(box).toContainText(t('summaryBox.model'));
    await expect(box.locator('time')).toBeVisible();
    await info.attach('thread summary inspector before', { body: await inspector.screenshot(), contentType: 'image/png' });
    // EN: Thread → Parameters → Summary details → read the declared quote or its missing state.
    // 中文：线程 → 参数 → 摘要详情 → 查看已声明费率或未提供费率状态。
    await box.getByRole('button', { name: t('summaryBox.details'), exact: true }).click();
    await expect(page.getByText(t('summaryBox.noRates'), { exact: true })).toBeVisible();
    await info.attach('thread summary details', { body: await page.screenshot(), contentType: 'image/png' });
    // EN: Open the summary guide from the details heading, then return to this thread's inspector.
    // 中文：从摘要详情标题打开帮助文章，再返回当前线程的参数视图。
    await page.locator('[data-help-guide="summaries"]').click();
    await expect(page.getByRole('heading', { level: 1, name: locale === 'en' ? 'Read and refresh summaries' : '阅读和刷新摘要', exact: true })).toBeVisible();
    await info.attach('authored summary guide', { body: await page.screenshot(), contentType: 'image/png' });
    await page.goto(`/thread/${ACME.mainThread}`); await showView(page, t('views.parameters.title'));
    const accepted = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname === `/api/v1/summaries/thread/${ACME.mainThread}/refresh`);
    await box.getByRole('button', { name: t('summaryBox.refresh'), exact: true }).click();
    expect((await accepted).status()).toBe(202);
    await expect(box.locator('[data-summary-text]')).toHaveText('Completed summary for thread.');
    await box.getByRole('button', { name: t('summaryBox.ask'), exact: true }).click();
    const input = page.locator('[data-view-window="ask"]').getByRole('textbox', { name: t('ask.composer'), exact: true });
    await expect(input).toHaveValue(new RegExp(`/thread/${ACME.mainThread}`));
    expect(ask.sends).toEqual([]);
    expect(ask.creates.every((body) => body.root_org_id === ACME.org)).toBe(true);
    await info.attach('thread inspector opens editable Ask', { body: await page.screenshot(), contentType: 'image/png' });
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`content results reuse cached summaries and open one-root Ask without generation (${locale}/${theme}/${width})`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 });
    const ask = await fixture(page); const t = await language(page, request, locale, theme);
    const cached = locale === 'en' ? 'An existing cached search paragraph.' : '已有的搜索摘要段落。';
    await request.post(`${DOUBLE}/__double/summaries/set`, { data: { kind: 'thread', id: ACME.mainThread, root: ACME.org, state: 'fresh', text: cached } });
    let refreshes = 0;
    page.on('request', (sent) => { if (sent.method() === 'POST' && new URL(sent.url()).pathname.endsWith('/refresh') && new URL(sent.url()).pathname.startsWith('/api/v1/summaries/')) refreshes += 1; });
    await page.goto(`/search?q=redirect&org=${ACME.org}`);
    await expect(page.locator('[data-search-summary]').filter({ hasText: cached }).first()).toBeVisible();
    await info.attach('content results with existing summaries', { body: await page.screenshot(), contentType: 'image/png' });
    await page.getByRole('button', { name: t('summaryBox.askResults'), exact: true }).click();
    const input = page.locator('[data-view-window="ask"]').getByRole('textbox', { name: t('ask.composer'), exact: true });
    await expect(input).toHaveValue(new RegExp(t('summaryBox.askResults')));
    expect(await input.inputValue()).toContain(cached);
    expect(ask.sends).toEqual([]); expect(refreshes).toBe(0);
    expect(ask.creates.every((body) => body.root_org_id === ACME.org)).toBe(true);
    await info.attach('visible results in editable same-root draft', { body: await page.screenshot(), contentType: 'image/png' });
  });
}

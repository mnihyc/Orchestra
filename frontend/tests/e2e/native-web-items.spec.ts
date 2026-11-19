/** Actual native search and page action families use the common timeline, with no inferred body or status. */
// EN: Open the organization, return through its thread tab, expand managed search and page cards,
// follow their source links and inspect the original page output at both viewport widths.
// 中文：打开组织，从线程标签返回，展开托管搜索及网页卡片，检查来源链接，
// 并在两种视口宽度下打开原始输出，核对实际网页内容。
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { rawOf, toolCall } from '../doubles/thread-rows.ts';
import { ACME, DOUBLE, emit, scenario, signIn } from './support.ts';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`native web action families survive the thread click path (${locale}, ${theme})`, async ({ page }, info) => {
    await scenario();
    await signIn(page);
    const appearance = await fetch(`${DOUBLE}/__double/first-steps/appearance`, { method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ locale, theme }), signal: AbortSignal.timeout(10_000) });
    expect(appearance.ok).toBe(true);
    const words = locale === 'en' ? english : chinese;
    const found = toolCall('native-plural-search', { kind: 'web_search', action: { type: 'search' }, query: 'Python official website\nPython docs', results: [] }, { title: 'Search the web', name: 'webSearch' });
    const opened = toolCall('native-open-page', { kind: 'web_search', action: { type: 'open_page', url: 'https://docs.python.org/' }, query: '', results: [] },
      { title: 'Open page: https://docs.python.org/', name: 'webSearch' });
    const searched = toolCall('native-find-page', { kind: 'web_search', action: { type: 'find_in_page', url: 'https://docs.python.org/tutorial/', pattern: 'classes' }, query: '', results: [] },
      { title: 'Find in page: classes', name: 'webSearch' });
    const serverSearch = toolCall('native-server-search', { kind: 'web_search', action: { type: 'search' }, query: 'Native server search', results: [{ title: 'Python', url: 'https://www.python.org/' }] },
      { title: 'Search the web: Native server search', name: 'web_search', output: [{ type: 'web_search_result', title: 'Python', url: 'https://www.python.org/' }] });
    const serverPage = toolCall('native-server-page', { kind: 'web_search', action: { type: 'open_page', url: 'https://docs.python.org/' }, query: '', results: [{ title: 'Python documentation', url: 'https://docs.python.org/' }] },
      { title: 'Open page: https://docs.python.org/', name: 'web_fetch', output: { type: 'web_fetch_result', url: 'https://docs.python.org/', content: { type: 'document', title: 'Python documentation', source: { type: 'text', media_type: 'text/plain', data: 'Only reported document text' } } } });
    const managedSearch = toolCall('managed-web-search', { kind: 'web_search', action: { type: 'search' }, query: 'Managed source query', results: [{ title: 'Managed source', url: 'https://service.example.invalid/source' }] },
      { title: 'Search the web: Managed source query', name: 'web_search', output: { query: 'Managed source query', results: [{ title: 'Managed source', url: 'https://service.example.invalid/source' }] } });
    const managedPage = toolCall('managed-web-page', { kind: 'web_search', action: { type: 'open_page', url: 'https://service.example.invalid/page' }, query: '', results: [{ title: 'Managed page', url: 'https://service.example.invalid/page' }] },
      { title: 'Open page: https://service.example.invalid/page', name: 'web_fetch', output: { url: 'https://service.example.invalid/page', title: 'Managed page', content: 'The actual managed page content', media_type: 'text/markdown' } });
    for (const [item, arguments_] of [[serverSearch, { query: 'Native server search' }], [serverPage, { url: 'https://docs.python.org/' }]] as const) {
      if (item.kind !== 'tool_call') throw new Error('the server operation is a tool item');
      const nativeInput = JSON.stringify(arguments_);
      rawOf(item).input = { inline: nativeInput, total_len: new TextEncoder().encode(nativeInput).length, blob: null, truncated: false };
    }
    if (searched.kind !== 'tool_call') throw new Error('the page operation is a tool item');
    const input = JSON.stringify({ query: null, action: { type: 'findInPage', url: 'https://docs.python.org/tutorial/', pattern: 'classes' } });
    rawOf(searched).input = { inline: input, total_len: new TextEncoder().encode(input).length, blob: null, truncated: false };
    rawOf(searched).output = null;
    await emit(ACME.mainThread, { items: [found, opened, searched, serverSearch, serverPage, managedSearch, managedPage] });
    await page.goto(`/thread/${ACME.mainThread}`);
    await expect(page.locator('[data-thread-header]')).toBeVisible();
    await page.goto('/org/acme');
    await page.getByRole('tab', { name: /Main thread/ }).first().click();
    const search = page.locator('[data-step="native-plural-search"]');
    const open = page.locator('[data-step="native-open-page"]');
    const find = page.locator('[data-step="native-find-page"]');
    const nativeSearch = page.locator('[data-step="native-server-search"]');
    const nativePage = page.locator('[data-step="native-server-page"]');
    const serviceSearch = page.locator('[data-step="managed-web-search"]');
    const servicePage = page.locator('[data-step="managed-web-page"]');
    await search.getByRole('button', { name: /Python official website.*Python docs/ }).click();
    await open.getByRole('button', { name: /docs\.python\.org/ }).click();
    await expect(open).toContainText(words.timeline.step.webOpen.replace('{{address}}', 'docs.python.org'));
    await expect(find).toContainText(words.timeline.step.webFind.replace('{{pattern}}', 'classes'));
    await find.getByRole('button', { name: /classes/ }).click();
    await nativeSearch.getByRole('button', { name: /Native server search/ }).click();
    await nativePage.getByRole('button', { name: /docs\.python\.org/ }).click();
    await serviceSearch.getByRole('button', { name: /Managed source query/ }).click();
    await servicePage.getByRole('button', { name: /service\.example\.invalid/ }).click();
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      await expect(search.getByText(words.timeline.web.noResults, { exact: true })).toBeVisible();
      await expect(open.getByRole('link')).toHaveAttribute('href', 'https://docs.python.org/');
      await expect(find.getByRole('link')).toHaveAttribute('href', 'https://docs.python.org/tutorial/');
      await expect(open.getByText(words.timeline.web.noResults, { exact: true })).toHaveCount(0);
      await expect(find.getByText(words.timeline.web.noResults, { exact: true })).toHaveCount(0);
      await expect(nativeSearch.getByRole('link')).toHaveAttribute('href', 'https://www.python.org/');
      await expect(nativePage.getByRole('link')).toHaveAttribute('href', 'https://docs.python.org/');
      await expect(nativePage).toContainText(words.timeline.step.webOpen.replace('{{address}}', 'docs.python.org'));
      await expect(nativePage.getByText('200', { exact: true })).toHaveCount(0);
      await expect(serviceSearch.getByRole('link')).toHaveAttribute('href', 'https://service.example.invalid/source');
      await expect(servicePage.getByRole('link')).toHaveAttribute('href', 'https://service.example.invalid/page');
      await expect(servicePage).toContainText(words.timeline.step.webOpen.replace('{{address}}', 'service.example.invalid'));
      await expect(servicePage.getByText('200', { exact: true })).toHaveCount(0);
      await expect(open.getByText('200', { exact: true })).toHaveCount(0);
      await expect(find.getByText('200', { exact: true })).toHaveCount(0);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: info.outputPath(`native-web-actions-${width}.png`) });
    }
    await find.getByRole('button', { name: words.timeline.step.showRaw, exact: true }).click();
    await find.getByRole('textbox', { name: words.timeline.raw.search, exact: true }).fill('classes');
    await expect(find.getByText(/classes/).first()).toBeVisible();
    await expect(find.getByText(words.timeline.raw.noOutput, { exact: true })).toBeVisible();
    await nativePage.getByRole('button', { name: words.timeline.step.showRaw, exact: true }).click();
    await expect(nativePage).toContainText('web_fetch');
    const source = words.ui.json.expand.replace('{{name}}', `${words.timeline.raw.output}.content.source`);
    await nativePage.getByRole('button', { name: source, exact: true }).click();
    await expect(nativePage).toContainText('Only reported document text');
    await servicePage.getByRole('button', { name: words.timeline.step.showRaw, exact: true }).click();
    await expect(servicePage).toContainText('web_fetch');
    await expect(servicePage).toContainText('The actual managed page content');
  });
}

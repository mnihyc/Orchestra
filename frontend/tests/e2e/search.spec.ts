import type { SearchHit, SearchStreamEvent } from '../../src/generated/api-types';
/** Search and finding in a conversation at work: words looked for in what was said, kept to a place and to kinds, results with the words marked and opened at the hit, more read as the reader comes to the end, and the bar that finds inside one thread. */
import { expect, type Page, type Request, test } from '@playwright/test';

import { ADA } from '../doubles/thread-scenes.ts';
import { userMessage } from '../doubles/thread-rows.ts';
import { chats } from './chat-support.ts';
import { DOCS } from './documents-support.ts';
import { id, MADE_AT } from '../doubles/fixtures.ts';
import { ACME, deny, emit, PEOPLE, playScene, scenario, signIn } from './support.ts';

const MAIN = ACME.mainThread;

test.beforeEach(async () => {
  await scenario();
  await playScene(MAIN, 'history');
  // What people said to each other in the topic's chat is the chats' own world: it is found beside what the agents said.
  await chats('/say', { node: 'Fix login', as: 'grace', text: 'Did anyone look at the redirect loop after login?', at: '2026-10-01T09:20:00.000Z' });
  await chats('/say', { node: 'Fix login', as: 'ada', text: 'Not yet. I will read the handler first.', at: '2026-10-01T09:25:00.000Z' });
});

const results = (page: Page) => page.getByRole('region', { name: 'Results', exact: true });
const resultCount = (page: Page) => results(page).locator(':scope > [role="status"]');
// Reference-name chips inspect a source; the result's direct body link opens the exact hit.
const cards = (page: Page) => results(page).locator('[data-search-rows] > li > a[aria-label]');
const threadCards = (page: Page) => results(page).locator('[data-search-rows] > li')
  .filter({ has: page.locator(`a[href^="/thread/${MAIN}?item="]`) }).locator(':scope > a[aria-label]');
const words = (page: Page) => page.getByRole('textbox', { name: 'Words to look for' });

test('a linked search starts without another submit after the page opens', async ({ page }) => {
  await signIn(page);
  const started = page.waitForResponse((response) => new URL(response.url()).pathname.endsWith('/search/stream'));
  await page.goto('/search?q=redirect');
  expect((await started).status()).toBe(200);
  await expect(cards(page).first()).toBeVisible();
  await expect(words(page)).toHaveValue('redirect');
});

test('turning off every conversation kind keeps the result set empty', async ({ page }) => {
  await signIn(page);
  const sent = page.waitForRequest((request) => new URL(request.url()).pathname.endsWith('/search/stream'));
  await page.goto('/search?q=redirect&tab=conversations&kinds=chat_message%2Cdocument');
  const url = new URL((await sent).url());
  expect(url.searchParams.has('kinds')).toBe(true);
  expect(url.searchParams.get('kinds')).toBe('');
  await expect(results(page).getByText('Nothing found for “redirect”')).toBeVisible();
  await expect(page.getByRole('button', {name:/Clear: Conversation kinds: 0 of 9/})).toBeVisible();
});

test('one search box leads the page with a desktop filter column, phone sheet and a quiet beside action', async ({ page }) => {
  await signIn(page);
  for (const [width, height] of [[1440, 900], [400, 860]] as const) {
    await page.setViewportSize({ width, height });
    await page.goto('/search');
    await expect(words(page)).toBeVisible();
    for (const name of ['All', 'Conversations', 'Chats', 'Documents', 'Files']) await expect(page.getByRole('tab', { name, exact: true })).toBeVisible();
    const sidebar = page.getByRole('complementary', { name: 'Narrow the search' });
    const button = page.getByRole('button', { name: 'Filters', exact: true });
    if (width >= 1024) {
      await expect(sidebar).toBeVisible();
      await expect(sidebar.getByRole('combobox', { name: 'Place' })).toBeVisible();
      await expect(button).toBeHidden();
    } else {
      await expect(sidebar).toBeHidden();
      await expect(button).toBeVisible();
      await button.click();
      const filters = page.getByRole('dialog', { name: 'Narrow the search', exact: true });
      await expect(filters.getByRole('combobox', { name: 'Place' })).toBeVisible();
      const filterBox = await filters.boundingBox();
      if (filterBox === null) throw new Error('the requested filters are visible');
      expect(filterBox.x).toBeGreaterThanOrEqual(0);
      expect(filterBox.x + filterBox.width).toBeLessThanOrEqual(width);
      await page.keyboard.press('Escape');
      await expect(button).toBeFocused();
    }
    await words(page).fill('redirect');
    await words(page).press('Enter');
    await expect(cards(page).first()).toBeVisible();
    const tabsBox = await page.getByRole('tablist', { name: 'Result categories' }).boundingBox();
    const resultBox = await results(page).boundingBox();
    if (tabsBox === null || resultBox === null) throw new Error('the tabs and results are visible');
    expect(Math.abs(tabsBox.width - resultBox.width)).toBeLessThanOrEqual(1);
    await expect(results(page).getByText('Open to the side', { exact: true })).toHaveCount(0);
    const card = cards(page).first().locator('..');
    const side = card.getByRole('button', { name: 'Open to the side', exact: true });
    await expect(side).toBeVisible();
    await expect(side).toHaveText('');
    const cardBox = await card.boundingBox();
    const sideBox = await side.boundingBox();
    if (cardBox === null || sideBox === null) throw new Error('the result and its icon action are visible');
    expect(sideBox.y).toBeGreaterThanOrEqual(cardBox.y);
    expect(sideBox.y + sideBox.height).toBeLessThanOrEqual(cardBox.y + cardBox.height);
    const before = page.url();
    const target = new URL(await cards(page).first().getAttribute('href') ?? '', before);
    await side.click();
    // Beside now opens an ordinary page in another group; that focused page owns the address.
    const opened = page.getByRole('tabpanel', { name: 'Rollout plan', exact: true });
    await expect(opened.getByRole('textbox', { name: 'Document text', exact: true })).toBeVisible();
    await expect.poll(() => {
      const address = new URL(page.url());
      return { pathname: address.pathname, block: address.searchParams.get('block') };
    }).toEqual({ pathname: target.pathname, block: target.searchParams.get('block') });
    const tab = page.getByRole('tab', { name: 'Rollout plan', exact: true });
    await tab.click({ button: 'right' });
    await page.getByRole('menuitem', { name: 'Close', exact: true }).click();
    await expect(tab).toHaveCount(0);
    await expect(page).toHaveURL(before);
    await expect(words(page)).toBeVisible();
    await expect(words(page)).toHaveValue('redirect');
  }
});

test('words are looked for in what was said, and each result says where it lies and shows the match', async ({ page }) => {
  await signIn(page);
  await page.goto('/search?q=redirect');

  await expect(cards(page).first()).toBeVisible();
  const first = threadCards(page).first();
  // Newest thread hit first; its card keeps the readable path beside the source-name chip.
  await expect(first.locator('..')).toContainText('Main thread');
  await expect(first.locator('..')).toContainText('Web › Fix login ›');
  await expect(first.locator('mark').first()).toHaveText(/redirect/i);
  // What was said by somebody else, in a chat, is found beside what the agents said.
  await expect(results(page).getByText('Did anyone look at the redirect loop after login?')).toBeVisible();
  await expect(resultCount(page)).toBeVisible();
});

test('Best matches puts a place name first and keeps its order in the address after reload', async ({ page }) => {
  await signIn(page);
  await page.goto('/search?q=fix+login');
  const order = page.getByRole('combobox', { name: 'Order', exact: true });
  await order.click();
  await page.getByRole('option', { name: 'Best matches', exact: true }).click();
  await expect(page).toHaveURL(/sort=relevance/);
  await expect(cards(page).first()).toHaveAttribute('href', `/topic/${ACME.fixLogin}`);
  await expect(cards(page).first().locator('..')).toContainText('In the name');
  await page.reload();
  await expect(order).toContainText('Best matches');
  await expect(cards(page).first()).toHaveAttribute('href', `/topic/${ACME.fixLogin}`);
  await cards(page).first().click();
  // The topic overview keeps its list choices in its address.
  await expect.poll(() => new URL(page.url()).pathname).toBe(`/topic/${ACME.fixLogin}`);
  await expect(page.getByRole('heading', { name: 'Fix login', level: 1, exact: true })).toBeVisible();
});

test('short search terms reach the streamed sources and keep their matches highlighted', async ({ page }) => {
  await signIn(page);
  await page.goto('/search');
  await expect(page.getByText('Search what was said')).toBeVisible();

  // Streamed sources own their minimum length; the former client-side two/three-character block is gone.
  for (const query of ['r', 're']) {
    const sent = page.waitForRequest((request) => {
      const url = new URL(request.url());
      return url.pathname.endsWith('/search/stream') && url.searchParams.get('q') === query;
    });
    await words(page).fill(query);
    await words(page).press('Enter');
    await sent;
    await expect(cards(page).first()).toBeVisible();
    await expect(cards(page).first().locator('mark').first()).toHaveText(new RegExp(query, 'i'));
    await expect(page.getByText('Type at least two characters')).toHaveCount(0);
    await expect(page.getByText('Type one more character, or search inside one topic')).toHaveCount(0);
  }
});

test('a search is kept to a place, and two characters are enough inside a topic', async ({ page }) => {
  await signIn(page);
  await page.goto('/search?q=re');
  await page.getByRole('combobox', { name: 'Place' }).fill('fix login');
  await page.getByRole('option', { name: /Fix login/ }).first().click();

  await expect(page).toHaveURL(/scope=[0-9a-f-]{36}/);
  await expect(page.getByRole('button', { name: /Clear: Place: Fix login/ })).toBeVisible();
  await expect(cards(page).first()).toBeVisible();
  // Everything found lies in the place that was named.
  for (const card of await cards(page).all()) await expect(card.locator('..')).toContainText(/Fix login/);
  await expect(page.getByText('Type one more character, or search inside one topic')).toHaveCount(0);

  await page.getByRole('button', { name: 'Clear', exact: true }).click();
  await expect(page).not.toHaveURL(/scope=/);
  // Clearing a place keeps the two-character streamed query active.
  await expect(cards(page).first()).toBeVisible();
  await expect(page.getByText('Type one more character, or search inside one topic')).toHaveCount(0);
});

test('kinds and days narrow it, and the address says how', async ({ page }) => {
  await signIn(page);
  await page.goto('/search?q=redirect');
  await expect(cards(page).first()).toBeVisible();

  await page.getByRole('tab', { name: 'Documents', exact: true }).click();
  await expect(page).toHaveURL(/tab=documents/);
  await expect(results(page).getByText('Release checklist').first()).toBeVisible();
  await expect(threadCards(page)).toHaveCount(0);

  const filters = page.getByRole('complementary', { name: 'Narrow the search' });
  await filters.getByLabel('From', { exact: true }).fill('2027-01-01');
  await expect(page).toHaveURL(/from=2027-01-01T/);
  await expect(page).toHaveURL(/date_field=edited/);
  await expect(page.getByText('Nothing found for “redirect”')).toBeVisible();
  await expect(page.getByText(/clear the filters to look everywhere/)).toBeVisible();
});

test('owner and edited date narrow documents on desktop and phone', async ({ page }) => {
  await signIn(page);
  for (const [width, height] of [[1440, 900], [400, 860]] as const) {
    await page.setViewportSize({ width, height });
    await page.goto('/search?q=canary&tab=documents');
    const requests: URL[] = [];
    const record = (request: Request) => {
      if (new URL(request.url()).pathname.endsWith('/search/stream')) requests.push(new URL(request.url()));
    };
    page.on('request', record);
    const filters = width >= 1024 ? page.getByRole('complementary', { name: 'Narrow the search' }) : page.getByRole('dialog', { name: 'Narrow the search' });
    if (width < 1024) await page.getByRole('button', { name: /Filters/ }).click();
    const owner = filters.getByRole('combobox', { name: 'Owner', exact: true });
    await owner.fill('Dora');
    await page.getByRole('option', { name: /Dora Developer/ }).click();
    await filters.getByLabel('From', { exact: true }).fill('2026-10-01');
    await filters.getByLabel('Up to', { exact: true }).fill('2026-10-01');
    if (width < 1024) await page.keyboard.press('Escape');
    const clearOwner = page.getByRole('button', { name: /Clear: Owner: Dora Developer/ });
    await expect(clearOwner).toBeVisible();
    await expect(clearOwner).toBeInViewport({ ratio: 1 });
    await expect(page.getByRole('button', { name: /Clear: Edited:/ })).toBeVisible();
    await expect(results(page).getByText('Notes from the canary').first()).toBeVisible();
    await expect(results(page).getByText('Rollout plan')).toHaveCount(0);
    await expect.poll(() => requests.some((url) => url.searchParams.get('owner_id') !== null
      && url.searchParams.get('date_field') === 'edited' && url.searchParams.has('from') && url.searchParams.has('to'))).toBe(true);
    page.off('request', record);
  }
});

test('an exact document selection reads its label without loading attachments', async ({ page }) => {
  await signIn(page);
  const requests: URL[] = [];
  page.on('request', (request) => {
    if (request.method() === 'GET') requests.push(new URL(request.url()));
  });
  await page.goto(`/search?q=canary&document_id=${DOCS.notes}`);
  await expect(page.getByRole('complementary', {name:'Narrow the search'}).getByRole('combobox', {name:'Place',exact:true})).toHaveValue('Notes from the canary');
  await expect(results(page).getByRole('link', {name:'Notes from the canary',exact:true}).first()).toBeVisible();
  expect(requests.filter((url) => url.pathname.endsWith(`/documents/${DOCS.notes}`))).toHaveLength(0);
  expect(requests.filter((url) => url.pathname.endsWith('/references/preview') && url.searchParams.get('kind') === 'document' && url.searchParams.get('id') === DOCS.notes)).toHaveLength(1);
});

test('the files category exposes a matching file and the file type narrows it', async ({ page }) => {
  const files = [
    {id:id(10, 20),name:'data.csv',media_type:'text/csv',size:10,is_text:true,added_by:null,added_at:MADE_AT},
    {id:id(10, 21),name:'report.pdf',media_type:'application/pdf',size:20,is_text:false,added_by:null,added_at:MADE_AT},
  ];
  const port = process.env.ORCH_DOUBLE_PORT ?? '41871';
  const configured = await fetch(`http://127.0.0.1:${port}/__double/documents/files`, {method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({doc:DOCS.plan,files})});
  expect(configured.ok).toBe(true);
  await signIn(page);
  await page.goto('/search?q=rollout');
  await page.getByRole('tab', {name:'Files',exact:true}).click();
  // The document title and its matching heading are separate results.
  const csv = results(page).getByRole('link', {name:'data.csv',exact:true});
  await expect(csv).toHaveCount(2);
  for (const chip of await csv.all()) await expect(chip).toBeVisible();
  const fileType = page.getByRole('complementary', {name:'Narrow the search'}).getByRole('combobox', {name:'File type'});
  await fileType.click();
  await page.getByRole('option', {name:'PDF',exact:true}).click();
  const pdf = results(page).getByRole('link', {name:'report.pdf',exact:true});
  await expect(pdf).toHaveCount(2);
  for (const chip of await pdf.all()) await expect(chip).toBeVisible();
  await expect(csv).toHaveCount(0);
});

test('a result opens the thread at the hit, and lights it', async ({ page }) => {
  await signIn(page);
  await page.goto('/search?q=redirect');
  const card = threadCards(page).first();
  await expect(card).toBeVisible();
  await card.click();

  await expect(page).toHaveURL(new RegExp(`/thread/${MAIN}\\?item=`));
  await expect(page.getByRole('log', { name: 'Conversation' })).toBeVisible();
  await expect(page.locator('.ring-accent').first()).toBeVisible();
});

test('a result is a real link: control and a click open it in a new tab', async ({ page, context }) => {
  await signIn(page);
  await page.goto('/search?q=redirect');
  const card = threadCards(page).first();
  await expect(card).toHaveAttribute('href', new RegExp(`^/thread/${MAIN}\\?item=`));
  const [opened] = await Promise.all([context.waitForEvent('page'), card.click({ modifiers: ['Control'] })]);
  await opened.waitForLoadState('domcontentloaded');
  expect(opened.url()).toContain(`/thread/${MAIN}`);
  await opened.close();
});

test('Show more reveals the received results and retains newest-first order', async ({ page }) => {
  await signIn(page);
  const make = (from: number, to: number) =>
    Array.from({ length: to - from }, (_, index) =>
      userMessage(`needle-${from + index}`, `needle number ${from + index}`, { author: ADA, at: new Date(Date.parse('2026-10-02T10:00:00.000Z') + (from + index) * 1_000).toISOString() }),
    );
  await emit(MAIN, { items: make(0, 45) });
  await page.goto('/search?q=needle');

  const shown = resultCount(page);
  // The status counts received hits; the visible page still starts at twenty.
  await expect(shown).toContainText('45 results');
  await expect(cards(page)).toHaveCount(20);
  // The newest comes first.
  await expect(cards(page).first()).toContainText('needle number 44');

  // The old HTTP pages became one initial screen plus all buffered stream hits; this receipt has no continuation cursor.
  await results(page).getByRole('button', { name: 'Show more', exact: true }).click();
  await expect(cards(page)).toHaveCount(45);
  await expect(cards(page).last()).toContainText('needle number 0');
  await expect(shown).toContainText('45 results');
  await expect(results(page).getByRole('button', { name: 'Show more', exact: true })).toHaveCount(0);
  await expect(results(page).getByText('That is everything that was found.')).toBeVisible();
});

test('a conversation the person may not read is not searched', async ({ page }) => {
  await signIn(page, PEOPLE.vera);
  await page.goto('/search?q=handler');
  await expect(threadCards(page).first()).toBeVisible();
  await deny('vera', ['thread.view']);
  await page.goto('/search?q=handler');
  // What was said in the topic's chat is still hers to read; what was said to the agent in its thread is not found, nor counted.
  await expect(cards(page).first()).toBeVisible();
  await expect(threadCards(page)).toHaveCount(0);
  await page.goto('/search?q=redirect%20after');
  await expect(threadCards(page)).toHaveCount(0);
});

test('find in the conversation: a bar under the header, the hits lit, and Enter going to the next older one', async ({ page }) => {
  await signIn(page);
  await page.goto(`/thread/${MAIN}`);
  await expect(page.getByRole('log', { name: 'Conversation' })).toBeVisible();

  await page.locator('[data-thread-header]').getByRole('button', { name: 'Find in the conversation', exact: true }).click();
  const bar = page.getByRole('search', { name: 'Find in the conversation' });
  await expect(bar).toBeVisible();
  const field = bar.getByRole('textbox', { name: 'Find in the conversation' });
  await expect(field).toBeFocused();

  await field.fill('redirect');
  await expect(bar.getByRole('status')).toHaveText(/^1 of \d+\+?$/);
  // The words are lit where they stand, and the item is lit.
  await expect.poll(() => page.evaluate(() => (CSS as unknown as { highlights?: { has: (name: string) => boolean } }).highlights?.has('orch-find') ?? true)).toBe(true);
  await expect(page.locator('.ring-accent').first()).toBeVisible();

  await field.press('Enter');
  await expect(bar.getByRole('status')).toHaveText(/^2 of \d+\+?$/);
  await field.press('Shift+Enter');
  await expect(bar.getByRole('status')).toHaveText(/^1 of \d+\+?$/);

  await field.fill('zzzzzz');
  await expect(bar.getByRole('status')).toHaveText('No match');
  await field.press('Escape');
  await expect(bar).toHaveCount(0);
});

test('the finder lists find in the conversation among the commands of a thread', async ({ page }) => {
  await signIn(page);
  await page.goto(`/thread/${MAIN}`);
  await expect(page.getByRole('log', { name: 'Conversation' })).toBeVisible();
  await page.keyboard.press('Control+k');
  await page.getByRole('combobox', { name: 'Search terms', exact: true }).pressSequentially('>find in', { delay: 20 });
  await page.getByRole('option', { name: /Find in the conversation/ }).click();
  await expect(page.getByRole('search', { name: 'Find in the conversation' })).toBeVisible();
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`keeps a name-only thread result free of private work facts (${locale}/${theme}/${width})`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    const name = locale === 'en' ? 'Quiet reference' : '仅有名称的引用';
    const node = { id: MAIN, kind: 'thread' as const, name, redacted: false };
    const hit: SearchHit = { kind: 'name', node, path: [node], href: `/thread/${MAIN}`, at: MADE_AT,
      snippet: locale === 'en' ? 'A name without a readable work state' : '仅有名称，没有可读的工作状态', matches: [] };
    const events: SearchStreamEvent[] = [
      { type: 'options', meaning_pause_ms: 0, page_size: 20, reorder_window_secs: 0 },
      { type: 'hit', id: 'quiet-reference', hit, found_by: 'words' },
      { type: 'order', received: 1, items: [{ id: 'quiet-reference', found_by: 'words', rank: 1 }] },
      { type: 'source', source: 'words', state: 'done', received: 1, reason: null },
      { type: 'source', source: 'meaning', state: 'done', received: 0, reason: null },
      { type: 'cursor', cursor: null },
    ];
    await page.route('**/api/v1/search/stream?**', (route) => route.fulfill({ contentType: 'application/x-ndjson', body: events.map((event) => JSON.stringify(event)).join('\n') + '\n' }));
    await page.goto(`/search?q=${encodeURIComponent(name)}`);
    const reference = page.getByRole('link', { name, exact: true });
    await expect(reference).toBeVisible();
    await expect(reference.locator('svg.lucide-message-square-text')).toHaveCount(1);
    await expect(reference.locator('[data-work-state]')).toHaveCount(0);
    await expect(reference.getByRole('img')).toHaveCount(0);
  });
}

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

const results = (page: Page) => page.getByRole('region', { name: 'Results' });
const cards = (page: Page) => results(page).getByRole('link');
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
    await side.click();
    await expect(page.locator('[data-search-result]').first()).toBeVisible();
    expect(page.url()).toBe(before);
    await page.locator('[data-search-result]').first().getByRole('button', { name: 'Return to the conversation', exact: true }).click();
    await expect(page.locator('[data-search-result]')).toHaveCount(0);
  }
});

test('words are looked for in what was said, and each result says where it lies and shows the match', async ({ page }) => {
  await signIn(page);
  await page.goto('/search?q=redirect');

  await expect(cards(page).first()).toBeVisible();
  const first = cards(page).first();
  // Newest first; the way to the thread is under the name of the thread.
  await expect(first).toContainText('Main thread');
  await expect(first).toContainText('Web › Fix login › Main thread');
  await expect(first.locator('mark').first()).toHaveText(/redirect/i);
  // What was said by somebody else, in a chat, is found beside what the agents said.
  await expect(results(page).getByText('Did anyone look at the redirect loop after login?')).toBeVisible();
  await expect(results(page).getByText(/results? shown/)).toBeVisible();
});

test('Best matches puts a place name first and keeps its order in the address after reload', async ({ page }) => {
  await signIn(page);
  await page.goto('/search?q=fix+login');
  const order = page.getByRole('combobox', { name: 'Order', exact: true });
  await order.click();
  await page.getByRole('option', { name: 'Best matches', exact: true }).click();
  await expect(page).toHaveURL(/sort=relevance/);
  await expect(cards(page).first()).toHaveAttribute('href', `/topic/${ACME.fixLogin}`);
  await expect(cards(page).first().locator('..')).toContainText('Place name');
  await page.reload();
  await expect(order).toContainText('Best matches');
  await expect(cards(page).first()).toHaveAttribute('href', `/topic/${ACME.fixLogin}`);
  await cards(page).first().click();
  await expect(page).toHaveURL(new RegExp(`/topic/${ACME.fixLogin}$`));
});

test('words that are too short say so, and what to do', async ({ page }) => {
  await signIn(page);
  await page.goto('/search');
  await expect(page.getByText('Search what was said')).toBeVisible();

  await words(page).fill('r');
  await words(page).press('Enter');
  await expect(page.getByText('Type at least two characters')).toBeVisible();

  await words(page).fill('re');
  await words(page).press('Enter');
  await expect(page.getByText('Type one more character, or search inside one topic')).toBeVisible();
  await expect(cards(page)).toHaveCount(0);
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
  for (const card of await cards(page).all()) await expect(card).toContainText(/Fix login/);
  await expect(page.getByText('Type one more character, or search inside one topic')).toHaveCount(0);

  await page.getByRole('button', { name: 'Clear', exact: true }).click();
  await expect(page).not.toHaveURL(/scope=/);
  await expect(page.getByText('Type one more character, or search inside one topic')).toBeVisible();
});

test('kinds and days narrow it, and the address says how', async ({ page }) => {
  await signIn(page);
  await page.goto('/search?q=redirect');
  await expect(cards(page).first()).toBeVisible();

  await page.getByRole('tab', { name: 'Documents', exact: true }).click();
  await expect(page).toHaveURL(/tab=documents/);
  await expect(results(page).getByText('Release checklist').first()).toBeVisible();
  await expect(cards(page).filter({ hasText: 'Main thread' })).toHaveCount(0);

  const filters = page.getByRole('complementary', { name: 'Narrow the search' });
  await filters.getByLabel('From').fill('2027-01-01');
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
  const csv = results(page).getByRole('button', {name:'data.csv',exact:true});
  await expect(csv).toHaveCount(2);
  for (const chip of await csv.all()) await expect(chip).toBeVisible();
  const fileType = page.getByRole('complementary', {name:'Narrow the search'}).getByRole('combobox', {name:'File type'});
  await fileType.click();
  await page.getByRole('option', {name:'PDF',exact:true}).click();
  const pdf = results(page).getByRole('button', {name:'report.pdf',exact:true});
  await expect(pdf).toHaveCount(2);
  for (const chip of await pdf.all()) await expect(chip).toBeVisible();
  await expect(csv).toHaveCount(0);
});

test('a result opens the thread at the hit, and lights it', async ({ page }) => {
  await signIn(page);
  await page.goto('/search?q=redirect');
  const card = cards(page).filter({ hasText: 'Main thread' }).first();
  await expect(card).toBeVisible();
  await card.click();

  await expect(page).toHaveURL(new RegExp(`/thread/${MAIN}\\?item=`));
  await expect(page.getByRole('log', { name: 'Conversation' })).toBeVisible();
  await expect(page.locator('.ring-accent').first()).toBeVisible();
});

test('a result is a real link: control and a click open it in a new tab', async ({ page, context }) => {
  await signIn(page);
  await page.goto('/search?q=redirect');
  const card = cards(page).filter({ hasText: 'Main thread' }).first();
  await expect(card).toHaveAttribute('href', new RegExp(`^/thread/${MAIN}\\?item=`));
  const [opened] = await Promise.all([context.waitForEvent('page'), card.click({ modifiers: ['Control'] })]);
  await opened.waitForLoadState('domcontentloaded');
  expect(opened.url()).toContain(`/thread/${MAIN}`);
  await opened.close();
});

test('more is read as the reader comes to the end of the list', async ({ page }) => {
  await signIn(page);
  const make = (from: number, to: number) =>
    Array.from({ length: to - from }, (_, index) =>
      userMessage(`needle-${from + index}`, `needle number ${from + index}`, { author: ADA, at: new Date(Date.parse('2026-10-02T10:00:00.000Z') + (from + index) * 1_000).toISOString() }),
    );
  await emit(MAIN, { items: make(0, 45) });
  await page.goto('/search?q=needle');

  const shown = results(page).getByText(/results? shown/);
  await expect(shown).toHaveText('20 results shown');
  await expect(cards(page)).toHaveCount(20);
  // The newest comes first.
  await expect(cards(page).first()).toContainText('needle number 44');

  await results(page).getByRole('button', { name: 'Show more results' }).scrollIntoViewIfNeeded();
  await expect(shown).toHaveText('40 results shown');
  await results(page).getByRole('button', { name: 'Show more results' }).scrollIntoViewIfNeeded();
  await expect(shown).toHaveText('45 results shown');
  await expect(results(page).getByText('That is everything that was found.')).toBeVisible();
});

test('a conversation the person may not read is not searched', async ({ page }) => {
  await signIn(page, PEOPLE.vera);
  await page.goto('/search?q=handler');
  await expect(cards(page).filter({ hasText: 'Main thread' }).first()).toBeVisible();
  await deny('vera', ['thread.view']);
  await page.goto('/search?q=handler');
  // What was said in the topic's chat is still hers to read; what was said to the agent in its thread is not found, nor counted.
  await expect(cards(page).first()).toBeVisible();
  await expect(cards(page).filter({ hasText: 'Main thread' })).toHaveCount(0);
  await page.goto('/search?q=redirect%20after');
  await expect(cards(page).filter({ hasText: 'Main thread' })).toHaveCount(0);
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

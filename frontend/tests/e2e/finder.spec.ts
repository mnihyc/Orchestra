/** The finder at work: opened by its key from anywhere, names found as they are typed and opened by Enter, commands run from it, the signs that keep it to one kind of answer, what a person may not see never listed, and the list of keys drawn from what is bound. */
import { expect, type Page, test } from '@playwright/test';

import { ACME, bodySent, deny, failNames, fillNames, holdSearch, PEOPLE, scenario, signIn } from './support.ts';
import { id, LATER_AT } from '../doubles/fixtures.ts';
import { DOCS } from './documents-support.ts';

test.beforeEach(async () => {
  await scenario();
});

const finder = (page: Page) => page.getByRole('dialog', { name: 'Find', exact: true });
const field = (page: Page) => finder(page).getByRole('combobox', { name: 'Search terms', exact: true });
const result = (page: Page, name: string | RegExp) => finder(page).getByRole('option', { name });
/** What the list says when nothing answers: said once for the eye and once for a screen reader, so it is looked for in the list. */
const nothingFound = (page: Page, query: string) => finder(page).getByRole('listbox').getByText(`Nothing found for “${query}”`);

/**
 * Waits until the page is at rest: what a key did is drawn a few turns of its event loop after the key. A hand presses the next key
 * when it has seen what the last did; the keys of the workbench are listened for from the moment its frame has been drawn.
 */
const atRest = (page: Page): Promise<void> =>
  page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => requestIdleCallback(() => resolve(), { timeout: 2_000 }))));

async function openFinder(page: Page): Promise<void> {
  // The frame is drawn, and with it the listener of the keys.
  await expect(page.getByRole('button', { name: 'Find anything' })).toBeVisible();
  await atRest(page);
  await page.keyboard.press('Control+k');
  await expect(finder(page)).toBeVisible();
  await expect(field(page)).toBeFocused();
}

/** Types as a person does, so that the pause the finder waits for is the one it would wait for. */
async function typeIn(page: Page, text: string): Promise<void> {
  await field(page).pressSequentially(text, { delay: 20 });
}

test('the finder opens by its key from anywhere, and gives the keyboard back to where it was', async ({ page }) => {
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  const composer = page.getByRole('textbox', { name: 'Message to the agent' });
  await composer.focus();

  // From inside a field, where a key that types is the field's but a chord with the modifier is the workbench's.
  await openFinder(page);
  await expect(finder(page).getByText('Find anything, or type > for a command, ? for help')).toHaveCount(0);
  await expect(field(page)).toHaveAttribute('placeholder', 'Name, words or command');

  await page.keyboard.press('Escape');
  await expect(finder(page)).toHaveCount(0);
  await expect(composer).toBeFocused();

  // The magnifier in the rail does the same.
  await page.getByRole('button', { name: 'Find anything' }).click();
  await expect(finder(page)).toBeVisible();
  await page.keyboard.press('Control+k');
  await expect(finder(page)).toHaveCount(0);
});

test('with nothing typed it lists where the person went and offers what can be done here', async ({ page }) => {
  await signIn(page);
  // Going to a place is told to the server, which is what makes it one of the person's recents.
  const told = page.waitForResponse((response) => response.url().includes('/me/recents') && response.request().method() === 'POST');
  await page.goto(`/topic/${ACME.fixLogin}`);
  await expect(page.getByRole('heading', { name: 'Fix login', level: 1 })).toBeVisible();
  await told;
  await openFinder(page);

  await expect(finder(page).getByText('Recent', { exact: true })).toBeVisible();
  await expect(result(page, /Fix login/)).toBeVisible();
  await expect(result(page, /What can I do here\?/)).toBeVisible();
  await expect(result(page, /Help for this screen/)).toBeVisible();
  // The list of every command is for the sign that asks for it.
  await expect(finder(page).getByText('Commands', { exact: true })).toHaveCount(0);

  await result(page, /What can I do here\?/).click();
  await expect(field(page)).toHaveValue('>');
  await expect(result(page, /Rename/)).toBeVisible();
});

test('documents and resources share Starred, Recent and the same selected-name star action', async ({ page }) => {
  await signIn(page);
  await openFinder(page);
  await typeIn(page, 'team anthropic');
  await expect(result(page, /Team Anthropic/)).toBeVisible();
  await finder(page).getByRole('button', { name: 'Star Team Anthropic', exact: true }).click();
  await expect(finder(page).getByRole('button', { name: 'Unstar Team Anthropic', exact: true })).toBeVisible();
  const providerVisit = page.waitForResponse(async (response) => response.url().endsWith('/me/visits')
    && response.request().method() === 'POST' && ((await bodySent(response.request())) as { kind?: string }).kind === 'provider');
  await field(page).focus();
  await field(page).press('Enter');
  await expect(page).toHaveURL(/\/resources\/providers\//);
  expect((await providerVisit).status()).toBe(204);

  const documentVisit = page.waitForResponse(async (response) => response.url().endsWith('/me/visits')
    && response.request().method() === 'POST' && ((await bodySent(response.request())) as { kind?: string }).kind === 'document');
  await page.goto(`/doc/${DOCS.plan}`);
  await expect(page.getByRole('textbox', { name: 'Title', exact: true })).toHaveValue('Rollout plan');
  expect((await documentVisit).status()).toBe(204);
  await openFinder(page);
  await expect(finder(page).getByText('Starred', { exact: true })).toBeVisible();
  await expect(result(page, /Team Anthropic/)).toBeVisible();
  await expect(finder(page).getByText('Recent', { exact: true })).toBeVisible();
  await expect(result(page, /Rollout plan/)).toBeVisible();

  await field(page).fill('rollout plan');
  await finder(page).getByRole('button', { name: 'Star Rollout plan', exact: true }).click();
  await expect(finder(page).getByRole('button', { name: 'Unstar Rollout plan', exact: true })).toBeVisible();
  await field(page).fill('team anthropic');
  await finder(page).getByRole('button', { name: 'Unstar Team Anthropic', exact: true }).click();
  await expect(finder(page).getByRole('button', { name: 'Star Team Anthropic', exact: true })).toBeVisible();
  await field(page).clear();
  await expect(result(page, /Rollout plan/)).toBeVisible();
  await expect(result(page, /Team Anthropic/)).toBeVisible();
});

test('a name is found by three letters, says where it lies, and is opened by Enter', async ({ page }) => {
  await signIn(page);
  await openFinder(page);
  await typeIn(page, 'fix');

  // The topic first, with the way to it under its name.
  const topic = result(page, /Fix login.*topic/);
  await expect(topic).toBeVisible();
  await expect(topic).toContainText('Acme › Research › Web');
  await expect(topic).toHaveAttribute('aria-selected', 'true');
  await page.keyboard.press('Enter');

  await expect(page).toHaveURL(new RegExp(`/topic/${ACME.fixLogin}$`));
  await expect(page.getByRole('heading', { name: 'Fix login', level: 1 })).toBeVisible();
  await expect(finder(page)).toHaveCount(0);
});

test('an org tag asks for an eligible project and opens its real List with that exact filter', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme');
  await expect(page.getByRole('heading', { name: 'Acme', level: 1 })).toBeVisible();
  await openFinder(page);
  await field(page).fill('bug');
  const tag = result(page, /^bug.*tag/);
  await expect(tag).toBeVisible();
  await tag.click();
  const chooser = page.getByRole('dialog', { name: 'Where to use “bug”' });
  await expect(chooser).toBeVisible();
  const filtered = page.waitForRequest((request) => {
    const url = new URL(request.url());
    return url.pathname.endsWith(`/projects/${ACME.web}/topics`) && url.searchParams.getAll('tag_id').includes(id(5, 1));
  });
  await chooser.getByRole('button', { name: 'Web', exact: true }).click();
  await filtered;
  const url = new URL(page.url());
  expect(url.pathname).toBe(`/project/${ACME.web}`);
  expect(url.searchParams.get('view')).toBe('list');
  expect(url.searchParams.get('tag')).toBe(JSON.stringify([id(5, 1)]));
  await expect(page.getByRole('heading', { name: 'Web', level: 1 })).toBeVisible();
  await expect(page.getByText('Fix login', { exact: true }).first()).toBeVisible();
  await expect(chooser).toHaveCount(0);
});

test('the arrow keys choose among the results and Enter opens the one chosen', async ({ page }) => {
  await signIn(page);
  await openFinder(page);
  await typeIn(page, 'main thread');
  await expect(result(page, /Main thread.*thread/).first()).toBeVisible();
  const rows = finder(page).getByRole('option');
  await expect(rows.first()).toHaveAttribute('aria-selected', 'true');
  await page.keyboard.press('ArrowDown');
  await expect(rows.nth(1)).toHaveAttribute('aria-selected', 'true');
  await page.keyboard.press('ArrowUp');
  await expect(rows.first()).toHaveAttribute('aria-selected', 'true');
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(/\/thread\//);
});

test('a folder is listed like a project, with the way to it', async ({ page }) => {
  await signIn(page);
  // A folder of the world is a project that lies in a project: Checkout, in Shop.
  await openFinder(page);
  await typeIn(page, 'checkout');
  const folder = result(page, /^Checkout folder/);
  await expect(folder).toBeVisible();
  await expect(folder).toContainText('Acme › Shop');
  await folder.click();
  await expect(page).toHaveURL(new RegExp(`/project/${ACME.checkout}$`));
});

test('control and Enter open a result in a new tab of the browser, and leave this one where it was', async ({ page, context }) => {
  await signIn(page);
  const here = page.url();
  await openFinder(page);
  await typeIn(page, 'fix');
  await expect(result(page, /Fix login.*topic/)).toBeVisible();
  const [opened] = await Promise.all([context.waitForEvent('page'), page.keyboard.press('Control+Enter')]);
  await opened.waitForLoadState('domcontentloaded');
  expect(opened.url()).toContain(`/topic/${ACME.fixLogin}`);
  expect(page.url()).toBe(here);
  await opened.close();
});

test('a command is run from it: found by what it does, and done by Enter', async ({ page }) => {
  await signIn(page);
  await openFinder(page);
  await typeIn(page, '>dark');
  const dark = result(page, /Theme: Dark/);
  await expect(dark).toBeVisible();
  await expect(finder(page).getByText('Commands', { exact: true })).toBeVisible();
  await page.keyboard.press('Enter');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await expect(finder(page)).toHaveCount(0);
});

test('a screen is gone to by a command, and the commands of the place in view are its own acts', async ({ page }) => {
  await signIn(page);
  await page.goto(`/project/${ACME.web}`);
  await expect(page.getByRole('heading', { name: 'Web', level: 1 })).toBeVisible();
  await openFinder(page);
  await typeIn(page, '>new topic');
  // The act of the place in view, which its own menu holds.
  const make = result(page, /New topic/);
  await expect(make).toBeVisible();
  await expect(make).toContainText('On Web');
  await page.keyboard.press('Enter');
  await expect(page.getByRole('dialog', { name: /New topic/ })).toBeVisible();
  await page.keyboard.press('Escape');

  await openFinder(page);
  await typeIn(page, '>approvals');
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(/\/approvals$/);
});

test('a command the person may not use is listed, switched off, with the reason', async ({ page }) => {
  await signIn(page, PEOPLE.vera);
  await page.goto(`/project/${ACME.web}`);
  await expect(page.getByRole('heading', { name: 'Web', level: 1 })).toBeVisible();
  await openFinder(page);
  await typeIn(page, '>new topic');
  const make = result(page, /New topic/);
  await expect(make).toBeVisible();
  await expect(make).toHaveAttribute('aria-disabled', 'true');
  await expect(make).toContainText(/Interact Project/);
  // Enter does nothing with it: nothing opens.
  await page.keyboard.press('Enter');
  await expect(page.getByRole('dialog', { name: /New topic/ })).toHaveCount(0);
});

test('nothing found says so, and says what to try', async ({ page }) => {
  await signIn(page);
  await openFinder(page);
  await typeIn(page, 'zzzqqq');
  await expect(nothingFound(page, 'zzzqqq')).toBeVisible();
  await expect(finder(page).getByText(/Try fewer letters/)).toBeVisible();
  await expect(finder(page).getByRole('status')).toHaveText('Nothing found for “zzzqqq”');
});

test('name rows show their current state and actual activity time', async ({ page }) => {
  await signIn(page);
  await openFinder(page);
  await field(page).fill('main thread');
  const main = result(page, /Main thread.*thread/).first();
  await expect(main).toContainText('Running');
  await expect(main.locator('time')).toHaveAttribute('datetime', LATER_AT);
});

test('the last row of a full name group opens the paged inventory for the same words', async ({ page }) => {
  await fillNames(90);
  await signIn(page);
  await openFinder(page);
  await field(page).fill('nightly report');
  const more = result(page, /See all in Everything/);
  await expect(more).toBeVisible();
  // Wrap past the final conversation-search offer to the preceding full name group's last row.
  await field(page).press('ArrowUp');
  await field(page).press('ArrowUp');
  await expect(more).toHaveAttribute('aria-selected', 'true');
  await field(page).press('Enter');
  await expect(page).toHaveURL(/\/everything\?q=nightly(\+|%20)report/);
  const url = new URL(page.url());
  expect(url.pathname).toBe('/everything');
  expect(url.searchParams.get('q')).toBe('nightly report');
  await expect(page.getByRole('searchbox', { name: 'Search by name' })).toHaveValue('nightly report');
});

test('Everything retains the ordinary focus carried by a finder continuation', async ({ page }) => {
  await signIn(page);
  await page.goto('/everything?q=main&participating=true');
  const focus = page.locator('[data-filter-bar]').getByRole('switch');
  await expect(focus).toBeChecked();
  await expect(page.getByRole('searchbox', { name: 'Search by name' })).toHaveValue('main');
  await focus.uncheck();
  await expect(page).not.toHaveURL(/participating=true/);
});

test('empty names explain an active focus and a single letter explains the minimum', async ({ page }) => {
  await signIn(page);
  await openFinder(page);
  await finder(page).getByRole('switch', { name: 'Everything I can see', exact: true }).check();
  await field(page).fill('zzzqqq');
  await expect(nothingFound(page, 'zzzqqq')).toBeVisible();
  await expect(finder(page).getByText('The “Only where I take part” focus is on. Turn it off to include other things you can see.')).toBeVisible();
  await finder(page).getByRole('switch', { name: 'Only where I take part', exact: true }).uncheck();
  await expect(finder(page).getByText(/focus is on/)).toHaveCount(0);
  await field(page).fill('@x');
  await expect(finder(page).getByText('Type at least two characters to find names.')).toBeVisible();
});

test('what the person may not see is never listed, though somebody else is shown it', async ({ page }) => {
  // Team Anthropic is Grace's, and shared with Ada and Dora; Vera has no part in it.
  await signIn(page, PEOPLE.vera);
  await openFinder(page);
  await typeIn(page, 'team anthropic');
  await expect(nothingFound(page, 'team anthropic')).toBeVisible();
  await expect(result(page, /Team Anthropic/)).toHaveCount(0);
});

test('the same name is listed for a person who may see it', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await openFinder(page);
  await typeIn(page, 'team anthropic');
  await expect(result(page, /Team Anthropic.*provider/)).toBeVisible();
});

test('a place that was taken from a person is not listed when they look again', async ({ page }) => {
  await signIn(page, PEOPLE.vera);
  await openFinder(page);
  await typeIn(page, 'fix login');
  await expect(result(page, /Fix login.*topic/)).toBeVisible();
  await page.keyboard.press('Escape');

  await deny('vera', ['topic.view', 'thread.view']);
  await openFinder(page);
  await typeIn(page, 'fix logi');
  await expect(nothingFound(page, 'fix logi')).toBeVisible();
  await expect(result(page, /Fix login/)).toHaveCount(0);
});

test('a person is found by their address with @, and opens the page served for them', async ({ page }) => {
  await signIn(page);
  await openFinder(page);
  await typeIn(page, '@acme.dora');
  const dora = result(page, /Dora Developer/);
  await expect(dora).toBeVisible();
  await expect(dora).toContainText('@dora');
  await expect(dora).not.toContainText('acme.dora');
  // Only people are listed under this sign.
  await expect(finder(page).getByText('Go to', { exact: true })).toHaveCount(0);
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(/\/u\/dora$/);
  await expect(finder(page)).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Dora Developer', exact: true }).first()).toBeVisible();
  await expect(page.getByText('@dora', { exact: true })).toBeVisible();
});

test('a chat found from another org opens the chat in its own org by its served address', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme/messages');
  await page.getByRole('button', { name: 'New message' }).first().click();
  const make = page.getByRole('dialog', { name: 'New message' });
  await make.getByRole('button', { name: /Dora Developer/ }).click();
  await make.getByRole('button', { name: 'Message Dora Developer' }).click();
  await expect(page).toHaveURL(/chat=/);
  const chat = new URL(page.url()).searchParams.get('chat');
  await page.goto(`/org/${id(2, 1)}`);
  await openFinder(page);
  await field(page).fill('dora');
  const direct = result(page, /Ada Lovelace, Dora Developer.*chat/);
  await expect(direct).toBeVisible();
  await direct.click();
  await expect(page).toHaveURL(new RegExp(`/org/${ACME.org}/messages\\?chat=`));
  expect(new URL(page.url()).searchParams.get('chat')).toBe(chat);
  await expect(page.getByText('No messages yet. Say hello to Dora Developer.')).toBeVisible();
});

test('an org result keeps its identifier address instead of guessing from its current slug', async ({ page }) => {
  await signIn(page);
  await openFinder(page);
  await field(page).fill('acme');
  await result(page, /^Acme.*org/).click();
  await expect(page).toHaveURL(new RegExp(`/org/${ACME.org}$`));
  await expect(page.getByRole('heading', { name: 'Acme', exact: true, level: 1 })).toBeVisible();
});

test('a setting is found by what it does, and opened at the nearest place where it can be set', async ({ page }) => {
  await signIn(page);
  await page.goto(`/topic/${ACME.fixLogin}`);
  await expect(page.getByRole('heading', { name: 'Fix login', level: 1 })).toBeVisible();
  await openFinder(page);
  await typeIn(page, ':idle stop');
  const setting = result(page, /Idle stop/);
  await expect(setting).toBeVisible();
  await expect(setting).toContainText('Environment');
  await page.keyboard.press('Enter');
  // The topic is where it can be set at its nearest, and its address carries the key for the row to be lit.
  await expect(page).toHaveURL(new RegExp(`/topic/${ACME.fixLogin}\\?.*setting=env\\.idle_stop_minutes`));
  await expect(page).toHaveURL(/tab=settings/);
});

test('help is found with ?, and an article opens beside what the person is doing', async ({ page }) => {
  await signIn(page);
  await openFinder(page);
  await typeIn(page, '?finding');
  const article = result(page, /Finding things/).first();
  await expect(article).toBeVisible();
  await page.keyboard.press('Enter');
  await expect(finder(page)).toHaveCount(0);
  await expect(page.locator('[data-workbench-bar="right"]')).toHaveAttribute('data-active-container', 'help');
  await expect(page.getByRole('heading', { name: 'Finding things', level: 1 })).toBeVisible();
});

test('/ and words lead to the search of what was said', async ({ page }) => {
  await signIn(page);
  await openFinder(page);
  await typeIn(page, '/redirect loop');
  const search = result(page, /Advanced search for "redirect loop"/);
  await expect(search).toBeVisible();
  await expect(finder(page).getByRole('option').last()).toHaveText(/Advanced search for "redirect loop"/);
  const started = page.waitForResponse((response) => new URL(response.url()).pathname.endsWith('/search/stream'));
  await page.keyboard.press('End');
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(/\/search\?q=redirect(\+|%20)loop$/);
  await expect(page.getByRole('textbox', { name: 'Words to look for' })).toHaveValue('redirect loop');
  expect((await started).status()).toBe(200);
});

test('the last finder row opens advanced search without words', async ({ page }) => {
  await signIn(page);
  await openFinder(page);
  const last = finder(page).getByRole('option').last();
  await expect(last).toHaveText('Advanced search');
  await last.click();
  await expect(page).toHaveURL(/\/search$/);
  await expect(page.getByRole('textbox', {name:'Words to look for'})).toHaveValue('');
});

test('the buttons under the field write the signs', async ({ page }) => {
  await signIn(page);
  await openFinder(page);
  await typeIn(page, 'dora');
  await finder(page).getByRole('button', { name: /People/ }).click();
  await expect(field(page)).toHaveValue('@ dora');
  await finder(page).getByRole('button', { name: 'Everything' }).click();
  await expect(field(page)).toHaveValue('dora');
});

test('a lookup that fails is said, and the local groups still answer', async ({ page }) => {
  await signIn(page);
  await failNames(true);
  await openFinder(page);
  await typeIn(page, 'new');
  await expect(finder(page).getByRole('alert').getByText('The server could not complete this request. Try again.', { exact: true })).toBeVisible();
  await expect(result(page, /New org/)).toBeVisible();
  await failNames(false);
  await finder(page).getByRole('button', { name: 'Try again', exact: true }).click();
  await expect(finder(page).getByRole('alert')).toHaveCount(0);
  await expect(result(page, /New org/)).toBeVisible();
});

test('a missing name-search answer shows its own refusal while local commands remain usable', async ({ page }) => {
  await signIn(page);
  await page.route('**/api/v1/search/names**', (route) => route.fulfill({ status: 404, contentType: 'application/problem+json', body: JSON.stringify({
    type: 'orch:error:not_found', code: 'not_found', status: 404, title: 'Not found', detail: 'The lookup was refused.', request_id: 'names-404',
  }) }));
  await openFinder(page);
  await typeIn(page, 'new');
  await expect(finder(page).getByRole('alert')).toHaveText('This does not exist, was deleted, or is not open to you. Return to a place you can open or ask its manager.');
  await expect(finder(page).getByRole('button', { name: 'Try again', exact: true })).toHaveCount(0);
  await expect(result(page, /New org/)).toBeVisible();
  await result(page, /New org/).click();
  await expect(page.getByRole('dialog', { name: 'New org', exact: true })).toBeVisible();
});

test('an answer that is slow does not hold up the rows the page already has', async ({ page }) => {
  await signIn(page);
  await holdSearch(1_500);
  await openFinder(page);
  await typeIn(page, '>sign');
  // A command is a thing the page holds: it is there at once.
  await expect(result(page, /Sign out/)).toBeVisible({ timeout: 1_000 });
  await field(page).fill('fix');
  await expect(finder(page).getByRole('status').first()).toBeAttached();
  await expect(result(page, /Fix login.*topic/)).toBeVisible();
});

// The key is bound through the module that keeps them, reached by its source path, which only the development server serves.
test('the list of keys is drawn from what is bound: a key bound while a screen is shown is listed, and goes with it', { tag: '@development' }, async ({ page }) => {
  await signIn(page);
  await expect(page.getByRole('button', { name: 'Find anything' })).toBeVisible();
  await atRest(page);
  await page.locator('body').press('?');
  const list = page.getByRole('dialog', { name: 'Keyboard shortcuts' });
  await expect(list).toBeVisible();
  await expect(list.getByText('Find anything, or run a command')).toBeVisible();
  await expect(list.getByText('Search what was said')).toBeVisible();
  await page.keyboard.press('Escape');

  // A screen binds a key of its own while it is shown, saying what it does with a text of the catalogue.
  await page.evaluate(async (module) => {
    const keys = (await import(/* @vite-ignore */ module)) as { bindKey: (id: string, shortcut: string, run: () => unknown, what: string) => () => void };
    (window as unknown as { unbind: () => void }).unbind = keys.bindKey('probe', 'Mod+Shift+Y', () => true, 'shell.shortcuts.inbox');
  }, '/src/shell/keys.ts');
  await page.locator('body').press('?');
  await expect(list.getByText('Mod+Shift+Y').or(list.getByText('Y', { exact: true }))).toBeVisible();
  await page.keyboard.press('Escape');
  await page.evaluate(() => (window as unknown as { unbind: () => void }).unbind());
  await page.locator('body').press('?');
  await expect(list).toBeVisible();
  await expect(list.getByText('Y', { exact: true })).toHaveCount(0);
});

test('a hint inside the finder opens its card over it, and Escape closes the card before the finder', async ({ page }) => {
  await signIn(page);
  await openFinder(page);
  await finder(page).getByRole('button', { name: 'Help: The signs of the finder' }).click();
  const card = page.getByRole('dialog', { name: 'The signs of the finder' });
  await expect(card).toBeVisible();
  await expect(card.getByText(/One field finds names/)).toBeVisible();
  await expect(finder(page)).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(card).toHaveCount(0);
  await expect(finder(page)).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(finder(page)).toHaveCount(0);
});

test('on a small screen the finder is opened from the header and fits the screen', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 800 });
  await signIn(page);
  await page.getByRole('button', { name: 'Find anything' }).click();
  await expect(finder(page)).toBeVisible();
  await typeIn(page, 'fix');
  const topic = result(page, /Fix login.*topic/);
  await expect(topic).toBeVisible();
  const box = await finder(page).boundingBox();
  expect(box?.width ?? 0).toBeLessThanOrEqual(390);
  await topic.click();
  await expect(page).toHaveURL(new RegExp(`/topic/${ACME.fixLogin}$`));
});

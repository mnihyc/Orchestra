/** The left bar at work: sections that fold and stay folded on every device, a finished topic that keeps its place, a colour for an icon, documents under their place, what is unread from what was really read and what a row that hides it carries, a tree narrowed to one place on one device, branches folded away by their reader, the kinds of thing the tree shows, chats in it, a way that goes on past a private place, the org's own rows, and the rail. */
import { expect, type Locator, type Page, test } from '@playwright/test';

import { OUTSIDER } from '../doubles/access.ts';
import { id } from '../doubles/fixtures.ts';
import { agentMessage } from '../doubles/thread-rows.ts';
import { chats, secondReader } from './chat-support.ts';
import { ACME, changeTree, deny, emit, PEOPLE, scenario, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

/** A request only the fake API has. */
async function double(path: string, body: unknown = {}): Promise<Record<string, unknown>> {
  const response = await fetch(`${DOUBLE}/__double${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`the fake API refused ${path}: ${response.status}`);
  return response.status === 204 ? {} : ((await response.json()) as Record<string, unknown>);
}

/** A document of one paragraph beside a node, made as the system would hold it; gives its id. */
async function makeDocument(node: string, title: string): Promise<string> {
  return String((await double('/documents/make', { count: 1, title, node })).id);
}

test.beforeEach(async () => {
  await scenario();
});

const bar = (page: Page) => page.getByRole('navigation', { name: 'Navigation' });
const rail = (page: Page) => page.getByRole('navigation', { name: 'Orgs and your own places' });
const tree = (page: Page) => page.getByRole('tree', { name: 'Orgs, projects, topics and threads' });
const crumbs = (page: Page) => page.getByRole('navigation', { name: 'Where this is' });
/** The head of the bar where there is a rail: the name of the org in view, which leads to its home. */
const head = (page: Page) => bar(page).getByRole('link').first();
/** The heading of a tree that was narrowed to a place: the way to that place. */
const begins = (page: Page) => bar(page).getByRole('group', { name: 'Where the tree begins' });
/** The place the tree begins at, as its heading names it. */
const beginsAt = (page: Page) => begins(page).locator('[data-tree-begins]');
const section = (page: Page, name: 'Starred' | 'Recent' | 'Projects') => bar(page).getByRole('button', { name, exact: true });
/** A whole section of the bar, with its heading and its rows. */
const sectionOf = (page: Page, name: 'Starred' | 'Recent' | 'Projects') => bar(page).locator('section').filter({ has: page.getByRole('button', { name, exact: true }) });
/** A row of the tree that is a place or a thread, by the name it begins with (what follows is its kind, what it holds, and the names of its buttons). */
const row = (page: Page, name: string): Locator => tree(page).getByRole('treeitem', { name: new RegExp(`^(Waiting for approval |Blocked |Running |Starting |Idle )?${name}( |$)`) });
/** The row of a document, which its glyph names before its title. */
const doc = (page: Page, title: string): Locator => tree(page).getByRole('treeitem', { name: new RegExp(`^Document(, changed since you read it)? ${title}( |$)`) });
/** The row of a place's chat, where chats have rows of their own: the team's is called so, every other one "Chat". */
const chatRow = (page: Page, name: 'Chat' | 'Team chat' = 'Chat'): Locator => tree(page).getByRole('treeitem', { name: new RegExp(`^${name}( |$)`) });
/** The names the rows of the tree show, in the order they stand. */
const standing = async (page: Page): Promise<string[]> => tree(page).locator('[role="treeitem"] a > span.truncate').allTextContents();

/** Opens the menu beside "Projects": what the tree shows. */
async function viewMenu(page: Page): Promise<Locator> {
  await bar(page).getByRole('button', { name: 'View options' }).click();
  return page.getByRole('menu', { name: 'View options' });
}

/** Narrows the tree to a row from the row's own menu. */
async function showOnly(page: Page, name: string): Promise<void> {
  await (await rowMenu(page, name)).getByRole('menuitem', { name: 'Show only this' }).click();
}

/** Ticks or unticks a kind of thing in the tree's own menu. */
async function tick(page: Page, kind: 'Threads' | 'Documents' | 'Chats' | 'Hide done'): Promise<void> {
  await (await viewMenu(page)).getByRole('menuitemcheckbox', { name: kind }).click();
  await expect(page.getByRole('menu', { name: 'View options' })).toHaveCount(0);
}

/** Opens the "⋯" of a row of the tree: the menu takes the name of the button that opened it. */
async function rowMenu(page: Page, name: string, of: (page: Page, name: string) => Locator = row): Promise<Locator> {
  await of(page, name).hover();
  await of(page, name).getByRole('button', { name: new RegExp(`^Actions for ${name}(?: · .+)?$`), exact: true }).click();
  return page.getByRole('menu', { name: new RegExp(`^Actions for ${name}(?: · .+)?$`), exact: true });
}

test('each section folds, stays folded after a reload, and stands the same for the person in a second browser', async ({ page, browser, baseURL }) => {
  await signIn(page);
  await page.goto('/org/acme');
  await expect(row(page, 'Web')).toBeVisible();

  // As shipped: Recent is folded, Projects is open, and nothing is starred, so there is no such section.
  await expect(section(page, 'Recent')).toHaveAttribute('aria-expanded', 'false');
  await expect(section(page, 'Projects')).toHaveAttribute('aria-expanded', 'true');
  await expect(section(page, 'Starred')).toHaveCount(0);

  // Something is starred: the section is there, open, with its row.
  const menu = await rowMenu(page, 'Infra');
  await menu.getByRole('menuitem', { name: 'Star' }).click();
  await expect(section(page, 'Starred')).toHaveAttribute('aria-expanded', 'true');
  await expect(bar(page).getByRole('link', { name: /^Infra/ }).first()).toBeVisible();

  // Each is folded or unfolded by its heading.
  await section(page, 'Starred').click();
  await section(page, 'Projects').click();
  await section(page, 'Recent').click();
  await expect(section(page, 'Starred')).toHaveAttribute('aria-expanded', 'false');
  await expect(section(page, 'Projects')).toHaveAttribute('aria-expanded', 'false');
  await expect(section(page, 'Recent')).toHaveAttribute('aria-expanded', 'true');
  await expect(tree(page)).toHaveCount(0);

  // After a reload the bar stands as it was left.
  await page.reload();
  await expect(section(page, 'Projects')).toHaveAttribute('aria-expanded', 'false');
  await expect(section(page, 'Starred')).toHaveAttribute('aria-expanded', 'false');
  await expect(section(page, 'Recent')).toHaveAttribute('aria-expanded', 'true');
  await expect(tree(page)).toHaveCount(0);

  // The same person in another browser finds it so, and what they change there changes here without a reload.
  const second = await secondReader(browser, baseURL, '/org/acme', PEOPLE.ada);
  await expect(section(second.page, 'Projects')).toHaveAttribute('aria-expanded', 'false');
  await expect(section(second.page, 'Recent')).toHaveAttribute('aria-expanded', 'true');
  await expect(section(second.page, 'Starred')).toHaveAttribute('aria-expanded', 'false');
  await section(second.page, 'Projects').click();
  await expect(row(second.page, 'Web')).toBeVisible();
  await expect(section(page, 'Projects')).toHaveAttribute('aria-expanded', 'true');
  await expect(row(page, 'Web')).toBeVisible();
  await second.context.close();
});

test('a folded section says what waits in it, and Recent lists the last eight things opened, or says that there are none', async ({ page }) => {
  // Nine things were opened lately; the newest first.
  const opened = [ACME.fixLogin, ACME.web, ACME.mainThread, ACME.infra, ACME.shop, ACME.checkout, ACME.payments, ACME.cartRewrite, ACME.upgradeDeps, ACME.idleStop];
  await signIn(page);
  await page.goto('/org/acme');
  await expect(row(page, 'Web')).toBeVisible();

  // Nothing was opened but the org itself: the section is there all the same, and says so when it is unfolded.
  await section(page, 'Recent').click();
  await expect(bar(page).getByText('Nothing opened here yet')).toBeVisible();

  await double('/search/recents', { username: 'ada', ids: opened });
  await page.reload();
  const recent = sectionOf(page, 'Recent');
  await expect(recent.getByRole('link')).toHaveCount(8);
  await expect(recent.getByRole('link').first()).toContainText('Fix login');
  await expect(recent.getByRole('link').nth(1)).toContainText('Web');

  // Folded, Projects says how many approvals wait in the whole tree (Repro waits for one).
  await section(page, 'Projects').click();
  const projects = sectionOf(page, 'Projects');
  // The rows are seen to leave before they are gone: what is looked at is the heading alone.
  await expect(tree(page)).toHaveCount(0);
  await expect(projects.getByText('1 approval waits inside')).toBeAttached();
  // Folded, Recent says it of the things it lists: the topic and the project that hold the thread are among them.
  await section(page, 'Recent').click();
  await expect(recent.getByRole('link')).toHaveCount(0);
  await expect(recent.getByText('1 approval waits inside')).toBeAttached();
});

test('a topic that is marked done keeps its place and is marked as done, and Hide done takes the done ones out for this person', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme');
  await expect(row(page, 'Upgrade deps')).toBeVisible();
  const web = async () => (await standing(page)).filter((name) => ['Fix login', 'Migrate CI', 'Upgrade deps'].includes(name));
  expect(await web()).toEqual(['Fix login', 'Migrate CI', 'Upgrade deps']);
  // The one that is done already says so, and no row groups the done ones.
  await expect(row(page, 'Migrate CI')).toHaveAccessibleName(/^Migrate CI topic, done/);
  await expect(tree(page).getByText(/^Done · /)).toHaveCount(0);

  // Fix login is marked done from its row: it stands where it stood, with the mark.
  const menu = await rowMenu(page, 'Fix login');
  await menu.getByRole('menuitem', { name: 'Mark done' }).click();
  await expect(row(page, 'Fix login')).toHaveAccessibleName(/^Fix login topic, done/);
  expect(await web()).toEqual(['Fix login', 'Migrate CI', 'Upgrade deps']);

  // Opened again, it is as it was, in the same place.
  await (await rowMenu(page, 'Fix login')).getByRole('menuitem', { name: 'Reopen' }).click();
  await expect(row(page, 'Fix login')).toHaveAccessibleName(/^Fix login topic /);
  await expect(row(page, 'Fix login')).not.toHaveAccessibleName(/done/);
  expect(await web()).toEqual(['Fix login', 'Migrate CI', 'Upgrade deps']);

  // Hide done is off as shipped; switched on, the done topic is out of the tree, and stays out after a reload.
  let view = await viewMenu(page);
  await expect(view.getByRole('menuitemcheckbox', { name: 'Hide done' })).toHaveAttribute('aria-checked', 'false');
  await view.getByRole('menuitemcheckbox', { name: 'Hide done' }).click();
  await expect(row(page, 'Migrate CI')).toHaveCount(0);
  expect(await web()).toEqual(['Fix login', 'Upgrade deps']);
  await page.reload();
  await expect(row(page, 'Upgrade deps')).toBeVisible();
  await expect(row(page, 'Migrate CI')).toHaveCount(0);

  // Its own screen still brings it into view: a thing is found where it lies.
  await page.goto(`/topic/${ACME.migrateCi}`);
  await expect(row(page, 'Migrate CI')).toBeVisible();
  await page.goto('/org/acme');
  await expect(row(page, 'Migrate CI')).toHaveCount(0);

  // Switched off again, it is back between the other two.
  view = await viewMenu(page);
  await expect(view.getByRole('menuitemcheckbox', { name: 'Hide done' })).toHaveAttribute('aria-checked', 'true');
  await view.getByRole('menuitemcheckbox', { name: 'Hide done' }).click();
  await expect(row(page, 'Migrate CI')).toBeVisible();
  expect(await web()).toEqual(['Fix login', 'Migrate CI', 'Upgrade deps']);
});

test('whoever manages a place gives its icon a colour, takes it back and picks one at random, and everybody sees it; somebody who does not manage it is not offered the choice', async ({ page, browser, baseURL }) => {
  const viewer = await secondReader(browser, baseURL, `/project/${ACME.web}`, PEOPLE.vera);
  // A reader of the project is not offered Colour in its row, and finds it switched off, with what it takes, on its screen.
  const quiet = await rowMenu(viewer.page, 'Web');
  await expect(quiet.getByRole('menuitem', { name: 'Open', exact: true })).toBeVisible();
  await expect(quiet.getByRole('menuitem', { name: 'Colour…' })).toHaveCount(0);
  await viewer.page.keyboard.press('Escape');
  await expect(quiet).toBeHidden();
  await viewer.page.getByTestId('main').getByRole('button', { name: /^Actions for Web(?: · .+)?$/, exact: true }).click();
  const off = viewer.page.getByRole('menuitem', { name: /^Colour…/ });
  await expect(off).toHaveAttribute('aria-disabled', 'true');
  await expect(off).toContainText('Manage');
  await viewer.page.keyboard.press('Escape');

  await signIn(page);
  await page.goto(`/project/${ACME.web}`);
  // By its place in the page and not by its role: while a dialog is open the page behind it is hidden from a screen reader.
  const icon = (on: Page) => on.locator(`[data-tree-row="${ACME.web}"] [data-node-icon]`);
  // As shipped a project has the colour of its kind, and a topic another.
  await expect(icon(page)).toHaveAttribute('data-node-color', '8');
  await expect(row(page, 'Fix login').locator('[data-node-icon]')).toHaveAttribute('data-node-color', '6');

  await (await rowMenu(page, 'Web')).getByRole('menuitem', { name: 'Colour…' }).click();
  const dialog = page.getByRole('dialog', { name: /^Colour of the project Web, in / });
  await expect(dialog.getByRole('group', { name: 'Colours' }).getByRole('button')).toHaveCount(12);
  await expect(dialog.getByRole('button', { name: 'As its kind' })).toHaveAttribute('aria-pressed', 'true');

  // One of the twelve, by its name: the row, the way above the screen and the other person's page all take it.
  await dialog.getByRole('button', { name: 'Green' }).click();
  await expect(dialog.getByRole('button', { name: 'Green' })).toHaveAttribute('aria-pressed', 'true');
  await expect(icon(page)).toHaveAttribute('data-node-color', '5');
  await expect(page.locator('nav[aria-label="Where this is"] [data-node-icon="project"]')).toHaveAttribute('data-node-color', '5');
  await expect(icon(viewer.page)).toHaveAttribute('data-node-color', '5');

  // At random: another than the one in use.
  await dialog.getByRole('button', { name: 'Random' }).click();
  await expect(icon(page)).not.toHaveAttribute('data-node-color', '5');
  const random = await icon(page).getAttribute('data-node-color');
  expect(Number(random)).toBeGreaterThanOrEqual(1);
  expect(Number(random)).toBeLessThanOrEqual(12);
  await expect(icon(viewer.page)).toHaveAttribute('data-node-color', random ?? '');

  // Taken back: the colour of its kind again, kept after a reload.
  await dialog.getByRole('button', { name: 'As its kind' }).click();
  await expect(dialog.getByRole('button', { name: 'As its kind' })).toHaveAttribute('aria-pressed', 'true');
  await expect(icon(page)).toHaveAttribute('data-node-color', '8');
  await dialog.getByRole('button', { name: 'Close' }).first().click();
  await page.reload();
  await expect(icon(page)).toHaveAttribute('data-node-color', '8');

  await viewer.context.close();
});

test('documents stand under the org, the project, the folder, the topic and the thread they are written beside, are read when the place is opened, opened and renamed from the tree, and gone with the switch off', async ({ page }) => {
  await makeDocument(ACME.research, 'Research handbook');
  await makeDocument(ACME.org, 'Company handbook');
  const asked: string[] = [];
  page.on('request', (request) => {
    const found = /\/nodes\/([^/]+)\/documents/.exec(request.url());
    if (found?.[1] !== undefined) asked.push(found[1]);
  });
  await signIn(page);
  await page.goto('/org/acme');

  // An org inside the org, a project and a folder are open the first time: their documents stand after what else they hold.
  await expect(doc(page, 'Research handbook')).toBeVisible();
  await expect(doc(page, 'Release checklist')).toBeVisible();
  await expect(doc(page, 'Checkout design')).toBeVisible();
  const names = await standing(page);
  expect(names.indexOf('Research handbook')).toBeGreaterThan(names.indexOf('Upgrade deps'));
  expect(names.indexOf('Release checklist')).toBeGreaterThan(names.indexOf('Upgrade deps'));
  expect(names.indexOf('Release checklist')).toBeLessThan(names.indexOf('Web handbook'));
  expect(names.indexOf('Checkout design')).toBeGreaterThan(names.indexOf('Payments'));
  // The root org is no row: its own stand at the foot of the tree.
  expect(names.at(-1)).toBe('Company handbook');
  await expect(doc(page, 'Checkout design')).toHaveAttribute('aria-level', '3');
  await expect(doc(page, 'Company handbook')).toHaveAttribute('aria-level', '1');

  // A topic that is closed has not been asked for its documents; opened, it is, and they stand under it after its threads.
  expect(asked).not.toContain(ACME.fixLogin);
  await expect(doc(page, 'Rollout plan')).toHaveCount(0);
  await row(page, 'Fix login').press('ArrowRight');
  await expect(doc(page, 'Rollout plan')).toBeVisible();
  expect(asked).toContain(ACME.fixLogin);
  const inTopic = await standing(page);
  expect(inTopic.indexOf('Rollout plan')).toBeGreaterThan(inTopic.indexOf('Repro'));
  // One that was made private is listed for whoever may read it, with a lock.
  await expect(doc(page, 'Only for me').getByRole('img', { name: 'Private', exact: true })).toBeVisible();

  // A thread that holds documents shows that it can be opened, and holds them when it is.
  await row(page, 'Card fees').press('ArrowRight');
  await expect(row(page, 'Fee table')).toHaveAttribute('aria-expanded', 'false');
  await row(page, 'Fee table').press('ArrowRight');
  await expect(doc(page, 'Fee table findings')).toBeVisible();
  await expect(doc(page, 'Fee table findings')).toHaveAttribute('aria-level', '6');

  // A row opens the document, and is the row of the screen in view while it is open.
  await doc(page, 'Rollout plan').getByRole('link').click();
  await expect(page).toHaveURL(new RegExp(`/doc/${id(7, 1)}$`));
  await expect(doc(page, 'Rollout plan')).toHaveAttribute('aria-current', 'page');
  await expect(crumbs(page)).toContainText('Fix login');
  await expect(page.getByRole('tablist', { name: 'Main area', exact: true }).getByRole('tab', { name: 'Rollout plan', exact: true })).toHaveAttribute('aria-selected', 'true');

  // Renamed from its menu in the tree: the row says the new title, and stands where the new title puts it.
  await (await rowMenu(page, 'Notes from the canary', doc)).getByRole('menuitem', { name: 'Rename…' }).click();
  const rename = page.getByRole('dialog', { name: 'Rename Notes from the canary' });
  await rename.getByRole('textbox', { name: 'Title' }).fill('Canary notes');
  await rename.getByRole('button', { name: 'Save' }).click();
  await expect(doc(page, 'Canary notes')).toBeVisible();
  await expect(doc(page, 'Notes from the canary')).toHaveCount(0);

  // Switched off, the tree is places and threads alone, and a thread that held only documents no longer opens.
  await (await viewMenu(page)).getByRole('menuitemcheckbox', { name: 'Documents' }).click();
  await expect(doc(page, 'Canary notes')).toHaveCount(0);
  await expect(doc(page, 'Release checklist')).toHaveCount(0);
  await expect(doc(page, 'Company handbook')).toHaveCount(0);
  await expect(row(page, 'Fee table')).not.toHaveAttribute('aria-expanded');
  await page.reload();
  await expect(row(page, 'Fix login')).toBeVisible();
  await expect(doc(page, 'Release checklist')).toHaveCount(0);
  // And on again, they are back.
  const view = await viewMenu(page);
  await expect(view.getByRole('menuitemcheckbox', { name: 'Documents' })).toHaveAttribute('aria-checked', 'false');
  await view.getByRole('menuitemcheckbox', { name: 'Documents' }).click();
  await expect(doc(page, 'Release checklist')).toBeVisible();
});

test('a document made beside a place arrives in the tree, and one deleted from its menu leaves it, with the way back', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme');
  await expect(doc(page, 'Idle-stop notes')).toBeVisible();
  // Upgrade deps holds none: it has threads and opens for them, and no document stands in it.
  await row(page, 'Upgrade deps').press('ArrowRight');
  await expect(row(page, 'Upgrade deps')).toHaveAttribute('aria-expanded', 'true');

  // Somebody writes one beside it: its row is told, and the document stands under it.
  await makeDocument(ACME.upgradeDeps, 'Upgrade steps');
  await changeTree([{ id: ACME.upgradeDeps }]);
  await expect(doc(page, 'Upgrade steps')).toBeVisible();

  await (await rowMenu(page, 'Idle-stop notes', doc)).getByRole('menuitem', { name: 'Delete…' }).click();
  const confirm = page.getByRole('alertdialog', { name: 'Delete “Idle-stop notes”?' });
  await confirm.getByRole('button', { name: 'Delete Idle-stop notes' }).click();
  await expect(doc(page, 'Idle-stop notes')).toHaveCount(0);
  // The reader stays where they were.
  await expect(page).toHaveURL(/\/org\/acme$/);
  await page.getByRole('region', { name: /^Notifications/ }).getByRole('button', { name: 'Undo' }).click();
  await expect(doc(page, 'Idle-stop notes')).toBeVisible();
});

test('the tree is walked by keyboard alone with documents in it: into a place, onto a document, into its menu and to its page', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme');
  await expect(row(page, 'Web')).toBeVisible();

  // Tab reaches the tree at one row; the arrows walk from there.
  await row(page, 'Web').focus();
  await expect(row(page, 'Web')).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(row(page, 'Fix login')).toBeFocused();
  // Right opens the topic; right again steps onto the first thing in it.
  await page.keyboard.press('ArrowRight');
  await expect(row(page, 'Fix login')).toHaveAttribute('aria-expanded', 'true');
  await expect(doc(page, 'Rollout plan')).toBeVisible();
  await page.keyboard.press('ArrowRight');
  await expect(row(page, 'Main thread').first()).toBeFocused();
  // A letter goes to the next row that begins with it: the document.
  await page.keyboard.press('l');
  await expect(doc(page, 'Login findings')).toBeFocused();
  // Left steps out to the place it is written beside; the arrows walk back in, past its threads, onto its documents.
  await page.keyboard.press('ArrowLeft');
  await expect(row(page, 'Fix login')).toBeFocused();
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowDown');
  await expect(row(page, 'Repro')).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(doc(page, 'Login findings')).toBeFocused();

  // Its menu opens under the keyboard and is a document's.
  await page.keyboard.press('Shift+F10');
  const menu = page.getByRole('menu', { name: 'Actions for Login findings', exact: true });
  await expect(menu.getByRole('menuitem', { name: 'Rename…' })).toBeVisible();
  await expect(menu.getByRole('menuitem', { name: 'Move to…' })).toBeVisible();
  await expect(menu.getByRole('menuitem', { name: 'Delete…' })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(doc(page, 'Login findings')).toBeFocused();

  // Enter goes where the row leads.
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(new RegExp(`/doc/${id(0x30, 4)}$`));
  await expect(doc(page, 'Login findings')).toHaveAttribute('aria-current', 'page');
});

test('a thread is unread from what its reader has really read, on its own row and as a dot on the nearest closed row above it, on every device of theirs, and a thread they may not see says nothing', async ({ page, browser, baseURL }) => {
  // Two browsers, and a thread that is read by being looked at for a while: the case is given the time of three.
  test.slow();
  // Dora may not see Repro, a thread of the topic she follows.
  await deny('dora', [`thread.view@${ACME.repro}`]);
  await emit(ACME.mainThread, { items: [agentMessage('m-1', 'A first answer.')], thread: {} });
  await signIn(page, PEOPLE.dora);
  await page.goto('/org/acme');
  await row(page, 'Fix login').press('ArrowRight');
  const main = row(page, 'Main thread').first();
  await expect(main).toBeVisible();
  await expect(row(page, 'Repro')).toHaveCount(0);
  // She has read nothing and follows nothing: nothing is marked.
  await expect(main.getByRole('img', { name: 'Unread' })).toHaveCount(0);

  // She follows the topic (set on another device): its threads that have something to read are unread at once.
  await double('/places/follow', { username: 'dora', node: ACME.fixLogin, level: 'all' });
  await expect(main.getByRole('img', { name: 'Unread', exact: true })).toBeVisible();
  await expect(main.locator('a > span.truncate')).toHaveClass(/font-semibold/);

  // Closed, the topic says it with a dot and its name in bold; closed above that, the project does. It is said, and never counted.
  const inside = (on: Page, name: string) => row(on, name).getByRole('img', { name: 'Unread inside' });
  await expect(inside(page, 'Fix login')).toHaveCount(0);
  await row(page, 'Fix login').press('ArrowLeft');
  await expect(inside(page, 'Fix login')).toBeVisible();
  await expect(row(page, 'Fix login').locator('a > span.truncate')).toHaveClass(/font-semibold/);
  await expect(row(page, 'Fix login').getByText(/\d+ unread/)).toHaveCount(0);
  await row(page, 'Web').press('ArrowLeft');
  await expect(inside(page, 'Web')).toBeVisible();
  await expect(row(page, 'Fix login')).toHaveCount(0);

  // She reads the thread in a second browser: two seconds on screen, and the first browser shows it read on every row.
  const second = await secondReader(browser, baseURL, `/thread/${ACME.mainThread}`, PEOPLE.dora);
  await expect(second.page.getByText('A first answer.')).toBeVisible();
  await expect(inside(page, 'Web')).toHaveCount(0, { timeout: 30_000 });
  await row(page, 'Web').press('ArrowRight');
  // Repro, which she follows with the topic and may not see, has as much to read as it had: it is no row of her tree, and says nothing on the rows above it.
  await expect(inside(page, 'Fix login')).toHaveCount(0);
  await expect(row(page, 'Repro')).toHaveCount(0);
  // The second browser leaves the thread, so that what comes next is not read by being looked at.
  await second.page.goto('/org/acme');
  await expect(row(second.page, 'Web')).toBeVisible();

  // Something happens in the thread: it is unread again on both browsers, on the topic that hides it and, opened, on its own row.
  await emit(ACME.mainThread, { items: [agentMessage('m-2', 'A second answer.')], thread: {} });
  await expect(inside(page, 'Fix login')).toBeVisible();
  // In the second browser the way to the thread she read was opened, and is open still: there the thread's own row says it.
  await expect(row(second.page, 'Main thread').first().getByRole('img', { name: 'Unread', exact: true })).toBeVisible();
  await row(page, 'Fix login').press('ArrowRight');
  await expect(row(page, 'Main thread').first().getByRole('img', { name: 'Unread', exact: true })).toBeVisible();
  await expect(inside(page, 'Fix login')).toHaveCount(0);

  // Alt+Shift+Down goes to the next unread thread.
  await page.keyboard.press('Alt+Shift+ArrowDown');
  await expect(page).toHaveURL(new RegExp(`/thread/${ACME.mainThread}$`));
  await second.context.close();
});

test('a closed row says the most urgent thing that goes on in it, and the keys go from thread to thread', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme');
  await expect(row(page, 'Infra')).toBeVisible();
  // Infra holds a thread that is blocked; Evals one that is starting. Open, a project says nothing of it itself: its topic does.
  await expect(row(page, 'Idle-stop rework').getByRole('img', { name: 'Blocked inside' })).toBeVisible();
  await expect(row(page, 'Infra').getByRole('img', { name: 'Blocked inside' })).toHaveCount(0);
  await row(page, 'Infra').press('ArrowLeft');
  await expect(row(page, 'Infra').getByRole('img', { name: 'Blocked inside' })).toBeVisible();
  // Web hides a thread that runs and one that waits for an approval: the number of decisions that wait says the more urgent of the two, once.
  await row(page, 'Web').press('ArrowLeft');
  await expect(row(page, 'Web').getByText('1 approval waits inside')).toBeAttached();
  await expect(row(page, 'Web').getByRole('img', { name: /inside$/ })).toHaveCount(0);

  // Alt+Down goes to the first thread of the tree from a screen that is none, and on to the next from there.
  await page.keyboard.press('Alt+ArrowDown');
  await expect(page).toHaveURL(/\/thread\//);
  const first = page.url();
  await page.keyboard.press('Alt+ArrowDown');
  await expect(page).not.toHaveURL(first);
  await expect(page).toHaveURL(/\/thread\//);
  await page.keyboard.press('Alt+ArrowUp');
  await expect(page).toHaveURL(first);
});

test('somebody let into a thread below places they may not see reads the way to it past them: a private place is named by its kind, is no link, and the way goes on', async ({ page }) => {
  await double('/places/grant', { username: OUTSIDER.username, node: ACME.mainThread, permissions: ['thread.view'] });
  await double('/places/grant', { username: OUTSIDER.username, node: ACME.fixLogin, permissions: ['topic.view'] });
  await signIn(page, { login: OUTSIDER.username, password: OUTSIDER.password });
  await page.goto(`/thread/${ACME.mainThread}`);

  const way = crumbs(page);
  await expect(page.getByRole('tablist', { name: 'Main area', exact: true }).getByRole('tab', { name: 'Main thread', exact: true })).toBeVisible();
  await expect(way.locator('a[aria-current="page"]')).toHaveCount(0);
  // The topic they may see is a link; the first place of the way is private, and no link.
  await expect(way.getByRole('link', { name: 'Fix login' })).toBeVisible();
  await expect(way.getByText('Private org').first()).toBeVisible();
  await expect(way.getByRole('link', { name: /Private/ })).toHaveCount(0);
  // A phone shows the nearest permitted parent, without inventing links to private ancestors.
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(way.getByRole('button', { name: 'Ancestor places', exact: true })).toHaveCount(0);
  await expect(way.locator('a:visible')).toHaveCount(1);
  await expect(way.getByRole('link', { name: 'Fix login', exact: true })).toHaveAttribute('href', `/topic/${ACME.fixLogin}`);
  await expect(way.getByRole('link', { name: /Private/ })).toHaveCount(0);
  // No name of a place they may not see is anywhere on the page.
  for (const hidden of ['Acme', 'Research', 'Web']) await expect(page.getByText(hidden, { exact: true })).toHaveCount(0);

  // The place they may see leads to its own screen.
  await page.setViewportSize({ width: 1280, height: 900 });
  await expect(way.getByRole('link', { name: 'Fix login' })).toHaveAttribute('href', `/topic/${ACME.fixLogin}`);
});

test('Messages in the left bar and the org in the rail say that a chat is unread, from one count: it rises with a message, falls when the chat is read in another browser, and never counts a chat that was hidden', async ({ page, browser, baseURL }) => {
  await signIn(page);
  await page.goto('/org/acme');
  const messages = bar(page).getByRole('link', { name: /^Messages/ });
  await expect(messages).toHaveAccessibleName('Messages');
  await expect(rail(page).getByRole('link', { name: 'Acme', exact: true })).toBeVisible();

  // Dora writes to Ada: both places say so without a reload.
  const made = await chats('/chat', { org: ACME.org, members: ['dora', 'ada'] });
  await chats('/say', { chat: made.chat, as: 'dora', text: 'Have you got a minute?' });
  await expect(messages).toHaveAccessibleName('Messages 1 unread chat');
  await expect(rail(page).getByRole('link', { name: 'Acme, unread chats' })).toBeVisible();
  await expect(rail(page).locator('[data-unread-chats]')).toHaveCount(1);

  // A second chat, a group: two are unread.
  const group = await chats('/chat', { org: ACME.org, members: ['grace', 'ada', 'dora'], name: 'Release' });
  await chats('/say', { chat: group.chat, as: 'grace', text: 'Shipping on Friday.' });
  await expect(messages).toHaveAccessibleName('Messages 2 unread chats');

  // Ada reads the first in another browser: the count falls here, and the Messages screen there says the same of each chat.
  const second = await secondReader(browser, baseURL, `/org/acme/messages?chat=${String(made.chat)}`, PEOPLE.ada);
  await expect(second.page.getByText('Have you got a minute?').first()).toBeVisible();
  await expect(messages).toHaveAccessibleName('Messages 1 unread chat', { timeout: 30_000 });
  await expect(bar(second.page).getByRole('link', { name: /^Messages/ })).toHaveAccessibleName('Messages 1 unread chat');
  const list = second.page.getByRole('list', { name: 'Chats' });
  await expect(list.getByRole('listitem').filter({ hasText: 'Release' }).getByText('Unread messages')).toBeVisible();
  await expect(list.getByRole('listitem').filter({ hasText: 'Dora Developer' }).getByText('Unread messages')).toHaveCount(0);

  // She hides the group there: a chat that is hidden is not counted, here or there.
  const hidden = list.getByRole('listitem').filter({ hasText: 'Release' });
  await hidden.hover();
  await hidden.getByRole('button', { name: /^Actions for / }).click();
  await second.page.getByRole('menuitem', { name: 'Hide' }).click();
  await expect(messages).toHaveAccessibleName('Messages');
  await expect(bar(second.page).getByRole('link', { name: /^Messages/ })).toHaveAccessibleName('Messages');
  await expect(rail(page).locator('[data-unread-chats]')).toHaveCount(0);
  await expect(rail(page).getByRole('link', { name: 'Acme', exact: true })).toBeVisible();
  await second.context.close();
});

test('the rail has every org one press away, their own first and each told from the others, hides the unbuilt shared screen, and names the keys of Inbox and Approvals in their hints', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme');
  const orgs = rail(page).getByRole('list').locator('a[href^="/org/"]');
  // A member of one team: their own org and the team's. Both names begin with an A, so each shows a second letter.
  await expect(orgs).toHaveText(['Ad', 'Ac']);
  await expect(rail(page).getByRole('link', { name: 'Ada Lovelace (your own)' })).toBeVisible();
  // The org in view is marked: as the page in view on its own home, as the current one anywhere else in it.
  await expect(rail(page).getByRole('link', { name: 'Acme', exact: true })).toHaveAttribute('aria-current', /^(page|true)$/);
  // A screen without a body has no navigation offer; the orgs remain together.
  await expect(rail(page).getByRole('link', { name: 'Shared with me' })).toHaveCount(0);
  await expect(rail(page).getByRole('list').getByRole('link').last()).toHaveAccessibleName('Acme');

  // A second team is made: it is in the rail at once, after the first, and one press brings each into the bar.
  await rail(page).getByRole('button', { name: 'New org or join one' }).click();
  await page.getByRole('menuitem', { name: 'New org…' }).click();
  const dialog = page.getByRole('dialog', { name: 'New org' });
  await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('Zeta Labs');
  await dialog.getByRole('button', { name: 'Create org' }).click();
  await expect(page).toHaveURL(/\/org\/zeta-labs$/);
  // A name that begins like no other shows its first letter alone.
  await expect(orgs).toHaveText(['Ad', 'Ac', 'Z']);
  await expect(rail(page).getByRole('link', { name: 'Zeta Labs' })).toHaveAttribute('aria-current', /^(page|true)$/);
  await expect(head(page)).toHaveText('Zeta Labs');

  await rail(page).getByRole('link', { name: 'Acme', exact: true }).click();
  await expect(head(page)).toHaveText('Acme');
  await expect(row(page, 'Web')).toBeVisible();

  // The org's own colour is the colour of its mark: given one, the rail shows it.
  // Found by its place in the page: behind an open dialog the page is hidden from a screen reader.
  const mark = page.locator('nav[aria-label="Orgs and your own places"] a[href="/org/acme"] .avatar');
  await bar(page).getByRole('button', { name: /^Actions for Acme(?: · .+)?$/ }).click();
  await page.getByRole('menuitem', { name: 'Colour…' }).click();
  await page.getByRole('dialog', { name: 'Colour of Acme' }).getByRole('button', { name: 'Green' }).click();
  await expect(mark).toHaveClass(/bg-\(--identity-4\)/);
  await page.getByRole('dialog', { name: 'Colour of Acme' }).getByRole('button', { name: 'Close' }).first().click();

  // Inbox and Approvals stand here, each hint with the key that goes there.
  await rail(page).getByRole('link', { name: /^Inbox/ }).hover();
  await expect(page.getByRole('tooltip')).toHaveText(/^Inbox\s*G\s*(then\s*)?I$/);
  await page.keyboard.press('Escape');
  await expect(page.getByRole('tooltip')).toHaveCount(0);
  await rail(page).getByRole('link', { name: /^Approvals/ }).hover();
  await expect(page.getByRole('tooltip')).toHaveText(/^Approvals\s*G\s*(then\s*)?A$/);
  await page.keyboard.press('Escape');
  await page.keyboard.press('g');
  await page.keyboard.press('i');
  await expect(page).toHaveURL(/\/inbox$/);

  await page.goto('/shared');
  await expect(page).toHaveURL(/\/shared$/);
  await rail(page).getByRole('link', { name: 'Ada Lovelace (your own)' }).click();
  await expect(head(page)).toHaveText('Ada Lovelace');
});

test('the bar begins with the org’s own rows, its head is the org’s name with the org’s menu, and Inbox, Approvals and what was shared are not in it; a small screen keeps the way to another org at its head', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme');
  await expect(row(page, 'Web')).toBeVisible();

  // The head names the org and leads to its home; beside it the org's own menu. It is no second way to another org.
  await expect(head(page)).toHaveText('Acme');
  await expect(head(page)).toHaveAttribute('href', '/org/acme');
  await bar(page).getByRole('button', { name: /^Actions for Acme(?: · .+)?$/ }).click();
  await expect(page.getByRole('menu', { name: /^Actions for Acme(?: · .+)?$/ }).getByRole('menuitem', { name: 'New project…' })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(bar(page).getByRole('button', { name: 'Acme', exact: true })).toHaveCount(0);

  // Its first rows are the org's: the team's chat and Messages. What is a person's own stands in the rail.
  await expect(bar(page).getByRole('link').nth(1)).toHaveText('Team chat');
  await expect(bar(page).getByRole('link').nth(2)).toHaveText('Messages');
  for (const gone of ['Inbox', 'Approvals', 'Shared with me']) await expect(bar(page).getByRole('link', { name: gone })).toHaveCount(0);
  for (const kind of ['Providers', 'Agents', 'Machines']) await expect(bar(page).getByRole('link', { name: kind, exact: true })).toHaveCount(0);
  await expect(rail(page).getByRole('link', { name: /^Inbox/ })).toBeVisible();
  await expect(rail(page).getByRole('link', { name: /^Approvals/ })).toBeVisible();

  // The team's chat is the chat of the org: its row leads there.
  await bar(page).getByRole('link', { name: 'Team chat' }).click();
  await expect(page).toHaveURL(/\/org\/acme\?panel=chat$/);

  // A person's own org has no team, and no such row.
  await rail(page).getByRole('link', { name: 'Ada Lovelace (your own)' }).click();
  await expect(head(page)).toHaveText('Ada Lovelace');
  await expect(bar(page).getByRole('link', { name: 'Messages' })).toBeVisible();
  await expect(bar(page).getByRole('link', { name: 'Team chat' })).toHaveCount(0);

  // A small screen has no rail: the head of the bar is the way to another org there, with what was shared after the last of them.
  await page.setViewportSize({ width: 390, height: 800 });
  await page.getByRole('button', { name: 'Navigation', exact: true }).click();
  const drawer = page.getByRole('dialog', { name: 'Navigation' });
  await drawer.getByRole('button', { name: 'Ada Lovelace', exact: true }).click();
  // The list takes the name of the button that opened it.
  const orgs = page.getByRole('menu', { name: 'Ada Lovelace', exact: true });
  // After the orgs it has what the rail has after its orgs: what was shared, and the resources.
  await expect(orgs.getByRole('menuitem')).toHaveText(['Ada Lovelace (your own)', 'Acme', 'Resources']);
  await orgs.getByRole('menuitem', { name: 'Acme' }).click();
  await expect(page).toHaveURL(/\/org\/acme$/);
  // The drawer is still open after another org was chosen in it: its tree is what was asked for. Its foot has no links.
  await expect(drawer.getByRole('treeitem', { name: /^Shop / })).toBeVisible();
  await expect(drawer.getByRole('link', { name: 'Resources', exact: true })).toHaveCount(0);
  await drawer.getByRole('button', { name: 'Acme', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Resources', exact: true }).click();
  await expect(page).toHaveURL(/\/resources\/providers$/);
  // The drawer is put away: the screen that was asked for is not left under it.
  await expect(drawer).toHaveCount(0);
  await expect(page.getByRole('navigation', { name: 'Kinds of resources' }).getByRole('link')).toHaveText(['Providers', 'Agents', 'Machines', 'Environments', 'Templates', 'Hosts']);
});

test('the rail’s Resources brings the kinds of resource to the front of the left bar, each with how many its reader may see and opening to its things; the main area shows the one chosen, and the view stays in front from screen to screen', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme');
  await expect(row(page, 'Web')).toBeVisible();
  // The bar has no links to the lists of resources at its foot.
  for (const kind of ['Providers', 'Agents', 'Machines', 'Environments', 'Templates', 'Hosts']) await expect(bar(page).getByRole('link', { name: kind, exact: true })).toHaveCount(0);

  await rail(page).getByRole('button', { name: 'Resource navigation' }).click();
  const view = page.getByRole('navigation', { name: 'Resource navigation', exact: true });
  const kinds = view.getByRole('tree', { name: 'Kinds of resource and what each holds' });
  // Every kind there is, in the order their own screens name them, each with a number; the heading names no org.
  await expect(kinds.getByRole('treeitem')).toHaveCount(6);
  await expect(kinds.getByRole('treeitem')).toHaveText([/^Providers\d+$/, /^Agents\d+$/, /^Machines\d+$/, /^Environments\d+$/, /^Templates\d+$/, /^Hosts\d+$/]);
  await expect(view.getByRole('heading', { name: 'Resource navigation' })).toBeVisible();
  await expect(view).not.toContainText('Acme');
  await expect(rail(page).getByRole('button', { name: 'Resource navigation' })).toHaveAttribute('aria-pressed', 'true');
  await expect(bar(page)).toBeHidden();

  // A kind opens to its things: as many rows as its number says, by name.
  const agents = kinds.getByRole('treeitem', { name: /^Agents / });
  const count = Number((await agents.textContent())?.replace(/\D/g, ''));
  expect(count).toBeGreaterThan(0);
  await agents.press('ArrowRight');
  await expect(agents).toHaveAttribute('aria-expanded', 'true');
  const things = kinds.locator('[role="treeitem"][aria-level="2"]');
  await expect(things).toHaveCount(count);
  const sorted = await things.allTextContents();
  expect([...sorted].sort((one, other) => one.localeCompare(other, 'en', { numeric: true, sensitivity: 'base' }))).toEqual(sorted);

  // The one chosen is shown in the main area, its row is the one in view, and the view is still in front though the screen is of another kind.
  const first = things.first();
  const name = (await first.textContent()) ?? '';
  await first.getByRole('link').click();
  await expect(page).toHaveURL(/\/resources\/agents\/[^/]+$/);
  await expect(page.getByTestId('main').getByRole('heading', { name }).first()).toBeVisible();
  await expect(view).toBeVisible();
  await expect(things.first()).toHaveAttribute('aria-current', 'page');

  // A kind's own row leads to the list of that kind.
  await kinds.getByRole('link', { name: 'Providers', exact: true }).click();
  await expect(page).toHaveURL(/\/resources\/providers/);
  await expect(kinds.getByRole('treeitem', { name: /^Providers / })).toHaveAttribute('aria-current', 'page');
  // A list that was left with something typed in it is opened from its row as it was left.
  await kinds.getByRole('link', { name: 'Agents', exact: true }).click();
  await page.getByTestId('main').getByRole('searchbox').first().fill('claude');
  await expect(page).toHaveURL(/\/resources\/agents\?q=claude/);
  await kinds.getByRole('link', { name: 'Machines', exact: true }).click();
  await expect(page).toHaveURL(/\/resources\/machines/);
  await kinds.getByRole('link', { name: 'Agents', exact: true }).click();
  await expect(page).toHaveURL(/\/resources\/agents\?q=claude/);
  await expect(page.getByTestId('main').getByRole('searchbox').first()).toHaveValue('claude');
  // Which kinds are open is kept on the device.
  await page.reload();
  await expect(view).toBeVisible();
  await expect(kinds.getByRole('treeitem', { name: /^Agents / })).toHaveAttribute('aria-expanded', 'true');

  // An org's stop in the rail brings its tree to the front again.
  await rail(page).getByRole('link', { name: 'Acme', exact: true }).click();
  await expect(row(page, 'Web')).toBeVisible();
  await expect(view).toBeHidden();
  await expect(rail(page).getByRole('button', { name: 'Resource navigation' })).toHaveAttribute('aria-pressed', 'false');
});

// ---------------------------------------------------------------- where the tree begins

test('the tree is narrowed to one place from the place’s menu and stays there: after a reload on this device, not in the person’s other browser, never in the address; its heading is the way back, one place at a time', async ({ page, browser, baseURL }) => {
  await signIn(page);
  await page.goto('/org/acme');
  await expect(row(page, 'Shop')).toBeVisible();
  await expect(begins(page)).toHaveCount(0);

  await showOnly(page, 'Shop');
  // The tree is what Shop holds, at the tree's own level; nothing else of the org is in it, and Shop itself is its heading.
  await expect(row(page, 'Checkout')).toHaveAttribute('aria-level', '1');
  await expect(row(page, 'Infra')).toHaveCount(0);
  await expect(row(page, 'Shop')).toHaveCount(0);
  await expect(beginsAt(page)).toHaveText('Shop');
  await expect(page).toHaveURL(/\/org\/acme$/);
  // The keyboard is in the tree, on its first row.
  await expect(row(page, 'Checkout')).toBeFocused();

  // Narrowed again, to a folder two levels down.
  await showOnly(page, 'Payments');
  await expect(beginsAt(page)).toHaveText('Payments');
  await expect(row(page, 'Card fees')).toHaveAttribute('aria-level', '1');
  await expect(row(page, 'Checkout')).toHaveCount(0);

  // After a reload it stands as it was left, and the address says nothing of it.
  await page.reload();
  await expect(row(page, 'Card fees')).toHaveAttribute('aria-level', '1');
  await expect(beginsAt(page)).toHaveText('Payments');
  await expect(row(page, 'Infra')).toHaveCount(0);
  await expect(page).toHaveURL(/\/org\/acme$/);

  // It belongs to the work on this device: the same person's other browser shows everything.
  const second = await secondReader(browser, baseURL, '/org/acme', PEOPLE.ada);
  await expect(row(second.page, 'Infra')).toBeVisible();
  await expect(begins(second.page)).toHaveCount(0);
  await second.context.close();

  // One place up at a time; from a place of the org itself, one up is everything.
  const up = begins(page).getByRole('button', { name: 'Show one place up' });
  await up.click();
  await expect(beginsAt(page)).toHaveText('Checkout');
  await expect(row(page, 'Payments')).toHaveAttribute('aria-level', '1');
  await up.click();
  await expect(beginsAt(page)).toHaveText('Shop');
  await up.click();
  await expect(begins(page)).toHaveCount(0);
  await expect(row(page, 'Infra')).toBeVisible();
  await expect(row(page, 'Shop')).toHaveAttribute('aria-level', '1');

  // Scoping still has an ordinary way up; the retired admin-wide switch never appears.
  await showOnly(page, 'Infra');
  await expect(row(page, 'Idle-stop rework')).toHaveAttribute('aria-level', '1');
  await expect((await viewMenu(page)).getByRole('menuitem', { name: 'Show everything' })).toHaveCount(0);
  await page.keyboard.press('Escape');
  await begins(page).getByRole('button', { name: 'Show one place up' }).click();
  await expect(row(page, 'Shop')).toBeVisible();
  await expect((await viewMenu(page)).getByRole('menuitem', { name: 'Show everything' })).toHaveCount(0);
  await page.keyboard.press('Escape');

  // A place that is starred is narrowed to from its row under Starred too.
  await (await rowMenu(page, 'Infra')).getByRole('menuitem', { name: 'Star' }).click();
  const starred = sectionOf(page, 'Starred');
  await starred.getByRole('link', { name: /^Infra/ }).hover();
  await starred.getByRole('button', { name: /^Actions for Infra(?: · .+)?$/ }).click();
  await page.getByRole('menu', { name: /^Actions for Infra(?: · .+)?$/ }).getByRole('menuitem', { name: 'Show only this' }).click();
  await expect(beginsAt(page)).toHaveText('Infra');

  // The place is deleted: nothing is narrowed any more, and the tree does not narrow by itself when the place is back.
  await changeTree([{ id: ACME.infra, deleted: true }]);
  await expect(begins(page)).toHaveCount(0);
  await expect(row(page, 'Shop')).toBeVisible();
  await changeTree([{ id: ACME.infra, deleted: false }]);
  await expect(row(page, 'Infra')).toHaveAttribute('aria-level', '1');
  await expect(row(page, 'Shop')).toBeVisible();
  await page.reload();
  await expect(row(page, 'Shop')).toBeVisible();
  await expect(begins(page)).toHaveCount(0);
});

test('“Show only where I am” narrows the tree to the place in view, a thread’s topic; while the reader looks at something outside it, the first row says where that lies and shows it there', async ({ page }) => {
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await expect(row(page, 'Main thread').first()).toBeVisible();

  // Where the reader is, on a thread's screen, is the thread's topic.
  await (await viewMenu(page)).getByRole('menuitem', { name: 'Show only where I am' }).click();
  await expect(beginsAt(page)).toHaveText('Fix login');
  await expect(row(page, 'Main thread')).toHaveAttribute('aria-level', '1');
  await expect(row(page, 'Main thread')).toHaveAttribute('aria-current', 'page');
  await expect(row(page, 'Infra')).toHaveCount(0);
  // The tree begins there already: the entry is off and says why.
  const off = (await viewMenu(page)).getByRole('menuitem', { name: /^Show only where I am/ });
  await expect(off).toHaveAttribute('aria-disabled', 'true');
  await expect(off).toContainText('The tree shows only this already.');
  await page.keyboard.press('Escape');

  // The reader goes elsewhere. The tree stays where it was put; its first row says where the thing in view lies.
  await page.goto(`/topic/${ACME.idleStop}`);
  await expect(page.getByRole('heading', { name: 'Idle-stop rework' })).toBeVisible();
  await expect(beginsAt(page)).toHaveText('Fix login');
  const outside = tree(page).getByRole('treeitem').first();
  await expect(outside).toContainText('Idle-stop rework is in Acme / Infra');
  await expect(row(page, 'Main thread')).toBeVisible();

  // "Show it there": the tree begins at the thing's own project, the heading says so, and the row has gone.
  await outside.getByRole('button', { name: 'Show it there' }).click();
  await expect(beginsAt(page)).toHaveText('Infra');
  await expect(row(page, 'Idle-stop rework')).toHaveAttribute('aria-current', 'page');
  await expect(tree(page).getByText(/ is in /)).toHaveCount(0);

  // A place above the one the tree begins at is outside it as well; back inside, there is no such row.
  await showOnly(page, 'Idle-stop rework');
  await expect(beginsAt(page)).toHaveText('Idle-stop rework');
  await expect(tree(page).getByText(/ is in /)).toHaveCount(0);
  await page.goto(`/project/${ACME.infra}`);
  await expect(tree(page).getByRole('treeitem').first()).toContainText('Infra is in Acme');
  // At the org's own home nothing is said: the org lies nowhere in its own tree.
  await page.goto('/org/acme');
  await expect(page.getByRole('heading', { name: 'Acme' }).first()).toBeVisible();
  await expect(beginsAt(page)).toHaveText('Idle-stop rework');
  await expect(tree(page).getByText(/ is in /)).toHaveCount(0);
  // And there is nowhere to narrow to from there: the entry says so.
  const top = (await viewMenu(page)).getByRole('menuitem', { name: /^Show only where I am/ });
  await expect(top).toHaveAttribute('aria-disabled', 'true');
  await expect(top).toContainText('You are at the org itself.');
});

test('by keyboard alone: a key narrows the tree to the row the keyboard is on, another steps one place up, each says why where it cannot, the line of a thing outside is acted on with Enter, and the list of keys names both keys', async ({ page }) => {
  await signIn(page);
  await page.goto(`/topic/${ACME.idleStop}`);
  await expect(row(page, 'Shop')).toBeVisible();
  const notices = page.getByRole('region', { name: /^Notifications/ });

  await row(page, 'Shop').focus();
  await page.keyboard.press('Alt+Shift+ArrowRight');
  await expect(beginsAt(page)).toHaveText('Shop');
  // The keyboard stays in the tree: on its first row, which is the line that says where the thing in view lies.
  const outside = tree(page).getByRole('treeitem').first();
  await expect(outside).toContainText('Idle-stop rework is in Acme / Infra');
  await expect(outside).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(row(page, 'Checkout')).toBeFocused();
  await page.keyboard.press('Alt+Shift+ArrowRight');
  await expect(beginsAt(page)).toHaveText('Checkout');
  // A topic is a place a tree can begin at; a thread is none, and on its row the key says what it takes and moves nothing.
  await row(page, 'Payments').focus();
  await page.keyboard.press('ArrowDown');
  await expect(row(page, 'Card fees')).toBeFocused();
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowDown');
  await expect(row(page, 'Fee table')).toBeFocused();
  await page.keyboard.press('Alt+Shift+ArrowRight');
  await expect(notices.getByText('The tree can begin at an org, a project, a folder or a topic.')).toBeVisible();
  await expect(beginsAt(page)).toHaveText('Checkout');
  await page.keyboard.press('ArrowUp');
  await expect(row(page, 'Card fees')).toBeFocused();
  await page.keyboard.press('Alt+Shift+ArrowRight');
  await expect(beginsAt(page)).toHaveText('Card fees');

  // One place up at a time, and from a place of the org itself to everything.
  for (const place of ['Payments', 'Checkout', 'Shop']) {
    await page.keyboard.press('Alt+Shift+ArrowLeft');
    await expect(beginsAt(page)).toHaveText(place);
  }
  // Enter on the first line shows the thing in view where it lies.
  await tree(page).getByRole('treeitem').first().focus();
  await page.keyboard.press('Enter');
  await expect(beginsAt(page)).toHaveText('Infra');
  await expect(row(page, 'Idle-stop rework')).toHaveAttribute('aria-current', 'page');
  await page.keyboard.press('Alt+Shift+ArrowLeft');
  await expect(begins(page)).toHaveCount(0);
  await expect(row(page, 'Shop')).toBeVisible();
  // With everything shown there is no place above: the key says so.
  await page.keyboard.press('Alt+Shift+ArrowLeft');
  await expect(notices.getByText('The tree shows everything already.')).toBeVisible();
  await expect(begins(page)).toHaveCount(0);

  // The list of keys has both, with what they do.
  await page.keyboard.press('?');
  const keys = page.getByRole('dialog', { name: 'Keyboard shortcuts' });
  await expect(keys.getByText('Show one place up in the tree')).toBeVisible();
  await expect(keys.getByText('Show only the row the keyboard is on')).toBeVisible();
});

test('the finder finds the acts that move the tree’s beginning by the words other tools use for them, and does them', async ({ page }) => {
  await signIn(page);
  await page.goto(`/topic/${ACME.fixLogin}`);
  await expect(row(page, 'Fix login')).toHaveAttribute('aria-current', 'page');
  const finder = page.getByRole('dialog', { name: 'Find', exact: true });

  for (const word of ['hoist', 'zoom', 'focus', 'root']) {
    await page.keyboard.press('Control+k');
    await finder.getByRole('combobox', { name: 'Search terms', exact: true }).fill(`>${word}`);
    await expect(finder.getByRole('option', { name: /^Show only where I am/ })).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(finder).toHaveCount(0);
  }

  // Chosen there, it narrows the tree as the tree's own menu does; then the finder has the way back as well.
  await page.keyboard.press('Control+k');
  await finder.getByRole('combobox', { name: 'Search terms', exact: true }).fill('>hoist');
  await finder.getByRole('option', { name: /^Show only where I am/ }).click();
  await expect(beginsAt(page)).toHaveText('Fix login');
  await page.keyboard.press('Control+k');
  await finder.getByRole('combobox', { name: 'Search terms', exact: true }).fill('>show everything');
  await expect(finder.getByRole('option', { name: /^Show everything/ })).toHaveCount(0);
  await finder.getByRole('combobox', { name: 'Search terms', exact: true }).fill('>show one place up');
  await finder.getByRole('option', { name: /^Show one place up/ }).click();
  // Topic -> project -> org, without an authority-changing shortcut.
  await begins(page).getByRole('button', { name: 'Show one place up' }).click();
  await expect(begins(page)).toHaveCount(0);
  await expect(row(page, 'Infra')).toBeVisible();
});

// ---------------------------------------------------------------- folding away

test('a place’s menu collapses other branches, navigation keeps the chosen folds, and the tree’s own menu opens the way explicitly', async ({ page }) => {
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await expect(row(page, 'Main thread').first()).toHaveAttribute('aria-current', 'page');
  await expect(row(page, 'Infra')).toHaveAttribute('aria-expanded', 'true');

  await (await rowMenu(page, 'Web')).getByRole('menuitem', { name: 'Collapse the others' }).click();
  // The way to Web and what was open in it stay; every other branch is closed.
  await expect(row(page, 'Web')).toHaveAttribute('aria-expanded', 'true');
  await expect(row(page, 'Fix login')).toHaveAttribute('aria-expanded', 'true');
  await expect(row(page, 'Main thread').first()).toBeVisible();
  for (const closed of ['ML', 'Infra', 'Shop']) await expect(row(page, closed)).toHaveAttribute('aria-expanded', 'false');
  await expect(row(page, 'Idle-stop rework')).toHaveCount(0);

  // Going elsewhere preserves every deliberate fold and closes nothing: Web remains open.
  await page.goto(`/topic/${ACME.idleStop}`);
  await expect(row(page, 'Infra')).toHaveAttribute('aria-expanded', 'false');
  await expect(row(page, 'Idle-stop rework')).toHaveCount(0);
  await expect(row(page, 'Web')).toHaveAttribute('aria-expanded', 'true');
  await expect(row(page, 'Shop')).toHaveAttribute('aria-expanded', 'false');

  // The deliberate fold also remains when the tree hears of a change.
  await expect(row(page, 'Idle-stop rework')).toHaveCount(0);
  await changeTree([{ id: ACME.payroll, name: 'Salaries' }]);
  await expect(row(page, 'Salaries')).toBeVisible();
  await expect(row(page, 'Infra')).toHaveAttribute('aria-expanded', 'false');
  await expect(row(page, 'Idle-stop rework')).toHaveCount(0);

  // "Open to where I am" opens it again.
  await (await viewMenu(page)).getByRole('menuitem', { name: 'Open to where I am' }).click();
  await expect(row(page, 'Idle-stop rework')).toHaveAttribute('aria-current', 'page');
  // "Collapse all" closes every row, and the reader's place with them.
  await (await viewMenu(page)).getByRole('menuitem', { name: 'Collapse all' }).click();
  await expect(row(page, 'Idle-stop rework')).toHaveCount(0);
  await expect(row(page, 'Research')).toHaveAttribute('aria-expanded', 'false');
});

test('while the tree scrolls, the places above its first row stay at its top, for no more than a third of its height, and a press on one goes to it', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 640 });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await expect(row(page, 'Main thread').first()).toBeVisible();
  const staying = bar(page).locator('[data-staying]');
  const scroller = bar(page).locator('[data-bar-scroll]');
  await expect(staying).toHaveCount(0);

  // The tree is longer than the bar: its reader scrolls until a thread of Fix login is the first row in sight.
  await row(page, 'Repro').evaluate((element) => element.scrollIntoView({ block: 'start' }));
  await expect(staying).toBeVisible();
  // What stays is the way to the first row that is still seen, nearest last, and it is rows of the tree drawn again: the same names.
  const names = await staying.getByRole('button', { includeHidden: true }).allTextContents();
  expect(names.length).toBeGreaterThan(0);
  expect(['Research', 'Web', 'Fix login'].slice(-names.length)).toEqual(names);
  const room = await scroller.evaluate((element) => element.clientHeight);
  const taken = await staying.evaluate((element) => element.getBoundingClientRect().height);
  expect(taken).toBeLessThanOrEqual(room / 3);
  // It lies at the top edge of what scrolls, over the rows.
  const top = await scroller.evaluate((element) => element.getBoundingClientRect().top);
  expect(Math.abs((await staying.evaluate((element) => element.getBoundingClientRect().top)) - top)).toBeLessThanOrEqual(1);

  // A press on one of them brings that place to the top, with the keyboard on it.
  await staying.getByRole('button', { name: 'Web', includeHidden: true }).click();
  await expect(row(page, 'Web')).toBeFocused();
  await expect(row(page, 'Web')).toBeInViewport();
  // Back at the top of the tree nothing stays.
  await scroller.evaluate((element) => element.scrollTo(0, 0));
  await expect(staying).toHaveCount(0);
});

test('the field at the top of the tree narrows it by a name: what is found stays with the way to it, the arrow goes on into it, Escape empties the field, and a key gives it the keyboard from the tree', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme');
  await expect(row(page, 'Web')).toBeVisible();
  const field = bar(page).getByRole('searchbox', { name: 'Narrow this tree' });
  await expect(field).toBeVisible();
  await expect(field).toHaveAttribute('placeholder', 'Narrow this tree…');

  // The keyboard is in the tree: the key gives it to the field, and what is typed goes into it.
  await row(page, 'Web').focus();
  await page.keyboard.press('Control+Alt+f');
  await expect(field).toBeFocused();
  await page.keyboard.type('FEE');
  // A topic and a thread have it in their names: they stay, with the project and the folders on the way to them, open.
  await expect(row(page, 'Fee table')).toBeVisible();
  expect(await standing(page)).toEqual(['Shop', 'Checkout', 'Payments', 'Card fees', 'Fee table']);
  await expect(row(page, 'Web')).toHaveCount(0);

  // The arrow down goes on into what was found, and the arrows walk it.
  await page.keyboard.press('ArrowDown');
  await expect(row(page, 'Shop')).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(row(page, 'Checkout')).toBeFocused();
  // Where the tree begins is not moved by narrowing it, and narrowing works in a tree that begins somewhere.
  await page.keyboard.press('Alt+Shift+ArrowRight');
  await expect(beginsAt(page)).toHaveText('Checkout');
  expect(await standing(page)).toEqual(['Payments', 'Card fees', 'Fee table']);
  await page.keyboard.press('Alt+Shift+ArrowLeft');
  await page.keyboard.press('Alt+Shift+ArrowLeft');
  await expect(begins(page)).toHaveCount(0);
  await expect(field).toHaveValue('FEE');

  // Nothing has the letters: the bar says so.
  await field.fill('no such name');
  await expect(bar(page).getByRole('status')).toHaveText('Nothing here has that in its name.');
  await expect(tree(page)).toHaveCount(0);
  // The button at the end of the field empties it.
  await bar(page).getByRole('button', { name: 'Clear', exact: true }).click();
  await expect(field).toHaveValue('');
  await expect(row(page, 'Web')).toBeVisible();

  // Escape empties the field too: the tree is whole, as its reader left it, and has the keyboard.
  await field.fill('fee');
  await expect(row(page, 'Web')).toHaveCount(0);
  await field.press('Escape');
  await expect(field).toHaveValue('');
  await expect(row(page, 'Web')).toBeVisible();
  await expect(row(page, 'Fee table')).toHaveCount(0);
  await expect(tree(page).locator('[role="treeitem"]:focus')).toHaveCount(1);

  // The key is the tree's own. With the keyboard elsewhere it is not bound, and the list of keys does not offer it there.
  await page.getByTestId('main').getByRole('tab', { name: /Overview/ }).focus();
  await page.keyboard.press('Control+Alt+f');
  await expect(field).not.toBeFocused();
  await page.keyboard.press('?');
  const keys = page.getByRole('dialog', { name: 'Keyboard shortcuts' });
  // What acts here is listed first; the keys of other screens follow, to be changed from anywhere.
  const here = keys.getByRole('list', { name: 'Here' });
  await expect(here.getByText('Previous thread', { exact: true })).toBeVisible();
  await expect(here.getByText('Next unread thread', { exact: true })).toBeVisible();
  await expect(here.getByText('Narrow the tree of the left bar by name')).toHaveCount(0);
  await expect(keys.getByRole('list', { name: 'On other screens' }).getByText('Narrow the tree of the left bar by name')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(keys).toHaveCount(0);
  // Asked from the tree, the list has it here, and still has it while the list itself holds the keyboard.
  await row(page, 'Web').focus();
  await page.keyboard.press('?');
  await expect(here.getByText('Narrow the tree of the left bar by name')).toBeVisible();
});

// ---------------------------------------------------------------- what the tree shows

test('the tree shows threads, documents and chats as its reader ticks them, kept with the person: after a reload, and in their other browser without one', async ({ page, browser, baseURL }) => {
  await chats('/say', { node: 'Fix login', as: 'dora', text: 'Anybody here?' });
  await signIn(page);
  await page.goto('/org/acme');
  await row(page, 'Fix login').press('ArrowRight');
  await expect(row(page, 'Repro')).toBeVisible();
  await expect(doc(page, 'Rollout plan')).toBeVisible();

  // As it comes: threads and documents, and no chats.
  const menu = await viewMenu(page);
  await expect(menu.getByRole('menuitemcheckbox', { name: 'Threads' })).toHaveAttribute('aria-checked', 'true');
  await expect(menu.getByRole('menuitemcheckbox', { name: 'Documents' })).toHaveAttribute('aria-checked', 'true');
  await expect(menu.getByRole('menuitemcheckbox', { name: 'Chats' })).toHaveAttribute('aria-checked', 'false');
  // Three content kinds, visibility choices and the ordinary participation filter have separate familiar names.
  const choices = menu.getByRole('menuitemcheckbox');
  const choiceNames = ['Threads', 'Documents', 'Chats', 'Hide done', 'Where I take part', 'Archived'];
  await expect(choices).toHaveCount(choiceNames.length);
  for (const [index, name] of choiceNames.entries()) await expect(choices.nth(index)).toHaveAccessibleName(name);
  await page.keyboard.press('Escape');
  await expect(chatRow(page)).toHaveCount(0);

  // Without threads the tree is the map of places with what is written beside them.
  await tick(page, 'Threads');
  await expect(row(page, 'Repro')).toHaveCount(0);
  await expect(row(page, 'Main thread')).toHaveCount(0);
  await expect(doc(page, 'Rollout plan')).toBeVisible();
  // The topic still says what its threads do, and how many decisions wait in them.
  await expect(row(page, 'Fix login').getByText('1 approval waits inside')).toBeAttached();
  await page.reload();
  await expect(row(page, 'Fix login')).toBeVisible();
  await expect(row(page, 'Repro')).toHaveCount(0);

  // The same person in another browser finds it so, and what they tick there shows here without a reload.
  const second = await secondReader(browser, baseURL, '/org/acme', PEOPLE.ada);
  await expect(row(second.page, 'Fix login')).toBeVisible();
  await expect((await viewMenu(second.page)).getByRole('menuitemcheckbox', { name: 'Threads' })).toHaveAttribute('aria-checked', 'false');
  await second.page.keyboard.press('Escape');
  await tick(second.page, 'Threads');
  await expect(row(page, 'Repro')).toBeVisible();

  // Chats: every place somebody wrote in has a row for its chat, after its documents, and the team's chat is the tree's last row.
  await tick(second.page, 'Chats');
  await expect(chatRow(page)).toHaveCount(1);
  await expect(chatRow(page, 'Team chat')).toHaveAttribute('aria-level', '1');
  const names = await standing(page);
  expect(names.indexOf('Chat')).toBeGreaterThan(names.indexOf('Rollout plan'));
  expect(names.indexOf('Chat')).toBeLessThan(names.indexOf('Migrate CI'));
  expect(names.at(-1)).toBe('Team chat');
  // The row "Team chat" above Starred has gone: the one in the tree is the one.
  await expect(bar(page).getByRole('link', { name: 'Team chat' })).toHaveCount(1);
  await page.reload();
  await expect(chatRow(page, 'Team chat')).toBeVisible();
  await expect(chatRow(page)).toHaveCount(1);

  // Unticked again, the tree has no chats and the team's chat is back at the top.
  await tick(page, 'Chats');
  await expect(chatRow(page)).toHaveCount(0);
  await expect(chatRow(page, 'Team chat')).toHaveCount(0);
  await expect(bar(page).getByRole('link', { name: 'Team chat' })).toHaveCount(1);
  await expect(chatRow(second.page, 'Team chat')).toHaveCount(0);
  await second.context.close();
});

test('a chat that is unread makes the closed row above it bold with a dot and no number, a mention is a number, and the glyph of a place speaks for its chat only while the chat has no row on screen', async ({ page }) => {
  // Ada wrote in the chat of Fix login before: she follows it.
  await chats('/say', { node: 'Fix login', as: 'ada', text: 'Shall we start?' });
  await signIn(page);
  await page.goto('/org/acme');
  const fix = row(page, 'Fix login');
  const inside = (name: string) => row(page, name).getByRole('img', { name: 'Unread inside' });
  // Chats have no rows as the bar comes: the topic carries the small glyph of its chat, which she has read.
  await expect(fix.getByRole('link', { name: 'Chat', exact: true })).toBeVisible();
  await expect(inside('Fix login')).toHaveCount(0);

  // Dora answers. Closed, the topic is bold with a dot; it is said and not counted.
  await chats('/say', { node: 'Fix login', as: 'dora', text: 'Yes, let us.' });
  await expect(inside('Fix login')).toBeVisible();
  await expect(fix.locator('a > span.truncate').first()).toHaveClass(/font-semibold/);
  await expect(fix.getByText(/\d+ unread/)).toHaveCount(0);
  // The project above says it only while it is closed: open, its rows say it themselves.
  await expect(inside('Web')).toHaveCount(0);
  await row(page, 'Web').press('ArrowLeft');
  await expect(inside('Web')).toBeVisible();
  await row(page, 'Web').press('ArrowRight');

  // She is mentioned: that waits for her in person, and is a number.
  await chats('/say', { node: 'Fix login', as: 'dora', text: '@ada can you look at this?' });
  await expect(fix.getByText('1 mention of you inside')).toBeAttached();

  // Open, with no rows for chats, the row says nothing of the chat itself: the glyph does, and leads to it.
  await fix.press('ArrowRight');
  await expect(inside('Fix login')).toHaveCount(0);
  await expect(fix.getByText('1 mention of you inside')).toHaveCount(0);
  const glyph = fix.getByRole('link', { name: 'Chat, 1 mention of you' });
  await expect(glyph).toBeVisible();

  // With rows for chats, the open topic has no glyph: its chat's own row says it, after the topic's documents.
  await tick(page, 'Chats');
  await expect(fix.getByRole('link', { name: /^Chat/ })).toHaveCount(0);
  await expect(chatRow(page).getByText('1 mention of you')).toBeAttached();
  // Closed again, the glyph is back and the row says it for everything it hides.
  await fix.press('ArrowLeft');
  await expect(fix.getByRole('link', { name: 'Chat', exact: true })).toBeVisible();
  await expect(fix.getByText('1 mention of you inside')).toBeAttached();
  await expect(chatRow(page)).toHaveCount(0);

  // The glyph leads to the chat; read there, nothing is left on the row.
  await fix.getByRole('link', { name: 'Chat', exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/topic/${ACME.fixLogin}\\?panel=chat`));
  // Her name in the message is drawn as a mention of its own: the words after it are what is looked for.
  await expect(page.getByText('can you look at this?').first()).toBeVisible();
  await expect(fix.getByText('1 mention of you inside')).toHaveCount(0, { timeout: 30_000 });
  await expect(inside('Fix login')).toHaveCount(0);
});

test('with chats in the tree, a message arrives on the chat’s row as it is written, the keys that go to what is unread take chats in, and “Mark as read” on a place reads the threads and chats under it', async ({ page }) => {
  await chats('/say', { node: 'Fix login', as: 'ada', text: 'Shall we start?' });
  await double('/places/follow', { username: 'ada', node: ACME.fixLogin, level: 'all' });
  await emit(ACME.mainThread, { items: [agentMessage('m-1', 'A first answer.')], thread: {} });
  await signIn(page);
  await page.goto('/org/acme');
  await tick(page, 'Chats');
  await row(page, 'Fix login').press('ArrowRight');
  const chat = chatRow(page);
  await expect(chat).toHaveCount(1);
  await expect(chat.getByRole('img', { name: 'Unread', exact: true })).toHaveCount(0);
  // The threads of the topic she follows are unread: the one that answered, and the one that waits.
  const main = row(page, 'Main thread').first();
  await expect(main.getByRole('img', { name: 'Unread', exact: true })).toBeVisible();

  // Dora writes: the row of the chat is bold with a dot at once.
  await chats('/say', { node: 'Fix login', as: 'dora', text: 'Yes, let us.' });
  await expect(chat.getByRole('img', { name: 'Unread', exact: true })).toBeVisible();
  await expect(chat.locator('a > span.truncate')).toHaveClass(/font-semibold/);

  // "Mark as read" on the project: every thread and chat under it is read, and the menu then says there is nothing left.
  await (await rowMenu(page, 'Web')).getByRole('menuitem', { name: 'Mark as read' }).click();
  await expect(chat.getByRole('img', { name: 'Unread', exact: true })).toHaveCount(0);
  await expect(main.getByRole('img', { name: 'Unread', exact: true })).toHaveCount(0);
  await expect(row(page, 'Repro').getByRole('img', { name: 'Unread', exact: true })).toHaveCount(0);
  const none = (await rowMenu(page, 'Web')).getByRole('menuitem', { name: /^Mark as read/ });
  await expect(none).toHaveAttribute('aria-disabled', 'true');
  await expect(none).toContainText('Nothing here is unread.');
  await page.keyboard.press('Escape');
  // The server holds it: after a reload nothing is unread.
  await page.reload();
  await expect(chatRow(page)).toHaveCount(1);
  await expect(chatRow(page).getByRole('img', { name: 'Unread', exact: true })).toHaveCount(0);
  await expect(row(page, 'Main thread').first().getByRole('img', { name: 'Unread', exact: true })).toHaveCount(0);

  // Something is said in the thread and then in the chat: the key goes to the thread first, as the tree stands, and then to the chat.
  await emit(ACME.mainThread, { items: [agentMessage('m-2', 'A second answer.')], thread: {} });
  await chats('/say', { node: 'Fix login', as: 'dora', text: 'One more thing.' });
  await expect(chatRow(page).getByRole('img', { name: 'Unread', exact: true })).toBeVisible();
  await page.keyboard.press('Alt+Shift+ArrowDown');
  await expect(page).toHaveURL(new RegExp(`/thread/${ACME.mainThread}$`));
  await page.keyboard.press('Alt+Shift+ArrowDown');
  await expect(page).toHaveURL(new RegExp(`/topic/${ACME.fixLogin}\\?panel=chat`));
  await expect(chatRow(page)).toHaveAttribute('aria-current', 'page');
  await expect(page.getByText('One more thing.').first()).toBeVisible();
});

test('a chat its reader does not follow is a quiet row that says nothing on the rows above it and still carries a mention; a done topic stays in view while its chat is unread, though done topics are hidden', async ({ page }) => {
  // Dora wrote in the chat of Upgrade deps; Ada never did, and follows nothing there.
  await chats('/say', { node: 'Upgrade deps', as: 'dora', text: 'Which versions?' });
  // In the chat of Migrate CI, which is done, Ada wrote before.
  await chats('/say', { node: 'Migrate CI', as: 'ada', text: 'Is this really done?' });
  await signIn(page);
  await page.goto('/org/acme');
  const deps = row(page, 'Upgrade deps');
  // Closed: a quiet glyph, and nothing on the row.
  await expect(deps.getByRole('link', { name: 'Chat, not followed' })).toBeVisible();
  await expect(deps.getByRole('img', { name: 'Unread inside' })).toHaveCount(0);

  await tick(page, 'Chats');
  await deps.press('ArrowRight');
  const quiet = chatRow(page);
  await expect(quiet).toHaveAccessibleName(/^Chat not followed/);
  await expect(quiet.getByRole('img', { name: 'Unread', exact: true })).toHaveCount(0);
  await expect(quiet.locator('a > span.truncate')).not.toHaveClass(/font-semibold/);

  // She sets the chat to nothing herself, and is mentioned there: the row stays quiet and carries the mention.
  await quiet.getByRole('link').click();
  await expect(page).toHaveURL(new RegExp(`/topic/${ACME.upgradeDeps}\\?panel=chat`));
  // The chat is shown before its bell is pressed: a list opened while the chat is still being read closes again.
  await expect(page.getByText('Which versions?').first()).toBeVisible();
  await page.getByRole('button', { name: 'Notification settings', exact: true }).click();
  await expect(page.getByText(/^You have not chosen here/)).toBeVisible();
  await page.getByRole('menuitemradio', { name: 'Off', exact: true }).click();
  await page.getByRole('button', { name: 'Notification settings', exact: true }).click();
  await expect(page.getByText('You chose no notifications', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'You chose no notifications', exact: true })).toHaveCount(0);
  await page.keyboard.press('Escape');
  await page.goto('/org/acme');
  await expect(chatRow(page)).toHaveCount(1);
  await chats('/say', { node: 'Upgrade deps', as: 'dora', text: '@ada the lock file is yours.' });
  await expect(chatRow(page).getByText('1 mention of you')).toBeAttached();
  await expect(chatRow(page)).toHaveAccessibleName(/^Chat not followed/);
  // Closed, the topic carries the mention and is not bold: nothing she follows is unread in it.
  await row(page, 'Upgrade deps').press('ArrowLeft');
  await expect(row(page, 'Upgrade deps').getByText('1 mention of you inside')).toBeAttached();
  await expect(row(page, 'Upgrade deps').getByRole('img', { name: 'Unread inside' })).toHaveCount(0);

  // Done topics are hidden: Migrate CI goes, since nothing in it waits.
  await tick(page, 'Hide done');
  await expect(row(page, 'Migrate CI')).toHaveCount(0);
  // Dora answers in its chat: the topic is back in view, bold with a dot, for as long as its chat is unread.
  await chats('/say', { node: 'Migrate CI', as: 'dora', text: 'One job still fails.' });
  await expect(row(page, 'Migrate CI')).toBeVisible();
  await expect(row(page, 'Migrate CI').getByRole('img', { name: 'Unread inside' })).toBeVisible();
  await (await rowMenu(page, 'Migrate CI')).getByRole('menuitem', { name: 'Mark as read' }).click();
  await expect(row(page, 'Migrate CI')).toHaveCount(0);
});

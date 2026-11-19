/** Folders, which are projects that lie in projects: one made in a project and one in a folder, a tree three folders deep walked by keyboard, a topic moved into a folder and out of it with its plan read first, a folder moved into another and refused into itself, the depth refused in words, what waits summed into a closed row, and who sees "New folder". */
import { randomUUID } from 'node:crypto';

import { expect, type Locator, type Page, test } from '@playwright/test';

import { ACME, changeTree, deny, PEOPLE, scenario, signIn } from './support.ts';

test.beforeEach(async () => {
  await scenario();
});

const tree = (page: Page) => page.getByRole('tree', { name: 'Orgs, projects, topics and threads' });
const crumbs = (page: Page) => page.getByRole('navigation', { name: 'Where this is' });
const notices = (page: Page) => page.getByRole('region', { name: /^Notifications/ });
const main = (page: Page) => page.getByTestId('main');
/** A row of the left bar by the name it begins with (what follows is the names of its buttons). */
const row = (page: Page, name: string): Locator => tree(page).getByRole('treeitem', { name: new RegExp(`^(Waiting for approval |Blocked |Running |Starting )?${name}( |$)`) });
const level = (page: Page, name: string) => row(page, name).getAttribute('aria-level');

/** Signs in and goes to a screen by its address. */
async function open(page: Page, path: string, person: { login: string; password: string } = PEOPLE.ada): Promise<void> {
  await signIn(page, person);
  await page.goto(path);
}

/** Opens the "⋯" of the screen in view: the last button of its name, since the left bar's row has the first. */
async function screenMenu(page: Page, name: string): Promise<void> {
  await page.getByRole('button', { name: `Actions for ${name}` }).last().click();
}

/** A project that lies where `parent` says, as the live tree is told of it. */
const project = (name: string, parent: string) => ({ id: randomUUID(), kind: 'project', name, parent_id: parent });
const topic = (name: string, parent: string) => ({ id: randomUUID(), kind: 'topic', name, parent_id: parent, topic: { status: 'open', tag_ids: [] } });

test('a folder is made in a project and another in a folder, each leading to its own screen, and the tree and the path name both', async ({ page }) => {
  await open(page, `/project/${ACME.shop}`);
  await expect(page.getByRole('heading', { name: 'Shop' })).toBeVisible();
  await expect(main(page).getByRole('region', { name: 'Folders' }).getByRole('link', { name: 'Checkout' })).toBeVisible();

  // A folder in a project: the dialog is the one that makes a project, with one line that says what a folder follows.
  await screenMenu(page, 'Shop');
  await page.getByRole('menuitem', { name: 'New folder…', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'New folder in Shop' });
  await expect(dialog).toContainText('A folder in Shop. It follows Shop’s defaults until you change its own.');
  await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('Receipts');
  await dialog.getByRole('textbox', { name: /^Description/ }).fill('Everything after the sale');
  await dialog.getByRole('button', { name: 'Create folder' }).click();

  await expect(page.getByRole('heading', { name: 'Receipts' })).toBeVisible();
  await page.getByTestId('main').getByRole('button', { name: 'Description and tags', exact: true }).click();
  await expect(page.getByText('Everything after the sale')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(crumbs(page)).toContainText('Shop');
  await expect(row(page, 'Receipts')).toBeVisible();
  expect(await level(page, 'Receipts')).toBe('2');

  // Both folders are directly reachable, after the topic list.
  await page.goto(`/project/${ACME.shop}`);
  await expect(main(page).getByRole('region', { name: 'Folders' }).getByRole('link', { name: 'Checkout', exact: true })).toBeVisible();
  await expect(main(page).getByRole('region', { name: 'Folders' }).getByRole('link', { name: 'Receipts' })).toBeVisible();

  // A folder in a folder, two levels down: the path folds its middle into "…" and the fold lists what it hid.
  await page.goto(`/project/${ACME.payments}`);
  await screenMenu(page, 'Payments');
  await page.getByRole('menuitem', { name: 'New folder…', exact: true }).click();
  await page.getByRole('dialog', { name: 'New folder in Payments' }).getByRole('textbox', { name: 'Name', exact: true }).fill('Refunds');
  await page.getByRole('dialog').getByRole('button', { name: 'Create folder' }).click();
  await expect(page.getByRole('heading', { name: 'Refunds' })).toBeVisible();
  expect(await level(page, 'Refunds')).toBe('4');
  await expect(crumbs(page).getByRole('link', { name: 'Payments' })).toBeVisible();
  await crumbs(page).getByRole('button', { name: 'Places in between' }).click();
  await expect(page.getByRole('menuitem', { name: 'Shop' })).toBeVisible();
  await page.getByRole('menuitem', { name: 'Checkout' }).click();
  await expect(page).toHaveURL(new RegExp(`/project/${ACME.checkout}$`));
  await expect(page.getByRole('heading', { name: 'Checkout' })).toBeVisible();
});

test('a tree three folders deep is walked by keyboard, and what was closed stays closed when the page is opened again', async ({ page }) => {
  const refunds = project('Refunds', ACME.payments);
  const chargebacks = topic('Chargebacks', refunds.id);
  await open(page, '/org/acme');
  await changeTree([refunds, chargebacks]);
  await expect(row(page, 'Chargebacks')).toBeVisible();

  // Down into the first folder and through the two below it, one level at a time, to the topic in the deepest.
  await row(page, 'Shop').focus();
  await page.keyboard.press('ArrowDown');
  await expect(row(page, 'Checkout')).toBeFocused();
  await page.keyboard.press('ArrowRight');
  await expect(row(page, 'Payments')).toBeFocused();
  await page.keyboard.press('ArrowRight');
  await expect(row(page, 'Refunds')).toBeFocused();
  await expect(row(page, 'Refunds')).toHaveAttribute('aria-level', '4');
  await page.keyboard.press('ArrowRight');
  await expect(row(page, 'Chargebacks')).toBeFocused();
  await expect(row(page, 'Chargebacks')).toHaveAttribute('aria-level', '5');

  // Out again: Left steps to the folder, and a second Left closes it, which takes its topic out of the list.
  await page.keyboard.press('ArrowLeft');
  await expect(row(page, 'Refunds')).toBeFocused();
  await page.keyboard.press('ArrowLeft');
  await expect(row(page, 'Refunds')).toHaveAttribute('aria-expanded', 'false');
  await expect(row(page, 'Chargebacks')).toHaveCount(0);
  await page.keyboard.press('ArrowLeft');
  await expect(row(page, 'Payments')).toBeFocused();

  // Home and End, a typed name, and Enter to go where the row leads.
  await page.keyboard.press('End');
  await expect(tree(page).getByRole('treeitem').last()).toBeFocused();
  await page.keyboard.press('Home');
  await expect(tree(page).getByRole('treeitem').first()).toBeFocused();
  await page.keyboard.type('refu');
  await expect(row(page, 'Refunds')).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(new RegExp(`/project/${refunds.id}$`));
  await expect(page.getByRole('heading', { name: 'Refunds' })).toBeVisible();

  // What was closed was remembered on this device.
  await page.reload();
  await expect(row(page, 'Refunds')).toBeVisible();
  await expect(row(page, 'Refunds')).toHaveAttribute('aria-expanded', 'false');
  await expect(row(page, 'Chargebacks')).toHaveCount(0);
  await expect(row(page, 'Payments')).toHaveAttribute('aria-expanded', 'true');
});

test('a topic is moved into a folder with its plan read first, and out of it again, and the tree and the place it names follow', async ({ page }) => {
  await open(page, `/topic/${ACME.cartRewrite}`);
  await expect(page.getByRole('heading', { name: 'Cart rewrite' })).toBeVisible();
  expect(await level(page, 'Cart rewrite')).toBe('3');

  await screenMenu(page, 'Cart rewrite');
  await page.getByRole('menuitem', { name: 'Move to…' }).click();
  const dialog = page.getByRole('dialog', { name: /^Move the topic Cart rewrite, in / });
  await expect(dialog).toContainText('To another project or folder of the same tree, with its threads.');
  // Nothing can be confirmed before somebody has chosen where, and what that would change has been read.
  const confirm = dialog.getByRole('button', { name: 'Move', exact: true });
  await expect(confirm).toHaveAttribute('aria-disabled', 'true');

  const places = dialog.getByRole('radiogroup', { name: 'Move to' });
  // The place it lies in is greyed and says why; a topic is offered projects and folders, and no org: the org is named
  // for the shape of the tree, and is no choice.
  await expect(places.getByRole('radio', { name: 'Checkout' })).toBeDisabled();
  await expect(places.getByText('It is here already')).toBeVisible();
  await expect(places.getByRole('radio', { name: 'Acme', exact: true })).toHaveCount(0);
  await places.getByRole('radio', { name: 'Payments' }).check();

  // The plan, in words, before anything moves.
  await expect(dialog.getByText('Cart rewrite moves from Checkout to Payments.')).toBeVisible();
  await expect(dialog.getByText('On the 1 page inside it that follows the defaults: 1 person gains access, 1 person loses it.')).toBeVisible();
  await expect(dialog.getByText('Roles it has from the place above: 1 person starts to hold one, 1 person stops.')).toBeVisible();
  await expect(dialog.getByText('Linus Pauling').first()).toBeVisible();
  await expect(dialog.getByText('gains View Topic, Interact Topic on 2 pages')).toBeVisible();
  await expect(dialog.getByText('loses View Topic, Interact Topic on 1 page')).toBeVisible();
  await expect(dialog.getByText('env.recycle_idle_minutes')).toBeVisible();
  await expect(dialog.getByText('a lock at Checkout decides it')).toBeVisible();
  await expect(dialog.getByText('Its threads, its documents and the homes of its environments go with it.')).toBeVisible();
  // The dialog is modal, so the tree behind it is found by its markup and not by its role: nothing has moved yet.
  await expect(page.locator(`[data-tree-row="${ACME.cartRewrite}"]`)).toHaveAttribute('aria-level', '3');

  await dialog.getByRole('button', { name: 'Move Cart rewrite to Payments' }).click();
  await expect(notices(page).getByText('Cart rewrite moved to Acme / Shop / Checkout / Payments', { exact: true })).toBeVisible();
  await expect(dialog).toHaveCount(0);

  // The tree has followed: the topic stands one level deeper, and the screen names the folders on its way.
  await expect(tree(page).getByRole('treeitem', { name: /^Cart rewrite/ })).toHaveAttribute('aria-level', '4');
  await expect(crumbs(page)).toContainText('Payments');
  await expect(main(page).getByRole('link', { name: 'Shop' })).toBeVisible();
  await expect(main(page).getByRole('link', { name: 'Checkout' })).toBeVisible();
  await expect(main(page).getByRole('link', { name: 'Payments' })).toBeVisible();

  // And out of the folder, into the project, which has no plan to speak of.
  await screenMenu(page, 'Cart rewrite');
  await page.getByRole('menuitem', { name: 'Move to…' }).click();
  await dialog.getByRole('radio', { name: 'Shop' }).check();
  await expect(dialog.getByText('Nothing changes for anybody: no access, no setting and no address.')).toBeVisible();
  await dialog.getByRole('button', { name: 'Move Cart rewrite to Shop' }).click();
  await expect(notices(page).getByText('Cart rewrite moved to Acme / Shop', { exact: true })).toBeVisible();
  await expect(tree(page).getByRole('treeitem', { name: /^Cart rewrite/ })).toHaveAttribute('aria-level', '2');
});

test('a folder is moved into another folder, to a project and to an org, and is refused into itself and into what lies in it', async ({ page }) => {
  const receipts = project('Receipts', ACME.shop);
  await open(page, `/project/${ACME.checkout}`);
  await changeTree([receipts]);
  await expect(row(page, 'Receipts')).toBeVisible();

  // Checkout may not go where it is, into itself, or into the folder that lies in it.
  await screenMenu(page, 'Checkout');
  await page.getByRole('menuitem', { name: 'Move to…' }).click();
  const dialog = page.getByRole('dialog', { name: /^Move the folder Checkout, in / });
  await expect(dialog).toContainText('To another org, or into another project or folder, of the same tree.');
  const places = dialog.getByRole('radiogroup', { name: 'Move to' });
  for (const name of ['Shop', 'Checkout', 'Payments']) await expect(places.getByRole('radio', { name })).toBeDisabled();
  await expect(places.getByText('It is here already')).toBeVisible();
  await expect(places.getByText('It lies inside what is moved')).toHaveCount(2);
  await expect(places.getByRole('radio', { name: 'Receipts' })).toBeEnabled();
  // Into the folder beside it: its plan is read, and then it is done.
  await places.getByRole('radio', { name: 'Receipts' }).check();
  await expect(dialog.getByText('Checkout moves from Shop to Receipts.')).toBeVisible();
  await dialog.getByRole('button', { name: 'Move Checkout to Receipts' }).click();
  await expect(notices(page).getByText('Checkout moved to Acme / Shop / Receipts', { exact: true })).toBeVisible();
  expect(await level(page, 'Checkout')).toBe('3');
  expect(await level(page, 'Payments')).toBe('4');

  // A folder is called one by where it lies: archiving it says it holds folders.
  await row(page, 'Checkout').click({ button: 'right' });
  await page.getByRole('menu', { name: 'Menu of Checkout' }).getByRole('menuitem', { name: 'Archive…' }).click();
  await expect(page.getByRole('alertdialog', { name: /^Archive the \w+ Checkout, in .+\?$/ })).toContainText('Its folders, topics and threads become read-only');
  await page.keyboard.press('Escape');

  // Out of every project to the org itself, where it is a project like the others and is no folder any more.
  await screenMenu(page, 'Checkout');
  await page.getByRole('menuitem', { name: 'Move to…' }).click();
  await dialog.getByRole('radio', { name: 'Acme', exact: true }).check();
  await expect(dialog.getByText('Checkout moves from Receipts to Acme.')).toBeVisible();
  await dialog.getByRole('button', { name: 'Move Checkout to Acme' }).click();
  await expect(notices(page).getByText('Checkout moved to Acme', { exact: true })).toBeVisible();
  await expect(tree(page).getByRole('treeitem', { name: /^Checkout/ })).toHaveAttribute('aria-level', '1');
  await row(page, 'Checkout').click({ button: 'right' });
  await page.getByRole('menu', { name: 'Menu of Checkout' }).getByRole('menuitem', { name: 'Archive…' }).click();
  await expect(page.getByRole('alertdialog', { name: /^Archive the \w+ Checkout, in .+\?$/ })).toContainText('Its topics and threads become read-only');
  await expect(page.getByRole('alertdialog', { name: /^Archive the \w+ Checkout, in .+\?$/ })).not.toContainText('folders');
});

test('how deep folders may lie is refused in words, where one is made and where one is moved', async ({ page }) => {
  // Shop, Checkout and Payments are three projects one inside another; five more make eight, which is all a path holds.
  let above = ACME.payments;
  const chain: ReturnType<typeof project>[] = [];
  for (const name of ['Level 4', 'Level 5', 'Level 6', 'Level 7', 'Level 8']) {
    const made = project(name, above);
    chain.push(made);
    above = made.id;
  }
  const deepest = chain.at(-1);
  if (deepest === undefined) throw new Error('no folder was made');
  await open(page, `/project/${ACME.shop}`);
  await changeTree(chain);
  await page.goto(`/project/${deepest.id}`);
  await expect(page.getByRole('heading', { name: 'Level 8' })).toBeVisible();

  // Making one more is refused, and the dialog stays with what was typed, and says why.
  await screenMenu(page, 'Level 8');
  await page.getByRole('menuitem', { name: 'New folder…', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'New folder in Level 8' });
  await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('One too many');
  await dialog.getByRole('button', { name: 'Create folder' }).click();
  await expect(dialog.getByRole('alert')).toContainText('Folders cannot be nested any deeper here. A path holds at most 8 projects, one inside another.');
  await expect(dialog.getByRole('textbox', { name: 'Name', exact: true })).toHaveValue('One too many');
  await page.keyboard.press('Escape');

  // Moving a project there says it in the plan, before anything moves, and cannot be confirmed.
  await page.goto(`/project/${ACME.infra}`);
  await screenMenu(page, 'Infra');
  await page.getByRole('menuitem', { name: 'Move to…' }).click();
  const move = page.getByRole('dialog', { name: /^Move the project Infra, in / });
  await move.getByRole('radio', { name: 'Level 8' }).check();
  await expect(move.getByRole('alert')).toContainText('Folders cannot be nested any deeper here.');
  await expect(move.getByRole('button', { name: /^Move Infra to / })).toHaveAttribute('aria-disabled', 'true');
});

test('somebody who may not open topics in a project sees no “New folder”, and is told what it takes; a developer makes one, and is refused in words when the right was taken away', async ({ page }) => {
  await open(page, `/project/${ACME.shop}`, PEOPLE.vera);
  await expect(page.getByRole('heading', { name: 'Shop' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'New folder' })).toHaveCount(0);
  const createTopic = page.getByTestId('main').getByRole('button', { name: 'New topic', exact: true });
  await expect(createTopic).toHaveAttribute('aria-disabled', 'true');
  await createTopic.focus();
  await expect(page.getByRole('tooltip')).toContainText('You can’t take part in Shop. Ask Ada Lovelace, who owns it.');
  // Nor does the left bar offer it, on the folder or on the project: what they may not do is left out.
  await row(page, 'Checkout').click({ button: 'right' });
  const menu = page.getByRole('menu', { name: 'Menu of Checkout' });
  await expect(menu.getByRole('menuitem', { name: 'Open', exact: true })).toBeVisible();
  await expect(menu.getByRole('menuitem', { name: /New folder/ })).toHaveCount(0);
  await expect(menu.getByRole('menuitem', { name: /Move to/ })).toHaveCount(0);
  await page.keyboard.press('Escape');
  // The place itself keeps the act in its menu, switched off, with what it takes.
  await screenMenu(page, 'Shop');
  await expect(page.getByRole('menuitem', { name: /New folder/ })).toHaveAttribute('aria-disabled', 'true');
  await page.keyboard.press('Escape');

  // A developer opens topics there: she makes a folder, and may move a topic but not a folder.
  await page.getByRole('button', { name: 'Your account' }).click();
  await page.getByRole('menuitem', { name: 'Sign out' }).click();
  await page.waitForURL(/\/login/);
  await signIn(page, PEOPLE.dora);
  await page.goto(`/project/${ACME.shop}`);
  await screenMenu(page, 'Shop');
  await expect(page.getByRole('menuitem', { name: 'New folder…', exact: true })).not.toHaveAttribute('aria-disabled', 'true');
  await page.keyboard.press('Escape');
  await row(page, 'Cart rewrite').click({ button: 'right' });
  await expect(page.getByRole('menu', { name: 'Menu of Cart rewrite' }).getByRole('menuitem', { name: 'Move to…' })).toBeVisible();
  await page.keyboard.press('Escape');
  await row(page, 'Checkout').click({ button: 'right' });
  const checkout = page.getByRole('menu', { name: 'Menu of Checkout' });
  await expect(checkout.getByRole('menuitem', { name: 'New folder…' })).toBeVisible();
  await expect(checkout.getByRole('menuitem', { name: /Move to/ })).toHaveCount(0);
  await page.keyboard.press('Escape');

  // Somebody takes the right away after the page was loaded: the server says what is missing and whom to ask.
  await deny('dora', ['project.interact']);
  await screenMenu(page, 'Shop');
  await page.getByRole('menuitem', { name: 'New folder…', exact: true }).click();
  await page.getByRole('dialog').getByRole('textbox', { name: 'Name', exact: true }).fill('Not allowed');
  await page.getByRole('dialog').getByRole('button', { name: 'Create folder' }).click();
  const refusal = page.getByRole('dialog').getByRole('alert');
  await expect(refusal).toContainText('You do not have access to this.');
  await expect(refusal.getByText('Interact Project')).toBeVisible();
  await expect(refusal).toContainText('on Shop');
  await expect(refusal).toContainText('Ask Ada Lovelace.');
});

test('a closed row carries the approvals that wait in what it hides, and an open row does not', async ({ page }) => {
  await open(page, '/org/acme');
  await changeTree([{ id: ACME.feeTable, thread: { state: 'waiting_approval', blocked_reason: null, pending_approvals: 2, activity_seq: 2, last_activity_at: '2026-10-11T10:00:00.000Z' } }]);

  // The topic is closed, and carries what its thread waits for; the folders above it are open, and carry nothing.
  await expect(row(page, 'Card fees')).toContainText('2 approvals wait inside');
  await expect(row(page, 'Payments')).not.toContainText('approvals wait inside');
  await expect(row(page, 'Shop')).not.toContainText('approvals wait inside');

  // Closing each in turn takes the sum up to it: the folder, the folder above it, the project.
  await row(page, 'Payments').focus();
  await page.keyboard.press('ArrowLeft');
  await expect(row(page, 'Payments')).toContainText('2 approvals wait inside');
  await row(page, 'Checkout').focus();
  await page.keyboard.press('ArrowLeft');
  await expect(row(page, 'Checkout')).toContainText('2 approvals wait inside');
  await expect(row(page, 'Card fees')).toHaveCount(0);
  await row(page, 'Shop').focus();
  await page.keyboard.press('ArrowLeft');
  await expect(row(page, 'Shop')).toContainText('2 approvals wait inside');
  await expect(row(page, 'Checkout')).toHaveCount(0);

  // Opened again, the sum goes back down to where it is.
  await page.keyboard.press('ArrowRight');
  await expect(row(page, 'Shop')).not.toContainText('approvals wait inside');
  await expect(row(page, 'Checkout')).toContainText('2 approvals wait inside');
});

test('a topic names the folders it lies in, and its project names what it holds: the folders as cards above the topics, and the topics of its own', async ({ page }) => {
  await open(page, `/topic/${ACME.cardFees}`);
  await expect(page.getByRole('heading', { name: 'Card fees' })).toBeVisible();
  const where = main(page).getByRole('definition').filter({ has: page.getByRole('link', { name: 'Payments' }) });
  await expect(where.getByRole('link', { name: 'Shop' })).toBeVisible();
  await expect(where.getByRole('link', { name: 'Checkout' })).toBeVisible();
  await expect(where.getByRole('link', { name: 'Payments' })).toBeVisible();
  await where.getByRole('link', { name: 'Checkout' }).click();

  // Checkout holds a topic and a folder: the folder stands as a card above the topic's table.
  await expect(page.getByRole('heading', { name: 'Checkout', level: 1 })).toBeVisible();
  await expect(main(page).getByText('1 topic', { exact: true })).toBeVisible();
  await expect(main(page).getByText('1 folder', { exact: true })).toBeVisible();
  const folders = main(page).getByRole('region', { name: 'Folders' });
  await expect(folders.getByRole('link', { name: 'Payments' })).toBeVisible();
  await expect(folders.getByText('Cards, fees and refunds')).toBeVisible();
  await expect(folders.getByText('1 topic · 0 folders')).toBeVisible();
  await expect(main(page).getByRole('table', { name: 'Topics' }).getByRole('link')).toHaveText(['Cart rewrite']);

  // The same above the board; and a search that matches no folder says so.
  await page.getByRole('radio', { name: 'Board' }).click();
  await expect(folders.getByRole('link', { name: 'Payments' })).toBeVisible();
  await page.getByRole('searchbox', { name: 'Search topics' }).fill('zzz');
  await expect(folders.getByText('No folder matches the filters.')).toBeVisible();
  await page.getByRole('searchbox', { name: 'Search topics' }).fill('pay');
  await expect(folders.getByRole('link', { name: 'Payments' })).toBeVisible();
});

test('the Activity of a project takes in its folders, says so, and says in which of them each line was', async ({ page }) => {
  await open(page, `/project/${ACME.shop}?view=activity`);
  await expect(page.getByText('Shop and the 2 folders in it')).toBeVisible();
  await expect(page.getByText('Ada Lovelace made the folder Payments')).toBeVisible();
  await expect(page.getByText('in Checkout', { exact: false }).first()).toBeVisible();
  await expect(page.getByText('Ada Lovelace made the folder Checkout')).toBeVisible();
  await expect(page.getByText('Ada Lovelace made the project Shop')).toBeVisible();

  // A folder with no folders in it takes in nothing, and says nothing of it.
  await page.goto(`/project/${ACME.payments}?view=activity`);
  await expect(page.getByText('Ada Lovelace made the folder Payments')).toBeVisible();
  await expect(page.getByText(/folders? in it/)).toHaveCount(0);
});

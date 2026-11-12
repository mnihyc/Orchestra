/** The workbench at work: a tree that follows its org as it changes, views put where their reader wants them, and an arrangement that is there again the next time. */
import { expect, type Page, test } from '@playwright/test';

import { ACME, changeTree, dropConnections, endTokens, PEOPLE, scenario, signIn } from './support.ts';
import { showWorkbenchView } from './workbench-support.ts';

test.beforeEach(async () => {
  await scenario();
});

const rail = (page: Page) => page.getByRole('navigation', { name: 'Orgs and your own places' });
const tree = (page: Page) => page.getByRole('tree', { name: 'Orgs, projects, topics and threads' });
const tabs = (page: Page, area: string) => page.getByRole('tablist', { name: area });
const said = (page: Page) => page.locator('[id^="DndLiveRegion"]');

/**
 * Waits until the page is at rest: what a key did is drawn and said a few turns of its event loop after the key, and on a busy machine that
 * is later than the test's next line. A hand presses the next key when it has seen what the last did.
 */
const atRest = (page: Page): Promise<void> =>
  page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => requestIdleCallback(() => resolve(), { timeout: 2_000 }))));

/** Signs in and brings the team's org into the left bar. */
async function openAcme(page: Page): Promise<void> {
  await signIn(page);
  // Signing in already opens Acme; switch from the personal place to exercise the org choice.
  await page.goto('/org/ada');
  await page.waitForLoadState('networkidle');
  await rail(page).getByRole('link', { name: 'Acme' }).click();
  await expect(page).toHaveURL(/\/org\/acme\/?(\?list=[^&]*&_list=true)?$/);
  await expect(tree(page).getByRole('treeitem', { name: /Fix login/ })).toBeVisible();
}

const project = (n: number, name: string, more: Record<string, unknown> = {}) => ({
  id: `00000000-0000-4000-8777-${String(n).padStart(12, '0')}`,
  kind: 'project',
  name,
  parent_id: ACME.org,
  ...more,
});

test('the tree of an org shows what its threads are doing, and follows them without a reload', async ({ page }) => {
  await openAcme(page);
  const fixLogin = tree(page).getByRole('treeitem', { name: /Fix login/ });

  // A topic shows the most urgent thing among its threads; a thread shows its own state. A decision that waits is the
  // most urgent thing in this one, and is said by how many wait.
  await expect(fixLogin.getByText('1 approval waits inside')).toBeAttached();
  await fixLogin.focus();
  await page.keyboard.press('ArrowRight');
  const main = tree(page).getByRole('treeitem', { name: 'Main thread' }).first();
  await expect(main.getByRole('img', { name: 'Running' })).toBeVisible();
  await expect(tree(page).getByRole('treeitem', { name: /Repro/ }).getByText('1 approval waits')).toBeAttached();
  await expect(tree(page).getByRole('treeitem', { name: /Payroll/ }).getByRole('img', { name: 'Not everybody here can see this' })).toBeVisible();

  // Something happens on the server: the row follows.
  const blocked = { state: 'blocked', blocked_reason: 'error', pending_approvals: 0, activity_seq: 2, last_activity_at: null };
  await changeTree([{ id: ACME.mainThread, thread: blocked }]);
  // A blocked thread's mark names its reason beside its state.
  await expect(main.getByRole('img', { name: 'Blocked · error', exact: true })).toBeVisible();
  // Contained work has one priority on every place: a decision that waits outranks a block, so the topic keeps saying
  // that one waits while Repro's approval is open, and shows the block once nothing waits.
  await expect(fixLogin.getByRole('img', { name: 'Waiting for approval inside', exact: true })).toBeVisible();
  await changeTree([{ id: ACME.repro, thread: { state: 'idle', blocked_reason: null, pending_approvals: 0, activity_seq: 3, last_activity_at: null } }]);
  await expect(fixLogin.getByRole('img', { name: 'Blocked inside', exact: true })).toBeVisible();

  // A project is made, and one is deleted.
  await changeTree([project(1, 'Launch'), { id: ACME.payroll, deleted: true }]);
  await expect(tree(page).getByRole('treeitem', { name: 'Launch' })).toBeVisible();
  await expect(tree(page).getByRole('treeitem', { name: /Payroll/ })).toHaveCount(0);
});

test('a frame that is lost on the way is made up for, and a connection that went comes back with what changed meanwhile', async ({ page }) => {
  await openAcme(page);

  // The frame that would bring the first project never arrives; the next one does.
  await changeTree([project(2, 'Lost in transit')], true);
  await changeTree([project(3, 'Arrived')]);
  await expect(tree(page).getByRole('treeitem', { name: 'Arrived' })).toBeVisible();
  await expect(tree(page).getByRole('treeitem', { name: 'Lost in transit' })).toBeVisible();

  // The server restarts; the org goes on changing while nobody is connected.
  await dropConnections();
  await changeTree([project(4, 'While away')], true);
  await expect(tree(page).getByRole('treeitem', { name: 'While away' })).toBeVisible({ timeout: 15_000 });
  // Nothing says "reconnecting" any more once the connection is back.
  await expect(page.getByText('Reconnecting…')).toHaveCount(0);
});

test('a token that ended is renewed behind the reader, who goes on as if nothing had happened', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/ada');
  await page.waitForLoadState('networkidle');
  await endTokens();
  const renewed = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname.endsWith('/auth/refresh'));
  await rail(page).getByRole('link', { name: 'Acme' }).click();
  expect((await renewed).status()).toBe(200);

  await expect(page).toHaveURL(/\/org\/acme\/?(\?list=[^&]*&_list=true)?$/);
  await expect(tree(page).getByRole('treeitem', { name: /Fix login/ })).toBeVisible();
});

test('a session that ended elsewhere leads to the sign-in, and back to where the reader was', async ({ page }) => {
  await signIn(page);
  await page.goto('/approvals');
  await expect(page.getByText('Nothing waits for you')).toBeVisible();

  // Every session is gone on the server, as after a password was reset on another device.
  await scenario();

  await expect(page).toHaveURL(/\/login\?redirect=%2Fapprovals$/, { timeout: 20_000 });
  await page.getByLabel('Username or email address').fill(PEOPLE.ada.login);
  await page.getByLabel('Password', { exact: true }).fill(PEOPLE.ada.password);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page).toHaveURL(/\/approvals$/);
});

/** Conversation, Code and Review are explicit choices from the Layout menu: opening a thread never swaps the device's workspace. */
const chooseLayout = async (page: Page, name: string): Promise<void> => {
  await page.getByRole('button', { name: 'Layout', exact: true }).click();
  await page.getByRole('menuitem', { name, exact: true }).click();
};

test('a thread is an ordinary tab of the main area, and choosing Conversation puts its other views at the right with Outline in front, each a view that is built', async ({ page }) => {
  await openAcme(page);
  await page.goto(`/thread/${ACME.mainThread}`);

  // Where it lies, from the root org down to its parent; the tab names the thread.
  const crumbs = page.getByRole('navigation', { name: 'Where this is' });
  await expect(crumbs).toContainText('Acme');
  await expect(crumbs).toContainText('Fix login');
  await expect(crumbs.getByText('Main thread')).toHaveCount(0);
  const wide = page.viewportSize();
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(crumbs.locator('a:visible')).toHaveCount(1);
  await expect(crumbs.getByRole('link', { name: 'Fix login', exact: true })).toBeVisible();
  await expect(crumbs.getByRole('button', { name: 'Ancestor places' })).toHaveCount(0);
  if (wide !== null) await page.setViewportSize(wide);

  const main = tabs(page, 'Main area');
  await expect(main.getByRole('tab', { name: 'Main thread' })).toHaveAttribute('aria-selected', 'true');
  // The conversation stands in the middle, with the box to write in under it.
  await expect(page.getByRole('region', { name: 'Main area' }).getByRole('textbox', { name: 'Message to the agent' })).toBeVisible();
  // Opening a thread did not rearrange the device's workspace: its right bar is still the one the person had.
  await expect(page.locator('[data-workbench-bar="right"]').getByRole('tab', { name: 'Team', exact: true })).toHaveAttribute('aria-selected', 'true');
  const opened = await main.getByRole('tab').evaluateAll((all) => all.map((tab) => tab.id));

  // Conversation is a choice: it sets that frame's bars and puts the thread's other views at the right, Outline in front.
  await chooseLayout(page, 'Conversation');
  await expect(page.locator('[data-workbench-bar="right"]').getByRole('button', { name: 'Outline', exact: true })).toHaveAttribute('aria-expanded', 'true');
  expect(await main.getByRole('tab').evaluateAll((all) => all.map((tab) => tab.id))).toEqual(opened);
  await expect(main.getByRole('tab', { name: 'Main thread' })).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByTestId('outline-panel')).toBeVisible();

  // The row of the thread is marked in the tree, with everything above it open.
  await expect(tree(page).getByRole('treeitem', { name: 'Main thread' }).first()).toHaveAttribute('aria-current', 'page');

  // Another view of the thread is one press away, and a view that is not built is not offered.
  await showWorkbenchView(page, 'Environment');
  await expect(page.getByTestId('environment-panel')).toBeVisible();
  await showWorkbenchView(page, 'People');
  await expect(page.locator('[data-workbench-bar="right"]').getByRole('button', { name: 'People', exact: true })).toHaveAttribute('aria-expanded', 'true');
});

test('a view is moved from its menu, by keyboard and put back, and the arrangement is there again after a reload', async ({ page }) => {
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  const side = page.locator('[data-workbench-bar="right"]');
  await chooseLayout(page, 'Conversation');
  const outline = side.getByRole('button', { name: 'Outline', exact: true });
  await outline.focus();
  await page.keyboard.press('Shift+F10');
  await page.getByRole('menuitem', { name: 'Move to', exact: true }).press('ArrowRight');
  const current = page.getByRole('menuitem', { name: 'Conversation', exact: true });
  await expect(current).toHaveAttribute('aria-disabled', 'true');
  await expect(current).toHaveAccessibleDescription(/Right bar/);
  await page.getByRole('menuitem', { name: 'Bottom', exact: true }).press('Enter');
  await expect(tabs(page, 'Bottom').getByRole('tab', { name: /^Outline\b/ })).toBeVisible();
  await showWorkbenchView(page, 'Chat');
  const chat = side.getByRole('button', { name: 'Chat', exact: true });
  await chat.focus(); await page.keyboard.press('Space'); await atRest(page);
  await expect(said(page)).toHaveText('Picked up Chat.');
  await page.keyboard.press('Escape');
  await expect(said(page)).toHaveText('Put back where it was.');
  await chat.focus(); await page.keyboard.press('Shift+F10');
  await page.getByRole('menuitem', { name: 'Move to', exact: true }).press('ArrowRight');
  await page.getByRole('menuitem', { name: 'Main area', exact: true }).press('Enter');
  await expect(tabs(page, 'Main area').getByRole('tab', { name: /^Chat\b/ })).toBeVisible();
  await page.waitForTimeout(500);
  await page.reload();
  await expect(tabs(page, 'Bottom').getByRole('tab', { name: /^Outline\b/ })).toBeVisible();
  await expect(tabs(page, 'Main area').getByRole('tab', { name: /^Chat\b/ })).toBeVisible();
  await page.goto(`/thread/${ACME.repro}`);
  await expect(tabs(page, 'Bottom').getByRole('tab', { name: /^Outline\b/ })).toBeVisible();
  await page.goto('/inbox');
  await expect(page.getByRole('tab', { name: /^Outline\b/ })).toHaveCount(0);
  await page.goto(`/thread/${ACME.mainThread}`);
  await page.getByRole('button', { name: 'Layout', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Reset layout', exact: true }).click();
  await expect(side.getByRole('button', { name: 'Outline', exact: true })).toHaveAttribute('aria-expanded', 'true');
  await expect(tabs(page, 'Bottom')).toHaveCount(0);
});

test('choosing Code sets its frame and adds Files without closing or moving the thread, and Reset layout restores the standard frame with the main tabs and their drafts kept', async ({ page }) => {
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  const main = tabs(page, 'Main area');
  const thread = main.getByRole('tab', { name: /Main thread/ });
  await expect(thread).toHaveAttribute('aria-selected', 'true');
  const box = page.getByRole('region', { name: 'Main area' }).getByRole('textbox', { name: 'Message to the agent' });
  await box.fill('A draft the layouts must keep');
  const ids = () => main.getByRole('tab').evaluateAll((all) => all.map((tab) => tab.id));
  const before = await ids();

  // Code is a choice from the Layout menu. It sets that frame's bars and adds its views.
  await chooseLayout(page, 'Code');
  await expect(main.getByRole('tab', { name: 'Files', exact: true })).toBeVisible();
  await expect(tabs(page, 'Bottom').getByRole('tab', { name: 'Terminal', exact: true })).toHaveAttribute('aria-selected', 'true');
  const side = page.locator('[data-workbench-bar="right"]');
  await expect(side).toHaveAttribute('data-active-container', 'conversation');
  await expect(side.getByRole('button', { name: 'Outline', exact: true })).toBeVisible();
  // It can never close or move a tab that is already in the main area: the thread's own tab is still there, in its order, and selected.
  const after = await ids();
  expect(after.filter((id) => before.includes(id))).toEqual(before);
  await expect(thread).toHaveAttribute('aria-selected', 'true');
  await expect(box).toHaveValue('A draft the layouts must keep');

  // Reset layout restores the standard frame and gathers the existing main tabs, with their content and drafts.
  await page.getByRole('button', { name: 'Layout', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Reset layout', exact: true }).click();
  await expect(tabs(page, 'Bottom')).toHaveCount(0);
  await expect(side.getByRole('button', { name: 'Outline', exact: true })).toHaveAttribute('aria-expanded', 'true');
  expect(await ids()).toEqual(after);
  await expect(thread).toHaveAttribute('aria-selected', 'true');
  await expect(box).toHaveValue('A draft the layouts must keep');
});

test('the bars are shown and hidden by their keys and by the switches in the title, and a view that was closed is added again', async ({ page }) => {
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await showWorkbenchView(page, 'Terminal');
  await page.getByRole('button', { name: 'Bottom panel', exact: true }).click();
  await showWorkbenchView(page, 'History');
  await showWorkbenchView(page, 'Outline');
  await expect(page.getByRole('button', { name: 'Bottom panel', exact: true })).toBeEnabled();
  const left = page.locator('[data-workbench-bar="left"]');
  const side = page.locator('[data-workbench-bar="right"]');
  await expect(left).toBeVisible();
  await page.keyboard.press('Control+\\');
  await expect(left).toHaveCount(0);
  await page.getByRole('button', { name: 'Left bar', exact: true }).click();
  await expect(left).toBeVisible();
  await page.keyboard.press('Control+Shift+Period');
  await expect(side).toHaveCount(0);
  await page.keyboard.press('Control+Shift+Period');
  await expect(side).toBeVisible();
  const temporary = page.getByRole('dialog', { name: 'Right bar', exact: true });
  await expect(temporary).toHaveCount(0);
  await page.keyboard.press('Control+Shift+Period');
  await expect(side).toHaveCount(0);
  await page.locator('[data-bar-edge="right"]').hover();
  await expect(temporary).toBeVisible();
  await temporary.getByRole('button', { name: 'Keep open as a bar', exact: true }).click();
  await expect(temporary).toHaveCount(0);
  await expect(side).toBeVisible();
  await page.getByRole('button', { name: 'Bottom panel', exact: true }).click();
  await expect(tabs(page, 'Bottom').getByRole('tab', { name: 'Terminal', exact: true })).toBeVisible();
  await side.getByRole('button', { name: 'Actions for Outline', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Hide', exact: true }).click();
  await expect(side.getByRole('button', { name: 'Outline', exact: true })).toHaveCount(0);
  await showWorkbenchView(page, 'Outline');
  await expect(side.getByRole('button', { name: 'Outline', exact: true })).toHaveAttribute('aria-expanded', 'true');
  await page.keyboard.press('F1');
  await expect(side).toHaveAttribute('data-active-container', 'help');
  await page.locator('body').press('?');
  await expect(page.getByRole('dialog', { name: 'Keyboard shortcuts' })).toBeVisible();
  await page.keyboard.press('Escape');
});

test('a path that no route answers shows that there is nothing there, inside the workbench', async ({ page }) => {
  await signIn(page);
  await page.goto('/nowhere/at/all');

  await expect(page.getByText('There is nothing at this address')).toBeVisible();
  await expect(rail(page)).toBeVisible();
  // Home leads to where the person signed in: the org they last had in view.
  await page.getByRole('link', { name: 'Home', exact: true }).last().click();
  await expect(page).toHaveURL(/\/org\/acme\/?(\?list=[^&]*&_list=true)?$/);
});

test('a preference is the reader\'s own: it shows at once, and is there again on the next visit', async ({ page }) => {
  await signIn(page);

  // The language is chosen on Interface; the account's menu keeps the theme.
  await page.goto('/settings/interface');
  await page.getByRole('combobox', { name: 'Language' }).click();
  await page.getByRole('option', { name: '简体中文' }).click();
  await expect(page.locator('html')).toHaveAttribute('lang', 'zh-CN');
  await expect(page.getByRole('link', { name: '收件箱' }).first()).toBeVisible();

  await page.getByRole('button', { name: '你的账号' }).click();
  await page.getByRole('menuitemradio', { name: '深色' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await page.keyboard.press('Escape');

  // The server holds it: a browser that never saw this person shows it as they left it.
  await page.evaluate(() => localStorage.clear());
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await expect(page.locator('html')).toHaveAttribute('lang', 'zh-CN');
});

test('a small screen shows one thing at a time: the screen, the navigation as a drawer, another view as a sheet', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 800 });
  await openAcmeSmall(page);

  // Five places at the foot, and no areas to arrange.
  const foot = page.getByRole('navigation', { name: 'Orgs and your own places' });
  for (const place of ['Home', 'Inbox', 'Messages', 'Approvals']) await expect(foot.getByRole('link', { name: place })).toBeVisible();
  await expect(foot.getByRole('button', { name: 'Me' })).toBeVisible();
  // (The strips of the areas: a phone has no bar or bottom to arrange; it keeps one strip of the pages that are open, named for its main area.)
  await expect(page.getByRole('tablist', { name: /^(Left bar|Right bar|Bottom)$/ })).toHaveCount(0);
  await expect(page.getByRole('tablist', { name: 'Main area', exact: true })).toHaveCount(1);

  // The navigation is a drawer; going somewhere puts it away.
  // The phone's containers include Resource navigation, which also has the word.
  await page.getByRole('button', { name: 'Navigation', exact: true }).click();
  const drawer = page.getByRole('dialog', { name: 'Navigation' });
  await expect(drawer.getByRole('treeitem', { name: /Fix login/ })).toBeVisible();
  await drawer.getByRole('link', { name: 'Fix login' }).click();
  await expect(page).toHaveURL(new RegExp(`/topic/${ACME.fixLogin}$`));
  await expect(drawer).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Fix login' })).toBeVisible();

  // Another view of the topic opens as a sheet over it.
  await page.locator('[data-narrow-containers]').getByRole('button', { name: 'Team', exact: true }).click();
  await expect(page.getByRole('dialog', { name: 'Team', exact: true }).getByText('Say something to the people on this topic.', { exact: true })).toBeVisible();
});

async function openAcmeSmall(page: Page): Promise<void> {
  await signIn(page);
  await page.goto('/org/acme');
  await expect(page.getByRole('heading', { name: 'Acme' })).toBeVisible();
}

test('the activity toolbar marks only Resources, toggles its bar, and switches by keyboard', async ({ page }) => {
  await openAcme(page);
  const activity = rail(page).getByRole('toolbar');
  const resources = activity.getByRole('button', { name: 'Resource navigation', exact: true });
  await resources.click();
  await expect(resources).toHaveAttribute('aria-pressed', 'true');
  await expect(activity.locator('[data-active-left-view]')).toHaveCount(1);
  await expect(page.locator('[data-workbench-bar="left"] [role="tablist"]')).toHaveCount(0);
  await resources.press('Enter');
  await expect(resources).toHaveAttribute('aria-pressed', 'false');
  await expect(page.locator('[data-workbench-bar="left"]')).toHaveCount(0);
  await resources.press('Enter');
  await expect(resources).toHaveAttribute('aria-pressed', 'true');
  await resources.press('Home');
  const first = activity.locator('a[href],button').first();
  await expect(first).toBeFocused();
  await first.press('ArrowDown');
  await expect(activity.locator('a[href],button').nth(1)).toBeFocused();
  await activity.getByRole('link', { name: 'Acme', exact: true }).press('Enter');
  await expect(activity.getByRole('link', { name: 'Acme', exact: true })).toHaveAttribute('data-active-left-view', 'navigation');
  await expect(resources).toHaveAttribute('aria-pressed', 'false');
  await expect(activity.locator('[data-active-left-view]')).toHaveCount(1);
});

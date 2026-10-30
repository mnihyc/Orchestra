/** The places of an org at work: an org made inside an org, a project and a topic in it, a name changed, archiving and bringing back, what a refusal says is missing, the board and its filters, an invitation in both forms, and a role changed. */
import { expect, type Page, test } from '@playwright/test';

import { ACME, deny, mailWorks, PEOPLE, scenario, signIn } from './support.ts';

test.beforeEach(async () => {
  await scenario();
});

const tree = (page: Page) => page.getByRole('tree', { name: 'Orgs, projects, topics and threads' });
const notices = (page: Page) => page.getByRole('region', { name: /^Notifications/ });
const dialogField = (page: Page, name: RegExp | string) => page.getByRole('dialog').getByRole('textbox', { name });

/** Signs in and goes to a screen by its address. */
async function open(page: Page, path: string, person: { login: string; password: string } = PEOPLE.ada): Promise<void> {
  await signIn(page, person);
  await page.goto(path);
}

test('an org is made inside an org, then a project in it and a topic in that, each leading to its own screen', async ({ page }) => {
  await open(page, '/org/acme');

  // A department: its short name follows its name until the person writes one.
  await page.getByRole('button', { name: 'Actions for Acme', exact: true }).last().click();
  await page.getByRole('menuitem', { name: 'New department…', exact: true }).click();
  const department = page.getByRole('dialog', { name: 'New department in Acme' });
  await expect(department).toBeVisible();
  await department.getByRole('textbox', { name: 'Name', exact: true }).fill('Platform Team');
  await expect(department.getByRole('textbox', { name: 'Short name' })).toHaveValue('platform-team');
  await expect(department.getByText('Its address will be acme.platform-team.')).toBeVisible();
  await department.getByRole('button', { name: 'Create department' }).click();

  // It lies at its address, and the left bar shows it at once.
  await expect(page).toHaveURL(/\/org\/acme\.platform-team$/);
  await expect(page.getByRole('heading', { name: 'Platform Team' })).toBeVisible();
  await expect(page.getByTestId('main').getByText('acme.platform-team', { exact: true })).toHaveCount(0);
  await expect(tree(page).getByRole('treeitem', { name: /Platform Team/ })).toBeVisible();
  await expect(page.getByRole('navigation', { name: 'Where this is' })).toContainText('Platform Team');

  // A project in the department.
  await page.getByRole('button', { name: 'New project', exact: true }).first().click();
  await expect(page.getByRole('dialog', { name: 'New project in Platform Team' })).toBeVisible();
  await dialogField(page, /^Name/).fill('Pipelines');
  await dialogField(page, /^Description/).fill('Builds and releases');
  await page.getByRole('dialog').getByRole('button', { name: 'Create project' }).click();
  await expect(page).toHaveURL(/\/project\//);
  await expect(page.getByRole('heading', { name: 'Pipelines' })).toBeVisible();
  await page.getByTestId('main').getByRole('button', { name: 'Description and tags', exact: true }).click();
  await expect(page.getByText('Builds and releases')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByText('No topics yet')).toBeVisible();

  // A topic in the project, with a description.
  await page.getByRole('button', { name: 'New topic', exact: true }).first().click();
  await expect(page.getByRole('dialog', { name: 'New topic in Pipelines' })).toBeVisible();
  await dialogField(page, /^Name/).fill('Speed up CI');
  await dialogField(page, /^Description/).fill('Builds take too long');
  await page.getByRole('dialog').getByRole('button', { name: 'Create topic' }).click();
  await expect(page).toHaveURL(/\/topic\//);
  await expect(page.getByRole('heading', { name: 'Speed up CI' })).toBeVisible();
  await expect(page.getByRole('navigation', { name: 'Where this is' })).toContainText('Pipelines');

  // UA-11: one state chip is the menu trigger; the field label is not repeated on screen.
  let status = page.getByRole('button', { name: 'Change status: Open', exact: true });
  await expect(status).toHaveText('Open');
  await status.click();
  await page.getByRole('menuitemradio', { name: 'Done', exact: true }).click();
  status = page.getByRole('button', { name: 'Change status: Done', exact: true });
  await expect(status).toHaveText('Done');
  await status.click();
  await page.getByRole('menuitemradio', { name: 'Open', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Change status: Open', exact: true })).toHaveText('Open');

  // QS-034: a new topic has no deleted documents, so its list has no empty disclosure.
  const topicUrl = new URL(page.url());
  const topicId = topicUrl.pathname.split('/').at(-1) ?? '';
  const deletedRead = page.waitForResponse((response) => {
    const url = new URL(response.url());
    return url.pathname.endsWith(`/nodes/${topicId}/documents`) && url.searchParams.get('deleted') === 'only';
  });
  topicUrl.searchParams.set('tab', 'documents');
  await page.goto(topicUrl.toString());
  expect((await deletedRead).status()).toBe(200);
  await expect(page.getByRole('button', { name: /^Recently deleted/ })).toHaveCount(0);

  // The topic is in the project's list without another header count.
  await page.getByRole('navigation', { name: 'Where this is' }).getByRole('link', { name: 'Pipelines' }).click();
  await expect(page.getByRole('link', { name: 'Speed up CI' }).first()).toBeVisible();
  await expect(page.getByTestId('main').getByRole('table', { name: 'Topics' }).getByRole('link', { name: 'Speed up CI', exact: true })).toBeVisible();
});

test('a topic made without a name is called New topic, and a short name that is taken is said beside its field', async ({ page }) => {
  await open(page, `/project/${ACME.web}`);
  await page.getByRole('button', { name: 'New topic', exact: true }).first().click();
  await page.getByRole('dialog').getByRole('button', { name: 'Create topic' }).click();
  await expect(page).toHaveURL(/\/topic\//);
  await expect(page.getByRole('heading', { name: 'New topic' })).toBeVisible();

  await page.goto('/org/acme');
  await page.getByRole('button', { name: 'Actions for Acme', exact: true }).last().click();
  await page.getByRole('menuitem', { name: 'New department…', exact: true }).click();
  await page.getByRole('dialog').getByRole('textbox', { name: 'Name', exact: true }).fill('Research');
  await page.getByRole('dialog').getByRole('button', { name: 'Create department' }).click();
  // The org that has the short name is named, and a free one is offered.
  await expect(page.getByRole('dialog').getByText('Research has the short name research here already, and two orgs of one place cannot share an address. Choose another.')).toBeVisible();
  await expect(page.getByRole('dialog').getByRole('button', { name: 'Use research-2' })).toBeVisible();
  // The dialog stays, with what was typed, for the person to change.
  await expect(page.getByRole('dialog').getByRole('textbox', { name: 'Short name' })).toHaveValue('research');
});

test('a name is changed in a dialog and the screen, the tree and the path all say the new one', async ({ page }) => {
  await open(page, `/topic/${ACME.upgradeDeps}`);
  await expect(page.getByRole('heading', { name: 'Upgrade deps' })).toBeVisible();

  await page.getByRole('button', { name: 'Actions for Upgrade deps' }).first().click();
  await page.getByRole('menuitem', { name: 'Rename…' }).click();
  const dialog = page.getByRole('dialog', { name: /^Rename the topic Upgrade deps, in / });
  await dialogField(page, /^Name/).fill('Upgrade the dependencies');
  await dialog.getByRole('button', { name: 'Save' }).click();

  await expect(page.getByRole('heading', { name: 'Upgrade the dependencies' })).toBeVisible();
  await expect(tree(page).getByRole('treeitem', { name: /Upgrade the dependencies/ })).toBeVisible();
  await expect(page.getByRole('navigation', { name: 'Where this is' })).toContainText('Upgrade the dependencies');
  await expect(page.getByRole('dialog')).toHaveCount(0);
});

test('an archive says what it does before it does it, shows as the topic’s own banner, and is undone from there', async ({ page }) => {
  await open(page, `/topic/${ACME.upgradeDeps}`);

  await page.getByRole('button', { name: 'Actions for Upgrade deps' }).first().click();
  await page.getByRole('menuitem', { name: 'Archive…' }).click();
  const confirm = page.getByRole('alertdialog', { name: /^Archive the topic Upgrade deps, in .+\?$/ });
  await expect(confirm).toContainText('Archive makes these places, their chats and documents read-only.');
  await confirm.getByRole('button', { name: 'Archive Upgrade deps' }).click();

  await expect(page.getByText('This topic is archived. It is read-only until it is brought back.')).toBeVisible();
  // What is archived is out of the tree until the person asks to see it.
  await expect(tree(page).getByRole('treeitem', { name: /Upgrade deps/ })).toHaveCount(0);
  await expect(notices(page).getByText('Upgrade deps archived')).toBeVisible();

  await page.getByRole('button', { name: 'Unarchive', exact: true }).click();
  await expect(page.getByText('This topic is archived.')).toHaveCount(0);
  await expect(tree(page).getByRole('treeitem', { name: /Upgrade deps/ })).toBeVisible();
});

test('a deleted topic is found in Recently deleted, with until when, and comes back archived', async ({ page }) => {
  await open(page, `/topic/${ACME.upgradeDeps}`);

  await page.getByRole('button', { name: 'Actions for Upgrade deps' }).first().click();
  await page.getByRole('menuitem', { name: 'Delete…' }).click();
  const confirm = page.getByRole('alertdialog', { name: /^Delete the topic Upgrade deps, in .+\?$/ });
  // The server's plan says until when, and who may bring it back.
  await expect(confirm).toContainText(/Whoever manages Web can bring it back from “Recently deleted” until .+; then it is gone for good\./);
  await confirm.getByRole('button', { name: 'Delete Upgrade deps' }).click();

  // The screen it was on has nothing left to show: the person is taken to the project it lay in.
  await expect(page).toHaveURL(new RegExp(`/project/${ACME.web}$`));
  await expect(notices(page).getByText('Upgrade deps deleted')).toBeVisible();
  await expect(tree(page).getByRole('treeitem', { name: /Upgrade deps/ })).toHaveCount(0);

  await page.getByRole('button', { name: 'Actions for Web' }).first().click();
  await page.getByRole('menuitem', { name: 'Recently deleted…' }).click();
  const list = page.getByRole('dialog', { name: /^Recently deleted in the project Web, in / });
  await expect(list.getByText('Upgrade deps')).toBeVisible();
  await expect(list.locator('li').filter({ hasText: 'Upgrade deps' }).getByText(/Can be brought back until/)).toBeVisible();
  await list.locator('li').filter({ hasText: 'Upgrade deps' }).getByRole('button', { name: 'Bring it back', exact: true }).click();
  await page.getByRole('dialog', { name: 'Bring back the selected things' }).getByRole('button', { name: 'Bring back', exact: true }).click();
  // The dialog is modal, so the notices behind it are hidden from the accessibility tree: found by their text.
  await expect(page.getByText('Upgrade deps is back, archived').first()).toBeVisible();
  await expect(list.getByText('Upgrade deps', { exact: true })).toHaveCount(0);
});

test('a refusal says what is missing and whom to ask, beside the act that was refused', async ({ page }) => {
  await open(page, `/project/${ACME.web}`, PEOPLE.dora);
  await expect(page.getByRole('button', { name: 'New topic', exact: true }).first()).toBeVisible();

  // Somebody takes the right away after the page was loaded: the page still believes it, the server does not.
  await deny('dora', ['project.interact']);
  await page.getByRole('button', { name: 'New topic', exact: true }).first().click();
  await dialogField(page, /^Name/).fill('Not allowed');
  await page.getByRole('dialog').getByRole('button', { name: 'Create topic' }).click();

  const refusal = page.getByRole('dialog').getByRole('alert');
  await expect(refusal).toContainText('You do not have access to this.');
  await expect(refusal.getByText('Interact Project')).toBeVisible();
  await expect(refusal).toContainText('on Web');
  await expect(refusal).toContainText('Ask Ada Lovelace.');
  // Nothing was made.
  await expect(page).toHaveURL(new RegExp(`/project/${ACME.web}$`));
});

test('somebody who may only read sees what is off, switched off with what it takes, and nothing they could not use', async ({ page }) => {
  await open(page, `/project/${ACME.web}`, PEOPLE.vera);

  // The one header action stays discoverable and tells why it is unavailable on focus.
  const createTopic = page.getByTestId('main').getByRole('button', { name: 'New topic', exact: true });
  await expect(createTopic).toHaveAttribute('aria-disabled', 'true');
  await createTopic.focus();
  await expect(page.getByRole('tooltip')).toContainText('You can’t take part in Web. Ask Ada Lovelace, who owns it.');

  // The menu of the project (the last of its name: the first is the row in the left bar, which leaves out what is denied)
  // keeps what they would look for, switched off, with the reason.
  await page.getByRole('button', { name: 'Actions for Web' }).last().click();
  const rename = page.getByRole('menuitem', { name: /Rename…/ });
  await expect(rename).toHaveAttribute('aria-disabled', 'true');
  await expect(rename).toContainText('You can’t manage Web.');
  await page.keyboard.press('Escape');

  // On the org's home the act stays, switched off, and says why in its description.
  await page.goto('/org/acme');
  const newProject = page.getByRole('button', { name: 'New project', exact: true });
  await expect(newProject).toHaveAttribute('aria-disabled', 'true');
  await expect(newProject).toHaveAttribute('aria-description', 'You can’t take part in Acme. Ask Ada Lovelace, who owns it.');
  await page.getByRole('button', { name: 'Actions for Acme', exact: true }).last().click();
  const newDepartment = page.getByRole('menuitem', { name: 'New department…', exact: true });
  await expect(newDepartment).toHaveAttribute('aria-disabled', 'true');
  await expect(newDepartment).toContainText('You can’t manage Acme. Ask Ada Lovelace, who owns it.');
});

test('the left bar offers a row only what its person may do with it, and a person’s own org begins empty and says what to do', async ({ page }) => {
  await open(page, '/org/acme', PEOPLE.vera);
  await tree(page).getByRole('treeitem', { name: /^Web project(?: |$)/ }).click({ button: 'right' });
  const menu = page.getByRole('menu', { name: /^Actions for Web(?: · .+)?$/ });
  await expect(menu.getByRole('menuitem', { name: 'Open', exact: true })).toBeVisible();
  await expect(menu.getByRole('menuitem', { name: 'Star' })).toBeVisible();
  await expect(menu.getByRole('menuitem', { name: 'Copy link' })).toBeVisible();
  await expect(menu.getByRole('menuitem', { name: /Rename/ })).toHaveCount(0);
  await expect(menu.getByRole('menuitem', { name: /Delete/ })).toHaveCount(0);
  await expect(menu.getByRole('menuitem', { name: /Archive/ })).toHaveCount(0);
  await page.keyboard.press('Escape');

  // Their own org: empty, and it says so, with the two ways to begin.
  await page.goto('/org/vera');
  await expect(page.getByRole('heading', { name: 'Vera Rubin', level: 1 })).toBeVisible();
  await expect(page.getByText('Nothing here yet')).toBeVisible();
  await expect(page.getByRole('tab', { name: /Members/ })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'New department', exact: true })).toHaveCount(0);
  // The document is made at once; its own title is selected before Enter moves into the body.
  const newDocument = page.getByTestId('main').getByRole('button', { name: 'New document', exact: true });
  await expect(newDocument).not.toHaveAttribute('aria-disabled', 'true');
  await newDocument.click();
  await expect(page).toHaveURL(/\/doc\/[0-9a-f-]+$/);
  await expect(page.getByRole('textbox', { name: 'Name' })).toHaveCount(0);
  await expect(page.getByRole('textbox', { name: 'Title' })).toHaveValue('Untitled');
  await expect(page.getByRole('textbox', { name: 'Title' })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('textbox', { name: 'Document text' })).toBeFocused();
});

test('a row has its menu by right click and by its button, and a place that is starred is listed under Starred until it is not', async ({ page }) => {
  await open(page, '/org/acme');
  await expect(page.getByText('Starred', { exact: true })).toHaveCount(0);

  // The same list from the "⋯" of the row.
  const row = tree(page).getByRole('treeitem', { name: /Fix login/ });
  await row.hover();
  await row.getByRole('button', { name: 'Actions for Fix login' }).click();
  // Opened by its button, the menu is named by that button.
  const menu = page.getByRole('menu', { name: 'Actions for Fix login' });
  for (const entry of ['Open', 'New thread…', 'Star', 'Mark done', 'Rename…', 'Copy link', 'Archive…', 'Delete…']) {
    await expect(menu.getByRole('menuitem', { name: entry, exact: true })).toBeVisible();
  }
  await menu.getByRole('menuitem', { name: 'Star' }).click();

  const starred = page.getByText('Starred', { exact: true });
  await expect(starred).toBeVisible();
  // The row names the place, what kind of place it is, and where it lies.
  await expect(page.getByRole('link', { name: /Fix login topic Web/ }).first()).toBeVisible();

  await row.click({ button: 'right' });
  await page.getByRole('menu', { name: /^Actions for Fix login(?: · .+)?$/ }).getByRole('menuitem', { name: 'Unstar' }).click();
  await expect(starred).toHaveCount(0);
});

test('the “+” of an org and of a project makes what its caller may make there, and the orgs inside are a press away: rows of the tree, and on a small screen a list at the head of the bar', async ({ page }) => {
  await open(page, '/org/acme');

  const research = tree(page).getByRole('treeitem', { name: /^Research org(?: |$)/ });
  await research.hover();
  await research.getByRole('button', { name: 'New in Research' }).click();
  const make = page.getByRole('menu', { name: 'New in Research' });
  await expect(make.getByRole('menuitem', { name: 'New project…' })).toBeVisible();
  await expect(make.getByRole('menuitem', { name: 'New department…' })).toBeVisible();
  await make.getByRole('menuitem', { name: 'New project…' }).click();
  await expect(page.getByRole('dialog', { name: 'New project in Research' })).toBeVisible();
  await page.keyboard.press('Escape');

  // Where there is a rail, the head of the bar names the org and is no second way to another: an org inside is a row of the tree.
  await expect(page.getByRole('navigation', { name: 'Navigation' }).getByRole('link', { name: 'Acme', exact: true })).toBeVisible();
  await tree(page).getByRole('link', { name: /^ML( |$)/ }).click();
  await expect(page).toHaveURL(/\/org\/acme\.research\.ml$/);
  await expect(page.getByRole('heading', { name: 'ML' })).toBeVisible();

  // A small screen has no rail: the head of the bar leads to another org there, and lists the orgs inside this one.
  await page.setViewportSize({ width: 390, height: 800 });
  await page.getByRole('button', { name: 'Navigation', exact: true }).click();
  await page.getByRole('dialog', { name: 'Navigation' }).getByRole('button', { name: /Acme/ }).first().click();
  await expect(page.getByRole('menuitem', { name: 'Research / ML' })).toBeVisible();
  await page.getByRole('menuitem', { name: 'Research', exact: true }).click();
  await expect(page).toHaveURL(/\/org\/acme\.research$/);
  await page.setViewportSize({ width: 1280, height: 800 });
  await expect(page.getByRole('heading', { name: 'Research' })).toBeVisible();

  // A new root org from the rail, and joining by a link.
  await page.getByRole('button', { name: 'New org or join one' }).click();
  await page.getByRole('menuitem', { name: 'New org…' }).click();
  await dialogField(page, /^Name/).fill('Zeta Labs');
  await page.getByRole('dialog').getByRole('button', { name: 'Create org' }).click();
  await expect(page).toHaveURL(/\/org\/zeta-labs$/);
  await expect(page.getByRole('navigation', { name: 'Orgs and your own places' }).getByRole('link', { name: 'Zeta Labs' })).toBeVisible();

  await page.getByRole('button', { name: 'New org or join one' }).click();
  await page.getByRole('menuitem', { name: 'Join with a link…' }).click();
  await dialogField(page, 'Invitation link').fill('http://127.0.0.1:41872/invite/good-invitation');
  await page.getByRole('dialog').getByRole('button', { name: 'Look at the invitation' }).click();
  await expect(page).toHaveURL(/\/invite\/good-invitation$/);
});

test('the board stands topics by what needs attention, and a card moves to Done by drag and by its menu', async ({ page }) => {
  await open(page, `/project/${ACME.web}?view=board`);

  const column = (name: RegExp) => page.getByRole('region', { name });
  await expect(column(/Needs attention/).getByRole('link', { name: 'Fix login' })).toBeVisible();
  await expect(column(/Idle/).getByRole('link', { name: 'Upgrade deps' })).toBeVisible();
  await expect(column(/Done/).getByRole('link', { name: 'Migrate CI' })).toBeVisible();
  await expect(column(/Running/).getByText('Nothing here')).toBeVisible();

  // Dragging a card into Done sets the status; nothing is moved before the server has answered.
  const card = column(/Idle/).getByRole('listitem').filter({ hasText: 'Upgrade deps' });
  const from = await card.boundingBox();
  const to = await column(/Done/).boundingBox();
  if (from === null || to === null) throw new Error('the board is not drawn');
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
  await page.mouse.down();
  await page.mouse.move(from.x + from.width / 2 + 12, from.y + from.height / 2 + 12, { steps: 4 });
  await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 12 });
  await page.mouse.up();
  await expect(notices(page).getByText('Upgrade deps marked done')).toBeVisible();
  await expect(column(/Done/).getByRole('link', { name: 'Upgrade deps' })).toBeVisible();
  await expect(column(/Idle/).getByRole('link', { name: 'Upgrade deps' })).toHaveCount(0);

  // And out of it again by the card's menu.
  const done = column(/Done/).getByRole('listitem').filter({ hasText: 'Migrate CI' });
  await done.hover();
  await done.getByRole('button', { name: 'Actions for Migrate CI' }).click();
  await page.getByRole('menuitem', { name: 'Reopen' }).click();
  await expect(column(/Idle/).getByRole('link', { name: 'Migrate CI' })).toBeVisible();
});

test('the list and the board are narrowed by status, tag, attention and a search, which the address keeps', async ({ page }) => {
  await open(page, `/project/${ACME.web}`);

  const names = page.getByRole('table', { name: 'Topics' }).getByRole('link');
  await expect(names).toHaveText(['Fix login', 'Upgrade deps']);

  await page.getByRole('radio', { name: 'Done' }).click();
  await expect(names).toHaveText(['Migrate CI']);
  await expect(page).toHaveURL(/status=done/);
  await page.getByRole('radio', { name: 'All' }).click();
  await expect(names).toHaveCount(3);

  await page.getByRole('button', { name: 'Tags' }).click();
  await page.getByRole('checkbox', { name: 'bug' }).click();
  await expect(names).toHaveText(['Fix login']);
  await page.keyboard.press('Escape');

  await page.getByRole('button', { name: 'Clear filters' }).click();
  await page.getByRole('combobox', { name: 'Attention' }).click();
  await page.getByRole('option', { name: 'Blocked' }).click();
  await expect(page.getByText('No topic matches')).toBeVisible();
  await page.getByRole('button', { name: 'Clear filters' }).first().click();

  await page.getByRole('searchbox', { name: 'Search topics' }).fill('upgrade');
  await expect(names).toHaveText(['Upgrade deps']);
  await expect(page).toHaveURL(/q=upgrade/);

  // A link to it shows the same: the address is all it needs.
  await page.reload();
  await expect(names).toHaveText(['Upgrade deps']);
  await expect(page.getByRole('searchbox', { name: 'Search topics' })).toHaveValue('upgrade');

  await page.getByRole('radio', { name: 'Board' }).click();
  await expect(page).toHaveURL(/view=board/);
  await expect(page.getByRole('region', { name: /Idle/ }).getByRole('link', { name: 'Upgrade deps' })).toBeVisible();
});

test('an invitation is made as a link for one address and as a link to share, and the ones still good can be taken back', async ({ page }) => {
  await open(page, '/org/acme?tab=members');
  // Its header row and the nine people.
  await expect(page.getByRole('table', { name: 'Members' }).getByRole('row')).toHaveCount(10);

  // A link to share, for up to three people.
  await page.getByRole('button', { name: 'Invite…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Invite to Acme' });
  await expect(dialog.getByText('Whoever uses it will be a Viewer of Acme.')).toBeVisible();
  await expect(dialog.getByText('30 days at most here, by the setting of Acme.')).toBeVisible();
  await dialog.getByRole('radio', { name: 'Link' }).click();
  await dialog.getByRole('spinbutton', { name: 'How many people' }).fill('3');
  await dialog.getByRole('button', { name: 'Create invitation' }).click();
  await expect(dialog.getByText(/Anyone who has this link can join, up to 3 people, until/)).toBeVisible();
  await expect(dialog.getByRole('textbox', { name: 'Link' })).toHaveValue(/\/invite\/inv_/);

  // A link for one address, where the org has no way to send mail: it says so and gives the link to hand on.
  await dialog.getByRole('button', { name: 'Make another' }).click();
  await dialog.getByRole('radio', { name: 'Email' }).click();
  await dialog.getByRole('textbox', { name: 'Email address' }).fill('dana@example.org');
  await dialog.getByRole('button', { name: 'Create invitation' }).click();
  await expect(dialog.getByText('Not sent: Acme has no way to send mail. Give dana@example.org this link yourself.')).toBeVisible();
  await dialog.getByRole('button', { name: 'Done' }).click();

  const pending = page.getByRole('table', { name: 'Invitations still good' });
  await expect(pending.getByText('Anyone with the link')).toBeVisible();
  await expect(pending.getByText('dana@example.org')).toBeVisible();

  await pending.getByRole('row').filter({ hasText: 'dana@example.org' }).getByRole('button', { name: 'Revoke' }).click();
  await page.getByRole('alertdialog', { name: 'Revoke this invitation?' }).getByRole('button', { name: 'Revoke the invitation' }).click();
  await expect(pending.getByText('dana@example.org')).toHaveCount(0);
  await expect(pending.getByText('Anyone with the link')).toBeVisible();
});

test('a personal link is said to be sent where the org can send mail', async ({ page }) => {
  await mailWorks(true);
  await open(page, '/org/acme?tab=members');
  await page.getByRole('button', { name: 'Invite…' }).click();
  await page.getByRole('dialog').getByRole('textbox', { name: 'Email address' }).fill('dana@example.org');
  await page.getByRole('dialog').getByRole('button', { name: 'Create invitation' }).click();
  await expect(page.getByRole('dialog').getByText('Sent to dana@example.org.')).toBeVisible();
});

test('an invitation made here is one a stranger can open, and what it asks of the org’s rules is refused where it goes beyond them', async ({ page }) => {
  await open(page, '/org/acme?tab=members');
  await page.getByRole('button', { name: 'Invite…' }).click();
  await page.getByRole('dialog').getByRole('radio', { name: 'Link' }).click();
  await page.getByRole('dialog').getByRole('spinbutton', { name: 'How many people' }).fill('500');
  await page.getByRole('dialog').getByRole('button', { name: 'Create invitation' }).click();
  // The most a shared link serves is the org's own number, and the field says so before anything is sent.
  await expect(page.getByRole('dialog').getByText('Choose a whole number from 1 to 100.')).toBeVisible();
});

test('a member’s role is changed from the member’s menu, and what the list says follows', async ({ page }) => {
  await open(page, '/org/acme?tab=members');

  const vera = page.getByRole('table', { name: 'Members' }).getByRole('row').filter({ hasText: 'Vera Rubin' });
  await expect(vera).toContainText('Viewer');
  await vera.getByRole('button', { name: 'Actions for Vera Rubin', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Change role…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Change the role of Vera Rubin' });
  await dialog.getByRole('combobox', { name: 'Role' }).click();
  await page.getByRole('option', { name: 'Developer' }).click();
  await dialog.getByRole('button', { name: 'Save role' }).click();

  await expect(notices(page).getByText('Vera Rubin is now Developer')).toBeVisible();
  await expect(vera).toContainText('Developer');
});

test('members are searched by name and narrowed to those whose home is the org itself', async ({ page }) => {
  await open(page, '/org/acme?tab=members');
  const rows = page.getByRole('table', { name: 'Members' }).getByRole('row');

  await page.getByRole('searchbox', { name: 'Search members' }).fill('hopper');
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(1)).toContainText('Grace Hopper');
  await page.getByRole('searchbox', { name: 'Search members' }).fill('zzz');
  await expect(page.getByText('Nobody matches “zzz”.')).toBeVisible();
});

test('the Activity of a place lists what was done to it, newest first, and keeps to management or to work', async ({ page }) => {
  await open(page, `/project/${ACME.web}?view=activity`);
  await expect(page.getByText('Ada Lovelace made the topic Fix login')).toBeVisible();

  // A topic made now is the first line.
  await page.goto(`/project/${ACME.web}`);
  await page.getByRole('button', { name: 'New topic', exact: true }).first().click();
  await dialogField(page, /^Name/).fill('Fresh topic');
  await page.getByRole('dialog').getByRole('button', { name: 'Create topic' }).click();
  await expect(page).toHaveURL(/\/topic\//);
  await page.getByRole('tab', { name: 'Activity' }).click();
  await expect(page.getByRole('list').getByText('Ada Lovelace made the topic Fresh topic')).toBeVisible();
  await page.getByRole('radio', { name: 'Work' }).click();
  await expect(page.getByText('Nothing of this kind has happened here yet.')).toBeVisible();
});

test('Activity opens at Summary and switching to Every change asks for that depth', async ({ page }) => {
  await open(page, `/project/${ACME.web}?view=activity`);
  const detail = page.getByRole('group', { name: 'Activity detail' });
  await expect(detail.getByText('Summary', { exact: true })).toHaveAttribute('data-state', 'on');
  const every = page.waitForResponse((response) => response.url().includes(`/nodes/${ACME.web}/activity`) && new URL(response.url()).searchParams.get('depth') === 'every');
  await detail.getByText('Every change', { exact: true }).click();
  expect((await every).ok()).toBe(true);
  await expect(detail.getByText('Every change', { exact: true })).toHaveAttribute('data-state', 'on');
});

test('a topic’s screen has its overview, its threads from the live tree and its environments, and no tab for a part that is not built', async ({ page }) => {
  await open(page, `/topic/${ACME.fixLogin}`);

  await expect(page.getByRole('heading', { name: 'Fix login' })).toBeVisible();
  await expect(page.getByText('Sessions end too early')).toBeVisible();
  await expect(page.getByRole('link', { name: 'Main thread' })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Repro' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'bug' }).or(page.getByText('bug', { exact: true })).first()).toBeVisible();

  await page.getByRole('tab', { name: /Threads/ }).click();
  await expect(page.getByRole('table', { name: 'Threads' }).getByRole('row')).toHaveCount(3);
  await expect(page).toHaveURL(/tab=threads/);

  await page.getByRole('tab', { name: /Environments/ }).click();
  await expect(page.getByText('Default for new threads')).toBeVisible();
  await expect(page.getByText('Workspace', { exact: true })).toBeVisible();

  // The tabs of the topic, not the ones of the right bar that have names of their own: the files of a workspace are a thread's, in the right bar beside it.
  await expect(page.getByTestId('main').getByRole('tab', { name: 'Files' })).toHaveCount(0);

  // The control that starts a thread opens the dialog that makes one, from the defaults of the place.
  await page.getByRole('tab', { name: 'Overview' }).click();
  await page.getByRole('button', { name: 'New thread', exact: true }).first().click();
  await expect(page.getByRole('dialog', { name: 'New thread in Fix login' })).toContainText('Choose what the thread runs on and where it works.');
});

test('a status and a bell are set from the topic’s header and show at once, and are put back where the server refuses', async ({ page }) => {
  await open(page, `/topic/${ACME.upgradeDeps}`);

  await page.getByRole('button', { name: 'Change status: Open', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Done', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Change status: Done', exact: true })).toHaveText('Done');
  // The tree says it too, on the topic's own row, which stands where it stood.
  await expect(tree(page).getByRole('treeitem', { name: /^Upgrade deps topic, done/ })).toBeVisible();

  await page.getByRole('button', { name: 'Notifications for this topic' }).click();
  await page.getByRole('menuitemradio', { name: 'Everything', exact: true }).click();
  // The menu has gone before it is opened again.
  await expect(page.getByRole('menu')).toHaveCount(0);
  await page.getByRole('button', { name: 'Notifications for this topic' }).click();
  await expect(page.getByRole('menuitemradio', { name: 'Everything', exact: true })).toBeChecked();
});

test('an act that the server is slow to answer shows as waiting before it is answered, and as done only when the server has said so', async ({ page }) => {
  await open(page, `/project/${ACME.web}`);
  // Every change is answered after three seconds; what is read is not slowed.
  await page.route(/\/api\/v1\//, async (route) => {
    if (route.request().method() !== 'GET') await new Promise((resolve) => setTimeout(resolve, 3000));
    await route.continue();
  });

  // A topic is made: the button of the dialog waits, and nothing has been made yet.
  await page.getByRole('button', { name: 'New topic', exact: true }).first().click();
  await dialogField(page, /^Name/).fill('Slow one');
  const create = page.getByRole('dialog').getByRole('button', { name: 'Create topic' });
  await create.click();
  await expect(create).toHaveAttribute('aria-busy', 'true', { timeout: 1500 });
  await expect(tree(page).getByRole('treeitem', { name: /Slow one/ })).toHaveCount(0);
  await expect(page).toHaveURL(/\/topic\//);

  // A status is chosen: what was asked is shown as waiting, and the tree still says what the server last said.
  await page.goto(`/topic/${ACME.upgradeDeps}`);
  const status = page.getByRole('button', { name: /^Change status: (Open|Done)$/ });
  await status.click();
  await page.getByRole('menuitemradio', { name: 'Done', exact: true }).click();
  await expect(status).toHaveAttribute('aria-busy', 'true', { timeout: 1500 });
  await expect(status).toBeDisabled();
  await expect(status).toContainText('Done');
  await expect(tree(page).getByRole('treeitem', { name: /^Upgrade deps topic( |$)/ })).not.toHaveAccessibleName(/done/);
  // ... and it is done when the server has said so.
  await expect(tree(page).getByRole('treeitem', { name: /^Upgrade deps topic, done/ })).toBeVisible();
  await expect(status).toBeEnabled();
});


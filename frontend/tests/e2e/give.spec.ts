/** Giving: sharing a place with everything in it or on its own, a form that offers only what its reader may give, a place that keeps sharing to owners, a sharer taking back their own, a role's people from its chip, sharing with agents (documents to edit, then to manage, taken back; a copy past the limit), asking for access and deciding with fewer permissions, telling people, Export travelling with a choice, and letting people into a thread's environment. */
import { expect, test } from '@playwright/test';

import { id, RUNS_ON } from '../doubles/fixtures.ts';
import { makePrivateDocument } from './documents-support.ts';
import { add, ACME, choose, dialog, field, line, options, PEOPLE, reach, second, sent, shareAs, shareThread, stage, steer, tab, ZOE } from './give-support.ts';
import { scenario, signIn } from './support.ts';

test.beforeEach(async () => {
  await scenario();
});

const web = `/project/${ACME.web}`;
const fixLogin = `/topic/${ACME.fixLogin}`;

test('a Developer shares a project with everything in it, and the person stands under People as a Viewer who can open what is inside', async ({ page, browser, baseURL }) => {
  // What stays closed is counted as far as the sharer may see it: a document she made private herself. The team's own private one is Ada's, and Dora is told nothing of it.
  await makePrivateDocument('dora', ACME.fixLogin, 'Dora’s draft');
  await shareAs(page, PEOPLE.dora, web, 'Web');
  await options(page);
  // The form opens on the place with everything in it, and counts what that reaches and what stays closed.
  await expect(dialog(page).getByRole('combobox', { name: 'What is shared' })).toContainText('Contents');
  await expect(dialog(page)).toContainText('Covers 3 topics and folders, 3 threads, and 4 documents that follow Web, and whatever is made here later.');
  await expect(dialog(page)).toContainText('Closed to them: 1 private document.');

  await stage(page, 'Zoe Outsider', 'zoe');
  await add(page);
  await tab(page, 'People');
  const table = dialog(page).getByRole('table', { name: 'Roles held in Web' });
  await expect(table).toContainText('Zoe Outsider');
  await expect(table.getByRole('combobox', { name: 'Role of Zoe Outsider' })).toContainText('Viewer');
  // Every role says who gave it and when.
  await expect(table).toContainText('Dora Developer, Oct 11');

  // The person is somebody else, in a browser of their own, and finds what is inside the project open to them.
  const zoe = await second(browser, baseURL, `/topic/${ACME.fixLogin}`, ZOE);
  await expect(zoe.page.getByRole('heading', { name: 'Fix login', level: 1 })).toBeVisible();
  await zoe.context.close();
});

test('the same for the project alone: the line says that nothing inside is opened, and what is inside stays closed to them', async ({ page, browser, baseURL }) => {
  await shareAs(page, PEOPLE.dora, web, 'Web');
  await reach(page, 'Place only');
  await expect(dialog(page)).toContainText('Nothing inside Web is opened: not its 3 topics and folders, their threads or their documents.');
  await stage(page, 'Zoe Outsider', 'zoe');
  await add(page);
  // They are named on the place's own page, and hold no role in it.
  await expect(line(page, 'Can view')).toContainText('Zoe Outsider');
  await tab(page, 'People');
  await expect(dialog(page)).not.toContainText('Zoe Outsider');

  const zoe = await second(browser, baseURL, web, ZOE);
  await expect(zoe.page.getByRole('heading', { name: 'Web', exact: true, level: 1 })).toBeVisible();
  await zoe.page.goto(`/topic/${ACME.fixLogin}`);
  await expect(zoe.page.getByRole('heading', { name: /You don.t have access/ })).toBeVisible();
  await zoe.context.close();
});

test('somebody who reads a place shares it to read, with Can edit disabled and the reason said in their own terms', async ({ page }) => {
  await shareAs(page, PEOPLE.vera, web, 'Web');
  await expect(dialog(page)).toContainText('You are a Viewer of Web: you can give up to Can view of its work, and not Interactor.');
  await dialog(page).getByRole('combobox', { name: 'What they can do' }).click();
  await expect(page.getByRole('option', { name: /Can interact/ })).toHaveAttribute('aria-disabled', 'true');
  await expect(page.getByRole('option', { name: /Can manage/ })).toHaveAttribute('aria-disabled', 'true');
  await page.getByRole('option', { name: /Can view/ }).click();

  await stage(page, 'Linus Pauling');
  await add(page);
  await tab(page, 'People');
  const table = dialog(page).getByRole('table', { name: 'Roles held in Web' });
  await expect(table).toContainText('Linus Pauling');
  await expect(table).toContainText('Vera Rubin, Oct 11');
  // The role they gave is theirs to take back, and the roles above their own are not theirs to give.
  await table.getByRole('combobox', { name: 'Role of Linus Pauling' }).click();
  await expect(page.getByRole('option', { name: 'Developer' })).toHaveAttribute('aria-disabled', 'true');
});

test('a place that keeps sharing to its owners shows the field off for everybody else, says who can, and lets them ask', async ({ page, browser, baseURL }) => {
  await steer('/holders-share', { place: ACME.web, allowed: false });
  await shareAs(page, PEOPLE.dora, web, 'Web');
  await expect(field(page)).toBeDisabled();
  await expect(dialog(page)).toContainText('Sharing in Web is with a thing’s owners: Ada Lovelace.');
  await dialog(page).getByRole('button', { name: 'Request access' }).click();
  const ask = page.getByRole('dialog', { name: 'Ask to share “Web”' });
  await ask.getByRole('textbox', { name: /Why do you need it/ }).fill('I lead the web work');
  await ask.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(ask).toContainText('Your request was sent to Ada Lovelace and waits to be decided.');

  // The owner finds the request on the page, and giving it lets the person share.
  const ada = await second(browser, baseURL, web, PEOPLE.ada);
  await ada.page.getByRole('button', { name: 'Actions for Web' }).first().click();
  await ada.page.getByRole('menuitem', { name: 'Share…' }).click();
  const card = ada.page.getByRole('region', { name: 'Waiting for you' });
  await expect(card).toContainText('Dora Developer');
  await card.getByRole('button', { name: 'Approve as asked' }).click();
  await expect(card).toContainText('Approved by Ada Lovelace.');
  await ada.context.close();
});

test('a sharer takes back what they gave and finds no way to remove what somebody else gave', async ({ page }) => {
  // Ada named Vera in Can view of the topic before Dora came.
  await steer('/touch', { as: 'ada', path: 'nodes', id: ACME.fixLogin, change: { name: [{ choice: 'full_view', holders: [{ kind: 'user', id: id(1, 8) }] }] } });
  await shareAs(page, PEOPLE.dora, fixLogin, 'Fix login');
  await reach(page, 'Place only');
  await stage(page, 'Linus Pauling');
  await add(page);

  const view = line(page, 'Can view');
  await expect(view).toContainText('Linus Pauling');
  await expect(view).toContainText('by Dora Developer, Oct 11');
  await expect(view).toContainText('Vera Rubin');
  await expect(view).toContainText('by Ada Lovelace');
  // The rest of the page is read, not changed: there is no cross on what Ada gave, and no menu of how the page stands.
  await expect(view.getByRole('button', { name: 'Change Vera Rubin’s access' })).toHaveCount(0);
  await expect(dialog(page).getByRole('button', { name: 'How the page stands' })).toHaveCount(0);
  await expect(dialog(page)).toContainText('You can add people up to what you hold, and take back those you added.');

  await view.getByRole('button', { name: 'Change Linus Pauling’s access' }).click();
  await page.getByRole('menuitem', { name: 'Take Linus Pauling out' }).click();
  const confirm = page.getByRole('alertdialog', { name: 'Take Linus Pauling out?' });
  // Whoever does not manage the page is not told whom it reaches: that is asked of its managers.
  await expect(confirm).not.toContainText('Working out whom this reaches');
  await confirm.getByRole('button', { name: 'Take out Linus Pauling' }).click();
  await expect(view).not.toContainText('Linus Pauling');
  await expect(view).toContainText('Vera Rubin');
});

test('the people behind a role open in its access row, with their level and route of access', async ({ page }) => {
  await shareAs(page, PEOPLE.ada, web, 'Web');
  const holder = dialog(page).getByRole('button', { name: 'Developers of Web', exact: true });
  await expect(holder).toContainText('4');
  const row = dialog(page).locator('li[data-access-level]').filter({ has: holder });
  await holder.click();
  for (const name of ['Ada Lovelace', 'Dora Developer', 'Grace Hopper']) await expect(row).toContainText(name);
  await expect(row).toContainText('Manage via Developers of Web');
  await expect(page.getByRole('dialog', { name: 'People in Developers' })).toHaveCount(0);
});

test('the topic’s documents are shared with the agents of a project to edit, changed to manage with what that means, and taken back', async ({ page }) => {
  await shareAs(page, PEOPLE.ada, fixLogin, 'Fix login');
  await tab(page, 'Agents');
  await expect(dialog(page)).toContainText('Nothing of Fix login is shared with agents.');
  await dialog(page).getByRole('button', { name: 'Share with agents…' }).click();

  const form = page.getByRole('dialog', { name: 'Share “Fix login” with agents' });
  await form.getByRole('combobox', { name: 'Whose agents' }).fill('Web');
  await page.getByRole('option', { name: /^Web/ }).click();
  await form.getByRole('button', { name: 'Permissions' }).click();
  await form.getByRole('combobox', { name: 'Conversations' }).click();
  await page.getByRole('option', { name: 'None' }).click();
  await form.getByRole('combobox', { name: 'Chats' }).click();
  await page.getByRole('option', { name: 'None' }).click();
  await form.getByRole('combobox', { name: 'Documents' }).click();
  await page.getByRole('option', { name: 'Edit' }).click();
  // The button says what happens, and what the agents will do is said under the cards.
  await expect(form).toContainText('What an agent changes shows under its name and in the document’s history, like anybody’s.');
  await form.getByRole('button', { name: 'Let the agents of Web read and edit these documents' }).click();

  const shared = dialog(page).getByRole('list', { name: 'Shared with agents' });
  await expect(shared).toContainText('Agents of Web');
  await expect(shared).toContainText('by Ada Lovelace, Oct 11');
  await expect(shared.getByRole('combobox', { name: 'Documents' })).toContainText('Edit');
  // On the page of what was shared, the agents are listed beside the people.
  await tab(page, 'This topic');
  await expect(line(page, 'Custom')).toContainText('Agents of Web');
  await expect(dialog(page)).toContainText('View Documents · Edit Documents');
  await tab(page, 'Agents');

  await shared.getByRole('combobox', { name: 'Documents' }).click();
  await page.getByRole('option', { name: 'Manage' }).click();
  const change = page.getByRole('alertdialog', { name: 'Change what the agents of Web may do?' });
  await expect(change).toContainText('Documents: from “Read and edit” to “Manage”.');
  await expect(change).toContainText('They can also remove documents here and move them. Nothing is destroyed: what they remove can be brought back until it is purged');
  await change.getByRole('button', { name: 'Change what the agents of Web may do' }).click();
  await expect(shared.getByRole('combobox', { name: 'Documents' })).toContainText('Manage');

  await shared.getByRole('button', { name: 'Stop sharing with the agents of Web' }).click();
  const stop = page.getByRole('alertdialog', { name: 'Stop sharing with the agents of Web?' });
  await expect(stop).toContainText('What they already read stays read.');
  await stop.getByRole('button', { name: 'Stop sharing' }).click();
  await expect(dialog(page)).toContainText('Nothing of Fix login is shared with agents.');
  await tab(page, 'This topic');
  await expect(dialog(page)).not.toContainText('Agents of Web');
});

test('a copy past the limit says so with its size against the limit, and the limit is raised from the form by whoever may change it', async ({ page }) => {
  await steer('/shares/snapshot', { limit: 1024 * 1024 });
  await shareAs(page, PEOPLE.ada, fixLogin, 'Fix login');
  await tab(page, 'Agents');
  await dialog(page).getByRole('button', { name: 'Share with agents…' }).click();
  const form = page.getByRole('dialog', { name: 'Share “Fix login” with agents' });
  await form.getByRole('combobox', { name: 'Whose agents' }).fill('Main');
  await page.getByRole('option', { name: /^Main thread/ }).first().click();
  await form.getByRole('radio', { name: /Give a copy/ }).click();
  await expect(form).toContainText('This copy is over the limit');
  await expect(form.getByRole('button', { name: /Give a copy to/ })).toBeDisabled();
  await expect(form).toContainText('15 MiB of 1 MiB');
  await expect(form).toContainText('Share less, share it so that it stays shared, or raise the limit.');

  await form.getByRole('button', { name: /Raise the limit for/ }).click();
  await form.getByRole('button', { name: /Give a copy to/ }).click();
  await expect(page.getByRole('dialog', { name: 'Share “Fix login” with agents' })).toHaveCount(0);
  await expect(dialog(page).getByRole('list', { name: 'Shared with agents' })).toContainText('A copy');
});

test('a request is asked, approved with fewer permissions than asked, and what was left out is said to whoever asked', async ({ page, browser, baseURL }) => {
  await steer('/refuse', { username: 'vera', node: ACME.fixLogin, permissions: ['topic.interact'] });
  await signIn(page, PEOPLE.vera);
  await page.goto(fixLogin);
  await expect(page.getByRole('radio', { name: 'Interact', exact: true })).toBeChecked();
  await page.getByRole('textbox', { name: /Why do you need it/ }).fill('I take part in the login work');
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(page.getByText('Your request was sent to Ada Lovelace and waits to be decided.')).toBeVisible();

  // Ada finds it on the page, takes one of the three permissions out, and approves the rest.
  const ada = await second(browser, baseURL, fixLogin, PEOPLE.ada);
  await ada.page.getByRole('button', { name: 'Actions for Fix login' }).first().click();
  await ada.page.getByRole('menuitem', { name: 'Share…' }).click();
  const card = ada.page.getByRole('region', { name: 'Waiting for you' });
  await expect(card).toContainText('Vera Rubin');
  await card.getByRole('checkbox', { name: 'Interact Chat' }).click();
  await card.getByRole('button', { name: 'Approve 2 of 3' }).click();
  await expect(card).toContainText('Approved by Ada Lovelace.');
  await ada.context.close();

  await page.reload();
  await expect(page.getByText(/given in part, Edit Documents, Interact Topic; not Interact Chat/)).toBeVisible();
});

test('too many requests waiting is said in words, with how many wait', async ({ page }) => {
  await steer('/requests/limit', { waiting: 1 });
  // What is asked is what she does not hold: the fake API refuses a request for what its asker already has.
  await steer('/refuse', { username: 'vera', node: ACME.web, permissions: ['project.interact'] });
  await steer('/refuse', { username: 'vera', node: ACME.fixLogin, permissions: ['topic.interact'] });
  await signIn(page, PEOPLE.vera);
  await page.goto(web);
  await page.getByRole('textbox', { name: /Why do you need it/ }).fill('The first');
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(page.getByText(/Your request was sent to/)).toBeVisible();

  await page.goto(fixLogin);
  await page.getByRole('textbox', { name: /Why do you need it/ }).fill('The second');
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(page.getByText('You have 1 requests waiting. Wait for an answer, or take one back, before you ask again.')).toBeVisible();
});

test('Tell them sends each person one message with the link and the sharer’s own line, in the chat the two have', async ({ page, browser, baseURL }) => {
  await shareAs(page, PEOPLE.dora, fixLogin, 'Fix login');
  await reach(page, 'Place only');
  await stage(page, 'Vera Rubin');
  const tell = dialog(page).getByRole('checkbox', { name: /Send Vera Rubin a message with the link/ });
  await expect(tell).toBeChecked();
  await dialog(page).getByRole('textbox', { name: 'A line to go with it' }).fill('Have a look at the login work');
  await add(page);
  await expect(line(page, 'Can view')).toContainText('Vera Rubin');

  const vera = await second(browser, baseURL, '/org/acme/messages', PEOPLE.vera);
  await vera.page.getByText('Dora Developer').first().click();
  const message = vera.page.locator('li[data-message]', { hasText: 'Have a look at the login work' });
  await expect(message).toBeVisible();
  // The link in the message is the thing's name, which opens its page.
  await expect(message.getByRole('link', { name: 'Fix login' })).toHaveAttribute('href', `/topic/${ACME.fixLogin}`);
  await expect(message).toHaveCount(1);
  await vera.context.close();
});

test('where direct chats are off, or the person is not in the org, nobody is told in a chat and the link is there to copy', async ({ page }) => {
  await shareAs(page, PEOPLE.dora, fixLogin, 'Fix login');
  await reach(page, 'Place only');
  await stage(page, 'Zoe Outsider', 'zoe');
  const tell = dialog(page).getByRole('checkbox', { name: /Send Zoe Outsider a message with the link/ });
  await expect(tell).toBeDisabled();
  await expect(dialog(page)).toContainText('Zoe Outsider is not in this org, so there is no chat to tell them in. Copy the link and send it yourself.');
  await expect(dialog(page).getByRole('button', { name: 'Copy link' }).first()).toBeVisible();
});

test('Export travels with a choice: it is on as the form opens, withheld when unticked, and absent with why for a sharer who may not export the thing', async ({ page, browser, baseURL }) => {
  await shareAs(page, PEOPLE.dora, fixLogin, 'Fix login');
  await reach(page, 'Place only');
  await stage(page, 'Linus Pauling');
  const tick = dialog(page).getByRole('checkbox', { name: 'They may take it away as a file' });
  await expect(tick).toBeChecked();
  await tick.click();
  await add(page);
  await expect(line(page, 'Can view')).toContainText('Linus Pauling');
  const made = (await sent()).filter((entry) => entry.change.name !== undefined);
  expect(made.at(-1)?.change.name?.[0]?.export).toBe(false);

  // A sharer who may not export the thing gives its reading and no copy of it, and the form says so. Somebody who only reads does not manage the thing, so the Export rows are not theirs through that.
  await steer('/touch', { as: 'ada', path: 'nodes', id: ACME.fixLogin, change: { rows: [{ permission: 'chat.export', inherit: false }, { permission: 'doc.export', inherit: false }] } });
  const vera = await second(browser, baseURL, fixLogin, PEOPLE.vera);
  await vera.page.getByRole('button', { name: 'Actions for Fix login' }).first().click();
  await vera.page.getByRole('menuitem', { name: 'Share…' }).click();
  await reach(vera.page, 'Place only');
  await stage(vera.page, 'Grace Hopper');
  await expect(dialog(vera.page).getByRole('checkbox', { name: 'They may take it away as a file' })).toHaveCount(0);
  await expect(dialog(vera.page)).toContainText('You may not take this away as a file yourself, so you cannot let them: they can read it here.');
  await vera.context.close();
});

test('the form is used by keyboard alone: the field, what is shared, what they can do and Add', async ({ page }) => {
  await shareAs(page, PEOPLE.ada, fixLogin, 'Fix login');
  await expect(field(page)).toBeFocused();
  await page.keyboard.type('gra');
  await expect(page.getByRole('option', { name: /Grace Hopper/ })).toBeVisible();
  // The arrows go down the list and back up; Enter chooses the one in front.
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('Enter');
  await expect(dialog(page).getByRole('list', { name: 'To be added' })).toContainText('Grace Hopper');

  await dialog(page).getByRole('button', { name: 'Options', exact: true }).focus();
  await page.keyboard.press('Enter');
  const shared = dialog(page).getByRole('combobox', { name: 'What is shared' });
  await shared.focus();
  await page.keyboard.press('Enter');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await expect(shared).toContainText('Only Fix login itself');

  const level = dialog(page).getByRole('combobox', { name: 'What they can do' });
  await level.focus();
  await page.keyboard.press('Enter');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await expect(level).toContainText('Can interact');

  await dialog(page).getByRole('button', { name: 'Add', exact: true }).focus();
  await page.keyboard.press('Enter');
  await expect(line(page, 'Can interact')).toContainText('Grace Hopper');
});

test('a row of a list of resources has Share… in its menu, which opens the page of that resource', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto('/resources/providers');
  await page.getByRole('button', { name: 'Actions for OpenAI key' }).click();
  await page.getByRole('menuitem', { name: 'Share…' }).click();
  await expect(dialog(page)).toBeVisible();
  await expect(page.getByRole('dialog', { name: 'Access to “OpenAI key”' })).toBeVisible();
});

test('somebody named on one thread from elsewhere is let into its environment in the same step, up to what the choice gives and no more', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  const environment = RUNS_ON.workspace;
  for (const [choice, rows] of [
    ['Can view', []],
    ['Can interact', []],
    ['Can manage', ['files.manage', 'terminal.manage']],
  ] as const) {
    await scenario();
    await signIn(page, PEOPLE.ada);
    await page.goto(`/thread/${ACME.mainThread}`);
    await shareThread(page);
    await stage(page, 'Zoe Outsider', 'zoe');
    await choose(page, choice);
    const tick = dialog(page).getByRole('checkbox', { name: /Let them into Workspace/ });
    await expect(tick).toBeChecked();
    await add(page);
    await expect(line(page, choice)).toContainText('Zoe Outsider');

    // The environment is named after the thread is: the change arrives a moment after the line shows the person.
    const made = async () => (await sent()).filter((entry) => entry.path === 'environments' && entry.id === environment);
    await expect.poll(async () => (await made()).length).toBe(1);
    const change = (await made()).at(-1)?.change;
    // The environment's Can view for any of the three, and with Can manage the Manage of Files and Terminal as well: never its own Manage, never Browsers or VNC.
    expect(change?.name?.[0]?.choice).toBe('full_view');
    expect((change?.rows ?? []).map((row) => row.permission)).toEqual(rows);
  }

  // Without the tick, nothing is given there: the conversation is theirs, and the files are not.
  await scenario();
  await signIn(page, PEOPLE.ada);
  await page.goto(`/thread/${ACME.mainThread}`);
  await shareThread(page);
  await stage(page, 'Zoe Outsider', 'zoe');
  await dialog(page).getByRole('checkbox', { name: /Let them into Workspace/ }).click();
  await add(page);
  await expect(line(page, 'Can view')).toContainText('Zoe Outsider');
  // Nothing arrives for the environment: a moment is given for it to, were it going to.
  await page.waitForTimeout(1500);
  expect((await sent()).filter((entry) => entry.path === 'environments')).toHaveLength(0);
});

test('a thread’s page says whose files its files are and how many look into them, and its environment’s page lists who reaches it', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto(`/thread/${ACME.mainThread}`);
  await shareThread(page);
  await expect(dialog(page)).toContainText('Its files, terminal, browsers and displays are those of Workspace');
  await expect(dialog(page)).toContainText(/\d+ (people look|person looks) into Workspace\./);

  // Making the thread private takes its conversation from the others and leaves its files to those who look into the environment: the confirmation says so.
  await dialog(page).getByRole('button', { name: 'Make private', exact: true }).click();
  const privately = page.getByRole('alertdialog', { name: /^Make .* private\?$/ });
  await expect(privately).toContainText(/Its files stay readable to the \d+ (people who look|person who looks) into Workspace\./);
  await privately.getByRole('button', { name: 'Cancel' }).click();

  await dialog(page).getByRole('button', { name: 'Open Workspace' }).click();
  await expect(page.getByRole('dialog', { name: 'Access to “Workspace”' })).toBeVisible();
  await expect(line(page, 'Can view')).toBeVisible();
  await expect(line(page, 'Can manage')).toBeVisible();
  await dialog(page).getByRole('button', { name: /people reach into it/ }).click();
  const who = page.getByRole('dialog', { name: 'Who can reach Workspace' });
  await expect(who).toContainText('Ada Lovelace');
  await expect(who).toContainText('owns it');
});

test('an environment’s door is given alone to somebody who does not manage it, and each Manage cell says what it is before it is given', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto(`/resources/environments/${RUNS_ON.workspace}`);
  await page.getByRole('tab', { name: 'Access' }).click();
  await page.getByRole('button', { name: 'Permissions' }).click();
  await expect(dialog(page)).toBeVisible();
  await dialog(page).getByRole('button', { name: 'Permissions' }).click();

  const grid = dialog(page).getByRole('table', { name: 'Permissions by area' });
  for (const area of ['Environment', 'Files', 'Terminal', 'Browsers', 'VNC']) await expect(grid.getByRole('rowheader', { name: area })).toBeVisible();
  // An environment is looked at or managed: no middle column, and each Manage cell says what it is.
  await expect(grid.getByRole('columnheader', { name: /Interact|Use/ })).toHaveCount(0);
  await expect(grid).toContainText('They change the files here');
  await expect(grid).toContainText('A shell here is everything here: every file, every process, the secrets given to it');
  await expect(grid).toContainText('They drive its browsers, and act at every site as whoever signed in to it there');
  await expect(grid).toContainText('They work in whatever its displays show');

  // Files to look, and a terminal: two doors, given alone.
  await dialog(page).getByRole('button', { name: 'Add to View Files' }).click();
  await page.getByRole('combobox', { name: 'Add to View Files' }).fill('zoe');
  await page.getByRole('option', { name: /Zoe Outsider/ }).click();
  await dialog(page).getByRole('button', { name: 'Add to Manage Terminal' }).click();
  await page.getByRole('combobox', { name: 'Add to Manage Terminal' }).fill('zoe');
  await page.getByRole('option', { name: /Zoe Outsider/ }).click();
  await expect(grid.getByRole('group', { name: 'View Files' })).toContainText('Zoe Outsider');
  await expect(grid.getByRole('group', { name: 'Manage Terminal' })).toContainText('Zoe Outsider');
  await expect(grid.getByRole('group', { name: 'Manage Files' })).not.toContainText('Zoe Outsider');
  await dialog(page).getByRole('button', { name: 'Back to the short form' }).click();
  // Somebody who holds some doors and none of the three choices whole is listed as Custom, with what they hold.
  await expect(dialog(page)).toContainText('View Files · Manage Terminal');
});

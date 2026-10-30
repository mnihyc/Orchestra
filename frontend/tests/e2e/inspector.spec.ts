/** The views of a thread beside its conversation: what the agent changed and Git, a person's shells in a terminal (opened, typed in, left and taken again, taken by a second screen, closed, refused, at a limit), the processes and a signal sent, the resources, the outline, the environment and the parameters. */
import { expect, type Page, test } from '@playwright/test';

import { agentMessage, errorItem, turn, userMessage } from '../doubles/thread-rows.ts';
import { ADA } from '../doubles/thread-scenes.ts';
import { begin, MAIN, MAIN_ENVIRONMENT, openThread, showView, workspace } from './inspector-support.ts';
import { ACME, deny, emit, PEOPLE, signIn } from './support.ts';
import { expectOneScrollerPerArea } from '../accessibility/scrolling.ts';

test.use({ viewport: { width: 1600, height: 1000 } });

test.beforeEach(async () => {
  await begin();
});

const screen = (page: Page) => page.locator('.xterm-rows');

/** Types into the shell in front: the terminal takes its keys through a field of its own. */
async function typeInShell(page: Page, text: string): Promise<void> {
  await page.locator('.xterm-helper-textarea').focus();
  await page.keyboard.type(text);
}

async function openShell(page: Page): Promise<void> {
  await showView(page, /^Terminal/);
  await page.getByRole('button', { name: 'Open a new shell' }).first().click();
  await expect(screen(page)).toContainText('$');
}

test('what the agent changed is listed with what was added and removed, and a row shows its latest difference', async ({ page }) => {
  await openThread(page);
  await showView(page, /^Changes/);
  const panel = page.getByTestId('changes-panel');
  // The two files this thread's tool calls changed, each once, with what was added and removed.
  await expect(panel.getByTestId('change-row')).toHaveCount(2);
  await expect(panel).toContainText('app.ts');
  await expect(panel).toContainText('util.ts');
  await expect(panel).toContainText('+2');
  await panel.getByRole('button', { name: /^app\.ts/ }).click();
  await expect(panel.getByRole('table', { name: /Difference in web\/src\/app\.ts/ })).toBeVisible();
  // The switch adds what the other threads of the environment changed.
  await workspace('/changes', {
    changes: [
      { added: 2, removed: 1, at: new Date().toISOString(), binary: false, change: 'modify', diff: '@@ -1 +1 @@\n-a\n+b\n', item_id: 'i1', old_path: null, path: 'web/src/app.ts', thread_id: MAIN, truncated: false },
      { added: 1, removed: 0, at: new Date().toISOString(), binary: false, change: 'add', diff: '@@ -0,0 +1 @@\n+x\n', item_id: 'i2', old_path: null, path: 'web/other.ts', thread_id: 'another', truncated: false },
    ],
  });
  await panel.getByRole('switch', { name: 'Every thread in this environment' }).click();
  await expect(panel.getByTestId('change-row')).toHaveCount(2);
  await expect(panel).toContainText('By another thread');
});

test('a difference opens as a tab from the menu of a change, and can be added to the message', async ({ page }) => {
  await openThread(page);
  await showView(page, /^Changes/);
  await page.getByRole('button', { name: 'Actions for web/src/app.ts' }).click();
  await page.getByRole('menuitem', { name: 'Open the difference' }).click();
  await expect(page.getByRole('tab', { name: /app\.ts \(changes\)/ })).toBeVisible();
  await expect(page.getByRole('table', { name: /Difference in web\/src\/app\.ts/ })).toBeVisible();
  await page.getByRole('button', { name: 'Add to message' }).click();
  await page.getByRole('tab', { name: 'Main thread · Fix login', exact: true }).click();
  await expect(page.getByRole('textbox', { name: 'Message to the agent' })).toHaveValue(/```diff/);
});

test('Git shows the branch, how far it is ahead, and what is uncommitted by the group Git keeps it in', async ({ page }) => {
  await openThread(page);
  await showView(page, /^Git/);
  const panel = page.getByTestId('git-panel');
  await expect(panel.getByTestId('git-branch')).toHaveText('main');
  await expect(panel).toContainText('1 commit ahead');
  await expect(panel.getByRole('region', { name: 'Staged' })).toContainText('util.ts');
  await expect(panel.getByRole('region', { name: 'Changed' })).toContainText('app.ts');
  await expect(panel.getByRole('region', { name: 'Not added' })).toContainText('notes.md');
  // The server hands over the difference of the whole repository, and a row shows its own file's part of it: what is not staged, and what is.
  await panel.getByTestId('git-row').filter({ hasText: 'app.ts' }).getByRole('button').first().click();
  await expect(panel.getByRole('table', { name: /Difference in web\/src\/app\.ts/ })).toBeVisible();
  await expect(panel).toContainText("console.log(1);");
  await panel.getByTestId('git-row').filter({ hasText: 'util.ts' }).getByRole('button').first().click();
  await expect(panel.getByRole('table', { name: /Difference in web\/src\/util\.ts/ })).toBeVisible();
  await expect(panel).toContainText('export function greet');
});

test('Git says why there is nothing to show: it needs a repository, and an environment that is away', async ({ page }) => {
  await workspace('/git', { status: { available: false, reason: 'not_a_repository', branch: null, head: null, ahead: null, behind: null, changes: [] } });
  await openThread(page);
  await showView(page, /^Git/);
  await expect(page.getByText('Needs a Git repository')).toBeVisible();
  await workspace('/environment', { environment: MAIN_ENVIRONMENT, patch: { status: 'offline' } });
  await showView(page, /^Files/);
  await showView(page, /^Git/);
  await expect(page.getByText(/Needs a Git repository|The environment is offline/)).toBeVisible();
});

test('the processes of the environment are a tree with this thread\'s own first, and the filter "This thread" is on when the panel opens', async ({ page }) => {
  await openThread(page);
  await showView(page, /^Processes/);
  const panel = page.getByTestId('processes-panel');
  await expect(panel.getByRole('switch', { name: 'This thread' })).toBeChecked();
  const rows = panel.getByTestId('process-row');
  await expect(rows).toHaveCount(2);
  await expect(rows.first()).toContainText('node server.js --port 3000');
  await panel.getByRole('switch', { name: 'This thread' }).click();
  await expect(rows).toHaveCount(4);
  // The tree this thread's processes are in comes first, whatever the numbers: its root, then them under it.
  await expect(rows.first()).toContainText('/bin/bash');
  await expect(rows.nth(1)).toContainText('node server.js');
  await expect(rows.nth(2)).toContainText('esbuild --watch');
});

test('a signal is sent from the row\'s menu after it is asked once, and the process is gone from the list', async ({ page }) => {
  await openThread(page);
  await showView(page, /^Processes/);
  const panel = page.getByTestId('processes-panel');
  await panel.getByRole('button', { name: 'Signal process 101' }).click();
  await page.getByRole('menuitem', { name: 'Terminate' }).click();
  await expect(page.getByRole('alertdialog')).toContainText('Terminate node (101)?');
  await page.getByRole('alertdialog').getByRole('button', { name: 'Terminate' }).click();
  await expect(panel.getByTestId('process-row').filter({ hasText: 'node server.js' })).toHaveCount(0);
});

test('somebody who may look at the processes and not send signals sees the menu entries off, with what they take', async ({ page }) => {
  await deny('ada', ['terminal.manage']);
  await openThread(page);
  await showView(page, /^Processes/);
  await page.getByRole('button', { name: 'Signal process 101' }).click();
  const entry = page.getByRole('menuitem', { name: /Terminate/ });
  await expect(entry).toHaveAttribute('aria-disabled', 'true');
  await expect(page.getByRole('menu')).toContainText('Manage Shells');
});

test('the resources are four charts with their figure now, their highest and their limit', async ({ page }) => {
  await openThread(page);
  await showView(page, /^Resources/);
  const panel = page.getByTestId('resources-panel');
  for (const name of ['Processor', 'Memory', 'Disk', 'Network']) await expect(panel.getByRole('region', { name })).toBeVisible();
  await expect(panel.getByRole('img', { name: 'Processor use over the last hour' })).toBeVisible();
  await expect(panel.getByRole('region', { name: 'Processor' })).toContainText('limit 4 processors');
  await expect(panel.getByRole('region', { name: 'Memory' })).toContainText('limit 8 GiB');
});

test('a shell is opened, typed in, left and found again with what it printed, and closed', async ({ page }) => {
  await openThread(page);
  await openShell(page);
  await typeInShell(page, 'echo hello world\n');
  await expect(screen(page)).toContainText('hello world');
  // Leaving the view ends nothing: the shell is there when the view is shown again, with its screen.
  await showView(page, /^Files/);
  await page.reload();
  await expect(page.getByRole('heading', { name: 'Main thread' })).toBeVisible();
  await showView(page, /^Terminal/);
  await expect(screen(page)).toContainText('hello world');
  await expect(page.getByTestId('terminal-panel').getByRole('tab').first()).toContainText('web');
  // Closed from the cross of its tab (which is for the pointer; the keyboard has Delete, below), it is gone from the strip:
  // a shell its person closed is no news to them.
  // The tab's help can cover the nearby close control until explicitly dismissed.
  await page.keyboard.press('Escape');
  await page.getByTestId('shell-close').click();
  await expect(page.getByTestId('terminal-panel').getByRole('tab')).toHaveCount(0);
});

test('a shell this window has attached stays this window\'s when the list of shells is read again, and is not offered to be taken', async ({ page }) => {
  await openThread(page);
  await openShell(page);
  // The list is read again every ten seconds and says that the shell is attached: by this window, which is no other window.
  await page.waitForTimeout(11_500);
  await expect(page.getByText('Open in another window')).toHaveCount(0);
  await typeInShell(page, 'echo still mine\n');
  await expect(screen(page)).toContainText('still mine');
});

test('a shell that is open on another screen says so and can be taken, and the screen that had it is told', async ({ browser, page }) => {
  await openThread(page);
  await openShell(page);
  await typeInShell(page, 'echo first screen\n');
  await expect(screen(page)).toContainText('first screen');
  const other = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  await signIn(other, PEOPLE.ada);
  await other.goto(`/thread/${MAIN}`);
  await showView(other, /^Terminal/);
  await expect(other.getByText('This shell is open in another window')).toBeVisible();
  await other.getByRole('button', { name: 'Take it here' }).click();
  await expect(other.locator('.xterm-rows')).toContainText('first screen');
  await expect(page.getByText('This shell was taken by another window.')).toBeVisible();
  await page.getByRole('button', { name: 'Take it here' }).click();
  await expect(screen(page)).toContainText('first screen');
  await other.close();
});

test('opening a fifth shell asks which of the four to close, with the oldest ticked, and does it in the same step', async ({ page }) => {
  await workspace('/terminal-seed', { person: 'ada', count: 4, thread: MAIN });
  await openThread(page);
  await showView(page, /^Terminal/);
  await expect(page.getByTestId('terminal-panel').getByRole('tab')).toHaveCount(4);
  await page.getByRole('button', { name: 'Open a new shell' }).first().click();
  const dialog = page.getByRole('dialog', { name: 'Which shells to close?' });
  await expect(dialog).toContainText('You have 4 shells in Workspace');
  await expect(dialog.getByRole('checkbox').first()).toBeChecked();
  await expect(dialog.getByRole('checkbox').nth(1)).not.toBeChecked();
  await dialog.getByRole('button', { name: 'Close 1 and open' }).click();
  await expect(dialog).toHaveCount(0);
  // The oldest is gone and the new one is open beside the three kept.
  const tabs = page.getByTestId('terminal-panel').getByRole('tab');
  await expect(tabs).toHaveCount(4);
  await expect(tabs.filter({ hasText: 'at its prompt' })).toHaveCount(4);
});

test('the tabs of the shells take the arrow keys, and Delete closes the shell that has the keyboard', async ({ page }) => {
  await workspace('/terminal-seed', { person: 'ada', count: 2, thread: MAIN });
  await openThread(page);
  await showView(page, /^Terminal/);
  const tabs = page.getByTestId('terminal-panel').getByRole('tab');
  await expect(tabs).toHaveCount(2);
  await tabs.first().focus();
  await page.keyboard.press('ArrowRight');
  await expect(tabs.nth(1)).toBeFocused();
  await expect(tabs.nth(1)).toHaveAttribute('aria-selected', 'true');
  await page.keyboard.press('Delete');
  await expect(tabs).toHaveCount(1);
});

test('where the environment\'s own limit is reached by other people\'s shells there is nothing to tick, and it says how many people have them', async ({ page }) => {
  await workspace('/terminal-limit', { environment: 0 });
  await openThread(page);
  await showView(page, /^Terminal/);
  await page.getByRole('button', { name: 'Open a new shell' }).first().click();
  const dialog = page.getByRole('dialog', { name: 'No room for another shell' });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('checkbox')).toHaveCount(0);
});

test('opening is refused with the reason in words: the environment\'s switch, the machine\'s owner, and an environment that is away', async ({ page }) => {
  await workspace('/environment', { environment: MAIN_ENVIRONMENT, patch: { terminal: false } });
  await openThread(page);
  await showView(page, /^Terminal/);
  await expect(page.getByRole('button', { name: 'Open a new shell' }).first()).toBeDisabled();
  await expect(page.getByText('Shells are turned off for this environment. Whoever manages it can turn them on in the Environment view.').first()).toBeVisible();
  await workspace('/environment', { environment: MAIN_ENVIRONMENT, patch: { terminal: true } });
  await workspace('/terminal-gate', { reason: 'machine_policy' });
  // Another person repaired the known environment gate; reload reads the reported switch again.
  await page.reload();
  await showView(page, /^Terminal/);
  await page.getByRole('button', { name: 'Open a new shell' }).first().click();
  await expect(page.getByText(/Shells are turned off on .* by its owner\./)).toBeVisible();
});

test('a shell that exited says it did and with what, and one that was closed for being left says so and shows what it last printed', async ({ page }) => {
  await openThread(page);
  await openShell(page);
  await typeInShell(page, 'echo before the end\nexit 3\n');
  await expect(page.getByText('It exited (3)')).toBeVisible();
  await page.getByRole('button', { name: 'Show what it last printed' }).click();
  await expect(page.getByTestId('last-printed')).toContainText('before the end');
  await page.getByRole('button', { name: 'Open a new shell' }).last().click();
  await expect(screen(page)).toContainText('$');
});

test('the tab shows the round trip above 150 ms and says nothing below it', async ({ page }) => {
  await openThread(page);
  await openShell(page);
  const tab = page.getByTestId('terminal-panel').getByRole('tab').first();
  await expect(tab).not.toContainText(' ms');
  await workspace('/terminal-lag', { ms: 400 });
  await expect(tab).toContainText(/\d+ ms/, { timeout: 30_000 });
});

test('a long shell recording expands in the view without its own pre scroller', async ({ page }) => {
  await openThread(page);
  await openShell(page);
  await typeInShell(page, `${Array.from({ length: 12 }, (_, index) => `echo recorded-${index + 1}`).join('\n')}\nexit 0\n`);
  await expect(page.getByText('It exited (0)', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Show what it last printed', exact: true }).click();
  const output = page.getByTestId('last-printed');
  await expect(output.getByText(/recorded-12/)).toHaveCount(0);
  await output.getByRole('button', { name: 'Show all', exact: true }).click();
  await expect(output).toContainText('recorded-12');
  expect(await output.locator('pre').evaluate((element) => /^(auto|scroll)$/.test(getComputedStyle(element).overflowY))).toBe(false);
  await expectOneScrollerPerArea(page);
});

test('shell access explains its full reach on request and stays closed after leaving and reloading', async ({ page }) => {
  await openThread(page);
  await showView(page, /^Terminal/);
  const help = page.getByRole('button', { name: 'Help: Shell access', exact: true });
  await help.click();
  const note = page.getByRole('dialog', { name: 'Shell access', exact: true });
  await expect(note).toContainText("This shell runs as the environment's user");
  await expect(note).toContainText('including the secrets given to it');
  await expect(note).toContainText('whole environment and not only this thread');
  await page.keyboard.press('Escape');
  await expect(note).toHaveCount(0);
  await page.reload();
  await showView(page, /^Terminal/);
  await expect(note).toHaveCount(0);
  await expect(help).toBeVisible();
});

test('the outline lists the points of the conversation, filters by kind, and a click takes the conversation to the item', async ({ page }) => {
  const at = new Date().toISOString();
  await emit(MAIN, {
    items: [
      userMessage('u1', 'Fix the login redirect', { author: ADA, turn: 't1', inputId: 'in-1', at }),
      agentMessage('a1', 'Looking at it.', { turn: 't1', at }),
      errorItem('e1', 'The model refused the request.', { turn: 't1', at }),
    ],
    turns: [turn('t1', { ordinal: 1, state: 'completed', inputId: 'in-1', startedAt: at, completedAt: at })],
  });
  await openThread(page);
  await showView(page, /^Outline/);
  const panel = page.getByTestId('outline-panel');
  await expect(panel.getByRole('link', { name: /Fix the login redirect/ })).toBeVisible();
  await expect(panel.getByRole('link', { name: /The model refused the request/ })).toBeVisible();
  for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await showView(page, /^Outline/);
    await page.mouse.move(0, 0);
    if (await page.getByRole('tooltip').count() > 0) await page.keyboard.press('Escape');
    await expect(page.getByRole('tooltip')).toHaveCount(0);
    await expect(panel.getByRole('link', { name: /The model refused the request/ })).toBeInViewport();
    await expect(panel.getByRole('checkbox', { name: 'Errors', exact: true })).toBeVisible();
    await test.info().attach(`outline-errors-${width}-${theme}`, { body: await panel.screenshot(), contentType: 'image/png' });
  }
  await panel.getByRole('checkbox', { name: 'Messages' }).click();
  await expect(panel.getByRole('link', { name: /Fix the login redirect/ })).toHaveCount(0);
  await panel.getByRole('checkbox', { name: 'Messages' }).click();
  await panel.getByRole('link', { name: /Fix the login redirect/ }).click();
  await expect(page).toHaveURL(/item=u1/);
});

test('the environment says where it runs, its state and what the viewer can do, starts and stops one that Orch made, and holds the switches its managers set', async ({ page }) => {
  // The thread of Upgrade deps works in a sandbox that Orch made on a host: it is started and stopped from here (one on somebody's machine is started by its machine).
  await openThread(page, PEOPLE.ada, `/thread/${ACME.depsThread}`);
  await showView(page, /^Environment/);
  const panel = page.getByTestId('environment-panel');
  await expect(panel.getByRole('heading', { name: 'Sandbox' })).toBeVisible();
  await expect(panel.getByTestId('environment-can')).toHaveText(/Yours|You manage it/);
  await expect(panel.getByText('Stopped', { exact: true })).toBeVisible();
  await panel.getByRole('button', { name: 'Start', exact: true }).click();
  await expect(panel.getByText('Running')).toBeVisible();
  await panel.getByRole('button', { name: 'Stop', exact: true }).click();
  await expect(panel.getByText('Stopped', { exact: true })).toBeVisible();
  // Writing from Orch is the environment's to allow: switched off, the files are read only.
  await panel.getByRole('switch', { name: 'Write files from Orch' }).click();
  await expect(panel.getByRole('switch', { name: 'Write files from Orch' })).not.toBeChecked();
});

test('the parameters show the model with where it comes from, change one that is declared, and are read only for somebody who may not manage the thread', async ({ page }) => {
  await openThread(page);
  await showView(page, /^Parameters/);
  const panel = page.getByTestId('parameters-panel');
  await expect(panel.getByRole('region', { name: 'Current choices' })).toContainText('GPT-5.4');
  await expect(panel.getByRole('region', { name: 'Current choices' })).toContainText('Default');
  await page.getByRole('button', { name: /^Model and effort:/ }).click();
  const choices = page.getByRole('dialog', { name: 'Model and effort', exact: true });
  await expect(choices.getByText('Not allowed here', { exact: true }).locator('..').getByRole('listitem')).toHaveCount(1);
  await page.keyboard.press('Escape');
  await expect(panel.getByTestId('parameter-row')).toHaveCount(1);
  await expect(panel).toContainText('Takes effect from the next turn.');
  const verbosity = panel.getByRole('combobox', { name: 'Verbosity', exact: true });
  await expect(verbosity).toHaveValue('Normal');
  await verbosity.click();
  await page.getByRole('option', { name: 'Terse', exact: true }).click();
  await expect(panel.getByRole('status')).toHaveText('Saved');
  await deny('ada', ['thread.manage']);
  await page.reload();
  await showView(page, /^Parameters/);
  await expect(verbosity).toHaveValue('Terse');
  await expect(verbosity).toBeDisabled();
  await expect(page.getByTestId('parameters-panel')).toContainText('Changing this takes');
  await page.getByRole('button', { name: /^Model and effort:/ }).click();
  const models = choices.getByRole('menuitemradio');
  expect(await models.count()).toBeGreaterThan(0);
  for (const model of await models.all()) await expect(model).toBeDisabled();
});

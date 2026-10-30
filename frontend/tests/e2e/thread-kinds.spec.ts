/** One of every kind of item, step and request of a harness in one conversation: none breaks the page, each says what it is, and the requests that are more than a button (questions to answer, a form an MCP server asks for, a change to files, more permissions) are decided in place. */
import { expect, type Page, test } from '@playwright/test';

import { ACME, emit, expectAccessibleInBothThemes, playScene, scenario, signIn } from './support.ts';

const MAIN = ACME.mainThread;

test.beforeEach(async () => {
  await scenario();
  await emit(MAIN, { thread: { state: 'idle' } });
});

const conversation = (page: Page) => page.getByRole('log', { name: 'Conversation' });

/**
 * The conversation is about 5 500 pixels tall, and the list draws what is in view and eight rows beyond it. This window is tall enough
 * for that to be every row of it: in a window of 4 200 pixels the first row (a notice of the system) was drawn only while the
 * header was a line shorter.
 */
const WINDOW_HEIGHT = 5_400;

async function open(page: Page, height = WINDOW_HEIGHT): Promise<string[]> {
  const broken: string[] = [];
  page.on('pageerror', (error) => broken.push(error.message));
  await page.setViewportSize({ width: 1280, height });
  await playScene(MAIN, 'everything');
  await signIn(page);
  await page.goto(`/thread/${MAIN}`);
  await expect(conversation(page).getByText('Working through the list.')).toBeVisible();
  return broken;
}

test('every kind of item and of step is drawn, says what it is, and breaks nothing', async ({ page }) => {
  const broken = await open(page);
  const log = conversation(page);
  const step = (name: RegExp) => log.getByRole('button', { name }).first();

  // What the system and others put into the conversation, and the thinking and the plan of the agent.
  await expect(log.getByText('The environment was restarted while this turn ran.')).toBeVisible();
  await expect(log.getByRole('region', { name: 'Announcement' }).getByText('Release freeze')).toBeVisible();
  await expect(log.getByRole('region', { name: 'Announcement' }).getByText('No merges until Monday.')).toBeVisible();
  await expect(step(/^Thought for/)).toBeVisible();
  await expect(log.getByRole('region', { name: 'Plan' }).getByText('Change the redirect')).toBeVisible();

  // Each kind of step, by its words.
  await expect(step(/^npm test exit 1/)).toBeVisible();
  await expect(step(/^Searched for “redirect” in src/)).toBeVisible();
  await expect(step(/^Listed src /)).toBeVisible();
  await expect(step(/^Edited src\/login\.ts/)).toBeVisible();
  await expect(step(/^Edited 4 files/)).toBeVisible();
  await expect(step(/^Read src\/login\.ts · lines 40–42/)).toBeVisible();
  await expect(step(/^Listed login\*/)).toBeVisible();
  await expect(step(/^Searched the web: redirect after login best practice/)).toBeVisible();
  await expect(step(/^Fetched example\.org\/safe/)).toBeVisible();
  await expect(step(/^Looked at login\.png/)).toBeVisible();
  await expect(step(/^tracker · search · login bug/)).toBeVisible();
  await expect(step(/^tracker · get_issue/)).toBeVisible();
  await expect(log.getByText('The tracker is not reachable.')).toBeVisible();
  await expect(step(/^Sub-agent reviewer/)).toBeVisible();
  await expect(step(/^Listed shared files/)).toBeVisible();
  await expect(step(/^custom_tool/)).toBeVisible();
  // A change to several files says what each came to.
  await expect(log.getByText('src/new.ts', { exact: false }).first()).toBeVisible();
  await expect(log.getByText('This file is not text, so there is no difference to show.')).toBeVisible();
  await expect(log.getByText('The model refused one request and the harness went on.')).toBeVisible();

  // Four requests that are a button wait, two that are more, and one was answered.
  await expect(log.getByRole('button', { name: /^Allow once · / })).toHaveCount(4);
  for (const target of ['rm -rf build && npm run build', 'Change src/login.ts', 'More permissions', 'Something else the harness asks']) {
    await expect(log.getByRole('button', { name: `Allow once · ${target}`, exact: true })).toBeVisible();
  }
  await expect(log.getByRole('button', { name: /^Run npm install · Allow once by Grace Hopper/ })).toBeVisible();

  // The beginning of it: the history lines, and a person's words with an image and a file.
  await log.evaluate((element) => {
    element.scrollTop = 0;
  });
  await expect(log.getByText('Look at these, then fix it:')).toBeVisible();
  await expect(log.getByText('screenshot.png')).toBeVisible();
  await expect(log.getByText('notes.txt')).toBeVisible();
  await expect(log.getByText(/renamed this thread to Everything at once/)).toBeVisible();
  expect(broken).toEqual([]);
  await expectAccessibleInBothThemes(page);
});

/** Taller than the whole conversation of the scene (about 5 500 pixels, header and composer apart), so that there is nothing to scroll. */
const WINDOW_TALLER_THAN_THE_CONVERSATION = 7_000;

test('a conversation shorter than its window is drawn whole, from its first row to its last', async ({ page }) => {
  const broken = await open(page, WINDOW_TALLER_THAN_THE_CONVERSATION);
  const log = conversation(page);
  // Nothing to scroll: every row is in view at once, and none of them may be missing.
  expect(await log.evaluate((element) => element.scrollHeight <= element.clientHeight)).toBe(true);
  await expect(log.getByText(/renamed this thread to Everything at once/)).toBeInViewport();
  await expect(log.getByText('Look at these, then fix it:')).toBeInViewport();
  await expect(log.getByText('The environment was restarted while this turn ran.')).toBeInViewport();
  await expect(log.getByRole('region', { name: 'Plan' }).getByText('Change the redirect')).toBeInViewport();
  await expect(log.getByText('Working through the list.')).toBeInViewport();
  expect(broken).toEqual([]);
});

test('questions of the agent are answered in place, one option, several, and words of one’s own', async ({ page }) => {
  await open(page);
  const card = conversation(page).getByRole('region', { name: 'The agent asks you' });
  await expect(card).toBeVisible();
  // Nothing is sent before every question has its answer.
  await expect(card.getByRole('button', { name: 'Send answers' })).toBeDisabled();
  await card.getByRole('radio', { name: /Vitest/ }).check();
  await card.getByRole('checkbox', { name: /Types/ }).check();
  await card.getByRole('checkbox', { name: /Lint/ }).check();
  await card.getByRole('textbox', { name: 'Is there anything it should know?' }).fill('Use the existing helper');
  await expect(card.getByRole('button', { name: 'Send answers' })).toBeEnabled();
  await card.getByRole('button', { name: 'Send answers' }).click();
  await expect(conversation(page).getByRole('button', { name: /^The agent asks you · Answered/ })).toBeVisible();
  await expect(conversation(page).getByRole('region', { name: 'The agent asks you' })).toHaveCount(0);
});

test('a form an MCP server asks for is filled in place and accepted', async ({ page }) => {
  await open(page);
  const card = conversation(page).getByRole('region', { name: 'The tracker asks for details' });
  await expect(card).toBeVisible();
  await expect(card.getByText('The tracker needs a few details to file the issue.')).toBeVisible();
  // What it needs comes first: the answer cannot be sent without it.
  await expect(card.getByRole('button', { name: 'Accept' })).toBeDisabled();
  await card.getByRole('textbox', { name: 'Title' }).fill('Login redirect');
  await card.getByRole('combobox', { name: /Priority/ }).click();
  await page.getByRole('option', { name: 'high' }).click();
  await card.getByRole('checkbox', { name: 'Urgent' }).check();
  await card.getByRole('button', { name: 'Accept' }).click();
  await expect(conversation(page).getByRole('region', { name: 'The tracker asks for details' })).toHaveCount(0);
  await expect(conversation(page).getByRole('button', { name: /^The tracker asks for details/ })).toBeVisible();
});

test('a form an MCP server asks for can be declined', async ({ page }) => {
  await open(page);
  const card = conversation(page).getByRole('region', { name: 'The tracker asks for details' });
  await card.getByRole('button', { name: 'Decline' }).click();
  await expect(conversation(page).getByRole('region', { name: 'The tracker asks for details' })).toHaveCount(0);
  await expect(conversation(page).getByRole('button', { name: /^The tracker asks for details · Denied/ })).toBeVisible();
});

test('a change to files and more permissions are decided with the options the harness gave', async ({ page }) => {
  await open(page);
  const files = conversation(page).getByRole('region', { name: 'Change src/login.ts' });
  await expect(files).toBeVisible();
  await expect(files.getByRole('table', { name: /src\/login\.ts/ })).toContainText('return redirect(next ?? "/");');
  await files.getByRole('button', { name: 'Deny' }).click();
  await expect(conversation(page).getByRole('button', { name: /^Change src\/login\.ts · Deny by Ada Lovelace/ })).toBeVisible();

  const permissions = conversation(page).getByRole('region', { name: 'More permissions' });
  await expect(permissions).toBeVisible();
  await expect(permissions.getByText('To fetch packages')).toBeVisible();
  await permissions.getByRole('button', { name: 'Allow for this session' }).click();
  await expect(conversation(page).getByRole('button', { name: /^More permissions · Allow for this session by Ada Lovelace/ })).toBeVisible();
});

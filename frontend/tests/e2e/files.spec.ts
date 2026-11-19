/** The files of a thread's workspace: the tree opened at its directory, a file opened in the code view from the tree and from the conversation, an edit saved on the version that was read and one refused because the file changed meanwhile, a search, making, renaming and deleting, a person who may read and not write, and an environment that is away. */
import { expect, type Page, test } from '@playwright/test';

import { agentMessage, change, fileChange, fileRead, turn, userMessage } from '../doubles/thread-rows.ts';
import { ADA } from '../doubles/thread-scenes.ts';
import { begin, editor, fileTree, filesPanel, MAIN, MAIN_ENVIRONMENT, onDisk, openThread, row, showView, workspace } from './inspector-support.ts';
import { deny, emit } from './support.ts';

// The workbench has three columns here; a wide window keeps the conversation readable beside a file.
test.use({ viewport: { width: 1600, height: 1000 } });

test.beforeEach(async () => {
  await begin();
});

/** The tree with `src` opened, which is where the code of the fake project is. */
async function openTree(page: Page): Promise<void> {
  await openThread(page);
  await showView(page, /^Files/);
  await expect(fileTree(page)).toBeVisible();
  await row(page, /^src/).click();
  await expect(row(page, /^app\.ts/)).toBeVisible();
}

async function openApp(page: Page): Promise<void> {
  await openTree(page);
  await row(page, /^app\.ts/).click();
  await expect(page.getByRole('textbox', { name: 'Text of app.ts' })).toBeVisible();
}

/** Types at the very beginning of the file in front. */
async function typeAtStart(page: Page, text: string): Promise<void> {
  await editor(page).click();
  await page.keyboard.press('Control+Home');
  await page.keyboard.type(text);
}

test('the tree opens at the thread\'s directory, and a file opens in the code view beside the conversation', async ({ page }) => {
  await openThread(page);
  await showView(page, /^Files/);
  await expect(filesPanel(page).getByRole('heading', { name: 'web/ in Workspace' })).toBeVisible();
  await expect(filesPanel(page).getByText('Live', { exact: true })).toBeVisible();
  // The directories come first, and what is there is what the fake workspace holds under web/.
  const names = await fileTree(page).getByRole('treeitem').allInnerTexts();
  expect(names[0]).toContain('src');
  await expect(row(page, /^README\.md/)).toBeVisible();
  await row(page, /^src/).click();
  await row(page, /^app\.ts/).click();
  await expect(page.getByRole('tab', { name: 'app.ts' })).toBeVisible();
  await expect(editor(page)).toContainText("import { greet } from './util';");
  // The conversation stays where it was.
  await expect(page.getByRole('tab', { name: 'Main thread' })).toBeVisible();
  // Looking at another file gives the tab of the one that was only looked at to the next.
  await row(page, /^util\.ts/).click();
  await expect(page.getByRole('tab', { name: 'util.ts' })).toBeVisible();
  await expect(page.getByRole('tab', { name: 'app.ts' })).toHaveCount(0);
  await expect(editor(page)).toContainText('export const VERSION = 3;');
});

test('a path in the conversation opens its file at the line it names', async ({ page }) => {
  const began = new Date().toISOString();
  await emit(MAIN, {
    items: [
      userMessage('u1', 'Show me the entry point', { author: ADA, turn: 't1', inputId: 'in-1', at: began }),
      fileRead('r1', { path: 'src/app.ts', content: 'export function main(name: string): string {', from: 4, to: 4, turn: 't1', at: began }),
      fileChange('c1', { changes: [change('src/util.ts', '@@ -1 +1,2 @@\n export function greet() {}\n+export const VERSION = 3;\n', { added: 1, removed: 0 })], turn: 't1', at: began }),
      agentMessage('a1', 'Done.', { turn: 't1', at: began }),
    ],
    turns: [turn('t1', { ordinal: 1, state: 'completed', inputId: 'in-1', startedAt: began, completedAt: began })],
  });
  await openThread(page);
  // The step that read the file is opened, and the path it names is a link.
  await page.getByRole('button', { name: /^Read src\/app\.ts/ }).click();
  await page.getByRole('button', { name: 'src/app.ts', exact: true }).click();
  await expect(page.getByRole('tab', { name: 'app.ts' })).toBeVisible();
  // The view is brought to the line the item names (the line is lit for a moment, which a slow machine can miss: the cursor stays).
  await expect(page.getByTestId('code-status')).toContainText('Line 4, column 1');
  await page.getByRole('tab', { name: /^Main thread(?: · Fix login)?$/ }).click();
  // A change names its file the same way, and a path of the workspace that is not there says so.
  const changeDetails = page.getByRole('button', { name: /^Edited src\/util\.ts/ });
  if (await changeDetails.getAttribute('aria-expanded') !== 'true') await changeDetails.click();
  await page.getByRole('button', { name: 'Open web/src/util.ts', exact: true }).click();
  await expect(page.getByRole('tab', { name: 'util.ts' })).toBeVisible();
  await expect(editor(page)).toContainText('export const VERSION = 3;');
});

test('an edit is saved on the version that was read, and what is written lands on the disk of the environment', async ({ page }) => {
  await openApp(page);
  await expect(page.getByRole('button', { name: 'Save' })).toBeDisabled();
  await typeAtStart(page, '// edited\n');
  // The tab says it holds something unsaved, and so does the line under the text.
  await expect(page.getByTestId('code-status')).toContainText('Unsaved');
  await expect(page.getByRole('tab', { name: /app\.ts/ })).toHaveAccessibleName(/unsaved/i);
  await page.keyboard.press('Control+s');
  await expect(page.getByTestId('code-status')).not.toContainText('Unsaved');
  expect(await onDisk('web/src/app.ts')).toMatch(/^\/\/ edited\nimport \{ greet \}/);
});

test('a file that changed while it was being edited is shown and never overwritten: Compare, Keep mine and Take theirs', async ({ page }) => {
  await openApp(page);
  await typeAtStart(page, '// mine\n');
  await workspace('/put', { files: { 'web/src/app.ts': 'export const theirs = true;\n' } });
  await expect(page.getByText('This file changed while you were editing')).toBeVisible();
  // Saving now is refused by the server, and nothing is written.
  await page.getByRole('button', { name: 'Save' }).click();
  expect(await onDisk('web/src/app.ts')).toBe('export const theirs = true;\n');
  // Compare shows the file as it is now beside the person's text, which stays editable.
  await page.getByRole('button', { name: 'Compare' }).click();
  await expect(page.getByTestId('their-version')).toContainText('export const theirs = true;');
  await expect(editor(page).first()).toContainText('// mine');
  // Saving from there writes on the new version.
  await page.getByRole('button', { name: 'Save' }).click();
  await expect.poll(() => onDisk('web/src/app.ts')).toMatch(/^\/\/ mine\n/);
  await expect(page.getByText('This file changed while you were editing')).toHaveCount(0);
});

test('Take theirs puts the file as it is now in place of the typed text, and Keep mine keeps the text to be saved over it', async ({ page }) => {
  await openApp(page);
  await typeAtStart(page, '// first\n');
  await workspace('/put', { files: { 'web/src/app.ts': 'export const second = 2;\n' } });
  await page.getByRole('button', { name: 'Take theirs' }).click();
  await expect(editor(page)).toContainText('export const second = 2;');
  await expect(editor(page)).not.toContainText('// first');
  await expect(page.getByTestId('code-status')).not.toContainText('Unsaved');

  await typeAtStart(page, '// third\n');
  await workspace('/put', { files: { 'web/src/app.ts': 'export const fourth = 4;\n' } });
  await page.getByRole('button', { name: 'Keep mine' }).click();
  await page.getByRole('button', { name: 'Save' }).click();
  await expect.poll(() => onDisk('web/src/app.ts')).toMatch(/^\/\/ third\nexport const second = 2;/);
});

test('a tab without unsaved text follows the file when somebody else writes it, and marks the lines that changed', async ({ page }) => {
  await openApp(page);
  await workspace('/put', { files: { 'web/src/app.ts': "// the agent wrote this\nimport { greet } from './util';\n" } });
  await expect(editor(page)).toContainText('// the agent wrote this');
  await expect(page.locator('.cm-agent-line').first()).toContainText('// the agent wrote this');
});

test('what was typed and not saved is kept on the device and put back when the page is loaded again', async ({ page }) => {
  await openApp(page);
  await typeAtStart(page, '// kept for later\n');
  // The text is kept a moment after the last key.
  await expect.poll(() => page.evaluate(() => Object.keys(localStorage).filter((key) => key.includes('.unsaved.')).length)).toBe(1);
  await page.reload();
  await page.getByRole('tab', { name: /app\.ts/ }).click();
  await expect(editor(page)).toContainText('// kept for later');
  await expect(page.getByText(/Your unsaved text from .* was put back/)).toBeVisible();
  await expect(page.getByTestId('code-status')).toContainText('Unsaved');
});

test('the search by name lists paths, and the search in files lists the lines that hold the words and opens a hit at its line', async ({ page }) => {
  await openThread(page);
  await showView(page, /^Files/);
  await filesPanel(page).getByRole('button', { name: 'Search the files', exact: true }).click();
  const search = filesPanel(page).getByRole('textbox', { name: 'Search the files' });
  await search.fill('util');
  await expect(page.getByTestId('search-results').getByRole('button', { name: 'web/src/util.ts' })).toBeVisible();
  await filesPanel(page).getByRole('radio', { name: 'Content', exact: true }).click();
  await search.fill('greet');
  const hits = page.getByTestId('search-results');
  await expect(hits).toContainText('web/src/app.ts');
  await expect(hits).toContainText('web/src/util.ts');
  await hits.getByRole('button', { name: /const message = greet\(name\);/ }).click();
  await expect(page.getByRole('tab', { name: 'app.ts' })).toBeVisible();
  await expect(page.getByTestId('code-status')).toContainText('Line 5, column 1');
});

test('a file is made, renamed, duplicated and deleted from the menu of the tree', async ({ page }) => {
  await openThread(page);
  await showView(page, /^Files/);
  // Made from the toolbar, in the directory the tree is opened at, and opened for typing.
  await filesPanel(page).getByRole('button', { name: 'New file…' }).click();
  await page.getByRole('textbox', { name: 'Name' }).fill('todo.txt');
  await page.getByRole('button', { name: 'Create' }).click();
  await expect(row(page, /^todo\.txt/)).toBeVisible();
  expect(await onDisk('web/todo.txt')).toBe('');
  // Renamed from its menu.
  await row(page, /^todo\.txt/).click({ button: 'right' });
  await page.getByRole('menuitem', { name: /Rename/ }).click();
  await page.getByRole('textbox', { name: 'Name' }).fill('tasks.txt');
  await page.getByRole('button', { name: 'Rename' }).click();
  await expect(row(page, /^tasks\.txt/)).toBeVisible();
  await expect(row(page, /^todo\.txt/)).toHaveCount(0);
  // Duplicated.
  await row(page, /^tasks\.txt/).click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Duplicate' }).click();
  await page.getByRole('button', { name: 'Duplicate' }).click();
  await expect(row(page, /^tasks copy\.txt/)).toBeVisible();
  // Deleted, once asked, with what Orch cannot do said first.
  await row(page, /^tasks copy\.txt/).click({ button: 'right' });
  await page.getByRole('menuitem', { name: /Delete/ }).click();
  await expect(page.getByText('Orch keeps no copy of it unless a snapshot has one.')).toBeVisible();
  await page.getByRole('button', { name: 'Delete', exact: true }).click();
  await expect(row(page, /^tasks copy\.txt/)).toHaveCount(0);
  expect(await onDisk('web/tasks copy.txt')).toBeNull();
  // A name that is there already is refused where the person is looking.
  await filesPanel(page).getByRole('button', { name: 'New folder…' }).click();
  await page.getByRole('textbox', { name: 'Name' }).fill('src');
  await page.getByRole('button', { name: 'Create' }).click();
  await expect(page.getByRole('alert')).toBeVisible();
});

test('the tree follows what changes on disk without a request of the person: a file made by somebody else appears, one removed goes', async ({ page }) => {
  await openThread(page);
  await showView(page, /^Files/);
  await expect(row(page, /^notes\.txt/)).toBeVisible();
  await workspace('/put', { files: { 'web/made-by-agent.ts': 'export {};\n' } });
  await expect(row(page, /^made-by-agent\.ts/)).toBeVisible();
  await workspace('/delete', { path: 'web/notes.txt' });
  await expect(row(page, /^notes\.txt/)).toHaveCount(0);
});

test('where the live feed is not served the tree stays quiet and an explicit refresh reads changes', async ({ page }) => {
  await workspace('/feeds', { on: false });
  await openThread(page);
  await showView(page, /^Files/);
  await expect(row(page, /^notes\.txt/)).toBeVisible();
  const reads: string[] = [];
  page.on('request', (request) => { if (request.method() === 'GET' && request.url().includes('/files?')) reads.push(request.url()); });
  await workspace('/put', { files: { 'web/refreshed.ts': 'export {};\n' } });
  await page.waitForTimeout(20_000);
  expect(reads).toEqual([]);
  await expect(row(page, /^refreshed\.ts/)).toHaveCount(0);
  await expect(filesPanel(page).getByText('Snapshot', { exact: true })).toBeVisible();
  await filesPanel(page).getByRole('button', { name: 'Refresh', exact: true }).click();
  await expect(row(page, /^refreshed\.ts/)).toBeVisible();
  expect(reads).toHaveLength(1);
});

test('somebody who may read the files and not write them sees why, and the editor is read only', async ({ page }) => {
  await deny('ada', ['files.manage']);
  await openThread(page);
  await showView(page, /^Files/);
  await expect(filesPanel(page).getByRole('button', { name: 'New file…' })).toBeDisabled();
  await expect(page.getByTestId('why-read-only')).toContainText('You can’t change files in Workspace. Ask Ada Lovelace, who owns it.');
  await row(page, /^src/).click();
  await row(page, /^app\.ts/).click();
  await expect(page.getByTestId('read-only-note')).toContainText('You can read the files here. Writing them takes');
  await expect(page.getByRole('button', { name: 'Save' })).toHaveCount(0);
  await row(page, /^app\.ts/).focus();
  await page.keyboard.press('F2');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await page.keyboard.press('Delete');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await row(page, /^app\.ts/).click({ button: 'right' });
  const rename = page.getByRole('menuitem', { name: /Rename/ });
  await expect(rename).toHaveAttribute('aria-disabled', 'true');
  await expect(page.getByRole('menu')).toContainText('You can’t change files in Workspace. Ask Ada Lovelace, who owns it.');
});

test('an environment that is away shows what was opened before, from when, and cannot be written', async ({ page }) => {
  await workspace('/environment', { environment: MAIN_ENVIRONMENT, patch: { status: 'offline' } });
  await openThread(page);
  await showView(page, /^Files/);
  await expect(filesPanel(page).getByText(/Cached view from .* — the source is offline/)).toBeVisible();
  await row(page, /^src/).click();
  // A file that was opened before is shown, as the copy it is, and cannot be edited.
  await row(page, /^app\.ts/).click();
  await expect(page.getByTestId('read-only-note')).toContainText('Workspace is offline: this is the copy from');
  await expect(editor(page)).toContainText("import { greet } from './util';");
  // One that was not is said not to be here, and the search in files says what it needs.
  await row(page, /^util\.ts/).click();
  await expect(page.getByText('This file was not opened before, and its environment is offline.')).toBeVisible();
  await filesPanel(page).getByRole('button', { name: 'Search the files', exact: true }).click();
  await filesPanel(page).getByRole('radio', { name: 'Content', exact: true }).click();
  await filesPanel(page).getByRole('textbox', { name: 'Search the files' }).fill('greet');
  await expect(page.getByText('Searching inside files needs the environment online.')).toBeVisible();
});

test('a file with nothing in it opens as an empty text that can be typed in and saved', async ({ page }) => {
  // The real server answers a range of an empty file with 416 and no body: that is the file being empty, and not a failure.
  await workspace('/put', { files: { 'web/empty.txt': '' } });
  await openThread(page);
  await showView(page, /^Files/);
  await row(page, /^empty\.txt/).click();
  await expect(editor(page)).toBeVisible();
  await expect(page.getByTestId('code-status')).toContainText('Line 1, column 1');
  await expect(page.getByText('Could not load')).toHaveCount(0);
  await editor(page).click();
  await page.keyboard.type('now it says something');
  await page.getByRole('button', { name: 'Save' }).click();
  await expect.poll(() => onDisk('web/empty.txt')).toBe('now it says something');
});

test('a text of two thousand lines is cut and read only; an image, Markdown, an archive and what has no preview open in their own viewers', async ({ page }) => {
  await workspace('/put', { files: { 'web/src/big.ts': `${'x'.repeat(100)}\n`.repeat(25_000) } });
  await openThread(page);
  await showView(page, /^Files/);
  await row(page, /^src/).click();
  await row(page, /^big\.ts/).click();
  await expect(page.getByTestId('too-large')).toContainText('Only the first 2 MiB of');
  await expect(page.getByTestId('read-only-note')).toBeVisible();

  await row(page, /^logo\.png/).click();
  await expect(page.locator('[data-viewer="image"] img')).toBeVisible();
  await expect(page.getByText('1 × 1 px')).toBeVisible();

  await row(page, /^README\.md/).click();
  await expect(page.getByTestId('markdown-text')).toContainText('The page.');
  await expect(page.getByTestId('markdown-text').getByRole('listitem')).toHaveCount(2);

  // The archive is listed where it lies, and a text inside it opens read-only.
  await filesPanel(page).getByRole('button', { name: 'Menu of the tree', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Up to the parent directory' }).click();
  await expect(fileTree(page)).toBeVisible();
  await row(page, /^data/).click();
  await row(page, /^bundle\.zip/).click();
  await page.getByRole('tree', { name: 'Contents of the archive' }).getByRole('button', { name: /bundle/ }).click();
  await page.getByRole('button', { name: /readme\.txt/ }).click();
  await expect(page.getByTestId('archive-entry')).toContainText('inside the archive');
  await page.getByRole('button', { name: /big\.bin/ }).click();
  await expect(page.getByText('This entry is too large to open here.')).toBeVisible();

  // A file with no viewer says what it is and offers its three ways: download, open as text, and its hash.
  await row(page, /^blob\.bin/).click();
  const binary = page.getByRole('tabpanel', { name: 'blob.bin', exact: true });
  await expect(binary.getByText('Preview unavailable', { exact: true })).toBeVisible();
  await expect(binary).toContainText('application/octet-stream');
  await expect(binary).toContainText('7 B');
  await page.getByRole('button', { name: 'Open as text' }).click();
  await expect(page.locator('[data-viewer="code"]')).toBeVisible();
});

test('a file that is slow to arrive shows its skeleton and then the text', async ({ page }) => {
  await workspace('/slow', { path: 'web/notes.txt' });
  await openThread(page);
  await showView(page, /^Files/);
  await row(page, /^notes\.txt/).click();
  await expect(editor(page)).toContainText('second');
});

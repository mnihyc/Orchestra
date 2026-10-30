/** Real connector files, measurements, shells and two independent browsers prove the workbench's kept state. */
import { readFileSync } from 'node:fs';
import { expect, test, type Page } from '@playwright/test';

interface ViewsWalk {
  storageState: string;
  thread: string;
  environment: string;
  /** Existing writable text files and a zero-byte file in the real connector workspace. */
  file: string;
  secondFile: string;
  emptyFile: string;
  absoluteCwd: string;
  /** A real collected zero-CPU observation; its provenance is retained by the runner. */
  zeroCpuEvidence: string;
  arrangement: { save: string; name: string; confirm: string; choose: string; cancelClose: string };
}

const manifest = process.env.ORCH_VIEWS_WALK;
const walk = manifest === undefined ? undefined : JSON.parse(readFileSync(manifest, 'utf8')) as ViewsWalk;
const basename = (path: string) => path.slice(path.lastIndexOf('/') + 1);

async function view(page: Page, name: string): Promise<void> {
  await page.getByRole('tab', { name, exact: true }).first().click();
}

async function openFile(page: Page, path: string): Promise<void> {
  if (walk === undefined) throw new Error('The real workspace manifest is required.');
  await page.goto(`/thread/${walk.thread}?file=${encodeURIComponent(path)}`);
  await view(page, 'Files');
  await expect(page.locator('[data-file-tab]').filter({ has: page.getByRole('textbox', { name: `Text of ${basename(path)}`, exact: true }) })).toBeVisible();
}

async function previewFile(page: Page, path: string): Promise<void> {
  await view(page, 'Files');
  await page.getByTestId('files-panel').getByRole('textbox', { name: 'Search the files', exact: true }).fill(basename(path));
  await page.getByTestId('search-results').getByRole('button', { name: path, exact: true }).click();
  await expect(page.getByRole('textbox', { name: `Text of ${basename(path)}`, exact: true })).toBeVisible();
}

test.describe('real workspace views', () => {
  test.skip(walk === undefined || process.env.ORCH_NATIVE_URL === undefined, 'The integration runner supplies ORCH_VIEWS_WALK and authenticated real-connector fixtures.');

  test('a named arrangement crosses browsers while previews and unsaved text keep their own meaning', async ({ browser }) => {
    if (walk === undefined) return;
    const first = await browser.newContext({ baseURL: process.env.ORCH_NATIVE_URL, storageState: walk.storageState, viewport: { width: 1600, height: 1000 } });
    const second = await browser.newContext({ baseURL: process.env.ORCH_NATIVE_URL, storageState: walk.storageState, viewport: { width: 1600, height: 1000 } });
    try {
      const page = await first.newPage();
      const other = await second.newPage();
      const doubles: string[] = [];
      for (const current of [page, other]) current.on('request', (request) => { if (new URL(request.url()).pathname.startsWith('/__double')) doubles.push(request.url()); });
      await page.goto(`/thread/${walk.thread}`);
      await page.getByRole('button', { name: 'Layout', exact: true }).click();
      await page.getByRole('menuitem', { name: 'Code', exact: true }).click();
      await page.getByRole('button', { name: 'Layout', exact: true }).click();
      await page.getByRole('menuitem', { name: walk.arrangement.save, exact: true }).click();
      const dialog = page.getByRole('dialog');
      const name = `Workspace proof ${test.info().workerIndex}`;
      await dialog.getByRole('textbox', { name: walk.arrangement.name, exact: true }).fill(name);
      await dialog.getByRole('button', { name: walk.arrangement.confirm, exact: true }).click();
      await expect(dialog).toHaveCount(0);
      await other.goto(`/thread/${walk.thread}`);
      await other.getByRole('button', { name: 'Layout', exact: true }).click();
      await other.getByRole('menuitem', { name, exact: true }).click();
      await expect(other.getByRole('region', { name: 'Bottom', exact: true })).toBeVisible();

      await previewFile(page, walk.file);
      await expect(page.getByRole('tab', { name: basename(walk.file), exact: true })).toBeVisible();
      await previewFile(page, walk.secondFile);
      await expect(page.getByRole('tab', { name: basename(walk.file), exact: true })).toHaveCount(0);
      await previewFile(page, walk.file);
      const editor = page.getByRole('textbox', { name: `Text of ${basename(walk.file)}`, exact: true });
      const original = await editor.innerText();
      await editor.click();
      await page.keyboard.press('Control+Home');
      await page.keyboard.insertText('// native unsaved proof\n');
      await expect(page.getByTestId('code-status')).toContainText('Unsaved');
      await previewFile(page, walk.secondFile);
      await expect(page.getByRole('tab', { name: new RegExp(basename(walk.file).replace(/[.*+?^${}()|[\]\\]/g, '\\$&')) })).toBeVisible();
      await page.getByRole('tab', { name: new RegExp(basename(walk.file).replace(/[.*+?^${}()|[\]\\]/g, '\\$&')) }).focus();
      await page.keyboard.press('Enter');
      await page.keyboard.press('Delete');
      const close = page.getByRole('dialog');
      await expect(close).toBeVisible();
      await close.getByRole('button', { name: walk.arrangement.cancelClose, exact: true }).click();
      await expect(editor).toContainText('// native unsaved proof');
      await expect(editor).toContainText(original.trim());
      await expect.poll(() => page.evaluate(() => Object.keys(localStorage).some((key) => key.includes('.unsaved.')))).toBe(true);
      await page.reload();
      await page.getByRole('tab', { name: new RegExp(basename(walk.file).replace(/[.*+?^${}()|[\]\\]/g, '\\$&')) }).click();
      await expect(page.getByRole('textbox', { name: `Text of ${basename(walk.file)}`, exact: true })).toContainText('// native unsaved proof');
      await openFile(other, walk.secondFile);
      await expect(other.getByRole('textbox', { name: `Text of ${basename(walk.secondFile)}`, exact: true })).not.toContainText('// native unsaved proof');
      expect(doubles).toEqual([]);
    } finally { await first.close(); await second.close(); }
  });

  test('a ranged empty file becomes an empty editable text, zero CPU stays zero and absolute cwd remains the real shell path', async ({ browser }) => {
    if (walk === undefined) return;
    expect(walk.zeroCpuEvidence).not.toBe('');
    expect(walk.absoluteCwd.startsWith('/')).toBe(true);
    const context = await browser.newContext({ baseURL: process.env.ORCH_NATIVE_URL, storageState: walk.storageState, viewport: { width: 1600, height: 1000 } });
    try {
      const page = await context.newPage();
      const broken: string[] = [];
      page.on('pageerror', (error) => broken.push(error.message));
      const range = page.waitForResponse((response) => new URL(response.url()).pathname.endsWith('/files/content') && new URL(response.url()).searchParams.get('path') === walk.emptyFile && response.request().headers().range !== undefined);
      await openFile(page, walk.emptyFile);
      const empty = await range;
      expect(empty.status()).toBe(416);
      expect(empty.headers()['content-range']).toMatch(/\/0$/);
      const emptyEditor = page.getByRole('textbox', { name: `Text of ${basename(walk.emptyFile)}`, exact: true });
      await expect(emptyEditor).toHaveText('');
      await emptyEditor.click();
      await page.keyboard.insertText('Empty files accept text.');
      await expect(page.getByTestId('code-status')).toContainText('Unsaved');
      await view(page, 'Resources');
      await expect(page.getByTestId('now-CPU')).toHaveText('0%');
      await expect(page.getByTestId('now-CPU')).not.toContainText('-');
      await view(page, 'Terminal');
      const shell = page.getByTestId('terminal-panel').getByRole('tab', { name: new RegExp(basename(walk.absoluteCwd)) }).first();
      await expect(shell).toBeVisible();
      await shell.click();
      await page.locator('.xterm-helper-textarea').focus();
      await page.keyboard.type('pwd\n');
      await expect(page.locator('.xterm-rows')).toContainText(walk.absoluteCwd);
      expect(broken).toEqual([]);
    } finally { await context.close(); }
  });
});

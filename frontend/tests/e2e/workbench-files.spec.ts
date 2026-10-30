/** File URLs and real desktop editors keep source, drafts, navigation and repeated line intent. */
import { expect, test } from '@playwright/test';
import { agentMessage, fileRead, turn, userMessage } from '../doubles/thread-rows';
import { ADA } from '../doubles/thread-scenes';
import { begin, editor, fileTree, MAIN, MAIN_ENVIRONMENT, openThread, row, showView } from './inspector-support';
import { emit } from './support';

test('a desktop file tab keeps its source URL, peers and drafts through normal navigation and refresh', async ({ page, context }, info) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.setViewportSize({ width: 1440, height: 900 }); await begin(); await openThread(page);
  const draft = page.getByRole('textbox', { name: 'Message to the agent' }); await draft.fill('Do not replace the thread draft when opening a file');
  await showView(page, 'Files'); await row(page, /^src/).click(); await row(page, /^app\.ts/).click();
  const code = page.getByRole('textbox', { name: 'Text of app.ts' }); await expect(code).toBeEditable();
  const url = new URL(page.url());
  expect(url.pathname).toBe('/workbench'); expect(url.searchParams.get('view')).toBe('file');
  expect(url.searchParams.get('environment')).toBe(MAIN_ENVIRONMENT); expect(url.searchParams.get('thread')).toBe(MAIN);
  expect(url.searchParams.get('path')).toBe('web/src/app.ts');
  await code.click(); await page.keyboard.press('Control+Home'); await page.keyboard.type('// Retained file draft\n');
  await expect(page.getByTestId('code-status')).toContainText('Unsaved');
  await page.getByRole('tab', { name: /^Main thread(?: · Fix login)?$/ }).click();
  await expect(draft).toHaveValue('Do not replace the thread draft when opening a file');
  await expect(fileTree(page)).toBeVisible(); await row(page, /^app\.ts/).click();
  await expect(code).toContainText('// Retained file draft');
  expect(new URL(page.url()).searchParams.get('path')).toBe('web/src/app.ts');
  await page.goBack(); await expect(page).toHaveURL(new RegExp(`/thread/${MAIN}$`));
  await expect(draft).toHaveValue('Do not replace the thread draft when opening a file');
  await page.goForward(); await expect(code).toContainText('// Retained file draft');
  const fileTab = page.getByRole('tab', { name: /^app\.ts/ });
  await expect(fileTab).toHaveCount(1); await fileTab.click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Copy link', exact: true }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(page.url());
  await expect.poll(() => page.evaluate(() => Array.from({ length: localStorage.length }, (_, i) => localStorage.key(i))
    .some((key) => key?.includes('.unsaved.') && localStorage.getItem(key)?.includes('// Retained file draft')))).toBe(true);
  page.on('dialog', (dialog) => void dialog.accept());
  await page.reload(); await expect(code).toContainText('// Retained file draft'); await expect(code).toBeEditable();
  await expect(page.getByRole('tab', { name: /^Main thread(?: · Fix login)?$/ })).toBeVisible();
  await page.screenshot({ path: info.outputPath('file-url-draft-reload.png'), animations: 'disabled' });
});

test('opening the same file line twice moves the existing editor cursor while preserving unsaved text', async ({ page }, info) => {
  await page.setViewportSize({ width: 1440, height: 900 }); await begin();
  const at = new Date().toISOString();
  await emit(MAIN, { items: [
    userMessage('line-user', 'Read the entry point', { author: ADA, turn: 'line-turn', inputId: 'line-input', at }),
    fileRead('line-read', { path: 'src/app.ts', content: 'export function main(name: string): string {', from: 4, to: 4, turn: 'line-turn', at }),
    agentMessage('line-answer', 'Read the file.', { turn: 'line-turn', at }),
  ], turns: [turn('line-turn', { ordinal: 1, state: 'completed', inputId: 'line-input', startedAt: at, completedAt: at })] });
  await openThread(page); await page.getByRole('button', { name: /^Read src\/app\.ts/ }).click();
  await page.getByRole('button', { name: 'src/app.ts', exact: true }).click();
  await expect(page.getByTestId('code-status')).toContainText('Line 4, column 1');
  await expect(page.getByTestId('outline-panel')).toHaveCount(0);
  await editor(page).click(); await page.keyboard.press('Control+Home'); await page.keyboard.type('// keep this draft ');
  await expect(page.getByTestId('code-status')).toContainText('Line 1,');
  await page.getByRole('tab', { name: /^Main thread(?: · Fix login)?$/ }).click();
  await page.getByRole('button', { name: 'src/app.ts', exact: true }).click();
  await expect(page.getByTestId('code-status')).toContainText('Line 4, column 1');
  await expect(editor(page)).toContainText('// keep this draft');
  await expect(page.getByRole('tab', { name: /^app\.ts/ })).toHaveCount(1);
  expect(new URL(page.url()).searchParams.get('line')).toBe('4');
  await page.screenshot({ path: info.outputPath('file-repeat-line-keeps-editor.png'), animations: 'disabled' });
});

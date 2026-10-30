/** Every view of the workspace and the runtime asked, in both themes, whether anything on it is a serious barrier: the tree, a file in the code view and in each viewer, the search, a menu and a dialog, Changes, Git, Processes, Resources, a shell, the outline, the environment and the parameters. */
import { expect, test } from '@playwright/test';

import { begin, editor, fileTree, filesPanel, openThread, row, showView, workspace } from '../e2e/inspector-support.ts';
import { expectAccessibleInBothThemes } from '../e2e/support.ts';

test.use({ viewport: { width: 1600, height: 1000 } });

test.beforeEach(async () => {
  await begin();
});

test('the tree, a file in the code view, the search and the menu of a row', async ({ page }) => {
  await openThread(page);
  await showView(page, /^Files/);
  await expect(fileTree(page)).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await row(page, /^src/).click();
  await row(page, /^app\.ts/).click();
  await expect(editor(page)).toContainText("import { greet }");
  await expectAccessibleInBothThemes(page);

  await filesPanel(page).getByRole('textbox', { name: 'Search the files' }).fill('util');
  await expect(page.getByTestId('search-results')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await filesPanel(page).getByRole('textbox', { name: 'Search the files' }).fill('');

  await row(page, /^app\.ts/).click({ button: 'right' });
  await expect(page.getByRole('menu')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="menu"]');
  await page.keyboard.press('Escape');

  await filesPanel(page).getByRole('button', { name: 'New file…' }).click();
  await expect(page.getByRole('dialog')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('the viewers: Markdown, an image, an archive, a file with no preview, and a file that changed under unsaved text', async ({ page }) => {
  await openThread(page);
  await showView(page, /^Files/);
  await row(page, /^README\.md/).click();
  await expect(page.getByTestId('markdown-text')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await row(page, /^logo\.png/).click();
  await expect(page.locator('[data-viewer="image"] img')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await filesPanel(page).getByRole('button', { name: 'Up to the parent directory' }).click();
  await row(page, /^data/).click();
  await row(page, /^bundle\.zip/).click();
  await expect(page.getByRole('tree', { name: 'Contents of the archive' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await row(page, /^blob\.bin/).click();
  await expect(page.getByText('blob.bin has no preview')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await filesPanel(page).getByRole('button', { name: 'Back to the thread\'s directory' }).click();
  await row(page, /^src/).click();
  await row(page, /^app\.ts/).click();
  await editor(page).click();
  await page.keyboard.type('x');
  await workspace('/put', { files: { 'web/src/app.ts': 'changed\n' } });
  await expect(page.getByText('This file changed while you were editing')).toBeVisible();
  await page.getByRole('button', { name: 'Compare' }).click();
  await expectAccessibleInBothThemes(page);
});

test('Changes, Git, Processes with a menu, Resources', async ({ page }) => {
  await openThread(page);
  await showView(page, /^Changes/);
  await page.getByTestId('changes-panel').getByRole('button', { name: /^app\.ts/ }).click();
  await expectAccessibleInBothThemes(page);
  await showView(page, /^Git/);
  await expect(page.getByTestId('git-branch')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await showView(page, /^Processes/);
  await expect(page.getByTestId('process-row').first()).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.getByRole('button', { name: 'Signal process 101' }).click();
  await page.getByRole('menuitem', { name: 'Terminate' }).click();
  await expect(page.getByRole('alertdialog')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="alertdialog"]');
  await page.keyboard.press('Escape');
  await showView(page, /^Resources/);
  await expect(page.getByTestId('resources-panel')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a shell with something printed in it, in both themes', async ({ page }) => {
  await openThread(page);
  await showView(page, /^Terminal/);
  await page.getByRole('button', { name: 'Open a new shell' }).first().click();
  await expect(page.locator('.xterm-rows')).toContainText('$');
  await page.locator('.xterm-helper-textarea').focus();
  await page.keyboard.type('colors\n');
  await expect(page.locator('.xterm-rows')).toContainText('cyan bold');
  await expectAccessibleInBothThemes(page);
});

test('the outline, the environment and the parameters', async ({ page }) => {
  await openThread(page);
  await showView(page, /^Outline/);
  await expect(page.getByTestId('outline-panel')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await showView(page, /^Environment/);
  await expect(page.getByTestId('environment-panel')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await showView(page, /^Parameters/);
  await expect(page.getByTestId('parameters-panel')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

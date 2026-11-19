/** A person's same announcement and tag controls at every level, answered by the fake world. */
import { expect, test } from '@playwright/test';
import { ACME, scenario, signIn } from './support.ts';

test.beforeEach(async () => { await scenario(); });

for (const [kind, path] of [['org', '/org/acme'], ['project', `/project/${ACME.web}`], ['topic', `/topic/${ACME.fixLogin}`]] as const) {
  test(`an ${kind} announces and keeps its own tags through the shared place endpoints`, async ({ page }) => {
    await signIn(page);
    await page.goto(path);
    if (kind === 'org') {
      await page.getByTestId('main').locator('header').filter({ has: page.getByRole('heading', { level: 1, name: 'Acme', exact: true }) }).locator('button[aria-haspopup="menu"]').click();
      await page.getByRole('menuitem', { name: 'Announcements', exact: true }).click();
    } else await page.getByRole('button', { name: 'Announcements', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: /Announcements in/ });
    await dialog.getByRole('textbox', { name: 'Title', exact: true }).fill(`Release for ${kind}`);
    await dialog.getByRole('textbox', { name: 'Message', exact: true }).fill('The same notice at every level.');
    const response = page.waitForResponse((answer) => answer.request().method() === 'POST' && /\/nodes\/[^/]+\/announcements$/.test(answer.url()));
    await dialog.getByRole('button', { name: 'Send announcement', exact: true }).click();
    expect((await response).status()).toBe(201);
    await expect(dialog.getByRole('status').filter({ hasText: /^Sent to \d+ topic/ })).toContainText(/Sent to \d+ topic/);
    // Each reached topic has its own announcement row with the submitted title.
    await expect(dialog.getByRole('heading', { name: `Release for ${kind}`, exact: true }).first()).toBeVisible();
    await page.keyboard.press('Escape');
    const details = async () => {
      if (kind === 'org') {
        await page.getByTestId('main').locator('header').filter({ has: page.getByRole('heading', { level: 1, name: 'Acme', exact: true }) }).locator('button[aria-haspopup="menu"]').click();
        await page.getByRole('menuitem', { name: 'Description and tags', exact: true }).click();
      } else if (kind === 'project') await page.getByTestId('main').getByRole('button', { name: 'Description and tags', exact: true }).click();
    };
    await details();
    await page.getByRole('button', { name: kind === 'topic' ? 'Edit tags' : 'Add tags', exact: true }).click();
    const label = page.getByRole('checkbox', { name: 'urgent', exact: true });
    const assigned = page.waitForResponse((answer) => answer.request().method() === 'PUT' && /\/nodes\/[^/]+\/tags$/.test(answer.url()));
    await label.click();
    expect((await assigned).status()).toBe(200);
    // Wait for the accepted tag's saved chip before reading the picker's saved selection.
    await expect(page.getByRole('button', { name: 'Take off the tag urgent', exact: true })).toBeVisible();
    if (!(await label.isVisible())) await page.getByRole('button', { name: 'Edit tags', exact: true }).click();
    await expect(label).toBeChecked();
    await page.keyboard.press('Escape');
    await expect(page.getByRole('button', { name: 'Edit tags', exact: true })).toBeVisible();
    await page.reload();
    await details();
    await expect(page.getByRole('button', { name: 'Edit tags', exact: true })).toBeVisible();
  });
}

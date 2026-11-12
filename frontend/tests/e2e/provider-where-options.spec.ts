/** A provider Use entry can say where its work may run: left out it reads Anywhere, and the names chosen are there when the access page is opened again. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import { RUNS_ON } from '../doubles/fixtures.ts';
import { GRACE } from '../doubles/thread-scenes.ts';
import { DOUBLE, scenario, signIn } from './support.ts';

const words = english;
const sharedCopy = words.choiceCatalog.arrangement.credential_shared.name;

for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`a provider entry keeps the places it is limited to ${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    const seeded = await fetch(`${DOUBLE}/__double/access/touch`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({
      as: 'ada', path: 'providers', id: RUNS_ON.openAiKey, change: { rows: [{ permission: 'provider.use', inherit: false,
        add: [{ holder: { kind: 'user', id: GRACE.id }, options: { models: ['@small'] } }] }] },
    }) });
    expect(seeded.ok).toBe(true);
    await signIn(page);
    await page.setViewportSize({ width, height: 900 });
    const appearance = await fetch(`${DOUBLE}/__double/first-steps/appearance`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ locale: 'en', theme }) });
    expect(appearance.ok).toBe(true);

    const openOptions = async () => {
      await page.goto(`/resources/providers/${RUNS_ON.openAiKey}`);
      await page.getByRole('tab', { name: words.providers.tabs.settings, exact: true }).click();
      await page.getByRole('button', { name: words.actions.permissions.label, exact: true }).click();
      const access = page.getByRole('dialog').filter({ has: page.locator('[data-tutorial-target="permission-page"]') });
      await access.getByRole('button', { name: words.access.footer.byArea, exact: true }).click();
      const use = access.locator('[data-access-permission="provider.use"]');
      const permission = await use.getAttribute('aria-label');
      expect(permission).not.toBeNull();
      await use.getByRole('button', { name: words.access.cell.menuOf.replace('{{permission}}', permission ?? ''), exact: true }).click();
      await page.getByRole('menuitem', { name: words.access.cell.optionsOf.replace('{{name}}', 'Grace Hopper'), exact: true }).click();
      return page.getByRole('dialog', { name: words.access.options.title.replace('{{holder}}', 'Grace Hopper').replace('{{permission}}', permission ?? ''), exact: true });
    };
    const pick = async (entry: ReturnType<typeof page.getByRole>, name: string, look?: string) => {
      await entry.getByRole('combobox', { name: words.access.options.whereAdd.replace('{{what}}', words.access.options.where), exact: true }).click();
      const option = page.getByRole('option', { name: new RegExp(name) });
      await expect(option).toBeVisible();
      if (look !== undefined) await page.screenshot({ path: info.outputPath(look), fullPage: true });
      await option.click();
    };

    let entry = await openOptions();
    const where = entry.getByRole('group', { name: words.access.options.where, exact: true });
    await expect(where.getByText(words.access.options.anywhere, { exact: true })).toBeVisible();
    await expect(entry.getByRole('textbox', { name: words.access.options.models, exact: true })).toHaveValue('@small');
    await pick(entry, 'Workspace', `where-picker-${theme}-${width}.png`);
    await pick(entry, 'host-1');
    await expect(where.getByText('Workspace')).toBeVisible();
    await expect(where.getByText('host-1')).toBeVisible();
    await expect(where.getByText(words.access.options.anywhere, { exact: true })).toHaveCount(0);
    // Naming a type in which the sign-in leaves is an explicit choice, and the entry says what leaves.
    await expect(entry.getByRole('checkbox', { name: sharedCopy, exact: true })).not.toBeChecked();
    await expect(entry.getByText(words.access.options.leaves.credential_shared, { exact: true })).toHaveCount(0);
    await entry.getByRole('checkbox', { name: sharedCopy, exact: true }).click();
    await expect(entry.getByText(words.access.options.leaves.credential_shared, { exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    // The dialog scrolls inside itself: the screenshot is taken with the control in view.
    await entry.getByRole('combobox', { name: words.access.options.whereAdd.replace('{{what}}', words.access.options.where), exact: true }).scrollIntoViewIfNeeded();
    await page.screenshot({ path: info.outputPath(`where-chosen-${theme}-${width}.png`), fullPage: true });
    await entry.getByRole('button', { name: words.access.options.save, exact: true }).click();
    await expect(entry).toHaveCount(0);

    // The page is read again from the server: the names are on the entry, not in the dialog that was closed.
    entry = await openOptions();
    const again = entry.getByRole('group', { name: words.access.options.where, exact: true });
    await expect(again.getByText('Workspace')).toBeVisible();
    await expect(again.getByText('host-1')).toBeVisible();
    await expect(entry.getByRole('checkbox', { name: sharedCopy, exact: true })).toBeChecked();
    await expect(entry.getByText(words.access.options.leaves.credential_shared, { exact: true })).toBeVisible();
    await expect(entry.getByRole('textbox', { name: words.access.options.models, exact: true })).toHaveValue('@small');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await entry.getByRole('combobox', { name: words.access.options.whereAdd.replace('{{what}}', words.access.options.where), exact: true }).scrollIntoViewIfNeeded();
    await page.screenshot({ path: info.outputPath(`where-reopened-${theme}-${width}.png`), fullPage: true });

    // Clearing every name returns it to Anywhere and writes no member.
    await again.getByRole('button', { name: words.access.options.whereRemove.replace('{{name}}', 'Workspace'), exact: true }).click();
    await again.getByRole('button', { name: words.access.options.whereRemove.replace('{{name}}', 'host-1'), exact: true }).click();
    await expect(again.getByText(words.access.options.anywhere, { exact: true })).toBeVisible();
    await entry.getByRole('checkbox', { name: sharedCopy, exact: true }).click();
    await entry.getByRole('button', { name: words.access.options.save, exact: true }).click();
    await expect(entry).toHaveCount(0);
    entry = await openOptions();
    await expect(entry.getByRole('group', { name: words.access.options.where, exact: true }).getByText(words.access.options.anywhere, { exact: true })).toBeVisible();
    await expect(entry.getByRole('checkbox', { name: sharedCopy, exact: true })).not.toBeChecked();
  });
}

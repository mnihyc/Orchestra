/** Taking things away: a thread as one file, a project as one archive with two of three folders ticked and documents alone, a plan past what the place allows, a job that ends and its Download, one that fails and why, a thing that may be read and not taken away (switched off in its own menu, greyed and counted in the dialog of its place), the list of one's exports with a removed and an expired one and what each left out, the line about workspace files, a phone, and the dialog by keyboard. */
import { readFile } from 'node:fs/promises';

import { expect, type Locator, type Page, test } from '@playwright/test';

import { id } from '../doubles/fixtures.ts';
import { ACME, changeTree, PEOPLE, scenario, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

/** A request only the fake API has, which steers what the export screens meet. */
async function steer(path: string, body: unknown = {}): Promise<unknown> {
  const response = await fetch(`${DOUBLE}/__double/export${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`the fake API refused ${path}: ${response.status}`);
  return response.status === 204 ? null : response.json();
}

/** The archives the fake API has made, with what each was asked for and written to hold. */
async function made(): Promise<{ id: string; state: string; request: Record<string, unknown>; lines: string[]; counts: Record<string, number> | null }[]> {
  return (await steer('/made')) as never;
}

/** The single files the fake API was asked for, with what they were asked with. */
async function asked(): Promise<{ path: string; query: Record<string, string>; person: string }[]> {
  return (await steer('/asked')) as never;
}

// Shop has the folder Checkout, with Payments inside it; these two make it three folders at its own level.
const RETURNS = id(3, 101);
const SEARCH = id(3, 102);

test.beforeEach(async () => {
  await scenario();
});

/** Opens Export… from the "⋯" of a place in the left bar, and waits for the tree of its dialog. */
async function exportFrom(page: Page, place: string): Promise<Locator> {
  await page.getByRole('button', { name: `Actions for ${place}` }).first().click();
  await page.getByRole('menuitem', { name: 'Export…' }).click();
  const dialog = page.getByRole('dialog', { name: `Export ${place}` });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('treeitem').first()).toBeVisible();
  return dialog;
}

const plan = (dialog: Locator) => dialog.locator('#export-plan');
const row = (dialog: Locator, name: string | RegExp) => dialog.getByRole('treeitem', { name });
const exportButton = (dialog: Locator) => dialog.getByRole('button', { name: 'Export', exact: true });
const statusBar = (page: Page) => page.getByRole('contentinfo', { name: 'Status' });
/** The notices that stand at the lower right: each is an item of the list the region holds. */
const notice = (page: Page, text: string) => page.getByRole('region', { name: /^Notifications/ }).getByRole('listitem').filter({ hasText: text });

/** Shop with three folders of its own, signed in as Ada. */
async function atShop(page: Page): Promise<void> {
  await changeTree([
    { id: RETURNS, kind: 'project', name: 'Returns', parent_id: ACME.shop },
    { id: SEARCH, kind: 'project', name: 'Search', parent_id: ACME.shop },
  ]);
  await signIn(page);
  await page.goto(`/project/${ACME.shop}`);
}

test.describe('one thing as one file', () => {
  test('a thread is exported from its menu as the file it was asked for: how much, the form, the turns and what only a person’s export adds', async ({ page }) => {
    await signIn(page);
    await page.goto(`/thread/${ACME.mainThread}`);
    await page.locator('[data-thread-header]').getByRole('button', { name: /^Actions for (Main thread|Repro)/ }).click();
    await page.getByRole('menuitem', { name: 'Export…' }).click();

    const dialog = page.getByRole('dialog', { name: 'Export Main thread' });
    await expect(dialog).toBeVisible();
    await dialog.getByRole('radio', { name: 'Full', exact: true }).click();
    await dialog.getByRole('checkbox', { name: /The agent.s reasoning/ }).click();
    await dialog.getByLabel('From turn').fill('2');
    await dialog.getByLabel('To turn').fill('5');

    const [download] = await Promise.all([page.waitForEvent('download'), exportButton(dialog).click()]);
    expect(download.suggestedFilename()).toBe('Main thread.md');
    const text = await readFile(await download.path(), 'utf8');
    expect(text).toContain('detail=full include=reasoning,sub_streams from=2 to=5');
    await expect(dialog).toBeHidden();

    const [one] = await asked();
    expect(one?.path).toBe(`/threads/${ACME.mainThread}/export`);
    expect(one?.query).toMatchObject({ format: 'markdown', detail: 'full', include: 'reasoning,sub_streams', from: '2', to: '5' });
  });

  test('a thread is exported as JSON with its chat, and the turns that are no turns are refused where they are typed', async ({ page }) => {
    await signIn(page);
    await page.goto(`/thread/${ACME.mainThread}`);
    await page.locator('[data-thread-header]').getByRole('button', { name: /^Actions for (Main thread|Repro)/ }).click();
    await page.getByRole('menuitem', { name: 'Export…' }).click();
    const dialog = page.getByRole('dialog', { name: 'Export Main thread' });

    await dialog.getByRole('radio', { name: 'JSON', exact: true }).click();
    // A program reads everything, so how much is not asked.
    await expect(dialog.getByRole('radio', { name: 'Standard', exact: true })).toBeDisabled();
    await dialog.getByRole('checkbox', { name: /The thread.s chat/ }).click();
    await dialog.getByLabel('From turn').fill('0');
    await exportButton(dialog).click();
    await expect(dialog.getByText(/is not a turn/)).toBeVisible();

    await dialog.getByLabel('From turn').fill('');
    const [download] = await Promise.all([page.waitForEvent('download'), exportButton(dialog).click()]);
    expect(download.suggestedFilename()).toBe('Main thread.json');
    expect((await asked())[0]?.query).toMatchObject({ format: 'json', include: 'sub_streams,chat' });
  });

  test('a chat is exported from the menu of its header, with its attachments as an archive', async ({ page }) => {
    await signIn(page);
    await page.goto(`/topic/${ACME.fixLogin}?panel=chat`);
    await page.getByRole('button', { name: 'Actions for this chat' }).first().click();
    await page.getByRole('menuitem', { name: 'Export…' }).click();
    const dialog = page.getByRole('dialog', { name: /^Export / });
    await dialog.getByRole('checkbox', { name: 'Attachments' }).click();
    const [download] = await Promise.all([page.waitForEvent('download'), exportButton(dialog).click()]);
    expect(download.suggestedFilename()).toMatch(/\.zip$/);
    expect((await asked())[0]?.query).toMatchObject({ format: 'markdown', files: 'include' });
  });
});

test.describe('one archive of a place', () => {
  test('a project is exported with two of its three folders ticked, and the request names the two', async ({ page }) => {
    await atShop(page);
    const dialog = await exportFrom(page, 'Shop');

    // Everything is ticked as the dialog opens, and the plan says what that comes to.
    await expect(row(dialog, /^Shop/)).toHaveAttribute('aria-checked', 'true');
    await expect(plan(dialog)).toContainText('allowed here');
    await expect(exportButton(dialog)).toBeEnabled();

    await row(dialog, /^Search/).click();
    await expect(row(dialog, /^Search/)).toHaveAttribute('aria-checked', 'false');
    await expect(row(dialog, /^Shop/)).toHaveAttribute('aria-checked', 'mixed');
    await expect(row(dialog, /^Checkout/)).toHaveAttribute('aria-checked', 'true');
    await expect(plan(dialog)).toContainText('allowed here');

    await exportButton(dialog).click();
    await expect(dialog).toBeHidden();
    const [archive] = await made();
    expect(archive?.request.what).toEqual({ nodes: [ACME.checkout, RETURNS] });
  });

  test('documents alone: the other kinds are unticked, the plan counts documents only, and the request says so', async ({ page }) => {
    await atShop(page);
    const dialog = await exportFrom(page, 'Shop');
    await expect(plan(dialog)).toContainText('allowed here');

    await dialog.getByRole('checkbox', { name: 'Conversations' }).click();
    await dialog.getByRole('checkbox', { name: 'Chats' }).click();
    // The three documents under Shop are 21 KiB.
    await expect(plan(dialog)).toContainText('About 21 KiB of 2 GiB allowed here');
    await expect(plan(dialog)).toContainText('3 things of 10,000');

    await exportButton(dialog).click();
    await expect(dialog).toBeHidden();
    const [archive] = await made();
    expect(archive?.request).toEqual({ what: 'all', parts: { conversations: false, chats: false } });
    expect(archive?.lines.filter((line) => line.startsWith('document:'))).toHaveLength(3);
    expect(archive?.lines.filter((line) => line.startsWith('conversation:'))).toHaveLength(0);
  });

  test('a document unticked inside a ticked place is left out by naming the others', async ({ page }) => {
    await signIn(page);
    await page.goto(`/project/${ACME.web}`);
    const dialog = await exportFrom(page, 'Web');
    await expect(row(dialog, /Web handbook/)).toBeVisible();
    await row(dialog, /Web handbook/).click();
    await expect(plan(dialog)).toContainText('allowed here');
    await exportButton(dialog).click();
    await expect(dialog).toBeHidden();
    const [archive] = await made();
    expect(archive?.request.what).toMatchObject({ nodes: expect.arrayContaining([ACME.fixLogin, ACME.upgradeDeps, ACME.migrateCi]) });
    expect(archive?.request.what).toMatchObject({ documents: [expect.any(String)] });
    expect(archive?.lines).not.toContain('document: Web handbook');
  });

  test('a plan past the size the place allows keeps the button off and says by how much, until less is chosen', async ({ page }) => {
    await steer('/limits', { max_bytes: 30 * 1024 });
    await atShop(page);
    const dialog = await exportFrom(page, 'Shop');

    // The conversation of Fee table, 24 KiB, and the three documents, 21 KiB: 45 KiB against 30.
    await expect(plan(dialog)).toContainText('About 45 KiB: 15 KiB more than the 30 KiB allowed here.');
    await expect(plan(dialog)).toContainText('Choose less.');
    await expect(exportButton(dialog)).toHaveAttribute('aria-disabled', 'true');
    await exportButton(dialog).click({ force: true });
    await expect(dialog).toBeVisible();
    expect(await made()).toHaveLength(0);

    await dialog.getByRole('checkbox', { name: 'Conversations' }).click();
    await expect(plan(dialog)).toContainText('About 21 KiB of 30 KiB allowed here');
    await expect(exportButton(dialog)).not.toHaveAttribute('aria-disabled', 'true');
  });

  test('a place that allows no export says so, and nothing that is ticked changes that', async ({ page }) => {
    await steer('/limits', { max_bytes: 0 });
    await atShop(page);
    const dialog = await exportFrom(page, 'Shop');
    await expect(plan(dialog)).toContainText('This place allows no export.');
    await expect(exportButton(dialog)).toHaveAttribute('aria-disabled', 'true');
  });

  test('nothing ticked, and no kind of thing included, are said in words and keep the button off', async ({ page }) => {
    await atShop(page);
    const dialog = await exportFrom(page, 'Shop');
    await row(dialog, /^Shop/).click();
    await expect(plan(dialog)).toContainText('Nothing is ticked');
    await expect(exportButton(dialog)).toHaveAttribute('aria-disabled', 'true');
    await row(dialog, /^Shop/).click();
    await dialog.getByRole('checkbox', { name: 'Conversations' }).click();
    await dialog.getByRole('checkbox', { name: 'Chats' }).click();
    await dialog.getByRole('checkbox', { name: 'Documents' }).click();
    await expect(plan(dialog)).toContainText('No kind of thing is included');
  });

  test('visible export options change the request: JSON, everything each step did, the files, and a span of days', async ({ page }) => {
    await atShop(page);
    const dialog = await exportFrom(page, 'Shop');
    await dialog.getByRole('radio', { name: 'JSON', exact: true }).click();
    await expect(dialog.getByRole('combobox', { name: /Conversations/ })).toBeVisible();
    await expect(dialog.getByRole('checkbox', { name: 'Attachments' })).toBeVisible();
    await expect(dialog.getByLabel('From', { exact: true })).toBeVisible();
    await expect(dialog.getByLabel('To', { exact: true })).toBeVisible();
    await dialog.getByRole('combobox', { name: /Conversations/ }).click();
    await page.getByRole('option', { name: 'Full', exact: true }).click();
    await dialog.getByRole('checkbox', { name: 'Attachments' }).click();
    await dialog.getByLabel('From').fill('2026-10-01');
    await dialog.getByLabel('To', { exact: true }).fill('2026-10-09');
    await expect(plan(dialog)).toContainText('allowed here');
    await exportButton(dialog).click();
    await expect(dialog).toBeHidden();
    const [archive] = await made();
    expect(archive?.request).toEqual({ what: 'all', format: 'json', detail: 'full', files: true, from: '2026-10-01', to: '2026-10-09' });
  });

  test('the dialog says workspace files are not included, where an environment is to be seen or that cannot be told, and leads to where they are taken away', async ({ page }) => {
    await atShop(page);
    // A project: its view does not tell what runs below it, so the line is always there, and leads to the environments.
    let dialog = await exportFrom(page, 'Shop');
    await expect(dialog.getByText('Workspace files are not included.')).toBeVisible();
    await expect(dialog.getByRole('link', { name: 'How to take them' })).toHaveAttribute('href', '/resources/environments');
    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();

    // A topic whose threads run in no environment has no workspace to speak of.
    dialog = await exportFrom(page, 'Card fees');
    await expect(plan(dialog)).toContainText('allowed here');
    await expect(dialog.getByText('Workspace files are not included.')).toHaveCount(0);
    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();

    // A topic whose threads run in one: the line leads to that topic's Environments tab, and the dialog is left behind.
    dialog = await exportFrom(page, 'Fix login');
    const how = dialog.getByRole('link', { name: 'How to take them' });
    await expect(how).toHaveAttribute('href', `/topic/${ACME.fixLogin}?tab=environments`);
    await how.click();
    await expect(dialog).toBeHidden();
    await expect(page).toHaveURL(new RegExp(`/topic/${ACME.fixLogin}\\?tab=environments`));
  });

  test('on a phone the names of the places in the tree are read whole, and it is what lies in them that gives way', async ({ page }) => {
    await page.setViewportSize({ width: 400, height: 800 });
    await atShop(page);
    const dialog = await exportFrom(page, 'Shop');
    for (const name of ['Shop', 'Checkout']) {
      const word = row(dialog, new RegExp(`^${name}`)).getByText(name, { exact: true });
      await expect(word).toBeVisible();
      expect(await word.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    }
  });
});

test.describe('the job and the download', () => {
  test('a job is followed in the status bar until it ends, and then its archive is downloaded', async ({ page }) => {
    await steer('/jobs', { hold: true });
    await atShop(page);
    const dialog = await exportFrom(page, 'Shop');
    await expect(plan(dialog)).toContainText('allowed here');
    await exportButton(dialog).click();
    await expect(dialog).toBeHidden();

    await expect(statusBar(page).getByText(/Exporting Shop/)).toBeVisible();
    await steer('/progress', { percent: 42 });
    await expect(statusBar(page).getByText('Exporting Shop · 42%', { exact: true })).toBeVisible();

    await steer('/finish');
    const toast = notice(page, 'Shop: export ready');
    await expect(toast).toBeVisible();
    await expect(statusBar(page).getByText('Shop: export ready', { exact: true })).toBeVisible();

    const [download] = await Promise.all([page.waitForEvent('download'), toast.getByRole('button', { name: 'Download' }).click()]);
    expect(download.suggestedFilename()).toMatch(/^Shop-\d{4}-\d{2}-\d{2}\.zip$/);
    const bytes = await readFile(await download.path());
    // A zip begins with PK.
    expect(bytes.subarray(0, 2).toString('latin1')).toBe('PK');
  });

  test('a job that fails says why, in a notice that stays and in the list', async ({ page }) => {
    await steer('/jobs', { hold: true });
    await atShop(page);
    const dialog = await exportFrom(page, 'Shop');
    await expect(plan(dialog)).toContainText('allowed here');
    await exportButton(dialog).click();
    await expect(dialog).toBeHidden();
    await steer('/finish', { fail: { code: 'export.too_large', message: 'the archive passed the bytes the place allows' } });

    const toast = notice(page, 'Shop: export failed');
    await expect(toast).toContainText('This archive would be larger than the place allows. Choose less.');
    await expect(statusBar(page).getByText('Export failed', { exact: true })).toBeVisible();
    await expect(statusBar(page).getByRole('button', { name: 'Export failed' })).toHaveCount(0);
    await expect(statusBar(page).getByRole('button', { name: 'Your exports' }).locator('[data-action-icon="yourExports"]')).toBeVisible();

    await statusBar(page).getByRole('button', { name: 'Your exports' }).click();
    await page.waitForURL(/\/settings\/exports/);
    const line = page.getByRole('row', { name: /Shop/ });
    await expect(line).toContainText('Failed');
    await expect(line).toContainText('This archive would be larger than the place allows. Choose less.');
    // The person has seen it: the status bar says nothing more.
    await expect(statusBar(page).getByText('Export failed', { exact: true })).toBeHidden();
  });

  test('a third archive at a time is refused in words that say two are being written, and offers no waiting', async ({ page }) => {
    await steer('/jobs', { hold: true });
    await atShop(page);
    for (let at = 0; at < 2; at += 1) {
      const dialog = await exportFrom(page, 'Shop');
      await expect(plan(dialog)).toContainText('allowed here');
      await exportButton(dialog).click();
      await expect(dialog).toBeHidden();
    }
    const third = await exportFrom(page, 'Shop');
    await expect(plan(third)).toContainText('allowed here');
    await exportButton(third).click();
    await expect(third).toContainText('Two of your exports are being written. Ask for this one when one of them has ended.');
    await expect(third).toBeVisible();
    expect(await made()).toHaveLength(2);
  });
});

test.describe('what may be read and not taken away', () => {
  test('a thread the person reads and may not export has Export… switched off in its menu, with one line and the way to ask', async ({ page }) => {
    await steer('/bar', { username: 'dora', permission: 'thread.export', object: ACME.repro });
    await signIn(page, PEOPLE.dora);
    await page.goto(`/thread/${ACME.repro}`);
    await page.locator('[data-thread-header]').getByRole('button', { name: /^Actions for (Main thread|Repro)/ }).click();
    const entry = page.getByRole('menuitem', { name: /^Export…/ });
    await expect(entry).toHaveAttribute('aria-disabled', 'true');
    await expect(entry).toContainText('Taking this away as a file is for people Ada Lovelace names');
    await entry.click({ force: true });
    await expect(page.getByRole('dialog', { name: /^Export / })).toBeHidden();

    await page.getByRole('menuitem', { name: 'Request access' }).click();
    const ask = page.getByRole('dialog', { name: 'Request access to export Repro' });
    await ask.getByLabel('Why').fill('I need the transcript for the post-mortem');
    await ask.getByRole('button', { name: 'Send request' }).click();
    // The system takes the request, and the form says whom it asked.
    await expect(ask).toContainText('Asked Ada Lovelace. You will be told when they answer.');
  });

  test('somebody who may take it away has it offered, and a viewer’s row menu leaves out what they may not do', async ({ page }) => {
    await steer('/bar', { username: 'dora', permission: 'thread.export', object: ACME.repro });
    await signIn(page, PEOPLE.dora);
    // The left bar opens the topic the thread lies in, so that both threads are in it.
    await page.goto(`/thread/${ACME.mainThread}`);
    // In the left bar the entry is left out for the thread dora may not export, and there for the one she may.
    await page.getByRole('button', { name: 'Actions for Repro' }).first().click();
    await expect(page.getByRole('menuitem', { name: 'Rename…' })).toBeVisible();
    await expect(page.getByText(/Checking what you can do/)).toHaveCount(0);
    await expect(page.getByRole('menuitem', { name: 'Export…' })).toHaveCount(0);
    await page.keyboard.press('Escape');
    await page.getByRole('button', { name: 'Actions for Main thread' }).first().click();
    await expect(page.getByRole('menuitem', { name: 'Export…' })).toBeVisible();
  });

  test('in the dialog of its place such a thread is greyed and unticked, and counted in the line of the plan', async ({ page }) => {
    await steer('/bar', { username: 'dora', permission: 'thread.export', object: ACME.repro });
    await signIn(page, PEOPLE.dora);
    await page.goto(`/topic/${ACME.fixLogin}`);
    const dialog = await exportFrom(page, 'Fix login');

    await expect(row(dialog, /^Repro/)).toHaveAttribute('aria-checked', 'false');
    await expect(row(dialog, /^Main thread/)).toHaveAttribute('aria-checked', 'true');
    await expect(plan(dialog)).toContainText('1 thing here that you may read and not export is left out.');
    // Pressing it changes nothing, and what can be taken away is still all of what is asked for.
    await row(dialog, /^Repro/).click();
    await expect(row(dialog, /^Repro/)).toHaveAttribute('aria-checked', 'false');
    await expect(row(dialog, /^Fix login/)).toHaveAttribute('aria-checked', 'true');
    await expect(exportButton(dialog)).toBeEnabled();

    await exportButton(dialog).click();
    await expect(dialog).toBeHidden();
    const [archive] = await made();
    expect(archive?.request).toEqual({ what: 'all' });
    expect(archive?.lines.filter((line) => line.startsWith('conversation:'))).toEqual(['conversation: Main thread']);
    expect(archive?.counts).toMatchObject({ conversations: 1 });
  });

  test('a document whose Export row is lacking is greyed too, and the dialog of a place counts it', async ({ page }) => {
    await steer('/bar', { username: 'dora', permission: 'doc.export', object: ACME.web });
    await signIn(page, PEOPLE.dora);
    await page.goto(`/project/${ACME.web}`);
    const dialog = await exportFrom(page, 'Web');
    await expect(row(dialog, /Web handbook/)).toHaveAttribute('aria-checked', 'false');
    await expect(plan(dialog)).toContainText('things here that you may read and not export are left out.');
  });
});

test.describe('Your exports', () => {
  test('lists what was made: ready with Download and Remove, writing with its progress, failed with why, removed, expired, and one whose place is gone', async ({ page }) => {
    await steer('/seed-archive', { state: 'ready', place: ACME.web, ago_ms: 60 * 60_000 });
    await steer('/seed-archive', { state: 'pending', place: ACME.infra, ago_ms: 5 * 60_000 });
    await steer('/seed-archive', { state: 'failed', place: ACME.shop, ago_ms: 3 * 3_600_000 });
    // Removed by its person before its time: the row stays, the archive does not.
    await steer('/seed-archive', { state: 'expired', place: ACME.research, ago_ms: 5 * 3_600_000, expires_in_ms: 10 * 3_600_000 });
    // Its time ran out.
    await steer('/seed-archive', { state: 'expired', place: ACME.payroll, ago_ms: 30 * 3_600_000, expires_in_ms: -3_600_000 });
    const gone = (await steer('/seed-archive', { state: 'ready', place: ACME.ml, ago_ms: 2 * 3_600_000 })) as { id: string };
    await steer('/purge-place', { place: ACME.ml });
    expect(gone.id).toBeTruthy();

    await signIn(page);
    await page.goto('/settings/exports');
    await expect(page.getByRole('heading', { name: 'Your exports' })).toBeVisible();
    const table = page.getByRole('table', { name: 'Your exports' });
    await expect(table.getByRole('row')).toHaveCount(7);

    const ready = table.getByRole('row', { name: /Web/ });
    await expect(ready).toContainText('Ready');
    await expect(ready.getByRole('button', { name: /^Download/ })).toBeVisible();
    await expect(table.getByRole('row', { name: /Infra/ })).toContainText('Writing');
    await expect(table.getByRole('row', { name: /Shop/ })).toContainText('Failed');
    await expect(table.getByRole('row', { name: /Research/ })).toContainText('Removed');
    await expect(table.getByRole('row', { name: /Payroll/ })).toContainText('Expired');
    const orphan = table.getByRole('row', { name: /A place that is gone/ });
    await expect(orphan).toContainText('Ready');
    await expect(orphan.getByRole('button', { name: /^Download/ })).toBeVisible();

    // Nothing can be done with the ones that are over.
    await expect(table.getByRole('row', { name: /Research/ }).getByRole('button')).toHaveCount(0);

    await ready.getByRole('button', { name: /Actions for the archive of Web/ }).click();
    await page.getByRole('menuitem', { name: 'Remove' }).click();
    const confirm = page.getByRole('alertdialog', { name: 'Remove the archive of Web?' });
    await expect(confirm).toContainText('cannot be downloaded again');
    await confirm.getByRole('button', { name: 'Remove the archive of Web' }).click();
    await expect(table.getByRole('row', { name: /Web/ })).toContainText('Removed');
    await expect(table.getByRole('row', { name: /Web/ }).getByRole('button', { name: /^Download/ })).toHaveCount(0);
  });

  test('under what each archive is of, one line says what it left out: the parts not ticked, and what the server left out of the rest', async ({ page }) => {
    await steer('/seed-archive', { state: 'ready', place: ACME.web, ago_ms: 60 * 60_000 });
    await steer('/seed-archive', {
      state: 'ready',
      place: ACME.infra,
      ago_ms: 2 * 3_600_000,
      files: 3,
      request: { what: 'all', files: true, comments: true, versions: true },
      left_out: [
        { count: 2, what: 'not_exportable', where: null },
        { count: 1, what: 'too_large', where: null },
      ],
    });
    // Without documents there are no comments and no earlier versions to leave out.
    await steer('/seed-archive', { state: 'ready', place: ACME.shop, ago_ms: 3 * 3_600_000, request: { what: 'all', parts: { documents: false } } });
    await signIn(page);
    await page.goto('/settings/exports');
    const table = page.getByRole('table', { name: 'Your exports' });
    await expect(table.getByRole('row')).toHaveCount(4);
    await expect(table.getByRole('row', { name: /Web/ })).toContainText('Left out: attachments, comments, earlier versions');
    const full = table.getByRole('row', { name: /Infra/ });
    await expect(full).toContainText('3 attachments');
    await expect(full).toContainText('Left out: 2 things you may not take away, 1 attachment too large');
    await expect(full).not.toContainText('comments');
    const bare = table.getByRole('row', { name: /Shop/ });
    await expect(bare).toContainText('Left out: attachments');
    await expect(bare).not.toContainText('comments');
    await expect(bare).not.toContainText('earlier versions');
  });

  test('an archive that is being written can be cancelled from the list, and nothing is kept', async ({ page }) => {
    await steer('/seed-archive', { state: 'pending', place: ACME.web, ago_ms: 60_000 });
    await signIn(page);
    await page.goto('/settings/exports');
    const line = page.getByRole('row', { name: /Web/ });
    await line.getByRole('button', { name: /Actions for the archive of Web/ }).click();
    await page.getByRole('menuitem', { name: 'Cancel export' }).click();
    await page.getByRole('alertdialog').getByRole('button', { name: 'Cancel the export of Web' }).click();
    await expect(line).not.toContainText('Writing');
    await expect(line).toContainText('Cancelled');
    await expect(line.getByRole('button', { name: /Acts on/ })).toHaveCount(0);
  });

  test('an archive is downloaded from the list, and again', async ({ page }) => {
    await steer('/seed-archive', { state: 'ready', place: ACME.web, ago_ms: 60 * 60_000 });
    await signIn(page);
    await page.goto('/settings/exports');
    for (let time = 0; time < 2; time += 1) {
      const [download] = await Promise.all([page.waitForEvent('download'), page.getByRole('row', { name: /Web/ }).getByRole('button', { name: /^Download/ }).click()]);
      expect(download.suggestedFilename()).toMatch(/^Web-.*\.zip$/);
    }
  });

  test('on a phone each archive is a card with its state in it, and its Download is in reach without scrolling aside', async ({ page }) => {
    await steer('/seed-archive', { state: 'ready', place: ACME.web, ago_ms: 60 * 60_000 });
    await steer('/seed-archive', { state: 'failed', place: ACME.shop, ago_ms: 3 * 3_600_000 });
    await steer('/seed-archive', { state: 'pending', place: ACME.infra, ago_ms: 5 * 60_000 });
    await page.setViewportSize({ width: 400, height: 800 });
    await signIn(page);
    await page.goto('/settings/exports');
    const table = page.getByRole('table', { name: 'Your exports' });
    await expect(table.getByRole('row')).toHaveCount(4);
    await expect(table.getByRole('row', { name: /Web/ })).toContainText('Ready');
    await expect(table.getByRole('row', { name: /Shop/ })).toContainText('Failed');
    await expect(table.getByRole('row', { name: /Shop/ })).toContainText('Choose less.');
    await expect(table.getByRole('row', { name: /Infra/ })).toContainText('Writing');

    // The act of the row is not out of sight to the right of a table that is wider than the screen.
    const download = table.getByRole('row', { name: /Web/ }).getByRole('button', { name: /^Download/ });
    await expect(download).toBeVisible();
    const box = await download.boundingBox();
    expect((box?.x ?? Number.POSITIVE_INFINITY) + (box?.width ?? 0)).toBeLessThanOrEqual(400);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  });

  test('says there is nothing yet, and how to make an export', async ({ page }) => {
    await signIn(page);
    await page.goto('/settings/exports');
    await expect(page.getByText('Nothing exported yet')).toBeVisible();
    await expect(page.getByText('Open the menu of an org, a project or a topic and choose Export…')).toBeVisible();
  });
});

test.describe('by keyboard', () => {
  test('the dialog of a place is walked and ticked without the pointer, its options reached with Tab, and closed with Escape', async ({ page }) => {
    await atShop(page);
    const dialog = await exportFrom(page, 'Shop');
    await expect(plan(dialog)).toContainText('allowed here');
    // The keyboard begins in the tree, on its first row.
    await expect(row(dialog, /^Shop/)).toBeFocused();
    await page.keyboard.press('ArrowDown');
    await expect(row(dialog, /^Checkout/)).toBeFocused();
    await page.keyboard.press('ArrowDown');
    await expect(row(dialog, /^Returns/)).toBeFocused();
    await page.keyboard.press('Space');
    await expect(row(dialog, /^Returns/)).toHaveAttribute('aria-checked', 'false');
    await expect(row(dialog, /^Shop/)).toHaveAttribute('aria-checked', 'mixed');
    await page.keyboard.press('Space');
    await expect(row(dialog, /^Returns/)).toHaveAttribute('aria-checked', 'true');

    // Tab leaves the tree for the options, in the order they are read, and the dialog is closed with Escape.
    await page.keyboard.press('Tab');
    await expect(dialog.getByRole('checkbox', { name: 'Conversations' })).toBeFocused();
    await page.keyboard.press('Space');
    await expect(dialog.getByRole('checkbox', { name: 'Conversations' })).not.toBeChecked();
    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();
  });

  test('the dialog of a thread takes its choices with the keyboard and makes the file with Enter on the button', async ({ page }) => {
    await signIn(page);
    await page.goto(`/thread/${ACME.mainThread}`);
    await page.locator('[data-thread-header]').getByRole('button', { name: /^Actions for (Main thread|Repro)/ }).click();
    await page.getByRole('menuitem', { name: 'Export…' }).focus();
    await page.keyboard.press('Enter');
    const dialog = page.getByRole('dialog', { name: 'Export Main thread' });
    await expect(dialog).toBeVisible();
    await dialog.getByRole('radio', { name: 'Brief', exact: true }).focus();
    await page.keyboard.press('Space');
    const button = exportButton(dialog);
    await button.focus();
    const [download] = await Promise.all([page.waitForEvent('download'), page.keyboard.press('Enter')]);
    expect(download.suggestedFilename()).toBe('Main thread.md');
    expect((await asked())[0]?.query).toMatchObject({ detail: 'brief' });
  });
});

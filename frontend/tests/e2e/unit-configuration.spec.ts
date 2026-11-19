/** A template's literal commands and four program kinds survive a reload; machine programs stay read-only. */
import { expect, test } from '@playwright/test';
import { RUNS_ON } from '../doubles/fixtures.ts';
import { scenario, signIn } from './support.ts';
import { templateJson, templateSection } from './template-support.ts';

test.beforeEach(async () => { await scenario(); });

test('a template writer saves literal commands and provider programs, then reads the same recipe again', async ({ page }) => {
  await signIn(page);
  await page.goto('/resources/templates');
  await page.getByRole('button', { name: 'Add template' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add template' });
  await dialog.getByRole('radio', { name: 'Provider', exact: true }).click();
  await dialog.getByLabel('Name', { exact: true }).fill('Team runtime tools');
  await dialog.getByLabel('Image reference').fill('example.invalid/team-runtime:1');
  await dialog.getByRole('button', { name: 'Add template' }).click();
  await expect(page.getByRole('heading', { name: 'Team runtime tools', level: 1 })).toBeVisible();
  await page.getByRole('tab', { name: 'Configuration', exact: true }).click();

  await templateSection(page, 'Commands after start');
  const commands = page.getByRole('region', { name: 'Commands after start' });
  await commands.getByRole('button', { name: 'Edit Commands after start', exact: true }).click();
  await commands.getByRole('button', { name: 'Add item' }).click();
  await commands.getByLabel('Command name').fill('10-tools');
  await commands.getByLabel('Program and arguments · one per line').fill('/opt/setup\na path with spaces\n$HOME/bin\n--literal');
  await commands.getByLabel('Time limit', { exact: true }).fill('45');
  await templateSection(page, 'Registered programs');
  const programs = page.getByRole('region', { name: 'Registered programs' });
  await programs.getByRole('button', { name: 'Edit Registered programs', exact: true }).click();
  const kinds = [{ name: 'Provider', id: 'provider' }, { name: 'Translator', id: 'translator' }, { name: 'Adapter', id: 'adapter' }, { name: 'Executor', id: 'executor' }];
  for (const [index, kind] of kinds.entries()) {
    await programs.getByRole('button', { name: 'Add item', exact: true }).first().click();
    const program = programs.getByRole('group', { name: `Item ${index + 1}`, exact: true });
    await program.getByRole('combobox', { name: 'Program kind' }).click();
    await page.getByRole('option', { name: kind.name, exact: true }).click();
    await program.getByLabel('Registered name').fill(`team-${kind.id}`);
    await program.getByLabel('Program and arguments · one per line').fill(`/opt/team-${kind.id}\none literal argument`);
  }
  await page.getByRole('button', { name: 'Save template', exact: true }).click();
  await expect(page.getByText('Team runtime tools was saved', { exact: true })).toBeVisible();
  await templateSection(page, 'Commands after start');
  await commands.getByRole('button', { name: 'Edit Commands after start', exact: true }).click();
  await expect(page.getByLabel('Command name')).toHaveValue('10-tools');
  await expect(page.getByLabel('Time limit', { exact: true })).toHaveValue('45');
  await templateSection(page, 'Registered programs');
  await programs.getByRole('button', { name: 'Edit Registered programs', exact: true }).click();
  const persisted = page.getByRole('region', { name: 'Registered programs' });
  await expect(persisted.getByRole('group', { name: /^Item \d+$/, exact: true })).toHaveCount(4);

  // The list editor writes real argv, including spaces and dollar signs, without a shell prefix.
  await templateJson(page);
  const recipe: unknown = JSON.parse(await page.getByLabel('Configuration (JSON)').innerText());
  expect(recipe).toMatchObject({ image: 'example.invalid/team-runtime:1', after_up: [{ name: '10-tools', command: ['/opt/setup', 'a path with spaces', '$HOME/bin', '--literal'], timeout_secs: 45 }],
    plugins: kinds.map((kind) => ({ kind: kind.id, name: `team-${kind.id}`, command: [`/opt/team-${kind.id}`, 'one literal argument'] })),
  });
  await page.reload();
  await templateSection(page, 'Commands after start');
  await page.getByRole('button', { name: 'Edit Commands after start', exact: true }).click();
  await expect(page.getByLabel('Command name')).toHaveValue('10-tools');
  await templateSection(page, 'Registered programs');
  await page.getByRole('button', { name: 'Edit Registered programs', exact: true }).click();
  for (const [index, kind] of kinds.entries()) {
    const program = page.getByRole('region', { name: 'Registered programs' }).getByRole('group', { name: `Item ${index + 1}`, exact: true });
    await expect(program.getByLabel('Registered name')).toHaveValue(`team-${kind.id}`);
    await expect(program.getByRole('combobox', { name: 'Program kind' })).toHaveText(kind.name);
    await expect(program.getByLabel('Program and arguments · one per line')).toHaveValue(`/opt/team-${kind.id}\none literal argument`);
  }
  await page.goto(`/resources/machines/${RUNS_ON.laptop}`);
  await expect(page.getByRole('heading', { name: 'ada-laptop', level: 1 })).toBeVisible();
  await expect(page.getByText("Read-only · set in this machine's configuration by its owner.")).toBeVisible();
  await expect(page.getByRole('button', { name: 'Add program' })).toHaveCount(0);
});

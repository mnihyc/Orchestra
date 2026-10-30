/** A creation question is answered at its guide section or the real containing place's permission row. */
import { expect, test } from '@playwright/test';
import { acmeTree } from '../doubles/fixtures';
import { ACME, changeTree, deny, PEOPLE, scenario, signIn } from './support';

test.beforeEach(async () => { await scenario(); });

test('the finder opens the creation section for create project and start thread', async ({ page }) => {
  await signIn(page);
  for (const query of ['create project', 'start thread']) {
    await page.goto('/help');
    await page.getByRole('button', { name: 'Find anything' }).click();
    const finder = page.getByRole('dialog', { name: /Find/ });
    await finder.getByRole('combobox').fill(`?${query}`);
    const section = finder.getByRole('option', { name: /Who may make projects, topics and threads/ });
    await expect(section).toBeVisible();
    await section.click();
    await expect(page.getByRole('heading', { name: 'Who may make projects, topics and threads', level: 2 })).toBeVisible();
  }
});

for (const kind of ['org', 'project'] as const) {
  test(`an empty ${kind}'s creation question opens its own permission row without a permission write`, async ({ page }) => {
    await deny(PEOPLE.vera.login, ['org.interact', 'project.create_topics']);
    const id = kind === 'org' ? ACME.org : ACME.web;
    await changeTree(acmeTree().filter((row) => row.parent_id === id).map((row) => ({ id: row.id, deleted: true })));
    await signIn(page, PEOPLE.vera);
    await page.goto(kind === 'org' ? '/org/acme' : `/project/${id}`);
    const writes: string[] = [];
    page.on('request', (request) => {
      if (['POST', 'PUT', 'PATCH', 'DELETE'].includes(request.method()) && /\/(?:page|defaults)(?:\/|$)/.test(new URL(request.url()).pathname)) writes.push(request.url());
    });
    await page.getByRole('button', { name: kind === 'org' ? 'Who may make projects here?' : 'Who may open topics here?' }).click();
    const row = page.locator(`[data-access-permission="${kind === 'org' ? 'org.interact' : 'project.create_topics'}"]`);
    await expect(row).toBeVisible();
    await expect(row).toBeFocused();
    expect(writes).toEqual([]);
  });
}

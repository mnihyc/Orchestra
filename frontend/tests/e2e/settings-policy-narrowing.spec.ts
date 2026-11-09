/** A stored loose policy stays harmless, and becomes effective only after its real parent relaxes. */
import { expect, test, type APIRequestContext } from '@playwright/test';
import { caption } from '../../e2e/capture/open.ts';
import { signInScreenRoute } from '../../e2e/layout-fixtures.ts';
import { ACME, DOUBLE, PEOPLE, scenario } from './support.ts';

const key = 'thread.side_max';
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
async function write(request: APIRequestContext, token: string, id: string, value: number) {
  const headers = { Authorization: `Bearer ${token}` };
  const read = await request.get(`${DOUBLE}/api/v1/nodes/${id}/settings`, { headers }); expect(read.ok()).toBe(true);
  const rows: unknown = await read.json(); if (!Array.isArray(rows)) throw new Error('The policy fixture requires the real settings list.');
  const row: unknown = rows.find((row: unknown) => record(row) && row.key === key);
  const revision = record(row) && record(row.own) && typeof row.own.revision === 'number' ? row.own.revision : null;
  const answer = await request.put(`${DOUBLE}/api/v1/nodes/${id}/settings/${key}`, { headers, data: { value, expected_revision: revision } });
  expect(answer.ok()).toBe(true);
}
for (const width of [400, 1440] as const) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`a policy only narrows its ancestor ${width}/${theme}/${locale}`, async ({ page, request }) => {
    await scenario();
    const actor = await request.post(`${DOUBLE}/api/v1/auth/login`, { data: PEOPLE.ada }); expect(actor.ok()).toBe(true);
    const login: unknown = await actor.json();
    if (!record(login) || typeof login.access_token !== 'string') throw new Error('The policy fixture needs its fake signed-in author.');
    await write(request, login.access_token, ACME.org, 4); await write(request, login.access_token, ACME.web, 8);
    await signInScreenRoute(page, { width, theme, locale });
    const address = `/project/${ACME.web}/settings?section=policies&setting=${key}`;
    const writes: string[] = [];
    page.on('request', (outgoing) => {
      const path = new URL(outgoing.url()).pathname;
      if (outgoing.method() === 'PUT' && path.endsWith(`/settings/${key}`)) writes.push(path);
    });
    const title = caption('@day100.builtin.thread.side_max.title', locale);
    const row = page.locator(`[data-setting="${key}"]:visible`);
    const edit = () => row.getByRole('button', { name: caption('@settings.compact.editValue', locale).replace('{{name}}', title), exact: true });
    const hint = caption('@settings.compact.aboveStricter', locale);
    await page.goto(address); await expect(row.getByText(hint, { exact: true })).toBeVisible(); await expect(edit()).toHaveAccessibleDescription('4');
    await edit().click(); const input = row.getByLabel(title, { exact: true }); await expect(input).toHaveValue('8');
    await input.fill('12');
    const saved = page.waitForResponse((answer) => answer.request().method() === 'PUT' && new URL(answer.url()).pathname === `/api/v1/nodes/${ACME.web}/settings/${key}`);
    await input.press('Enter'); const response = await saved; expect(response.ok()).toBe(true);
    const accepted: unknown = await response.json();
    if (!record(accepted) || !record(accepted.own) || !record(accepted.source) || !record(accepted.source.node)) throw new Error('A policy write must retain its own value and effective source.');
    expect(accepted.own.value).toBe(12); expect(accepted.value).toBe(4); expect(accepted.source.node.id).toBe(ACME.org);
    const parentName = accepted.source.node.name;
    if (typeof parentName !== 'string') throw new Error('The policy fixture author needs its readable parent source.');
    await expect(row.getByText(hint, { exact: true })).toBeVisible(); await expect(edit()).toHaveAccessibleDescription('4');
    expect(writes).toEqual([`/api/v1/nodes/${ACME.web}/settings/${key}`]);
    await page.reload(); await expect(row.getByText(hint, { exact: true })).toBeVisible();
    await edit().click(); await expect(row.getByLabel(title, { exact: true })).toHaveValue('12'); await page.keyboard.press('Escape');

    const inherited = row.getByRole('link', { name: caption('@settings.compact.inheritedFrom', locale).replace('{{place}}', parentName), exact: true });
    await inherited.click(); await expect(page).toHaveURL(/\/org\/[^/]+\/settings/);
    await expect(edit()).toHaveAccessibleDescription('4'); await edit().click();
    const parentInput = row.getByLabel(title, { exact: true }); await parentInput.fill('20');
    const relaxed = page.waitForResponse((answer) => answer.request().method() === 'PUT' && new URL(answer.url()).pathname === `/api/v1/nodes/${ACME.org}/settings/${key}`);
    await parentInput.press('Enter'); expect((await relaxed).ok()).toBe(true);
    await page.goto(address); await expect(edit()).toHaveAccessibleDescription('12');
    await expect(row.getByText(hint, { exact: true })).toHaveCount(0);
    expect(writes).toEqual([
      `/api/v1/nodes/${ACME.web}/settings/${key}`,
      `/api/v1/nodes/${ACME.org}/settings/${key}`,
    ]);
    await expect(page.locator('html')).toHaveAttribute('lang', locale); await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
  });
}

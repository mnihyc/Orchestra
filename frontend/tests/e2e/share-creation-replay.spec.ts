/** A lost share answer retains the native client identity and the account-owned creation header. */
import { expect, test } from '@playwright/test';
import { caption } from '../../e2e/capture/open.ts';
import { begin, MAIN, openThread, showView } from './inspector-support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const)
for (const width of [400, 1440]) test(`sharing replays an accepted receipt in ${locale}/${theme}/${width}`, async ({ page }, info) => {
  await begin();
  await page.addInitScript(({ theme }) => localStorage.setItem('orch.appearance', JSON.stringify({ locale: 'en', theme, density: 'comfortable' })), { theme });
  await page.setViewportSize({ width, height: 1000 });
  await page.emulateMedia({ colorScheme: theme });
  await openThread(page);
  if (locale === 'zh-CN') {
    await page.goto('/settings/interface');
    await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
    await page.getByRole('option', { name: '简体中文', exact: true }).click();
    await expect(page.locator('html')).toHaveAttribute('lang', locale);
    await page.goto(`/thread/${MAIN}`);
  }
  const word = (key: string) => caption(`@${key}`, locale);
  await showView(page, 'Knowledge');
  await page.getByRole('button', { name: word('threadViews.knowledge.share'), exact: true }).click();
  // The shared name picker now lives in this dialog as a combobox, without a second picker sheet.
  await page.getByRole('combobox', { name: word('threadViews.knowledge.choose'), exact: true }).fill('Web');
  await page.getByRole('listbox', { name: word('threadViews.knowledge.choose'), exact: true }).getByRole('option', { name: /^Web(?:\s|$)/ }).first().click();
  const dialog = page.getByRole('dialog', { name: word('access.agents.dialog.title').replace('{{name}}', 'Web'), exact: true });
  const attempts: { key: string | undefined; body: string | null }[] = [];
  let accepted: unknown;
  await page.route('**/api/v1/share/agents', async (route) => {
    attempts.push({ key: route.request().headers()['idempotency-key'], body: route.request().postData() });
    if (attempts.length === 1) {
      const response = await route.fetch();
      expect(response.status()).toBe(201);
      const reply = await response.json() as { shares: { id: string; to: { id: string; kind: string }; mode: string; input: { id: string } | null }[] };
      accepted = { projection: 'receipt', shares: reply.shares.map((share) => ({ id: share.id,
        to: { id: share.to.id, kind: share.to.kind }, mode: share.mode, created: true, input_id: share.input?.id ?? null })) };
      await route.abort('connectionfailed');
    } else await route.fulfill({ status: 201, json: accepted });
  });
  const who = word('access.agents.dialog.button.ofAgents').replace('{{name}}', 'Main thread');
  const what = word('access.agents.dialog.button.this').replace('{{name}}', 'Web');
  const submit = dialog.getByRole('button', { name: word('access.agents.dialog.button.read').replace('{{who}}', who).replace('{{what}}', what), exact: true });
  await expect(submit).toBeEnabled();
  await submit.click();
  await expect(dialog.getByRole('alert')).toBeVisible();
  await submit.focus();
  await page.keyboard.press('Enter');
  await expect(dialog).toHaveCount(0);
  expect(attempts).toHaveLength(2);
  expect(attempts[0]?.key).toMatch(/^[a-f0-9-]{36}$/i);
  expect(attempts[1]).toEqual(attempts[0]);
  const body = JSON.parse(attempts[0]?.body ?? 'null') as { client_id: string; to: { thread_id: string }[] };
  expect(body.client_id).toBe(attempts[0]?.key);
  expect(body.to).toEqual([{ thread_id: MAIN }]);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await info.attach('share-replay', { body: await page.screenshot(), contentType: 'image/png' });
});

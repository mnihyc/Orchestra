/** Team chat opens beside the current document and receives the caret without a route change. */
import { expect, test } from '@playwright/test';
import { ACME, scenario, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
async function documentBesideChat(): Promise<string> {
  const response = await fetch(`${DOUBLE}/__double/documents/make`, { method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ user: 'ada', node: ACME.fixLogin, title: 'Keep this document open', count: 1 }) });
  if (!response.ok) throw new Error(`The document fixture failed: ${response.status}`);
  const answer: unknown = await response.json();
  if (typeof answer !== 'object' || answer === null || !('id' in answer) || typeof answer.id !== 'string') throw new Error('The fixture returned no document identifier');
  return answer.id;
}
test.beforeEach(async () => { await scenario(); });

for (const width of [400, 1440]) for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`team chat stays beside the document at ${width}/${locale}/${theme}`, async ({ page }) => {
    const id = await documentBesideChat();
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    await page.goto(`/doc/${id}`);
    const paragraph = page.getByText('Paragraph 1: the quick brown fox jumps over the lazy dog.');
    await expect(paragraph).toBeVisible();
    const address = page.url();
    if (width === 400) await page.getByRole('button', { name: locale === 'en' ? 'Navigation' : '导航', exact: true }).click();
    const shortcut = page.getByRole('button', { name: locale === 'en' ? 'Team chat' : '团队聊天', exact: true });
    await expect(shortcut).toBeVisible();
    await shortcut.focus(); await shortcut.press('Enter');
    const box = page.locator('[data-chat-composer]:visible textarea');
    await expect(box).toHaveCount(1);
    await expect(box).toBeFocused();
    expect(page.url()).toBe(address);
    if (width === 1440) {
      await expect(paragraph).toBeVisible();
      await expect(page.locator('[data-workbench-bar="right"] [data-active-container-title]')).toHaveText(locale === 'en' ? 'Team' : '团队');
    }
    await box.fill('A draft for the team');
    if (width === 400) {
      await page.getByRole('dialog').getByRole('button', { name: locale === 'en' ? 'Close' : '关闭', exact: true }).click();
      await expect(paragraph).toBeVisible();
    } else {
      await page.locator('[data-workbench-bar="right"]').getByRole('button', { name: locale === 'en' ? 'Hide the right bar' : '隐藏右栏', exact: true }).click();
    }
    if (width === 400) await page.getByRole('button', { name: locale === 'en' ? 'Navigation' : '导航', exact: true }).click();
    await page.getByRole('button', { name: locale === 'en' ? 'Team chat' : '团队聊天', exact: true }).click();
    await expect(box).toBeFocused();
    await expect(box).toHaveValue('A draft for the team');
    expect(page.url()).toBe(address);
  });
}

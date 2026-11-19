/** Thread controls and kept Ask work stay readable through live layout changes in both languages. */
import { expect, test } from '@playwright/test';
import { ASK_ID, askSnapshot } from '../unit/ask-fixtures';
import { threadDraft } from '../doubles/thread-rows';
import type { ThreadOpening } from '../../src/generated/api-types';
import { fixture, language } from './ask-support';
import { ACME, playScene, scenario } from './support';

test.use({ timezoneId: 'Asia/Singapore' });

for (const locale of ['en', 'zh-CN'] as const) for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  // Thread → expand connection → scroll conversation → collapse; the binding names each keep room.
  // 线程 → 展开工具连接 → 滚动对话 → 折叠；各个绑定名称都有可读空间。
  // Interface → This device → leave → hover the full value; Ask → draft → resize → reload → keyboard hints.
  // 界面 → 此设备 → 离开 → 悬停查看完整值；问一问 → 草稿 → 调整宽度 → 刷新 → 键盘提示。
  test(`thread and Ask keep their layout through resize ${locale}/${width}/${theme}`, async ({ page, request }, info) => {
    await scenario(); await playScene(ACME.mainThread, 'long', 12);
    await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    const server = await fixture(page, [askSnapshot()]), t = await language(page, request, locale, theme);
    server.drafts.set(ASK_ID, threadDraft([{ type: 'text', text: 'A saved question' }]));
    await page.route(`**/threads/${ACME.mainThread}/opening**`, async (route) => {
      const response = await route.fetch(), opening = await response.json() as ThreadOpening;
      const thread = opening.sync.thread;
      if (thread.binding === null) throw new Error('This screen has an active binding');
      await route.fulfill({ response, json: { ...opening, sync: { ...opening.sync, thread: { ...thread,
        binding: { ...thread.binding, agent: { ...thread.binding.agent, name: 'Example coding agent' }, provider: { ...thread.binding.provider, name: 'Scripted example provider' } },
        model_display: 'Large example model',
      } } } });
    });
    await page.goto(`/thread/${ACME.mainThread}`);
    const thread = page.locator('[data-screen="thread"]'), controls = thread.locator('[data-composer-controls]');
    const disclosure = thread.locator('summary').filter({ hasText: t('regionalGateways.toolConnection') });
    await expect(disclosure).toBeVisible();
    const log = thread.getByRole('log', { name: t('timeline.label'), exact: true });
    const noOverlap = async () => {
      const summary = await disclosure.boundingBox(), conversation = await log.boundingBox();
      if (summary === null || conversation === null) throw new Error('Both the disclosure and conversation are laid out');
      expect(summary.y + summary.height).toBeLessThanOrEqual(conversation.y + 1);
      const reserved = await disclosure.evaluate((element) => {
        const details = element.closest('details');
        return details !== null && details.parentElement !== null && details.getBoundingClientRect().bottom <= details.parentElement.getBoundingClientRect().bottom + 1;
      });
      expect(reserved).toBe(true);
    };
    await noOverlap(); await disclosure.click(); await expect(disclosure.locator('..')).toHaveAttribute('open', '');
    await noOverlap(); await thread.evaluate((element) => { element.scrollTop = 0; }); await noOverlap();
    await disclosure.click(); await noOverlap();
    await expect(controls.getByText('Example coding agent', { exact: true })).toBeVisible();
    await expect(controls.getByText('Scripted example provider', { exact: true })).toBeVisible();
    await expect(controls.locator('[data-model-name]')).toHaveText('Large example model');
    if (width === 400) {
      const binding = await controls.locator('[data-composer-control="agent"]').boundingBox(), model = await controls.locator('[data-composer-control="model"]').boundingBox();
      if (binding === null || model === null) throw new Error('The binding and model controls are visible');
      expect(model.y).toBeGreaterThanOrEqual(binding.y + binding.height - 1);
      for (const name of ['Example coding agent', 'Scripted example provider', 'Large example model']) {
        const label = controls.getByText(name, { exact: true });
        expect(await label.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
      }
    }
    expect(await thread.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    await info.attach('thread-controls', { body: await page.screenshot({ animations: 'disabled' }), contentType: 'image/png' });

    await page.goto('/settings/interface');
    const zone = page.getByRole('combobox', { name: t('profile.look.timeZone'), exact: true });
    await zone.click(); await page.getByRole('option', { name: t('choiceCatalog.timeZone.device.name'), exact: true }).click();
    await page.getByRole('heading', { level: 1 }).click();
    const value = await zone.inputValue(); expect(value).toContain('Asia/Singapore');
    expect(await zone.evaluate((element) => getComputedStyle(element).textOverflow)).toBe('ellipsis');
    await zone.hover();
    if (await zone.evaluate((element) => element.scrollWidth > element.clientWidth)) await expect(page.getByRole('tooltip')).toHaveText(value);
    await info.attach('time-zone-value', { body: await page.screenshot({ animations: 'disabled' }), contentType: 'image/png' });

    await page.getByRole('button', { name: t('ask.open'), exact: true }).click();
    const panel = page.locator('[data-view-window="ask"]'), box = panel.getByRole('textbox', { name: t('ask.composer'), exact: true });
    await expect(box).toHaveValue('A saved question'); await expect(box).toBeEnabled(); await box.fill('Keep this question through a live resize');
    await expect.poll(() => [...server.drafts.values()].some((draft) => draft?.content.some((part) => part.type === 'text' && part.text === 'Keep this question through a live resize'))).toBe(true);
    const editor = await box.elementHandle();
    for (const nextWidth of width === 1440 ? [400, 1440, 400] : [1440, 400]) {
      await page.setViewportSize({ width: nextWidth, height: 900 });
      await expect(box).toHaveValue('Keep this question through a live resize');
      expect(await box.evaluate((element, original) => element === original, editor)).toBe(true);
      await expect(box).toBeInViewport(); await expect(panel.getByRole('button', { name: t('composer.send'), exact: true })).toBeInViewport();
      if (nextWidth === 400) {
        await expect.poll(() => panel.evaluate((element) => {
          const sheet = element.closest('[role="dialog"]'), chrome = document.querySelector('[data-workbench-chrome]');
          return sheet === null || chrome === null ? null : Math.abs(sheet.getBoundingClientRect().top - Math.ceil(chrome.getBoundingClientRect().bottom));
        })).toBeLessThanOrEqual(1);
      }
    }
    await page.mouse.move(1, 899);
    page.once('dialog', (dialog) => dialog.accept()); await page.reload();
    const label = t('workbench.containers.menu', { name: t('ask.title') });
    const action = page.locator('[data-active-container="ask"] [data-container-title]').getByRole('button', { name: label, exact: true });
    await expect(action).toBeFocused(); await expect(page.getByRole('tooltip').filter({ hasText: label })).toHaveCount(0);
    await info.attach('ask-quiet-restoration', { body: await page.screenshot({ animations: 'disabled' }), contentType: 'image/png' });
    await action.press('Tab'); await page.keyboard.press('Shift+Tab');
    await expect(action).toBeFocused(); await expect(page.getByRole('tooltip').filter({ hasText: label })).toBeVisible();
    await page.keyboard.press('Escape'); await action.hover(); await expect(page.getByRole('tooltip').filter({ hasText: label })).toBeVisible();
    expect(server.sends).toHaveLength(0);
    await info.attach('ask-resize-restored', { body: await page.screenshot({ animations: 'disabled' }), contentType: 'image/png' });
  });
}

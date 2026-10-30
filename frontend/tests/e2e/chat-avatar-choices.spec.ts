/** Real right-bar scope selection and the uncovered avatar palette at desktop/phone widths. */
import { expect, type Locator, test } from '@playwright/test';

import { contrast } from '../accessibility/contrast.ts';
import { chats } from './chat-support.ts';
import { ACME, bodySent, scenario, signIn } from './support.ts';
import { showWorkbenchView } from './workbench-support.ts';

const WORDS = {
  en: { chats: 'Chats of this place', topic: 'Topic', project: 'Project', palette: 'Avatar colour', colour: 'Colour', reset: 'Reset avatar colour' },
  'zh-CN': { chats: '此处的聊天', topic: '话题', project: '项目', palette: '头像颜色', colour: '颜色', reset: '重置头像颜色' },
} as const;

/** A physical stroke lasts across the radio group's deferred focus step, as in the resource keyboard cases. */
const KEY_HELD_MS = 60;

async function scopePaint(option: Locator) {
  await option.evaluate(async (element) => { await Promise.all(element.getAnimations().map((animation) => animation.finished.catch(() => undefined))); });
  return option.evaluate((element) => {
    const style = getComputedStyle(element);
    const group = element.closest('[data-segmented]');
    if (group === null) throw new Error('The chat scope must use the shared segmented control.');
    const words = element.querySelector('[data-choice-name]');
    if (words === null) throw new Error('The scope has no visible name.');
    return { background: style.backgroundColor, border: style.borderTopColor,
      foreground: getComputedStyle(words).color, surface: getComputedStyle(group).backgroundColor };
  });
}

for (const width of [1440, 400]) for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`chat scopes and avatar rings stay visible at ${width}px in ${locale}/${theme}`, async ({ page }, info) => {
    await scenario();
    await chats('/locale', { as: 'ada', locale });
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    await page.goto(`/topic/${ACME.fixLogin}`);
    await showWorkbenchView(page, 'Chat');
    const words = WORDS[locale];
    const scopes = page.getByRole('radiogroup', { name: words.chats, exact: true });
    const topic = scopes.getByRole('radio', { name: words.topic, exact: true });
    const project = scopes.getByRole('radio', { name: words.project, exact: true });
    await expect(scopes).toHaveAttribute('data-segmented');
    await expect(scopes.getByRole('radio')).toHaveCount(2);
    await expect(topic).toBeChecked();
    await expect(topic).toHaveAttribute('data-state', 'checked');
    const selected = await scopePaint(topic);
    expect(selected.background).not.toBe('rgba(0, 0, 0, 0)');
    expect(contrast(selected.foreground, selected.background)).toBeGreaterThanOrEqual(4.5);
    expect(contrast(selected.border, selected.surface)).toBeGreaterThanOrEqual(3);
    expect((await scopePaint(project)).background).toBe('rgba(0, 0, 0, 0)');
    await topic.hover();
    await expect(page.getByRole('tooltip')).toContainText('Fix login');
    expect(await scopePaint(topic)).toEqual(selected);
    await project.click();
    await expect(project).toBeChecked();
    await expect(project).toBeFocused();
    await expect(project).toHaveAttribute('data-state', 'checked');
    expect((await scopePaint(project)).background).toBe(selected.background);
    await page.keyboard.press('ArrowLeft', { delay: KEY_HELD_MS });
    await expect(topic).toBeChecked();
    await expect(topic).toBeFocused();
    expect(await scopePaint(topic)).toEqual(selected);
    const scopeBox = await scopes.boundingBox();
    if (scopeBox === null) throw new Error('The scope is not drawn.');
    expect(scopeBox.x).toBeGreaterThanOrEqual(0);
    expect(scopeBox.x + scopeBox.width).toBeLessThanOrEqual(width);
    await info.attach('chat-scope-selection', { body: await page.screenshot(), contentType: 'image/png' });

    await page.goto('/settings/profile');
    const palette = page.getByRole('radiogroup', { name: words.palette, exact: true });
    await expect(palette).toBeVisible();
    await expect(page.getByText(words.palette, { exact: true })).toHaveCount(1);
    await expect(palette.getByRole('radio')).toHaveCount(12);
    const third = palette.getByRole('radio', { name: `${words.colour} 3`, exact: true });
    const colour = await third.isChecked() ? 3 : 2;
    const next = palette.getByRole('radio', { name: `${words.colour} ${colour + 1}`, exact: true });
    const accepted = page.waitForResponse((response) => response.request().method() === 'PATCH' && new URL(response.url()).pathname.endsWith('/me'));
    await next.click();
    const reply = await accepted;
    expect(reply.ok()).toBe(true);
    const payload: unknown = await bodySent(reply.request());
    expect(payload).toEqual({ avatar_color: colour });
    await expect(next).toBeChecked();
    await expect(next).toBeEnabled();
    await next.evaluate(async (element) => { await Promise.all(element.getAnimations().map((animation) => animation.finished.catch(() => undefined))); });
    const ring = await next.evaluate((element) => {
      const avatar = element.querySelector('.avatar');
      if (avatar === null) throw new Error('The colour choice has no sample.');
      const sample = avatar.getBoundingClientRect(), button = element.getBoundingClientRect();
      const style = getComputedStyle(element);
      const centre = document.elementFromPoint(sample.x + sample.width / 2, sample.y + sample.height / 2);
      let backdrop = element.parentElement;
      while (backdrop !== null && getComputedStyle(backdrop).backgroundColor === 'rgba(0, 0, 0, 0)') backdrop = backdrop.parentElement;
      if (backdrop === null) throw new Error('The palette needs an opaque page ground.');
      return { background: style.backgroundColor, border: style.borderTopColor, borderWidth: parseFloat(style.borderTopWidth),
        radius: parseFloat(style.borderTopLeftRadius), surface: getComputedStyle(backdrop).backgroundColor,
        sampleWidth: sample.width, sampleHeight: sample.height, buttonWidth: button.width, buttonHeight: button.height,
        gap: sample.left - button.left - parseFloat(style.borderLeftWidth),
        uncovered: centre !== null && avatar.contains(centre), overlays: element.querySelectorAll('svg, .absolute').length };
    });
    expect(ring.background).toBe('rgba(0, 0, 0, 0)');
    expect(ring.borderWidth).toBe(2);
    expect(ring.radius).toBeGreaterThanOrEqual(ring.buttonWidth / 2);
    expect(contrast(ring.border, ring.surface)).toBeGreaterThanOrEqual(3);
    expect(ring.sampleWidth).toBe(24); expect(ring.sampleHeight).toBe(24);
    expect(ring.buttonWidth).toBe(32); expect(ring.buttonHeight).toBe(32);
    expect(ring.gap).toBeGreaterThanOrEqual(2);
    expect(ring.uncovered).toBe(true); expect(ring.overlays).toBe(0);
    for (const option of await palette.getByRole('radio').all()) {
      const rect = await option.boundingBox();
      if (rect === null) throw new Error('A palette choice is not drawn.');
      expect(rect.x).toBeGreaterThanOrEqual(0);
      expect(rect.x + rect.width).toBeLessThanOrEqual(width);
    }
    await info.attach('uncovered-avatar-ring', { body: await page.screenshot(), contentType: 'image/png' });
    const reset = page.waitForResponse((response) => response.request().method() === 'PATCH' && new URL(response.url()).pathname.endsWith('/me'));
    await page.getByRole('button', { name: words.reset, exact: true }).click();
    const resetReply = await reset;
    expect(resetReply.ok()).toBe(true);
    const resetPayload: unknown = await bodySent(resetReply.request());
    expect(resetPayload).toEqual({ avatar_color: null });
    await expect(palette.locator('[role="radio"][aria-checked="true"]')).toHaveCount(1);
  });
}

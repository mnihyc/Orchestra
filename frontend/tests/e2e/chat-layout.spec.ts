/** Chats leave space for conversation at desktop and phone widths, beside the thread's ordinary controls. */
import { expect, type Locator, type Page, test } from '@playwright/test';

import { chats, messageSaying } from './chat-support.ts';
import { ACME, emit, scenario, signIn } from './support.ts';
import { showWorkbenchView } from './workbench-support.ts';

const WORDS = {
  en: { header: 'Chat controls', composer: 'Chat message controls', send: 'Send message', reply: 'Reply', cancelReply: 'Cancel reply', edit: 'Edit', editor: 'Edit message', save: 'Save', cancel: 'Cancel' },
  'zh-CN': { header: '聊天操作', composer: '聊天消息输入操作', send: '发送消息', reply: '回复', cancelReply: '取消回复', edit: '编辑', editor: '编辑消息', save: '保存', cancel: '取消' },
} as const;
// These are the shared control sizes at the default 16px root font; one pixel allows layout rounding.
const COMPACT_PX = 28;
const ORDINARY_PX = 36;
const ROUNDING_PX = 1;
const SCOPE_WORDS = {
  en: { group: 'Chats of this place', thread: 'Thread', topic: 'Topic', placeholder: 'Message…', context: 'Message Main thread…' },
  'zh-CN': { group: '此处的聊天', thread: '会话', topic: '话题', placeholder: '输入消息…', context: '向“Main thread”发消息…' },
} as const;

async function bounds(locator: Locator) {
  await expect(locator).toBeVisible();
  const rect = await locator.boundingBox();
  expect(rect).not.toBeNull();
  return rect!;
}

async function containedButtons(group: Locator, frame: { x: number; width: number }) {
  const buttons = group.getByRole('button');
  const result: Awaited<ReturnType<typeof bounds>>[] = [];
  for (const button of await buttons.all()) {
    const rect = await bounds(button);
    expect(rect.height).toBeLessThanOrEqual(ORDINARY_PX + ROUNDING_PX);
    expect(rect.x).toBeGreaterThanOrEqual(frame.x - ROUNDING_PX);
    expect(rect.x + rect.width).toBeLessThanOrEqual(frame.x + frame.width + ROUNDING_PX);
    await expect(button).toHaveAccessibleName(/\S/);
    result.push(rect);
  }
  // A wrapped control takes a new line, rather than covering another control.
  for (const [index, one] of result.entries()) for (const other of result.slice(index + 1)) {
    const horizontal = Math.min(one.x + one.width, other.x + other.width) - Math.max(one.x, other.x);
    const vertical = Math.min(one.y + one.height, other.y + other.height) - Math.max(one.y, other.y);
    expect(horizontal <= ROUNDING_PX || vertical <= ROUNDING_PX).toBe(true);
  }
  return result;
}

async function finishFonts(page: Page) {
  await page.evaluate(async () => { await document.fonts.ready; });
}

for (const width of [1440, 400]) for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`the panel chat uses one scope-and-action row at ${width}px in ${locale}, ${theme}`, async ({ page }, info) => {
    await scenario();
    await emit(ACME.mainThread, { thread: { state: 'idle', description: '' } });
    await chats('/locale', { as: 'ada', locale });
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    await page.goto(`/thread/${ACME.mainThread}`);
    await showWorkbenchView(page, 'Chat');
    const words = SCOPE_WORDS[locale];
    const scope = page.getByRole('radiogroup', { name: words.group, exact: true });
    await expect(scope).toBeVisible();
    const view = page.locator('[data-chat-view]').filter({ has: scope });
    const row = view.locator('[data-chat-header]');
    const frame = await bounds(row);
    expect(frame.height).toBeCloseTo(ORDINARY_PX, 0);
    const left = await bounds(scope);
    const icons = view.getByRole('group', { name: WORDS[locale].header, exact: true });
    const right = await bounds(icons);
    expect(left.x + left.width).toBeLessThanOrEqual(right.x + ROUNDING_PX);
    expect(Math.abs(left.y + left.height / 2 - (right.y + right.height / 2))).toBeLessThanOrEqual(ROUNDING_PX);
    await containedButtons(icons, frame);
    await expect(scope.getByRole('radio', { name: words.thread, exact: true })).toBeChecked();
    const box = view.getByRole('textbox');
    await expect(box).toHaveAttribute('placeholder', words.placeholder);
    await expect(box).toHaveAccessibleName(words.context);
    expect(await box.inputValue()).toBe('');
    const emptyLine = await box.evaluate((element) => {
      const style = getComputedStyle(element);
      const one = parseFloat(style.lineHeight) + parseFloat(style.paddingTop) + parseFloat(style.paddingBottom);
      return { actual: element.scrollHeight, expected: Math.max(parseFloat(style.minHeight) || 0, one) };
    });
    expect(emptyLine.actual).toBeLessThanOrEqual(emptyLine.expected + ROUNDING_PX);
    await scope.getByRole('radio', { name: words.thread, exact: true }).hover();
    await expect(page.getByRole('tooltip')).toContainText('Main thread');
    // The scope's keyboard changes the actual chat, including its composer destination and name.
    await scope.getByRole('radio', { name: words.thread, exact: true }).focus();
    await page.keyboard.press('ArrowRight');
    await expect(page.getByRole('radiogroup', { name: words.group }).getByRole('radio', { name: words.topic, exact: true })).toBeChecked();
    await expect(view.getByRole('textbox')).toHaveAttribute('placeholder', words.placeholder);
    await expect(view.getByRole('textbox')).toHaveAccessibleName(locale === 'en' ? 'Message Fix login…' : '向“Fix login”发消息…');
    await expect(page.getByRole('radiogroup', { name: words.group }).getByRole('radio', { name: words.topic, exact: true })).toBeFocused();
    await page.keyboard.press('ArrowLeft');
    await expect(page.getByRole('radiogroup', { name: words.group }).getByRole('radio', { name: words.thread, exact: true })).toBeChecked();
    await expect(view.getByRole('textbox')).toHaveAttribute('placeholder', words.placeholder);
    await expect(view.getByRole('textbox')).toHaveAccessibleName(words.context);
    await info.attach('single-chat-row', { body: await page.screenshot(), contentType: 'image/png' });
  });

  test(`chat controls leave conversation space at ${width}px in ${locale}, ${theme}`, async ({ page }, info) => {
    await scenario();
    const seeded = await chats('/seed');
    if (typeof seeded.chat !== 'string') throw new Error('The seeded conversation needs its chat id.');
    await emit(ACME.mainThread, { thread: { state: 'idle' } });
    await chats('/locale', { as: 'ada', locale });
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);

    // Keep the thread view's actual painted Send as the comparison, at exactly the same viewport/theme.
    await page.goto(`/thread/${ACME.mainThread}`);
    const threadSend = page.locator('[data-composer-control="send"]').getByRole('button').first();
    await expect(threadSend).toBeVisible();
    await finishFonts(page);
    const reference = await bounds(threadSend);
    expect(reference.height).toBeLessThanOrEqual(ORDINARY_PX + ROUNDING_PX);
    await info.attach('thread-reference', { body: await page.screenshot(), contentType: 'image/png' });

    await page.goto(`/org/acme/messages?chat=${seeded.chat}`);
    const view = page.locator(`[data-chat-view="${seeded.chat}"]`);
    await expect(view).toBeVisible();
    const frame = await bounds(view);
    expect(frame.x).toBeGreaterThanOrEqual(-ROUNDING_PX);
    expect(frame.x + frame.width).toBeLessThanOrEqual(width + ROUNDING_PX);
    const words = WORDS[locale];
    const header = view.getByRole('group', { name: words.header, exact: true });
    const controls = view.getByRole('group', { name: words.composer, exact: true });
    const box = controls.getByRole('textbox');
    await expect(box).toBeVisible();
    const footer = await bounds(view.locator('[data-chat-composer]'));
    const input = await bounds(box);
    // One line plus compact framing: no empty upload, attachment, typing or draft-status rows.
    expect(footer.height).toBeLessThanOrEqual(input.height + COMPACT_PX + ROUNDING_PX);
    const history = await bounds(view.locator('[data-chat-history]'));
    expect(history.height).toBeGreaterThan(frame.height - history.height);
    await containedButtons(header, frame);
    await containedButtons(controls, frame);
    const send = await bounds(controls.getByRole('button', { name: words.send, exact: true }));
    expect(send.height).toBeLessThanOrEqual(reference.height + ROUNDING_PX);
    expect(send.width).toBeLessThanOrEqual(COMPACT_PX + ROUNDING_PX);
    await info.attach('chat-one-line', { body: await page.screenshot(), contentType: 'image/png' });

    await box.fill('First line\nSecond line\nThird line\nFourth line\nFifth line\nSixth line');
    const grown = await bounds(box);
    expect(grown.height).toBeGreaterThan(input.height);
    const compact = await containedButtons(controls, frame);
    for (const button of compact) {
      expect(button.height).toBeLessThanOrEqual(COMPACT_PX + ROUNDING_PX);
      expect(Math.abs(button.y + button.height - grown.y - grown.height)).toBeLessThanOrEqual(ROUNDING_PX);
    }
    await box.focus();
    for (const button of await controls.getByRole('button').all()) {
      await page.keyboard.press('Tab');
      await expect(button).toBeFocused();
    }

    await box.fill('Compact chat layout review');
    await controls.getByRole('button', { name: words.send, exact: true }).click();
    const posted = messageSaying(page, 'Compact chat layout review');
    await expect(posted).toBeVisible();
    const messageId = await posted.getAttribute('data-message');
    if (messageId === null) throw new Error('The accepted message needs its durable id.');
    // A later reply repeats the excerpt, so keep the original by its accepted identity.
    const mine = view.locator(`[data-message="${messageId}"]`);
    await mine.hover();
    await mine.getByRole('button', { name: words.reply, exact: true }).click();
    const cancelReply = await bounds(view.getByRole('button', { name: words.cancelReply, exact: true }));
    expect(cancelReply.height).toBeLessThanOrEqual(COMPACT_PX + ROUNDING_PX);
    await box.fill('Compact reply layout review');
    await controls.getByRole('button', { name: words.send, exact: true }).click();
    const reply = messageSaying(page, 'Compact reply layout review');
    const jump = reply.getByRole('button', { name: /Ada Lovelace.*Compact chat layout review/ });
    expect((await bounds(jump)).height).toBeLessThanOrEqual(COMPACT_PX + ROUNDING_PX);
    await jump.click();
    await expect(mine).toBeVisible();
    await mine.hover();
    await mine.locator('button[data-action="actions"]').click();
    await page.getByRole('menuitem', { name: words.edit, exact: true }).click();
    await expect(mine.getByRole('textbox', { name: words.editor, exact: true })).toBeVisible();
    for (const name of [words.save, words.cancel]) {
      const action = await bounds(mine.getByRole('button', { name, exact: true }));
      expect(action.height).toBeLessThanOrEqual(COMPACT_PX + ROUNDING_PX);
      expect(action.width).toBeLessThan(frame.width / 2);
      expect(action.x + action.width).toBeLessThanOrEqual(frame.x + frame.width + ROUNDING_PX);
    }
    await info.attach('chat-edit', { body: await page.screenshot(), contentType: 'image/png' });
    await mine.getByRole('button', { name: words.cancel, exact: true }).click();
    await expect(mine.getByRole('textbox')).toHaveCount(0);
    await expect(mine.getByText('Compact chat layout review', { exact: true })).toBeVisible();
  });
}

/** Recovery from real saved layouts, through visible controls on the production frontend. */
import { writeFile } from 'node:fs/promises';
import { expect, type Page, type TestInfo, test } from '@playwright/test';
import type { Layout } from '../../src/features/workbench/layout';
import { ACME, emit, scenario, signIn } from './support.ts';
import { ADA } from '../doubles/thread-scenes.ts';
import { agentMessage, turn, userMessage } from '../doubles/thread-rows.ts';
import { showWorkbenchView } from './workbench-support.ts';

async function capture(page: Page, info: TestInfo, name: string) {
  const path = info.outputPath(`${name}.png`);
  await page.evaluate(() => document.fonts.ready);
  await page.screenshot({ path, fullPage: true, animations: 'disabled' });
  await info.attach(name, { path, contentType: 'image/png' });
}

test.beforeEach(async () => { await scenario(); });

for (const theme of ['light', 'dark'] as const) test(`a cold main Chat retains its addressed thread and hidden sections can be recovered in ${theme}`, async ({ page }, info) => {
  await page.emulateMedia({ colorScheme: theme });
  await page.setViewportSize({ width: 1440, height: 900 });
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await signIn(page);
  await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
  await page.goto(`/thread/${ACME.mainThread}`);
  // Navigation preserves the existing arrangement. Explicitly open the sections this recovery walk will hide.
  await showWorkbenchView(page, 'Outline');
  await showWorkbenchView(page, 'History');
  await showWorkbenchView(page, 'Chat');
  const side = page.locator('[data-workbench-bar="right"]');
  const chat = side.getByRole('button', { name: 'Chat', exact: true });
  await chat.focus();
  await page.keyboard.press('Shift+F10');
  await page.getByRole('menuitem', { name: 'Move to', exact: true }).press('ArrowRight');
  await page.getByRole('menuitem', { name: 'Main area', exact: true }).press('Enter');
  const tabs = page.getByRole('tablist', { name: 'Main area', exact: true });
  const movedChat = tabs.getByRole('tab', { name: /^Chat · Main thread(?: · Fix login)?$/ });
  const conversation = tabs.getByRole('tab', { name: /^Main thread(?: · Fix login)?$/ });
  await expect(movedChat).toHaveAttribute('aria-selected', 'true');
  await expect(conversation.locator('svg.lucide-message-square-text')).toBeVisible();
  const chatIdentity = await movedChat.getAttribute('id'), threadIdentity = await conversation.getAttribute('id');
  if (chatIdentity === null || threadIdentity === null) throw new Error('The actual view tabs must have stable identities');
  // Wait for the existing layout debounce, rather than injecting a layout into storage.
  await expect.poll(() => page.evaluate((threadId) => Object.keys(localStorage).some((key) => {
    if (!key.endsWith('.workbench.workspace')) return false;
    const saved: unknown = JSON.parse(localStorage.getItem(key) ?? 'null');
    if (typeof saved !== 'object' || saved === null || !('layout' in saved) || typeof saved.layout !== 'string') return false;
    const layout = JSON.parse(saved.layout) as Layout, main = layout.main;
    return main.kind === 'group' && main.tabs.some((tab) => tab.view === 'chat' && tab.id === main.active
      && tab.scope?.mode === 'fixed' && tab.scope.target.kind === 'thread' && tab.scope.target.id === threadId);
  }), ACME.mainThread)).toBe(true);
  const reads: string[] = [];
  page.on('request', (request) => { if (request.method() === 'GET' && new URL(request.url()).pathname.startsWith('/api/')) reads.push(new URL(request.url()).pathname); });
  await page.reload();
  await expect(conversation).toBeVisible(); await expect(conversation).toHaveAttribute('id', threadIdentity);
  await expect(movedChat).toHaveAttribute('aria-selected', 'true'); await expect(movedChat).toHaveAttribute('id', chatIdentity);
  await expect(page.getByRole('navigation', { name: 'Where this is' })).toContainText('Fix login');
  await expect(page.getByRole('radiogroup', { name: 'Chats of this place' })).toBeVisible();
  await expect(page.getByText('No chat on this screen', { exact: true })).toHaveCount(0);
  await capture(page, info, `cold-main-chat-${theme}`);
  const readPath = info.outputPath('cold-main-chat-reads.json');
  await writeFile(readPath, JSON.stringify(reads, null, 2));
  await info.attach('cold-main-chat-reads', { path: readPath, contentType: 'application/json' });

  await side.getByRole('tab', { name: 'Conversation', exact: true }).click();
  for (const name of ['Outline', 'History']) {
    await side.getByRole('button', { name: 'Conversation actions', exact: true }).click();
    const item = page.getByRole('menuitemcheckbox', { name, exact: true });
    await expect(item).toBeChecked();
    await item.click();
  }
  await expect(side.getByTestId('outline-panel')).toHaveCount(0);
  await page.getByRole('button', { name: 'Right bar', exact: true }).click();
  await expect(side).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Right bar', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: 'Right bar', exact: true }).click();
  await expect(side).toBeVisible();
  await side.getByRole('button', { name: 'Conversation actions', exact: true }).click();
  await page.getByRole('menuitemcheckbox', { name: 'Outline', exact: true }).click();
  await expect(side.getByTestId('outline-panel')).toBeVisible();
  await capture(page, info, `recovered-outline-${theme}`);
  await page.getByRole('button', { name: 'Layout', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Reset layout', exact: true }).click();
  // Reset recovers the frame and consolidates main views without closing them or changing their identity.
  await expect(movedChat).toHaveAttribute('aria-selected', 'true'); await expect(movedChat).toHaveAttribute('id', chatIdentity);
  await expect(conversation).toHaveAttribute('id', threadIdentity);
  await expect(side.getByRole('button', { name: 'Outline', exact: true })).toHaveAttribute('aria-expanded', 'true');
  await conversation.click(); await expect(conversation).toHaveAttribute('aria-selected', 'true');
  await page.setViewportSize({ width: 400, height: 900 });
  await expect(page.getByRole('textbox', { name: 'Message to the agent' })).toBeVisible();
  await capture(page, info, `restored-thread-400-${theme}`);
  expect(errors).toEqual([]);
});

for (const theme of ['light', 'dark'] as const) test(`agent replies use compact turn actions and the same named right-click actions in ${theme}`, async ({ page }, info) => {
  await page.emulateMedia({ colorScheme: theme });
  await page.setViewportSize({ width: 1440, height: 900 });
  const made = turn('compact-turn', { ordinal: 1, inputId: 'compact-input', state: 'completed', completedAt: '2026-10-16T00:00:02Z', startedAt: '2026-10-16T00:00:00Z' });
  await emit(ACME.mainThread, { thread: { state: 'idle', current_turn_id: null }, turns: [made], items: [
    userMessage('compact-question', 'Check the release.', { author: ADA, turn: made.id }),
    agentMessage('compact-answer', 'The release investigation is complete.', { turn: made.id }),
  ] });
  await signIn(page); await page.goto(`/thread/${ACME.mainThread}`);
  await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
  const message = page.locator('[data-item="compact-answer"]').getByText('The release investigation is complete.', { exact: true });
  await expect(message).toBeVisible();
  const footer = page.locator('[data-footer="compact-turn"]');
  const select = footer.getByRole('button', { name: 'Select this turn', exact: true });
  await expect(select).toBeVisible();
  await expect(select).toHaveText('');
  await message.click({ button: 'right' });
  await expect(page.getByRole('menuitem', { name: 'Select this turn', exact: true })).toBeVisible();
  await capture(page, info, `agent-context-${theme}`);
  await page.getByRole('menuitem', { name: 'Select this turn', exact: true }).click();
  await expect(page.locator('[data-picked]')).toHaveCount(2);
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(page.locator('[data-picked]')).toHaveCount(0);
  await capture(page, info, `agent-footer-${theme}`);
});

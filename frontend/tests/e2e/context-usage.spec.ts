/** What the tokens are made of and what fills the context: the ring beside the model, the coloured bar it opens with each part's numbers, a turn's bar and its details, and the thread's turns over time with an export. */
import { expect, type Page, test } from '@playwright/test';

import { agentMessage, threadInput, turn, userMessage } from '../doubles/thread-rows.ts';
import { ADA } from '../doubles/thread-scenes.ts';
import { ACME, emit, PEOPLE, scenario, signIn } from './support.ts';
import { showWorkbenchView } from './workbench-support.ts';

const MAIN = ACME.mainThread;
const AT = '2026-10-14T10:00:00.000Z';

test.beforeEach(async () => {
  await scenario();
  // One turn has ended: its harness reported 12 345 tokens, a sub-agent's share, the context and a shortening; Orch estimated what filled it.
  await emit(MAIN, {
    items: [
      userMessage('u1', 'Fix the login redirect', { author: ADA, turn: 't1', at: AT }),
      agentMessage('a1', 'The redirect now falls back to the home page.', { turn: 't1', at: AT, phase: 'final' }),
    ],
    inputs: [threadInput('in-1', 'Fix the login redirect', { author: ADA, position: '1', state: 'delivered' })],
    turns: [
      turn('t1', {
        ordinal: 1,
        state: 'completed',
        inputId: 'in-1',
        startedAt: AT,
        completedAt: AT,
        tokens: 12_345,
        detail: {
          subagents: { input_tokens: 2_000, cached_input_tokens: 0, output_tokens: 1_000, reasoning_tokens: 0, total_tokens: 3_000 },
          by_subagent: [{ stream: 'sub:helper', tokens: { input_tokens: 2_000, cached_input_tokens: 0, output_tokens: 1_000, reasoning_tokens: 0, total_tokens: 3_000 } }],
          models: [],
          context_window: 200_000,
          context_used: 142_000,
          compact_at: 180_000,
          shortenings: [{ at: AT, before: 170_000, after: 40_000, automatic: true }],
          breakdown: false,
        },
        fill: {
          count: 'orch',
          used: 142_000,
          window: 200_000,
          compact_at: 180_000,
          scaled: false,
          at: AT,
          parts: [
            { kind: 'harness', tokens: 20_000, count: 'orch', rows: [] },
            { kind: 'orch_instructions', tokens: 1_000, count: 'orch', rows: [] },
            { kind: 'people_messages', tokens: 1_000, count: 'orch', rows: [] },
            { kind: 'tool_calls', tokens: 120_000, count: 'orch', rows: [{ kind: 'tool', name: 'exec_command', tokens: 120_000, calls: 4_000, count: 'orch' }] },
          ],
        },
      }),
    ],
    thread: {
      state: 'idle',
      usage: { input_tokens: 9_876, cached_input_tokens: 6_172, output_tokens: 2_469, reasoning_tokens: 0, total_tokens: 12_345, context_used: 142_000, context_window: 200_000, compact_at: 180_000, updated_at: AT },
    },
  });
});

async function open(page: Page): Promise<void> {
  await signIn(page, PEOPLE.ada);
  await page.goto(`/thread/${MAIN}`);
}

test('the ring beside the model says how full the context is, and one click opens what fills it, part by part', async ({ page }) => {
  await open(page);
  const controls = page.getByRole('group', { name: 'Message controls' });
  const ring = controls.getByRole('button', { name: /^Show what fills the context \(Context 142k of 200k \(71%\); shortens at 90%\)$/ });
  await expect(page.locator('[data-thread-header]').getByRole('button', { name: /^Show what fills the context/ })).toHaveCount(0);
  await expect(ring).toBeVisible();
  await expect(ring.locator('[data-tone="amber"]')).toBeVisible();
  await ring.click();
  const panel = page.locator('[data-context-panel]');
  await expect(panel).toBeVisible();
  await expect(panel.getByText('estimated by Orch')).toBeVisible();
  // Each part is reached by the keyboard, and says its numbers.
  const tools = panel.getByRole('img', { name: /^Tool calls and their results: 120k, 60%$/ });
  await expect(tools).toBeVisible();
  await tools.focus();
  await expect(tools).toBeFocused();
  await expect(panel.locator('[data-context-part="reserve"]')).toContainText('Kept free for shortening');
  await expect(panel.locator('[data-context-shortened]')).toContainText('freed 130k');
  await panel.getByRole('button', { name: 'Show its 1 part' }).click();
  await expect(panel.getByText('exec_command')).toBeVisible();
});

test('a turn’s footer has the bar of what its tokens were, with the numbers under the pointer, and its details one click away', async ({ page }) => {
  await open(page);
  const footer = page.getByRole('button', { name: /^Show what the turn used: 12\.3k tokens/ });
  const target = await footer.boundingBox();
  expect(target).not.toBeNull();
  expect(target!.height).toBeGreaterThanOrEqual(24);
  expect(target!.width).toBeGreaterThanOrEqual(24);
  await expect(footer).toBeVisible();
  await expect(footer.locator('[data-part="subagents"]')).toHaveCount(1);
  await footer.click();
  const details = page.locator('[data-turn-cost="t1"]');
  await expect(details.getByText('By sub-agent')).toBeVisible();
  await expect(details.getByText(/credits/)).toBeVisible();
  await expect(details.getByText(/142k of 200k \(71%\)/)).toBeVisible();
});

test('the thread’s Usage view has its turns over time, the same as a table, and an export', async ({ page }) => {
  await open(page);
  await showWorkbenchView(page, 'Usage');
  const over = page.locator('[data-thread-composition]');
  await expect(over.getByRole('heading', { name: 'Over the turns' })).toBeVisible();
  await expect(over.locator('[data-usage-turn="1"]')).toBeVisible();
  await expect(over.locator('[data-usage-shortened]')).toHaveCount(1);
  await expect(over.locator('[data-usage-row="1"]')).toContainText('12.3k');
  const saved = page.waitForEvent('download');
  await over.getByRole('button', { name: 'Export as CSV' }).click();
  expect((await saved).suggestedFilename()).toMatch(/^orch-thread-.+-turns\.csv$/);
});

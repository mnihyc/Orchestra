/** Goal states, evidence and the space above the composer on the shipped application. */
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import { expect, type Page, test } from '@playwright/test';

import type { Item, ThreadGoal, ThreadOpening } from '../../src/generated/api-types';
import { agentMessage, ALLOW_DENY, approvalItem, harnessApproval, marker, plan, threadInput, turn, userMessage } from '../doubles/thread-rows';
import { ADA } from '../doubles/thread-scenes';
import { begin, MAIN } from './inspector-support';
import { addApproval, emit, PEOPLE, replySent, signIn } from './support';

const AT = '2026-10-28T10:00:00.000Z';
const GOAL = '00000000-0000-4000-8000-000000000091';
const CONDITION = 'npm test exits 0 and the login page returns people to the same project after they sign in';
const BASE: ThreadGoal = {
  id: GOAL, objective: 'Fix the login redirect and prove it with the tests', condition: CONDITION, set_by: ADA, set_at: AT,
  status: 'active', reason: null, detail: null, waiting: null, checking: false, plan_first: false, notify_on_end: false,
  limits: { max_turns: null, max_minutes: null, max_credits: null },
  budgets: { max_turns: 30, max_minutes: 0, max_credits: '0', locked: false },
  used: { turns: 7, elapsed_secs: 720, running_since: null, tokens: 1_200_000, credits: '0' },
  helper: true, agent_reports: true, last_report: null, ended_at: null,
};
const SCREEN_DIR = resolve(process.env.ORCH_GOAL_SCREEN_DIR ?? 'output/playwright/goals');

async function photograph(page: Page, name: string) {
  await mkdir(SCREEN_DIR, { recursive: true });
  await expect(page.locator('[data-main-area]')).toBeVisible();
  await page.screenshot({ path: resolve(SCREEN_DIR, `${name}.png`) });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
}

function nextOpening(page: Page): Promise<ThreadOpening> {
  return page.waitForRequest((request) => request.method() === 'GET' && new URL(request.url()).pathname.endsWith(`/threads/${MAIN}/opening`)).then(async (request) => {
    const response = await request.response();
    if (response === null) throw new Error('The opening request was cancelled before its response');
    expect(response.ok()).toBe(true);
    return replySent(response) as Promise<ThreadOpening>;
  });
}

for (const width of [1440, 400]) {
  for (const theme of ['light', 'dark'] as const) {
    test(`reloaded paused and stopped goals keep their actions and editor at ${width}px in ${theme}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
      await begin('');
      await signIn(page, PEOPLE.ada);
      await page.goto('about:blank');
      const initialOpening = nextOpening(page);
      await page.goto(`/thread/${MAIN}`);
      await initialOpening;
      let creates = 0;
      page.on('request', (request) => {
        if (request.method() === 'POST' && new URL(request.url()).pathname.endsWith(`/threads/${MAIN}/goal`)) creates += 1;
      });
      for (const status of ['paused', 'stopped'] as const) {
        const goal: ThreadGoal = { ...BASE, status, reason: status === 'paused' ? 'person' : 'interrupted', ended_at: status === 'stopped' ? AT : null };
        await emit(MAIN, { thread: { goal } });
        const response = nextOpening(page);
        await page.reload();
        const opening = await response;
        expect(opening.sync.thread.goal?.id).toBe(GOAL);
        expect(opening.sync.thread.goal?.status).toBe(status);
        const line = page.locator(`[data-goal-line="${status}"]`);
        await expect(line).toBeVisible();
        if (width === 400) await line.getByRole('button', { name: `Done when ${CONDITION}`, exact: true }).click();
        await expect(line.getByRole('button', { name: 'Resume', exact: true })).toBeVisible();
        await expect(line.getByRole('button', { name: 'Resume', exact: true })).not.toHaveAttribute('aria-disabled', 'true');
        await photograph(page, `${width}-${theme}-reload-${status}`);
        await line.getByRole('button', { name: 'Goal actions', exact: true }).click();
        await expect(page.getByRole('menuitem', { name: 'Clear goal', exact: true })).toBeVisible();
        await expect(page.getByRole('menuitem', { name: 'Edit goal…', exact: true })).toBeVisible();
        await photograph(page, `${width}-${theme}-reload-${status}-actions`);
        await page.getByRole('menuitem', { name: 'Edit goal…', exact: true }).click();
        const dialog = page.getByRole('dialog', { name: 'Edit goal Help: Goals', exact: true });
        await expect(dialog.getByRole('button', { name: 'Help: Goals', exact: true })).toBeVisible();
        await expect(dialog.getByLabel('Objective', { exact: true })).toHaveValue(BASE.objective);
        await expect(dialog.getByLabel('Done when', { exact: true })).toHaveValue(CONDITION);
        await photograph(page, `${width}-${theme}-reload-${status}-editor`);
        await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();

        await page.getByRole('button', { name: 'Actions for Main thread', exact: true }).click();
        await page.getByRole('menuitem', { name: 'Edit goal…', exact: true }).click();
        await expect(dialog.getByLabel('Objective', { exact: true })).toHaveValue(BASE.objective);
        await expect(dialog.getByLabel('Done when', { exact: true })).toHaveValue(CONDITION);
        await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
        const composer = page.getByRole('textbox', { name: 'Message to the agent', exact: true });
        await composer.fill('/goal a different condition');
        await composer.press('Enter');
        await expect(dialog).toBeVisible();
        await expect(dialog.getByLabel('Objective', { exact: true })).toHaveValue(BASE.objective);
        await expect(dialog.getByLabel('Done when', { exact: true })).toHaveValue(CONDITION);
        await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
        expect(creates).toBe(0);
      }
    });

    test(`goal states, editor, evidence and a busy strip fit at ${width}px in ${theme}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
      await begin('');
      await emit(MAIN, { thread: { goal: BASE } });
      await signIn(page, PEOPLE.ada);
      await page.goto(`/thread/${MAIN}`);
      await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
      const prefix = `${width}-${theme}`;
      const states: { chip: string; goal: ThreadGoal }[] = [
        { chip: 'working', goal: BASE },
        { chip: 'waiting', goal: { ...BASE, waiting: 'approval' } },
        { chip: 'paused', goal: { ...BASE, status: 'paused', reason: 'person' } },
        { chip: 'met', goal: { ...BASE, status: 'met', ended_at: AT } },
        { chip: 'stopped', goal: { ...BASE, status: 'stopped', reason: 'turns', ended_at: AT, used: { ...BASE.used, turns: 30 } } },
      ];
      for (const state of states) {
        await emit(MAIN, { thread: { goal: state.goal } });
        await expect(page.locator(`[data-goal-line="${state.chip}"]`)).toBeVisible();
        await photograph(page, `${prefix}-${state.chip}`);
      }
      await emit(MAIN, { thread: { goal: null } });
      await page.getByRole('button', { name: 'Actions for Main thread', exact: true }).click();
      await page.getByRole('menuitem', { name: 'Set a goal…', exact: true }).click();
      const dialog = page.getByRole('dialog', { name: 'Set a goal Help: Goals', exact: true });
      await expect(dialog.getByRole('button', { name: 'Help: Goals', exact: true })).toBeVisible();
      await expect(dialog.getByRole('button', { name: 'Set goal', exact: true })).toBeDisabled();
      await expect(dialog.getByRole('checkbox', { name: 'Notify me when it ends' })).not.toBeChecked();
      await photograph(page, `${prefix}-editor-closed`);
      await dialog.getByLabel('Objective', { exact: true }).fill(BASE.objective);
      await dialog.getByLabel('Done when', { exact: true }).fill(CONDITION);
      await dialog.locator('summary').filter({ hasText: /^Limits: none$/ }).click();
      await expect(dialog.getByLabel('Turns', { exact: true })).toBeVisible();
      await expect(dialog.getByRole('button', { name: 'Set goal', exact: true })).toBeEnabled();
      await photograph(page, `${prefix}-editor-open`);
      await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();

      const goalMessage = userMessage('goal-message', 'Goal: keep working until the tests pass', { author: ADA, turn: 'goal-turn' });
      if (goalMessage.kind !== 'user_message') throw new Error('The goal input must be a user-message item');
      const goalInput: Item = { ...goalMessage, payload: { ...goalMessage.payload, source: 'goal', goal: { goal_id: GOAL, condition: CONDITION, turn: 7, max_turns: 30, tokens: 1_200_000, not_yet: 'Prove that an external redirect is refused', plan: false } } };
      await emit(MAIN, {
        thread: { goal: { ...BASE, status: 'met', ended_at: AT } },
        turns: [turn('goal-turn', { ordinal: 1, state: 'completed', inputId: 'goal-input', completedAt: AT })],
        items: [
          marker('goal-set', { marker: 'goal_set', data: { goal_id: GOAL, by: ADA, objective: BASE.objective, condition: CONDITION } }),
          marker('goal-paused', { marker: 'goal_paused', data: { goal_id: GOAL, by: ADA, reason: 'person', detail: null } }),
          goalInput,
          agentMessage('goal-answer', 'The redirect stays on this origin and the tests pass.', { turn: 'goal-turn' }),
          marker('goal-met', { marker: 'goal_met', data: { goal_id: GOAL, condition: CONDITION, evidence: 'npm test: 19 passed\nExternal redirects are refused.', checked_by: 'helper', reason: null, turns: 7 } }),
          marker('goal-stopped', { marker: 'goal_stopped', data: { goal_id: GOAL, reason: 'turns', turns: 30, limit: '30' } }),
        ],
      });
      await expect(page.locator('[data-goal-marker="goal_paused"]')).toBeVisible();
      await page.getByRole('button', { name: 'Show the evidence', exact: true }).click();
      await expect(page.locator('[data-goal-evidence]')).toContainText('19 passed');
      await expect(page.locator('[data-goal-turn]')).toContainText('Not yet');
      await photograph(page, `${prefix}-timeline-evidence`);

      await addApproval(harnessApproval('goal-approval', { threadId: MAIN, itemId: 'goal-approval-item', title: 'Run the login tests', request: { request_kind: 'command', detail: { command: 'npm test', cwd: '/work', reason: 'Prove the redirect works' }, options: ALLOW_DENY }, requester: ADA }));
      await emit(MAIN, {
        thread: { state: 'blocked', current_turn_id: 'busy-turn', blocked: { reason: 'binding_invalid', detail: 'The sandbox is unavailable. Choose another agent.' }, goal: { ...BASE, waiting: 'blocked' } },
        turns: [turn('busy-turn', { ordinal: 2, state: 'running', inputId: 'busy-input' })],
        inputs: [threadInput('busy-queued', 'Then check the session cookies', { author: ADA, position: '2' })],
        items: [approvalItem('goal-approval-item', 'goal-approval', { turn: 'busy-turn' }),
          ...Array.from({ length: 12 }, (_, at) => agentMessage(`busy-work-${at}`, 'Checking the redirect and the response headers. '.repeat(30), { turn: 'busy-turn' })),
          plan('busy-plan', [{ text: 'Check the redirect', status: 'in_progress' }, { text: 'Run the tests', status: 'pending' }], { turn: 'busy-turn' })],
      });
      await expect(page.locator('[data-passive="blocked"]')).toBeVisible();
      const approvalBar = page.getByRole('status').filter({ hasText: '1 approval waiting' });
      const planLine = page.getByRole('button', { name: 'Step 1 of 2: Check the redirect', exact: true });
      const queue = page.locator('[data-queue]');
      await expect(approvalBar).toBeVisible();
      await expect(planLine).toBeVisible();
      await expect(queue).toBeVisible();
      const topEdges = await Promise.all([page.locator('[data-passive-strip]'), approvalBar, planLine, queue].map(async (row) => {
        const box = await row.boundingBox();
        if (box === null) throw new Error('A busy composer row has no bounds');
        return box.y;
      }));
      expect(topEdges.every((top, at) => at === 0 || top > (topEdges[at - 1] ?? 0))).toBe(true);
      const more = page.getByRole('button', { name: 'Show 1 more', exact: true });
      await expect(more).toBeVisible();
      await expect(page.locator('[data-goal-line]')).not.toBeVisible();
      await photograph(page, `${prefix}-busy`);
      await more.click();
      const passiveCard = page.getByRole('dialog', { name: 'Show 1 more', exact: true });
      await expect(passiveCard).toBeVisible();
      await expect(page.locator('[data-goal-line]')).toBeVisible();
      const primaryBounds = await page.locator('[data-passive="blocked"]').boundingBox();
      expect(primaryBounds?.y).toBe(topEdges[0]);
      await photograph(page, `${prefix}-busy-expanded`);
      await passiveCard.getByRole('button', { name: `Done when ${CONDITION}`, exact: true }).click();
      await expect(passiveCard.getByRole('button', { name: 'Pause', exact: true })).toBeVisible();
      await photograph(page, `${prefix}-busy-goal-expanded`);
      await passiveCard.locator('[data-goal-line]').click({ button: 'right' });
      await expect(page.getByRole('menuitem', { name: 'Clear goal', exact: true })).toBeVisible();
      await page.keyboard.press('Escape');
      await expect(passiveCard).toBeVisible();
      await page.keyboard.press('Escape');
      await expect(page.locator('[data-goal-line]')).not.toBeVisible();
      await expect(more).toBeFocused();
      await more.click();
      await expect(passiveCard).toBeVisible();
      await page.getByRole('heading', { name: 'Main thread', exact: true }).click();
      await expect(passiveCard).not.toBeVisible();

      await emit(MAIN, { thread: { state: 'idle', current_turn_id: null, blocked: null, goal: BASE }, turns: [turn('busy-turn', { ordinal: 2, state: 'completed', inputId: 'busy-input', completedAt: AT })], inputs: [{ ...threadInput('busy-queued', '', { author: ADA, position: '2', state: 'cancelled' }) }] });
      const line = page.locator('[data-goal-line="working"]');
      await expect(line).toBeVisible();
      if (width === 1440) {
        await expect(line.getByRole('button', { name: 'Pause', exact: true })).toBeVisible();
        await expect(line.locator('[data-goal-figures]')).toBeVisible();
        await expect(line.locator('[data-goal-figures]')).toContainText('7 of 30 turns');
      }
      if (width === 400) {
        await line.getByRole('button', { name: `Done when ${CONDITION}`, exact: true }).click();
        await expect(line.getByRole('button', { name: 'Pause', exact: true })).toBeVisible();
        await photograph(page, `${prefix}-phone-expanded`);
        const condition = line.getByText(`Done when ${CONDITION}`, { exact: true }).filter({ visible: true });
        expect(await condition.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
      }
      await line.click({ button: 'right' });
      await expect(page.getByRole('menuitem', { name: 'Clear goal', exact: true })).toBeVisible();
      await page.keyboard.press('Escape');
    });
  }
}

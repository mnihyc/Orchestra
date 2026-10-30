/** Goal outline entries use the same human words as the timeline after opening and live updates. */
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import { expect, test } from '@playwright/test';
import { begin, MAIN, showView } from './inspector-support';
import { emit, PEOPLE, signIn } from './support';
import { ADA } from '../doubles/thread-scenes';
import { turn, userMessage } from '../doubles/thread-rows';
const CONDITION = 'npm test exits 0 and the login page returns people to the same project after they sign in';
const SCREEN_DIR = resolve(process.env.ORCH_GOAL_SCREEN_DIR ?? 'output/playwright/goals');
for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) {
  test(`goal outline keeps human words after reload and live changes at ${width}px in ${theme}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await begin('');
    const message = userMessage('goal-outline', '<orch>\nInternal instructions\n</orch>', { author: ADA, turn: 'goal-outline-turn' });
    if (message.kind !== 'user_message') throw new Error('The fixture must be a user message');
    const goal = { goal_id: '00000000-0000-4000-8000-000000000091', condition: CONDITION, turn: 7, max_turns: 30, tokens: 1_200_000, not_yet: null, plan: false };
    const item = { ...message, payload: { ...message.payload, source: 'goal' as const, goal } };
    await emit(MAIN, { items: [item], turns: [turn('goal-outline-turn', { ordinal: 7, state: 'completed', inputId: 'goal-outline-input' })] });
    await signIn(page, PEOPLE.ada);
    await page.goto(`/thread/${MAIN}`);
    await page.reload();
    await showView(page, /^Outline/);
    const panel = page.getByTestId('outline-panel');
    await expect(panel.getByText(`Goal: keep working until ${CONDITION}`, { exact: true })).toBeVisible();
    await expect(panel).not.toContainText('<orch>');
    await expect(panel).not.toContainText('Internal instructions');
    await mkdir(SCREEN_DIR, { recursive: true });
    await page.screenshot({ path: resolve(SCREEN_DIR, `${width}-${theme}-outline-reloaded.png`) });
    const changed = 'The redirect tests pass and the person returns to the same project';
    await emit(MAIN, { items: [{ ...item, payload: { ...item.payload, goal: { ...goal, condition: changed } } }] });
    await expect(panel.getByText(`Goal: keep working until ${changed}`, { exact: true })).toBeVisible();
    await expect(panel).not.toContainText('<orch>');
    await page.screenshot({ path: resolve(SCREEN_DIR, `${width}-${theme}-outline-live.png`) });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
}

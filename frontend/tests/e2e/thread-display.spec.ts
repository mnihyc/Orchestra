/** The shipping frontend shows one supplied thread state in its tree, tab and header without icon reads. */
import { writeFile } from 'node:fs/promises';
import { expect, type Page, test } from '@playwright/test';
import type { Thread, ThreadDisplayState } from '../../src/generated/api-types';
import { display } from '../doubles/thread-display.ts';
import { ADA } from '../doubles/thread-scenes.ts';
import { agentMessage, ALLOW_DENY, approvalItem, harnessApproval, threadInput, turn, userMessage } from '../doubles/thread-rows.ts';
import { ACME, addApproval, emit, lookedAt, scenario, signIn, threadHeld } from './support.ts';

const MAIN = ACME.mainThread;
const SHAPES: Partial<Record<ThreadDisplayState, string>> = { idle: 'message-square-text', running: 'loader-circle', waiting_approval: 'shield-question-mark', blocked: 'circle-slash', failed: 'octagon-alert', queued: 'clock-3', held: 'circle-pause', finished: 'circle-check' };
const attention = (failed = false): Thread['attention'] => ({ failed, uncommitted: null, unpushed: null, as_of: null, stale: false, reason: null });

async function state(state: ThreadDisplayState, more: Partial<Thread> = {}) {
  await emit(MAIN, { thread: { state: state === 'running' ? 'running' : state === 'waiting_approval' ? 'waiting_approval' : state === 'blocked' ? 'blocked' : 'idle',
    pending_approvals: 0, blocked: state === 'blocked' ? { reason: 'provider_unavailable', detail: null } : null,
    attention: attention(state === 'failed'), display: display(state, { blocked_reason: state === 'blocked' ? 'provider_unavailable' : null, queued_inputs: state === 'queued' ? 2 : 0, held_inputs: state === 'held' ? 2 : 0 }), ...more } });
}

async function icons(page: Page, state: ThreadDisplayState, width: number) {
  const shape = SHAPES[state];
  if (shape === undefined) throw new Error('The walked state needs an icon');
  await expect(page.locator('[data-thread-header]').locator(`svg.lucide-${shape}`).first()).toBeVisible();
  const title = await page.locator('[data-thread-header]').getByRole('heading', { level: 1 }).innerText();
  await expect(page.getByRole('tab').filter({ hasText: title }).locator(`svg.lucide-${shape}`).first()).toBeVisible();
  if (width >= 900) {
    const row = page.getByRole('treeitem').filter({ has: page.locator(`a[href="/thread/${MAIN}"]`) });
    await expect(row.locator(`[data-work-state="${state}"]`).first()).toBeVisible();
  }
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
}

for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) test(`thread icons follow work and a reader's finished check (${theme}, ${width})`, async ({ page }, info) => {
  await scenario();
  await state('idle');
  await page.emulateMedia({ colorScheme: theme });
  await page.setViewportSize({ width, height: 900 });
  const faults: string[] = [];
  page.on('pageerror', (error) => faults.push(error.message));
  await signIn(page);
  await page.goto(`/thread/${MAIN}`);
  await expect(page.locator('[data-thread-header]')).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
  await writeFile(info.outputPath('initial.yml'), await page.locator('body').ariaSnapshot());
  await icons(page, 'idle', width);
  await page.getByRole('textbox', { name: 'Message to the agent' }).fill('Check the redirect');
  const sent = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname.endsWith(`/threads/${MAIN}/inputs`));
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  expect((await sent).ok()).toBe(true);
  const queued = (await threadHeld(MAIN)).inputs.find((input) => input.state === 'pending');
  if (queued === undefined) throw new Error('The sent message must be retained');
  const began = new Date().toISOString();
  await emit(MAIN, { items: [userMessage('icon-prompt', 'Check the redirect', { author: ADA, turn: 'icon-turn', inputId: queued.id }), agentMessage('icon-answer', '', { turn: 'icon-turn', status: 'in_progress' })],
    inputs: [{ ...threadInput(queued.id, 'Check the redirect', { author: ADA, position: '1', state: 'delivered' }), turn_id: 'icon-turn' }],
    turns: [turn('icon-turn', { ordinal: 1, state: 'running', inputId: queued.id, startedAt: began })], thread: { state: 'running', display: display('running') } });
  await icons(page, 'running', width);
  await page.screenshot({ path: info.outputPath('running.png') });
  const approval = harnessApproval('icon-review', { threadId: MAIN, itemId: 'icon-request', title: 'Review the redirect', requester: ADA,
    request: { request_kind: 'command', detail: { command: 'check redirect', cwd: '/work', reason: null }, options: ALLOW_DENY } });
  await addApproval(approval);
  await emit(MAIN, { items: [approvalItem('icon-request', approval.id, { turn: 'icon-turn' })], thread: { state: 'waiting_approval', pending_approvals: 1, display: display('waiting_approval') } });
  await icons(page, 'waiting_approval', width);
  await page.screenshot({ path: info.outputPath('waiting.png') });
  const allow = page.getByRole('button', { name: /Allow once.*check redirect/ });
  if (!await allow.isVisible()) await page.getByRole('button', { name: 'Review the redirect', exact: true }).click();
  await allow.click();
  await emit(MAIN, { turns: [turn('icon-turn', { ordinal: 1, state: 'interrupted', inputId: queued.id, startedAt: began, completedAt: new Date().toISOString() })] });
  await state('blocked');
  await icons(page, 'blocked', width);
  await page.screenshot({ path: info.outputPath('blocked.png') });
  await state('failed');
  await icons(page, 'failed', width);
  const waiting = [threadInput('icon-q1', 'Check the first path', { author: ADA, position: '1' }), threadInput('icon-q2', 'Check the second path', { author: ADA, position: '2' })];
  await emit(MAIN, { inputs: waiting });
  await state('queued');
  await icons(page, 'queued', width);
  await emit(MAIN, { inputs: waiting.map((input) => ({ ...input, queue_held: true })) });
  await state('held', { queue: { held: true, by: ADA, at: new Date().toISOString(), held_inputs: 2, revision: 2 } });
  await icons(page, 'held', width);
  await page.screenshot({ path: info.outputPath('held.png') });
  await page.goto('/org/acme');
  await expect(page.locator('[data-org-work]')).toBeVisible();
  await emit(MAIN, { inputs: waiting.map((input) => ({ ...input, state: 'cancelled' as const })), items: [agentMessage('icon-result', 'The redirect is checked', { turn: 'icon-complete' })],
    turns: [turn('icon-complete', { ordinal: 2, state: 'completed', inputId: queued.id, completedAt: new Date().toISOString() })] });
  await state('finished', { queue: { held: false, by: null, at: null, held_inputs: 0, revision: 3 }, display: display('finished', { finished_at: new Date().toISOString() }) });
  const completed = page.locator(`[data-work-thread="${MAIN}"] [data-work-state="finished"]`);
  await expect(completed).toBeVisible();
  await page.screenshot({ path: info.outputPath('finished.png') });
  await page.goto(`/thread/${MAIN}`);
  await expect(page.locator('[data-thread-header]')).toBeVisible();
  await expect(page.locator('[data-thread-header] svg.lucide-message-square-text').first()).toBeVisible();
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await state('running');
  await icons(page, 'running', width);
  const animation = await page.locator('[data-thread-header] svg.lucide-loader-circle').first().evaluate((icon) => getComputedStyle(icon).animationName);
  expect(animation).toBe('none');
  expect(faults).toEqual([]);
});


test('icon facts reuse list reads on cold and warm screens', async ({ page }, info) => {
  await scenario();
  await emit(MAIN, { items: [agentMessage('icon-measure', 'A result for iconmeasure')] });
  await state('idle');
  await signIn(page);
  const rail = () => page.getByRole('navigation', { name: 'Orgs and your own places' });
  let reads: string[] = [];
  page.on('request', (request) => {
    const path = new URL(request.url()).pathname;
    if (request.method() === 'GET' && path.startsWith('/api/v1/')) reads.push(path);
  });
  const figures: Record<string, { total: number; paths: Record<string, number> }> = {};
  const take = async (name: string) => {
    const paths: Record<string, number> = {};
    for (const path of reads) paths[path] = (paths[path] ?? 0) + 1;
    figures[name] = { total: reads.length, paths };
    await writeFile(info.outputPath('requests.json'), JSON.stringify(figures, null, 2));
    expect(reads.filter((path) => path === `/api/v1/nodes/${MAIN}` || path.startsWith(`/api/v1/threads/${MAIN}/`))).toEqual([]);
    reads = [];
  };
  const settled = () => page.waitForLoadState('networkidle');
  await page.goto('/org/acme');
  await expect(page.getByRole('heading', { name: 'Acme', exact: true, level: 1 })).toBeVisible();
  const topic = page.getByRole('treeitem', { name: /^Fix login topic/ });
  await topic.focus(); await page.keyboard.press('ArrowRight');
  await expect(page.getByRole('treeitem').filter({ has: page.locator(`a[href="/thread/${MAIN}"]`) }).locator('[data-work-state="idle"]')).toBeVisible();
  await settled(); await take('tree_cold');
  await rail().getByRole('link', { name: 'Everything', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Everything', exact: true, level: 1 })).toBeVisible();
  await settled(); reads = [];
  await page.goBack();
  await expect(page.getByRole('heading', { name: 'Acme', exact: true, level: 1 })).toBeVisible();
  await settled(); await take('tree_warm');
  await page.goto('/everything?q=Main');
  const threadLink = page.getByRole('treegrid').locator(`a[data-open][href="/thread/${MAIN}"]`);
  await expect(threadLink).toBeVisible();
  await expect(threadLink).toHaveText('Main thread');
  await settled(); await take('everything_cold');
  await rail().getByRole('link', { name: 'Acme', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Acme', exact: true, level: 1 })).toBeVisible();
  await settled(); reads = [];
  await page.goBack();
  await expect(page.getByRole('heading', { name: 'Everything', exact: true, level: 1 })).toBeVisible();
  await settled(); await take('everything_warm');
  await page.goto('/search?q=iconmeasure');
  await expect(page.getByRole('region', { name: 'Results', exact: true })).toContainText('iconmeasure');
  await settled(); await take('search_cold');
  await rail().getByRole('link', { name: 'Everything', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Everything', exact: true, level: 1 })).toBeVisible();
  await settled(); reads = [];
  await page.goBack();
  await expect(page.getByRole('region', { name: 'Results', exact: true })).toContainText('iconmeasure');
  await settled(); await take('search_warm');
  expect(Object.values(await lookedAt()).flat()).toEqual([]);
  await writeFile(info.outputPath('requests.json'), JSON.stringify(figures, null, 2));
});

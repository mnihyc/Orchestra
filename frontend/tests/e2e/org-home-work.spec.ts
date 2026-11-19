/** Named daily work comes first, stays current, and reads in either language/theme at laptop and phone widths. */
import { expect, type Request, test } from '@playwright/test';

import { acmeTree, me, userRef } from '../doubles/fixtures.ts';
import { ALLOW_DENY, harnessApproval } from '../doubles/thread-rows.ts';
import { settleAs } from './inbox-support.ts';
import { ACME, addApproval, changeTree, expectAccessibleInBothThemes, PEOPLE, scenario, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
/** These fixture controls have no external work; fail a stalled local answer within the normal test budget. */
const CONTROL_TIMEOUT_MS = 10_000;
const TITLE = 'Hunt the login bug with two helpers and keep the complete result available for review';

async function control(path: string, body: Record<string, unknown>) {
  const response = await fetch(`${DOUBLE}/__double${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body), signal: AbortSignal.timeout(CONTROL_TIMEOUT_MS) });
  expect(response.ok).toBe(true);
}

function thread(id: string, state: 'idle' | 'blocked' | 'waiting_approval') {
  const held = acmeTree().find((row) => row.id === id)?.thread;
  if (held === null || held === undefined) throw new Error('the scenario names an existing thread');
  return { ...held, state };
}

async function dailyWork() {
  const requester = userRef(me(1, 'ada', 'Ada Lovelace'));
  await addApproval(harnessApproval('home-deploy', { threadId: ACME.repro, itemId: 'home-deploy-item', title: 'Deploy staging', requester,
    request: { request_kind: 'command', detail: { command: 'deploy staging', cwd: '/work', reason: null }, options: ALLOW_DENY } }));
  await addApproval(harnessApproval('home-origin', { threadId: ACME.depsThread, itemId: 'home-origin-item', title: 'Choose the redirect', requester,
    request: { request_kind: 'user_input', detail: { questions: [{ id: 'origin', header: 'Origin', question: 'Which origin?', multiple: false, allow_other: true, options: null }] }, options: [] } }));
  await changeTree([
    { id: ACME.repro, name: 'Deploy staging', thread: { ...thread(ACME.repro, 'waiting_approval'), pending_approvals: 1 } },
    { id: ACME.depsThread, name: 'Choose the redirect', thread: { ...thread(ACME.depsThread, 'waiting_approval'), pending_approvals: 1 } },
    { id: ACME.mainThread, name: TITLE, thread: thread(ACME.mainThread, 'idle') },
    { id: ACME.idleThread, name: 'Upgrade the session store', thread: { ...thread(ACME.idleThread, 'blocked'), blocked_reason: 'binding_invalid', blocked_cause: 'sandbox' } },
  ]);
  for (const username of ['ada', 'vera']) await control('/search/recents', { username, ids: [ACME.mainThread, ACME.repro, ACME.depsThread] });
}

test.beforeEach(async () => { await scenario(); await dailyWork(); });

for (const locale of ['en', 'zh-CN'] as const) {
  test(`daily work leads the org at 1440 and 400 in both themes (${locale})`, async ({ page }) => {
    await control('/first-steps/appearance', { locale });
    const detailReads: string[] = [];
    page.on('request', (request) => {
      const path = new URL(request.url()).pathname;
      if (request.method() === 'GET' && (/\/threads\/[^/]+\/sync$/.test(path) || /\/approvals(?:\/[^/]+)?$/.test(path))) detailReads.push(path);
    });
    await signIn(page); await page.goto('/org/acme');
    const work = page.locator('[data-org-work]');
    const approval = work.locator(`[data-work-thread="${ACME.repro}"]`);
    await expect(approval.getByText('Deploy staging', { exact: true })).toBeVisible();
    await expect(approval.getByText(locale === 'en' ? 'Waiting for your approval' : '等待你批准', { exact: true })).toBeVisible();
    await expect(work.getByText(locale === 'en' ? 'Waiting for your answer' : '等待你回答', { exact: true })).toBeVisible();
    await expect(work.getByText(locale === 'en' ? 'Blocked: no suitable sandbox' : '已阻塞：没有适用的沙箱', { exact: true })).toBeVisible();
    await expect(work.getByText(TITLE, { exact: true })).toHaveCount(1);
    await expect(work.getByText('Deploy staging', { exact: true })).toHaveCount(1);
    await expect(page.getByTestId('first-steps')).toHaveCount(0);
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      const positions = await work.evaluate((element) => {
        const filters = document.querySelector('[data-filter-bar]');
        const rows = Array.from(element.querySelectorAll('[data-work-thread]'));
        return {
          beforeFilters: filters !== null && Boolean(element.compareDocumentPosition(filters) & Node.DOCUMENT_POSITION_FOLLOWING),
          rows: rows.map((row) => {
            const title = row.querySelector<HTMLElement>('[data-thread-name]');
            const icon = row.querySelector('svg');
            if (title === null || icon === null) throw new Error('each row has one title and state icon');
            const top = title.getBoundingClientRect().top;
            const firstLine = Number.parseFloat(getComputedStyle(title).lineHeight);
            const sign = icon.getBoundingClientRect();
            return { fits: title.scrollWidth <= title.clientWidth + 1, offset: Math.abs(sign.top + sign.height / 2 - top - firstLine / 2) };
          }),
        };
      });
      expect(positions.beforeFilters).toBe(true);
      for (const row of positions.rows) { expect(row.fits).toBe(true); expect(row.offset).toBeLessThan(1); }
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
      await expectAccessibleInBothThemes(page, '[data-org-work]');
    }
    expect(detailReads).toEqual([]);
    const recent = work.getByRole('link', { name: new RegExp(TITLE) });
    await recent.focus(); await page.keyboard.press('Enter');
    await expect(page).toHaveURL(new RegExp(`/thread/${ACME.mainThread}$`));
  });
}

test('a viewer is not told that another person’s approval or question awaits them', async ({ page }) => {
  await signIn(page, PEOPLE.vera); await page.goto('/org/acme');
  const work = page.locator('[data-org-work]');
  await expect(work.getByText('Waiting for approval', { exact: true })).toBeVisible();
  await expect(work.getByText('Waiting for an answer', { exact: true })).toBeVisible();
  await expect(work.getByText(/Waiting for your/)).toHaveCount(0);
  await expect(page.getByTestId('first-steps')).toHaveCount(0);
});

test('a decision, rename and removal update the existing home without detail or list refetches', async ({ page }) => {
  const initialReads = new Set<Request>();
  page.on('request', (request) => { if (request.method() === 'GET' && request.url().includes('/api/v1/')) initialReads.add(request); });
  page.on('requestfinished', (request) => initialReads.delete(request));
  page.on('requestfailed', (request) => initialReads.delete(request));
  await signIn(page); await page.goto('/org/acme');
  const work = page.locator('[data-org-work]');
  const row = work.locator(`[data-work-thread="${ACME.repro}"]`);
  await expect(row.getByText('Waiting for your approval', { exact: true })).toBeVisible();
  // Two quiet samples avoid counting the dependent first-load reads as work caused by this act.
  let quiet = 0;
  await expect.poll(() => { quiet = initialReads.size === 0 ? quiet + 1 : 0; return quiet; }, { timeout: CONTROL_TIMEOUT_MS }).toBeGreaterThanOrEqual(2);
  const reads: string[] = [];
  page.on('request', (request) => { if (request.method() === 'GET' && request.url().includes('/api/v1/')) reads.push(request.url()); });
  await settleAs('home-deploy', 'grace');
  await changeTree([{ id: ACME.repro, name: 'Deployment reviewed', thread: thread(ACME.repro, 'idle') }]);
  await expect(row.getByText('Deployment reviewed', { exact: true })).toBeVisible();
  await expect(row.getByText('Ready to continue', { exact: true })).toBeVisible();
  await expect(row.getByText(/Waiting/)).toHaveCount(0);
  await changeTree([{ id: ACME.repro, deleted: true, name: '', thread: null }]);
  await expect(row).toHaveCount(0);
  expect(reads).toEqual([]);
});

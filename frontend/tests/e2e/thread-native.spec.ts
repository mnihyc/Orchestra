/** Real-harness requests and nested streams are shown and decided in the joined thread page, separately from constructed scenes. */
import { readFileSync } from 'node:fs';
import { expect, test } from '@playwright/test';

interface NativeCase {
  kind: 'command' | 'user_input' | 'mcp_elicitation';
  thread: string;
  title: string;
  evidence: string;
  harness: 'codex' | 'claude-code';
  version: string;
  status: 'native';
  decisionButton: string;
  fields?: { label: string; value: string }[];
  choices?: string[];
  resolvedText: string;
}
interface NativeWalk {
  managerState: string;
  viewerState: string;
  deniedState: string;
  cases: NativeCase[];
  nested: { thread: string; title: string; childText: string; evidence: string };
  empty: { thread: string; text: string };
  lostPermission: { thread: string; text: string };
  /** A fresh thread bound to the actual scripted adapter, provisioned by the integration runner. */
  scripted?: { thread: string; evidence: string };
  /** Another fresh scripted-adapter thread whose exact profile has a verified safe side mode. */
  scriptedAside?: { thread: string; evidence: string };
}

// The runner supplies actual pending threads and its authenticated browser
// states. This file neither emits a scene nor manufactures a harness event.
const manifestPath = process.env.ORCH_NATIVE_WALK;
const walk = manifestPath === undefined ? undefined : JSON.parse(readFileSync(manifestPath, 'utf8')) as NativeWalk;

test.describe('joined native harness thread', () => {
  // These real pending fixtures are consumed once; a viewer must inspect them before the decider.
  test.describe.configure({ mode: 'serial' });
  test.skip(walk === undefined || process.env.ORCH_NATIVE_URL === undefined,
    'Native UI proof needs a deployed candidate, real pending requests and integration-runner sign-ins.');
  test('a scripted sub command renders its child message and command inside the lane', async ({ browser }) => {
    if (walk === undefined) return;
    const fixture = walk.scripted;
    expect(fixture, 'The integration manifest must provide a fresh scripted-adapter thread').toBeDefined();
    if (fixture === undefined) throw new Error('Missing NativeWalk.scripted fixture');
    expect(fixture.evidence).toBeTruthy();
    const context = await browser.newContext({ baseURL: process.env.ORCH_NATIVE_URL, storageState: walk.managerState, viewport: { width: 1440, height: 1000 } });
    try {
      const page = await context.newPage();
      const constructed: string[] = [];
      page.on('request', (request) => { if (new URL(request.url()).pathname.startsWith('/__double')) constructed.push(request.url()); });
      await page.goto(`/thread/${fixture.thread}`);
      await page.getByRole('textbox', { name: 'Message to the agent', exact: true }).fill('#!fake\nsub reviewer: say I checked the redirect.; run pwd => 0 /workspace/web\nsay The review is complete.');
      await page.getByRole('group', { name: 'Message controls', exact: true }).getByRole('button', { name: 'Send', exact: true }).click();
      const row = page.getByRole('log', { name: 'Conversation', exact: true }).getByRole('button', { name: /Sub-agent reviewer/ });
      await expect(row).toBeVisible();
      if (await row.getAttribute('aria-expanded') !== 'true') await row.click();
      const lane = page.getByRole('region', { name: 'Sub-agent reviewer', exact: true });
      await expect(lane.getByRole('log')).toContainText('I checked the redirect.');
      await expect(lane.getByRole('button', { name: /pwd/ })).toBeVisible();
      expect(constructed).toEqual([]);
    } finally { await context.close(); }
  });

  test('an actual aside opens under its source turn, sends inline and moves to the same compact panel', async ({ browser }) => {
    if (walk === undefined) return;
    const fixture = walk.scriptedAside;
    expect(fixture, 'The integration manifest must supply a fresh thread with a verified safe scripted side mode').toBeDefined();
    if (fixture === undefined) throw new Error('Missing NativeWalk.scriptedAside fixture');
    expect(fixture.evidence).toBeTruthy();
    const manager = await browser.newContext({ baseURL: process.env.ORCH_NATIVE_URL, storageState: walk.managerState, viewport: { width: 1440, height: 1000 } });
    const viewer = await browser.newContext({ baseURL: process.env.ORCH_NATIVE_URL, storageState: walk.viewerState, viewport: { width: 400, height: 900 } });
    try {
      const page = await manager.newPage(), watcher = await viewer.newPage();
      const constructed: string[] = [], lists: string[] = [];
      page.on('request', (request) => {
        const path = new URL(request.url()).pathname;
        if (path.startsWith('/__double')) constructed.push(path);
        if (request.method() === 'GET' && /^\/api\/v1\/threads\/[^/]+\/side$/.test(path)) lists.push(path);
      });
      await page.goto(`/thread/${fixture.thread}`);
      const mainBox = page.getByRole('textbox', { name: 'Message to the agent', exact: true });
      const controls = page.getByRole('group', { name: 'Message controls', exact: true });
      const main = page.getByRole('log', { name: 'Conversation', exact: true });
      await mainBox.fill('#!fake\nsay The source answer stays in the main thread.');
      await controls.getByRole('button', { name: 'Send', exact: true }).click();
      const source = main.getByText('The source answer stays in the main thread.', { exact: true });
      await expect(source).toBeVisible();
      await expect(controls.getByRole('button', { name: 'Send', exact: true })).toBeVisible();
      await mainBox.fill('#!fake\nsay The later main answer.');
      await controls.getByRole('button', { name: 'Send', exact: true }).click();
      await expect(main.getByText('The later main answer.', { exact: true })).toBeVisible();
      await source.click({ button: 'right' });
      await page.getByRole('menuitem', { name: 'Side thread from here…', exact: true }).click();
      const ask = page.getByRole('dialog', { name: 'Ask on the side', exact: true });
      await ask.getByRole('textbox', { name: 'What to ask' }).fill('#!fake\nsay The first aside answer stays here.');
      await ask.getByRole('textbox').press('Enter');
      await expect(ask).toHaveCount(0);
      const lane = main.getByRole('region', { name: /^Side thread/ });
      await expect(lane.getByRole('log')).toContainText('The first aside answer stays here.');
      await expect(lane).toHaveAttribute('data-conversation-lane', 'side');
      await expect(lane).toHaveAttribute('data-density', 'compact');
      const id = await lane.getAttribute('data-side-lane');
      if (id === null) throw new Error('The aside names its actual thread');
      const [laneBox, laterBox] = await Promise.all([lane.boundingBox(), main.getByText('The later main answer.', { exact: true }).boundingBox()]);
      expect(laneBox).not.toBeNull(); expect(laterBox).not.toBeNull();
      if (laneBox !== null && laterBox !== null) expect(laterBox.y).toBeGreaterThan(laneBox.y);
      await mainBox.fill('A newer main-thread draft');
      const sideBox = lane.getByRole('textbox');
      await sideBox.fill('#!fake\nsay The second aside answer stays here.');
      await sideBox.press('Enter');
      await expect(lane.getByRole('log')).toContainText('The second aside answer stays here.');
      await expect(mainBox).toHaveValue('A newer main-thread draft');
      await expect(main.getByText('The second aside answer stays here.', { exact: true })).toHaveCount(1);
      for (const width of [1440, 400]) {
        await page.setViewportSize({ width, height: 1000 });
        const row = lane.locator('[data-side-composer]');
        const send = row.getByRole('button', { name: 'Send', exact: true });
        await expect(send).toBeVisible();
        const size = await send.boundingBox();
        expect(size?.height).toBeLessThanOrEqual(32);
      }
      await watcher.goto(`/thread/${fixture.thread}`);
      const watched = watcher.locator(`[data-side-slot="${id}"]`);
      await watched.getByRole('button', { expanded: false }).click();
      await expect(watched.getByRole('log')).toContainText('The second aside answer stays here.');
      await expect(watched.getByRole('textbox')).toHaveCount(0);
      await expect(watched.getByRole('button', { name: 'Close', exact: true })).toHaveCount(0);
      await lane.getByRole('button', { name: 'Open in panel', exact: true }).click();
      const panel = page.locator(`[data-side-panel="${id}"]`);
      await expect(lane).toHaveCount(0);
      await expect(panel.getByRole('log')).toContainText('The second aside answer stays here.');
      await expect(panel.getByRole('textbox')).toHaveValue('');
      await expect(mainBox).toHaveValue('A newer main-thread draft');
      await panel.getByRole('button', { name: 'Close', exact: true }).click();
      await expect(main.locator(`[data-side-closed="${id}"]`)).toContainText('2 turns');
      expect(lists).toEqual([]); expect(constructed).toEqual([]);
    } finally { await viewer.close(); await manager.close(); }
  });

  test('every required native request has evidence and a supported form', () => {
    expect(walk).toBeDefined();
    for (const kind of ['command', 'user_input', 'mcp_elicitation']) {
      const row = walk?.cases.find((row) => row.kind === kind);
      expect(row, `${kind} must be induced by the native binary`).toBeDefined();
      expect(row?.status).toBe('native');
      expect(row?.evidence).toBeTruthy();
      expect(row?.version).toBeTruthy();
    }
  });

  test('a viewer reads pending native requests and cannot decide them', async ({ browser }) => {
    if (walk === undefined) return;
    const context = await browser.newContext({ baseURL: process.env.ORCH_NATIVE_URL, storageState: walk.viewerState });
    try {
      const page = await context.newPage();
      for (const row of walk.cases) {
        await page.goto(`/thread/${row.thread}`);
        const card = page.getByRole('log', { name: 'Conversation' }).getByRole('region', { name: row.title, exact: true });
        await expect(card).toBeVisible();
        await expect(card.getByRole('button', { name: row.decisionButton, exact: true })).toBeDisabled();
      }
    } finally { await context.close(); }
  });

  test('a person decides each native request in its real conversation', async ({ browser }) => {
    if (walk === undefined) return;
    const context = await browser.newContext({ baseURL: process.env.ORCH_NATIVE_URL, storageState: walk.managerState });
    const watching = await browser.newContext({ baseURL: process.env.ORCH_NATIVE_URL, storageState: walk.managerState });
    try {
      const page = await context.newPage();
      const observer = await watching.newPage();
      const documents: string[] = [];
      const approvalReads: string[] = [];
      observer.on('request', (request) => {
        if (request.resourceType() === 'document') documents.push(request.url());
        if (request.method() === 'GET' && /^\/api\/v1\/approvals\/[^/]+$/.test(new URL(request.url()).pathname)) approvalReads.push(request.url());
      });
      const broken: string[] = [];
      page.on('pageerror', (error) => broken.push(error.message));
      const remaining = new Map<string, number>();
      for (const row of walk.cases) remaining.set(row.thread, (remaining.get(row.thread) ?? 0) + 1);
      for (const row of walk.cases) {
        await observer.goto(`/thread/${row.thread}`);
        const remoteLog = observer.getByRole('log', { name: 'Conversation', exact: true });
        const remoteCard = remoteLog.getByRole('region', { name: row.title, exact: true });
        await expect(remoteCard).toBeVisible();
        await expect(remoteCard.locator('[data-help-guide], [data-help-article], [data-action="help"]')).toHaveCount(0);
        await page.goto(`/thread/${row.thread}`);
        const log = page.getByRole('log', { name: 'Conversation' });
        const card = log.getByRole('region', { name: row.title, exact: true });
        await expect(card).toBeVisible();
        if (row.kind !== 'command') {
          await expect(card.getByRole('button', { name: row.decisionButton, exact: true })).toBeDisabled();
        }
        for (const choice of row.choices ?? []) await card.getByRole('radio', { name: choice, exact: true }).check();
        for (const field of row.fields ?? []) await card.getByRole('textbox', { name: field.label, exact: true }).fill(field.value);
        const beforeDocuments = documents.length;
        const beforeReads = approvalReads.length;
        await card.getByRole('button', { name: row.decisionButton, exact: true }).click();
        await expect(card).toHaveCount(0);
        await expect(log.getByText(row.resolvedText, { exact: false }).first()).toBeVisible();
        await expect(remoteCard).toHaveCount(0);
        await expect(remoteLog.getByText(row.resolvedText, { exact: false }).first()).toBeVisible();
        const pending = (remaining.get(row.thread) ?? 1) - 1;
        remaining.set(row.thread, pending);
        // A shared fixture can have overlapping prompts: its thread waits until the last one settles.
        if (pending === 0) await expect(observer.locator('[data-thread-header] [data-state="waiting_approval"]')).toHaveCount(0);
        expect(documents).toHaveLength(beforeDocuments);
        expect(approvalReads).toHaveLength(beforeReads);
      }
      expect(broken).toEqual([]);
    } finally { await context.close(); await watching.close(); }
  });

  test('native nested streams, an empty thread and lost access have usable states', async ({ browser }) => {
    if (walk === undefined) return;
    expect(walk.nested.evidence).toBeTruthy();
    const manager = await browser.newContext({ baseURL: process.env.ORCH_NATIVE_URL, storageState: walk.managerState });
    const denied = await browser.newContext({ baseURL: process.env.ORCH_NATIVE_URL, storageState: walk.deniedState });
    try {
      const page = await manager.newPage();
      await page.goto(`/thread/${walk.nested.thread}`);
      await page.getByRole('button', { name: walk.nested.title, exact: false }).click();
      await expect(page.getByText(walk.nested.childText, { exact: false }).first()).toBeVisible();
      await page.goto(`/thread/${walk.empty.thread}`);
      await expect(page.getByText(walk.empty.text, { exact: false })).toBeVisible();
      const other = await denied.newPage();
      await other.goto(`/thread/${walk.lostPermission.thread}`);
      await expect(other.getByText(walk.lostPermission.text, { exact: false })).toBeVisible();
      await expect(other.getByRole('textbox', { name: /message/i })).toHaveCount(0);
    } finally { await manager.close(); await denied.close(); }
  });
});

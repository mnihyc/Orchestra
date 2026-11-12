/** Every shipped route has an HTTP budget; idle time and successful acts never hide automatic list refreshes. */
import { readdirSync } from 'node:fs';
import { resolve, relative } from 'node:path';
import { expect, test, type Page, type Request } from '@playwright/test';
import { ACME, changeTree, configureThread, emit, scenario, signIn } from '../e2e/support.ts';
import { recordedConversation } from '../doubles/recorded-conversation.ts';
import { asAdmin } from '../e2e/admin-mode-support.ts';
import { DOCS } from '../e2e/documents-support.ts';
import { documentTreeFixture, TREE } from '../e2e/document-tree-support.ts';
import { openLayoutRoute, personalReportFixtures, prepareLayoutRoute, resourceFixtures, signInScreenRoute } from '../../e2e/layout-fixtures.ts';
import { layoutRouteName, SCREEN_GATE_ROUTES, SCREEN_PRESENTATIONS, SKILL_ID, type LayoutRoute } from '../../e2e/layout-routes.ts';
import type { AdminMessage } from '../../src/generated/api-types';
import { me, userRef, id, RUNS_ON, TRACE } from '../doubles/fixtures.ts';

/** These are product limits, independent of machine speed. The socket and static assets cost no HTTP API read. */
const FIRST_LOAD = 8;
const IDLE_MS = 60_000;
const DOCUMENT_SUMMARY = 'The document paragraph arrives with its opening sync.';
const THREAD_SUMMARY = 'The thread paragraph arrives with its existing chat opening.';
const MESSAGE: AdminMessage = {
  id: '00000000-0000-4000-8000-000000000321', org: null,
  title: 'A message for everyone', body: 'Read this **important** change.',
  sender: userRef(me(1, 'ada', 'Ada Lovelace')), created_at: '2026-11-08T12:00:00Z',
  recipient_count: 4, delivery_job_id: null,
};
const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
interface RouteCase { template: string; path: string; anonymous?: boolean; admin?: boolean; screen?: LayoutRoute }

const routes: RouteCase[] = [
  { template: '/', path: '/' },
  { template: '/resources', path: '/resources' },
  { template: '/settings', path: '/settings' },
  { template: '/workbench', path: '/workbench' },
  { template: '/storage', path: '/storage' },
  ...['login', 'register', 'recover', 'setup'].map((name) => ({ template: `/${name}`, path: `/${name}`, anonymous: true })),
  ...['reset', 'verify-email', 'invite'].map((name) => ({ template: `/${name}/$token`, path: `/${name}/expired-token`, anonymous: true })),
  { template: '/org/$slug', path: '/org/acme' },
  { template: '/org/$slug/messages', path: '/org/acme/messages' },
  { template: '/messages/$id', path: `/messages/${MESSAGE.id}` },
  { template: '/chat/$chatId', path: `/chat/${id(9, 1)}` },
  { template: '/place-chat/$nodeId', path: `/place-chat/${ACME.org}` },
  { template: '/project/$projectId', path: `/project/${ACME.web}` },
  { template: '/project/$projectId/settings', path: `/project/${ACME.web}/settings` },
  { template: '/topic/$topicId', path: `/topic/${ACME.fixLogin}` },
  { template: '/thread/$threadId', path: `/thread/${ACME.mainThread}` },
  { template: '/doc/$documentId', path: `/doc/${DOCS.plan}` },
  { template: '/documents/$nodeId', path: `/documents/${ACME.web}` },
  { template: '/document-folder/$folderId', path: `/document-folder/${TREE.research}` },
  { template: '/trace/$nodeId', path: `/trace/${ACME.web}` },
  { template: '/inspect/$kind/$id', path: `/inspect/topic/${TRACE.topic}`, admin: true },
  { template: '/u/$username', path: '/u/ada' },
  ...['inbox', 'approvals', 'shared', 'everything', 'search', 'help', 'usage', 'admin'].map((name) => ({ template: `/${name}`, path: `/${name}`, admin: name === 'admin' })),
  { template: '/help/$article', path: '/help/speed-first-output-and-errors' },
  ...['interface', 'tokens', 'usage', 'your-page', 'notifications', 'imports', 'exports', 'security', 'profile'].map((section) => ({ template: `/settings/${section}`, path: `/settings/${section}` })),
  ...['general', 'departments', 'members', 'roles', 'groups', 'defaults', 'service-accounts', 'channels', 'destinations', 'tags', 'audit', 'everything'].map((section) => ({ template: '/org/$slug/settings/$section', path: `/org/acme/settings/${section}` })),
  ...['overview', 'users', 'orgs', 'everything', 'sign-in', 'email', 'hosts', 'runtimes', 'templates', 'mcp', 'capabilities', 'harnesses', 'agents', 'plugins', 'defaults', 'rates', 'settings', 'storage', 'system', 'pki', 'audit'].map((section) => ({ template: '/admin/$section', path: `/admin/${section}`, admin: true })),
  ...Object.entries({ machines: RUNS_ON.laptop, providers: RUNS_ON.openAiKey, agents: RUNS_ON.codex, environments: RUNS_ON.workspace, hosts: RUNS_ON.buildHost, templates: RUNS_ON.defaultTemplate, mcp: id(0x31, 1), mounts: id(0x32, 1) }).flatMap(([kind, resource]) => [
    { template: `/resources/${kind}`, path: `/resources/${kind}` },
    { template: `/resources/${kind}/$id`, path: `/resources/${kind}/${resource}` },
  ]),
  ...['performance', 'requests'].map((view) => ({ template: '/usage', path: `/usage?view=${view}&range=7d` })),
  { template: '/resources/skills', path: '/resources/skills' },
  { template: '/resources/skills/$id', path: `/resources/skills/${SKILL_ID}` },
  { template: '/not-found', path: '/a-page-that-does-not-exist' },
  ...SCREEN_GATE_ROUTES.map((screen) => ({ template: screen.pattern, path: screen.url, admin: true, screen })),
];

function routeTemplates(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const file = resolve(directory, entry.name);
    if (entry.isDirectory()) return routeTemplates(file);
    if (!entry.name.endsWith('.tsx') || entry.name.startsWith('-') || entry.name === '__root.tsx') return [];
    const route = `/${relative(resolve(import.meta.dirname, '../../src/routes'), file).replaceAll('\\', '/').replace(/(?:\/)?index\.tsx$|\.tsx$/g, '')}`.replace(/\/$/, '') || '/';
    // The shared-control gallery is a development-only route, covered in its own project.
    return route.startsWith('/dev/') ? [] : [route];
  });
}

test('a new route cannot escape request-budget coverage', () => {
  const covered = new Set(routes.map((one) => one.template));
  expect(routeTemplates(resolve(import.meta.dirname, '../../src/routes')).filter((route) => !covered.has(route))).toEqual([]);
});

function traffic(page: Page) {
  const requests: string[] = [];
  const listen = (request: Request) => {
    const url = new URL(request.url());
    if (url.pathname.startsWith('/api/v1/') && url.pathname !== '/api/v1/ws') requests.push(`${request.method()} ${url.pathname}${url.search}`);
  };
  page.on('request', listen);
  return { requests, stop: () => page.off('request', listen) };
}

/**
 * How long a page must make no request, in real time, before its first load counts as finished: the render between two reads of a
 * chain takes longer than the half second of the network's idleness when the machine is busy.
 */
const LOAD_QUIET_MS = 3_000;
/**
 * The receipts of looking: a shown thread is marked read after two seconds and a shown document is marked seen after three
 * (`READ_AFTER_MS`, `SEEN_AFTER_MS`). Each is one write, sent once, a while after the page was drawn, so it is not a read of the load and
 * does not count in the eight; it must still come once at most and never in the idle minute that follows, which is why a page that can
 * send one is waited on until its receipt is due.
 */
const RECEIPT_QUIET_MS = 6_000;
const RECEIPT = /^POST \/api\/v1\/(?:threads\/[^/]+\/read|documents\/[^/]+\/seen)$/;
const SENDS_RECEIPTS = new Set(['/thread/$threadId', '/doc/$documentId']);

/**
 * The page has finished loading when no request has begun for a further few seconds of its own time. The network being idle
 * for half a second is not enough: a read that follows a render, or a receipt that is due, would then be counted as traffic of
 * the idle minute that follows, though it belongs to the first load.
 */
async function loaded(page: Page, measured: { requests: string[] }, quietMs: number): Promise<void> {
  await settled(page);
  let seen = measured.requests.length;
  let quiet = Date.now();
  while (Date.now() - quiet < quietMs) {
    await page.waitForTimeout(250);
    if (measured.requests.length !== seen) { seen = measured.requests.length; quiet = Date.now(); await settled(page); }
  }
}

async function settled(page: Page): Promise<void> {
  await page.waitForLoadState('networkidle');
  expect(await page.locator('script[src]').evaluateAll((scripts) => scripts.some((script) => script.getAttribute('src')?.startsWith('/src/') === true)), 'Request budgets require the built application, not the development server').toBe(false);
  await expect(page.locator('body')).not.toContainText('Unexpected Application Error');
}

for (const navigation of ['open', 'closed'] as const) test(`a cold thread with two recorded turns and ${navigation} navigation loads in at most eight requests`, async ({ page }) => {
  await scenario();
  await configureThread(ACME.mainThread, { reset: true });
  await emit(ACME.mainThread, recordedConversation());
  await page.setViewportSize({ width: 1440, height: 1000 });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await expect(page.locator('[data-screen="thread"]')).toBeVisible();
  await expect(page.locator('[data-restore-turn]')).toHaveCount(2);
  if (navigation === 'closed') await page.getByRole('button', { name: 'Left bar', exact: true }).click();
  await settled(page);
  // Reload gives this document fresh caches, keeping only the person's native sign-in and chosen layout.
  const measured = traffic(page);
  try {
    await page.reload();
    await expect(page.locator('[data-restore-turn]')).toHaveCount(2);
    await settled(page);
    const initial = [...measured.requests];
    expect(initial.length, initial.join('\n')).toBeLessThanOrEqual(FIRST_LOAD);
    expect(initial.filter((entry) => entry.includes(`/threads/${ACME.mainThread}/opening?`))).toHaveLength(1);
    expect(initial.filter((entry) => entry.includes('/snapshots/'))).toEqual([]);
    expect(initial.filter((entry) => /\/usage\/(?:limits|health)(?:\?|$)/.test(entry))).toEqual([]);
    expect(initial.filter((entry) => new RegExp(`/threads/${ACME.mainThread}/(?:sync|draft|parameters|limits|outline)(?:\\?|$)`).test(entry))).toEqual([]);
    expect(initial.filter((entry) => entry === `GET /api/v1/nodes/${ACME.mainThread}` || entry === `GET /api/v1/nodes/${ACME.mainThread}/settings`)).toEqual([]);
    if (navigation === 'closed') {
      expect(initial.filter((entry) => entry.startsWith(`GET /api/v1/orgs/${ACME.org}/tree`) || entry.startsWith(`GET /api/v1/orgs/${ACME.org}/chats`) || entry === `GET /api/v1/nodes/${ACME.org}/settings`)).toEqual([]);
    } else await expect(page.getByRole('tree', { name: 'Orgs, projects, topics and threads' })).toBeVisible();
    await page.clock.install(); await page.clock.fastForward(120_000); await settled(page);
    expect(measured.requests).toEqual(initial);
    await test.info().attach('thread-initial-traffic', { body: initial.join('\n'), contentType: 'text/plain' });
  } finally { measured.stop(); }
});

for (const route of routes) for (const mode of route.screen === undefined ? [null] : SCREEN_PRESENTATIONS) test(`${route.screen === undefined ? route.path : layoutRouteName(route.screen)}${mode === null ? '' : ` ${mode.width}px ${mode.theme} ${mode.locale}`}: at most eight initial requests and none in a minute idle`, async ({ page, request }) => {
  await scenario();
  if (route.template === '/messages/$id') await page.route(`**/api/v1/messages/${MESSAGE.id}`, (route) => route.fulfill({ json: MESSAGE }));
  if (route.template === '/document-folder/$folderId') await documentTreeFixture(page);
  if (route.template.startsWith('/resources/skills')) await resourceFixtures(page);
  if (route.template === '/storage' || route.template === '/settings/imports') await personalReportFixtures(page);
  expect((await request.post(`${DOUBLE}/__double/usage/performance`)).ok()).toBe(true);
  expect((await request.post(`${DOUBLE}/__double/request-feeds/fixtures`)).ok()).toBe(true);
  if (route.template === '/topic/$topicId') expect((await request.post(`${DOUBLE}/__double/summaries/set`, { data: { kind: 'topic', id: ACME.fixLogin, text: 'The topic paragraph arrives with its existing opening.', state: 'fresh' } })).ok()).toBe(true);
  if (route.template === '/thread/$threadId' && route.screen === undefined) expect((await request.post(`${DOUBLE}/__double/summaries/set`, { data: { kind: 'thread', id: ACME.mainThread, text: THREAD_SUMMARY, state: 'fresh' } })).ok()).toBe(true);
  if (route.template === '/doc/$documentId') {
    expect((await request.post(`${DOUBLE}/__double/summaries/set`, { data: { kind: 'document', id: DOCS.plan, root: ACME.org, text: DOCUMENT_SUMMARY, state: 'fresh' } })).ok()).toBe(true);
  }
  if (route.screen !== undefined) await prepareLayoutRoute(page, route.screen);
  if (route.anonymous !== true) {
    if (mode !== null) await signInScreenRoute(page, mode);
    else await signIn(page);
    if (route.admin === true && mode === null) {
      await asAdmin(page, true);
    }
    // Every cold route owns its traffic after the setup document is retired.
    await page.goto('about:blank');
  }
  const measured = traffic(page);
  await page.goto(route.path);
  if (route.screen !== undefined) await openLayoutRoute(page, route.screen);
  if (route.template === '/messages/$id') {
    await expect(page.getByRole('heading', { name: MESSAGE.title, exact: true })).toBeVisible();
    await expect(page.getByText('Read this important change.', { exact: true })).toBeVisible();
  }
  await loaded(page, measured, SENDS_RECEIPTS.has(route.template) ? RECEIPT_QUIET_MS : LOAD_QUIET_MS);
  const reads = measured.requests.filter((entry) => !RECEIPT.test(entry));
  expect(reads.length, `Initial traffic on ${route.path}\n${measured.requests.join('\n')}`).toBeLessThanOrEqual(FIRST_LOAD);
  for (const receipt of new Set(measured.requests.filter((entry) => RECEIPT.test(entry)))) expect(measured.requests.filter((entry) => entry === receipt), `Receipt sent more than once: ${receipt}`).toHaveLength(1);
  for (const read of new Set(measured.requests.filter((entry) => entry.startsWith('GET ')))) expect(measured.requests.filter((entry) => entry === read), `Duplicate startup read: ${read}`).toHaveLength(1);
  if (route.template === '/topic/$topicId') {
    await expect(page.locator('[data-summary-target="topic"] [data-summary-text]')).toHaveText('The topic paragraph arrives with its existing opening.');
    expect(measured.requests.filter((entry) => entry.startsWith('POST /api/v1/summaries/batch'))).toEqual([]);
    expect(measured.requests.filter((entry) => entry.startsWith(`GET /api/v1/nodes/${ACME.fixLogin}/chat-opening?`))).toHaveLength(1);
  }
  if (route.template === '/thread/$threadId' && route.screen === undefined) {
    await expect(page.locator('[data-summary-target="thread"] [data-summary-text]')).toHaveText(THREAD_SUMMARY);
    expect(measured.requests.filter((entry) => entry.startsWith('POST /api/v1/summaries/batch'))).toEqual([]);
    expect(measured.requests.filter((entry) => entry.startsWith(`GET /api/v1/nodes/${ACME.mainThread}/chat-opening?`))).toHaveLength(1);
  }
  if (route.template === '/doc/$documentId') {
    await expect(page.locator('[data-summary-target="document"] [data-summary-text]')).toHaveText(DOCUMENT_SUMMARY);
    expect(measured.requests.filter((entry) => entry.startsWith('POST /api/v1/summaries/batch'))).toEqual([]);
    expect(measured.requests.filter((entry) => entry.startsWith(`GET /api/v1/documents/${DOCS.plan}/sync`))).toEqual([
      `GET /api/v1/documents/${DOCS.plan}/sync?summary=true`,
    ]);
  }
  if (route.path === '/' || route.path === '/org/acme') {
    await expect(page.locator('[data-org-work]')).toBeVisible();
    const openings = measured.requests.filter((entry) => entry.startsWith(`GET /api/v1/orgs/${ACME.org}/overview?`));
    expect(openings.filter((entry) => !new URL(entry.slice(4), 'http://fixture.test').searchParams.has('resolve_ids'))).toHaveLength(1);
    expect(openings.filter((entry) => new URL(entry.slice(4), 'http://fixture.test').searchParams.has('resolve_ids')).length).toBeLessThanOrEqual(1);
    expect(measured.requests.filter((entry) => /GET \/api\/v1\/(?:meta$|search\/names|me\/first-steps|me\/authority|orgs$|me\/state$)/.test(entry))).toEqual([]);
    expect(measured.requests.filter((entry) => new RegExp(`GET /api/v1/nodes/${ACME.org}/(?:children|activity|port-openings)(?:\\?|$)`).test(entry))).toEqual([]);
  }
  const initial = measured.requests.length;
  await page.clock.install();
  await page.clock.fastForward(IDLE_MS);
  await settled(page);
  expect(measured.requests.slice(initial), `Idle traffic on ${route.path}`).toEqual([]);
  // Returning focus must not ask every stale projection again either.
  await page.evaluate(() => { window.dispatchEvent(new Event('focus')); window.dispatchEvent(new Event('online')); });
  await settled(page);
  expect(measured.requests.slice(initial), `Focus/reconnect traffic on ${route.path}`).toEqual([]);
  await test.info().attach('api-traffic', { body: measured.requests.join('\n'), contentType: 'text/plain' });
  measured.stop();
});

test('saving the displayed name sends one act and applies its answer without refetching lists', async ({ page }) => {
  await scenario(); await signIn(page); await page.goto('/settings/profile');
  const name = page.getByRole('textbox', { name: 'Display name', exact: true });
  await expect(name).toBeVisible(); await settled(page);
  const measured = traffic(page);
  const saved=page.waitForResponse((response)=>response.request().method()==='PATCH' && new URL(response.url()).pathname==='/api/v1/me');
  await name.fill('Ada Research');await name.press('Enter');await name.blur();
  expect((await saved).ok()).toBe(true);
  await expect(name).toHaveValue('Ada Research'); await settled(page);
  expect(measured.requests).toEqual(['PATCH /api/v1/me']);
  await page.clock.install(); await page.clock.fastForward(IDLE_MS); await settled(page);
  expect(measured.requests).toEqual(['PATCH /api/v1/me']);
  measured.stop();
});

test('another person’s live rename updates the loaded row with no HTTP refresh', async ({ page }) => {
  await scenario(); await signIn(page); await page.goto('/org/acme'); await settled(page);
  const measured = traffic(page);
  await changeTree([{ id: ACME.web, name: 'Web services' }]);
  const changed = page.locator(`[role="treeitem"][data-tree-row="${ACME.web}"]`);
  await expect(changed).toBeVisible();
  await expect(changed).toHaveAccessibleName(/^Web services(?:\s|$)/);
  await settled(page);
  expect(measured.requests).toEqual([]);
  measured.stop();
});

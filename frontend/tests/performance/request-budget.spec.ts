/** Every shipped route has an HTTP budget; idle time and successful acts never hide automatic list refreshes. */
import { readdirSync } from 'node:fs';
import { resolve, relative } from 'node:path';
import { expect, test, type Page, type Request } from '@playwright/test';
import { ACME, changeTree, configureThread, emit, scenario, signIn } from '../e2e/support.ts';
import { agentMessage, turn } from '../doubles/thread-rows.ts';
import { asAdmin } from '../e2e/admin-mode-support.ts';
import { DOCS } from '../e2e/documents-support.ts';
import { documentTreeFixture, TREE } from '../e2e/document-tree-support.ts';
import { resourceFixtures } from '../../e2e/layout-fixtures.ts';
import { SKILL_ID } from '../../e2e/layout-routes.ts';
import { id, RUNS_ON, TRACE } from '../doubles/fixtures.ts';

/** These are product limits, independent of machine speed. The socket and static assets cost no HTTP API read. */
const FIRST_LOAD = 8;
const IDLE_MS = 60_000;
const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
interface RouteCase { template: string; path: string; anonymous?: boolean; admin?: boolean }

const routes: RouteCase[] = [
  { template: '/', path: '/' },
  { template: '/resources', path: '/resources' },
  { template: '/settings', path: '/settings' },
  { template: '/workbench', path: '/workbench' },
  ...['login', 'register', 'recover', 'setup'].map((name) => ({ template: `/${name}`, path: `/${name}`, anonymous: true })),
  ...['reset', 'verify-email', 'invite'].map((name) => ({ template: `/${name}/$token`, path: `/${name}/expired-token`, anonymous: true })),
  { template: '/org/$slug', path: '/org/acme' },
  { template: '/org/$slug/messages', path: '/org/acme/messages' },
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
  ...['interface', 'tokens', 'usage', 'your-page', 'notifications', 'exports', 'security', 'profile'].map((section) => ({ template: `/settings/${section}`, path: `/settings/${section}` })),
  ...['general', 'departments', 'members', 'roles', 'groups', 'policies', 'service-accounts', 'channels', 'destinations', 'tags', 'audit', 'everything'].map((section) => ({ template: '/org/$slug/settings/$section', path: `/org/acme/settings/${section}` })),
  ...['overview', 'users', 'orgs', 'everything', 'sign-in', 'email', 'hosts', 'runtimes', 'templates', 'mcp', 'capabilities', 'harnesses', 'agents', 'plugins', 'defaults', 'rates', 'settings', 'storage', 'system', 'pki', 'audit'].map((section) => ({ template: '/admin/$section', path: `/admin/${section}`, admin: true })),
  ...Object.entries({ machines: RUNS_ON.laptop, providers: RUNS_ON.openAiKey, agents: RUNS_ON.codex, environments: RUNS_ON.workspace, hosts: RUNS_ON.buildHost, templates: RUNS_ON.defaultTemplate, mcp: id(0x31, 1), mounts: id(0x32, 1) }).flatMap(([kind, resource]) => [
    { template: `/resources/${kind}`, path: `/resources/${kind}` },
    { template: `/resources/${kind}/$id`, path: `/resources/${kind}/${resource}` },
  ]),
  ...['performance', 'requests'].map((view) => ({ template: '/usage', path: `/usage?view=${view}&range=7d` })),
  { template: '/resources/skills', path: '/resources/skills' },
  { template: '/resources/skills/$id', path: `/resources/skills/${SKILL_ID}` },
  { template: '/not-found', path: '/a-page-that-does-not-exist' },
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

async function settled(page: Page): Promise<void> {
  await page.waitForLoadState('networkidle');
  expect(await page.locator('script[src]').evaluateAll((scripts) => scripts.some((script) => script.getAttribute('src')?.startsWith('/src/') === true)), 'Request budgets require the built application, not the development server').toBe(false);
  await expect(page.locator('body')).not.toContainText('Unexpected Application Error');
}

for (const navigation of ['open', 'closed'] as const) test(`a cold thread with two recorded turns and ${navigation} navigation loads in at most eight requests`, async ({ page }) => {
  await scenario();
  await configureThread(ACME.mainThread, { reset: true });
  const completed = '2026-10-15T10:00:01Z';
  await emit(ACME.mainThread, {
    turns: [
      { ...turn('recorded-one', { ordinal: 1, state: 'completed', inputId: 'input-one', completedAt: completed }), before_snapshot_id: 'copy-one' },
      { ...turn('recorded-two', { ordinal: 2, state: 'completed', inputId: 'input-two', completedAt: completed }), before_snapshot_id: 'copy-two' },
    ],
    items: [agentMessage('result-one', 'The first change is ready.', { turn: 'recorded-one' }), agentMessage('result-two', 'The second change is ready.', { turn: 'recorded-two' })],
  });
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

for (const route of routes) test(`${route.path}: at most eight initial requests and none in a minute idle`, async ({ page, request }) => {
  await scenario();
  if (route.template === '/document-folder/$folderId') await documentTreeFixture(page);
  if (route.template.startsWith('/resources/skills')) await resourceFixtures(page);
  expect((await request.post(`${DOUBLE}/__double/usage/performance`)).ok()).toBe(true);
  expect((await request.post(`${DOUBLE}/__double/request-feeds/fixtures`)).ok()).toBe(true);
  if (route.anonymous !== true) {
    await signIn(page);
    if (route.admin === true) await asAdmin(page, true);
  }
  const measured = traffic(page);
  await page.goto(route.path);
  await settled(page);
  expect(measured.requests.length, `Initial traffic on ${route.path}\n${measured.requests.join('\n')}`).toBeLessThanOrEqual(FIRST_LOAD);
  for (const read of new Set(measured.requests.filter((entry) => entry.startsWith('GET ')))) expect(measured.requests.filter((entry) => entry === read), `Duplicate startup read: ${read}`).toHaveLength(1);
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
  await expect(page.getByRole('treeitem', { name: /Web services/ })).toBeVisible();
  await settled(page);
  expect(measured.requests).toEqual([]);
  measured.stop();
});

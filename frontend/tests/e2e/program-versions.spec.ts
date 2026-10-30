/** Operators review checked upgrades, an environment downgrade, a project pin and interrupting a running unit. */
import { expect, test, type Page, type TestInfo } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { writeFile } from 'node:fs/promises';
import type { Environment, EnvironmentUpdatePlan, HarnessBuild, Job, ProgramDetail, ResolvedSetting } from '../../src/generated/api-types';
import { SETTINGS_REGISTRY } from '../../src/generated/settings-registry';
import { RUNS_ON } from '../doubles/fixtures';
import { ACME, scenario } from './support';
import { signInForManagement } from './admin-mode-support';
import english from '../../src/i18n/en.json' with { type: 'json' };

// Native scrollbars are part of this screen's interface; headless Chromium normally hides them.
test.use({ launchOptions: { args: ['--disable-gpu'], ignoreDefaultArgs: ['--hide-scrollbars'] } });

const KEY = 'agents.harness_versions';
const NAMED = { harness: { kind: 'codex', name: 'Codex', names: {} } };
/** A valid long version id exercises compact rows without changing the stable checked target. */
const LONG_VERSION = `build-${'a'.repeat(50)}`;
function build(version: string): HarnessBuild {
  const nativeImportOff = version === '0.162.1';
  const fallback = { carry: 'items' as const };
  return { ...fallback, ...NAMED, kind: 'codex', version, os: 'linux', arch: 'x86_64', url: 'https://build.test/program', sha256: 'c'.repeat(64), is_default: version === '0.162.1', source: nativeImportOff ? 'admin' : 'channel', status: nativeImportOff ? 'failed' : 'checked', check: { at: '2026-10-26T00:00:00Z', machine: 'Check host', missing: [], off: nativeImportOff ? ['import_native'] : [], error: null, stage: 'complete', cases: ['startup','command_approval','user_input','elicitation','nested_subagent','native_import','tools.result_form'].map((name)=>({name:nativeImportOff && name === 'native_import' ? 'history.native_import' : name,passed:!nativeImportOff || name !== 'native_import',feature:name === 'native_import' ? 'import_native' : null,error:nativeImportOff && name === 'native_import' ? 'Paginated native archive unsupported' : null,...(nativeImportOff && name === 'native_import' ? {unsupported:'Paginated native archive unsupported · The exported archive depends on its original home'} : {}),elapsed_ms:12})) } };
}

async function fixture(page: Page) {
  // Reopen the page through the session cookie and use the same current bearer as its bootstrap reader.
  const bootstrap = page.waitForResponse((response) => new URL(response.url()).pathname === '/api/v1/me/bootstrap' && response.ok());
  await page.reload();
  const headers = await (await bootstrap).request().allHeaders();
  const authorization = headers.authorization;
  if (authorization === undefined) throw new Error('The signed-in fixture has no current bearer');
  const envReply = await page.request.get(`/api/v1/environments/${RUNS_ON.sandbox}`, { headers: { Authorization: authorization } });
  expect(envReply.ok(), `Environment fixture read answered ${envReply.status()}`).toBeTruthy();
  const environment = await envReply.json() as Environment;
  const planReply = await page.request.post(`/api/v1/environments/${environment.id}/updates/preflight`, { headers: { Authorization: authorization, 'Idempotency-Key': randomUUID() }, data: {} });
  expect(planReply.ok(), `Environment fixture plan answered ${planReply.status()}`).toBeTruthy();
  const original = await planReply.json() as EnvironmentUpdatePlan;
  const programReply = await page.request.get('/api/v1/admin/programs/codex', { headers: { Authorization: authorization } });
  expect(programReply.ok(), `Program fixture read answered ${programReply.status()}`).toBeTruthy();
  const originalProgram = await programReply.json() as ProgramDetail;
  const builds = [build('0.162.1'), build('0.164.0'), {...build(LONG_VERSION),source:'admin' as const,status:'untested' as const,check:undefined,is_default:false}];
  const longEnvironment={...environment,id:randomUUID(),name:'Long pinned workspace',updates:[],config:{[KEY]:{codex:{use:'exact',version:LONG_VERSION}}}};
  const values = new Map<string, unknown>([['system', {}], ['environment', {}], ['project', {}]]);
  let current = '0.162.1';
  const writes: { scope: string; value: unknown }[] = [];
  const starts: { options: unknown; notify: string | null; batch: boolean }[] = [];
  const requests: string[] = [];
  const jobs = new Map<string, Job>();
  const setting = (scope: string): ResolvedSetting => ({ key: KEY, value: scope === 'system' ? values.get(scope) : { ...values.get('system') as object, ...values.get(scope) as object }, own: { value: values.get(scope), revision: writes.length + 1, locked: false }, writable: true, locked_by: null, source: { scope: scope === 'system' ? 'system' : scope === 'environment' ? 'environment' : 'project', node: null }, parts: [] });
  const target = () => {
    const own = values.get('environment') as { codex?: { use?: string; version?: string } };
    const system = values.get('system') as { codex?: { use?: string; version?: string } };
    const choice = own.codex ?? system.codex;
    return choice?.use === 'exact' ? choice.version ?? '0.162.1' : choice?.use === 'newest_checked' || choice?.use === 'newest' ? '0.164.0' : '0.162.1';
  };
  const updated = (): Environment => ({ ...environment, status: 'running', job: null, revision: writes.length + 1, config: { ...environment.config as object, [KEY]: values.get('environment') }, updates: target() === current ? [] : [{ ...NAMED, what: 'harness', name: 'codex', from: current, to: target() }] });
  const plan = (): EnvironmentUpdatePlan => ({ ...original, environment: updated(), preflight_id: randomUUID(), blockers: [], threads: [{ thread: null, turn_running: true }], updates: [{ ...NAMED, what: 'harness', name: 'codex', from: current, to: target() }], target_programs: [{ ...NAMED, kind: 'codex', version: target(), build: builds.find((b) => b.version === target()) ?? null }], notify: { people: [], hidden: 1 } });
  // This fixture owns the environment snapshot; its synthetic feed stays followed and quiet.
  const feed = `env:${environment.id}`;
  await page.routeWebSocket(/\/api\/v1\/ws(?:\?|$)/, (socket) => {
    const server = socket.connectToServer(), answered = new Set<number>();
    socket.onMessage((raw) => {
      const frame = JSON.parse(String(raw)) as { op: string; id?: number; feed?: string };
      if (frame.op === 'sub' && frame.feed === feed && frame.id !== undefined) {
        answered.add(frame.id);
        socket.send(JSON.stringify({ op: 'subscribed', id: frame.id, feed, seq: null, mode: 'ephemeral' }));
      } else if (frame.op === 'unsub' && frame.id !== undefined && answered.delete(frame.id)) {
        socket.send(JSON.stringify({ op: 'closed', id: frame.id, feed, reason: 'unsubscribed' }));
      } else server.send(raw);
    });
  });
  await page.route('**/api/v1/**', async (route) => {
    const request = route.request(), url = new URL(request.url()), path = url.pathname.replace('/api/v1', ''), method = request.method();
    requests.push(`${method} ${path}`);
    if (method === 'GET' && path === '/settings/registry') return route.fulfill({ json: SETTINGS_REGISTRY });
    const summary = { ...originalProgram.summary, platforms: [{ os: 'linux', arch: 'x86_64', builds: builds.length, shipped: '0.162.1', newest: '0.164.0', newest_checked: '0.164.0', default_version: '0.162.1' }] };
    if (method === 'GET' && path === '/admin/programs') return route.fulfill({ json: { items: [summary], next_cursor: null } });
    if (method === 'GET' && path === '/admin/programs/codex') {
      const system = values.get('system') as { codex?: { use?: string; version?: string } };
      const choice = system.codex;
      const selected = choice?.use === 'exact' ? choice.version : choice?.use === 'newest' || choice?.use === 'newest_checked' ? '0.164.0' : '0.162.1';
      return route.fulfill({ json: { ...originalProgram, summary, resolved_builds: builds.filter((b) => b.version === selected), versions: values.get('system') } });
    }
    if (method === 'GET' && path === '/admin/programs/codex/builds') return route.fulfill({ json: { items: builds, next_cursor: null } });
    if (method === 'GET' && path.startsWith('/admin/programs/codex/builds/')) return route.fulfill({ json: builds.filter((b) => b.version === decodeURIComponent(path.slice('/admin/programs/codex/builds/'.length))) });
    if (method === 'GET' && (path === '/harnesses/builds' || path === '/admin/harnesses')) return route.fulfill({ json: builds });
    if (method === 'GET' && path === '/admin/harnesses/usage') return route.fulfill({ json: [{ ...NAMED, kind: 'codex', version: current, environments: [updated()], providers: [], update_environments: target() === current ? [] : [environment.id], update_providers: [] },{...NAMED,kind:'codex',version:LONG_VERSION,environments:[longEnvironment],providers:[],update_environments:[],update_providers:[]}] });
    if (method === 'GET' && path === `/environments/${longEnvironment.id}`) return route.fulfill({json:longEnvironment});
    if (path === '/admin/settings' && method === 'GET') return route.fulfill({ json: [setting('system')] });
    if (path.endsWith(`/settings/${KEY}`) && method === 'PUT') {
      const scope = path.startsWith('/admin/') ? 'system' : 'project';
      const body = request.postDataJSON() as { value: unknown };
      values.set(scope, body.value); writes.push({ scope, value: body.value });
      return route.fulfill({ json: setting(scope) });
    }
    if (path === `/nodes/${ACME.web}/settings` && method === 'GET') return route.fulfill({ json: [setting('project')] });
    if (path === `/environments/${environment.id}/settings` && method === 'GET') return route.fulfill({ json: [setting('environment')] });
    if (path === `/environments/${environment.id}/harnesses` && method === 'GET') return route.fulfill({ json: [{ ...NAMED, kind: 'codex', version: current, build: builds.find((build) => build.version === current) }] });
    if (path === `/environments/${environment.id}/sync` && method === 'GET') return route.fulfill({ json: { environment: updated(), seq: updated().revision, complete: true, since: null, jobs: [...jobs.values()] } });
    if (path === `/environments/${environment.id}` && method === 'GET') return route.fulfill({ json: updated() });
    if (path === `/environments/${environment.id}` && method === 'PATCH') {
      const body = request.postDataJSON() as { config?: Record<string, unknown> };
      if (body.config !== undefined && KEY in body.config) { values.set('environment', body.config[KEY]); writes.push({ scope: 'environment', value: body.config[KEY] }); }
      return route.fulfill({ json: { ...updated(), revision: writes.length + 1 } });
    }
    if (path === `/environments/${environment.id}/updates/preflight` && method === 'POST') return route.fulfill({ json: plan() });
    if (path === '/environments/updates/preflight' && method === 'POST') {
      const one = plan();
      return route.fulfill({ json: { preflight_id: one.preflight_id, expires_at: one.expires_at, executable: 1, options: one.options, estimate: one.estimate, plans: [one], skipped: [], notify: { people: [], hidden: 1 } } });
    }
    if (method === 'POST' && (path === '/environments/updates' || path === `/environments/${environment.id}/updates`)) {
      const body = request.postDataJSON() as { options: unknown };
      starts.push({ options: body.options, notify: url.searchParams.get('notify'), batch: path === '/environments/updates' });
      const job: Job = { id: randomUUID(), kind: 'environment_recreate', environment_id: environment.id, node_id: null, provider_id: null, batch_id: null, state: 'queued', result: null, error: null, log: null, created_by: environment.owner, created_at: new Date().toISOString(), started_at: null, finished_at: null,
        progress: { phase: 'notify', percent: 0, phases: [], files_done: 0, files_total: null, bytes_done: 0, bytes_total: null, rate_bps: null, eta_seconds: null, message: null } };
      jobs.set(job.id, job); current = target();
      return route.fulfill({ status: 202, json: path === '/environments/updates' ? { batch_id: randomUUID(), jobs: [{ environment: updated(), job }] } : job });
    }
    if (method === 'GET' && path.startsWith('/jobs/') && jobs.has(path.slice('/jobs/'.length))) return route.fulfill({ json: jobs.get(path.slice('/jobs/'.length)) });
    return route.continue();
  });
  return { environment, writes, starts, requests };
}

async function exact(page: Page, version: string) {
  const row = page.locator(`[data-setting="${KEY}"]:visible`).first();
  await expect(row).toBeFocused();
  await row.locator('[data-setting-editor-trigger]').click();
  await row.getByRole('combobox', { name: 'Codex', exact: true }).click();
  await page.getByRole('option', { name: 'Exact version', exact: true }).click();
  const input = row.getByRole('combobox', { name: 'Version', exact: true });
  await input.fill(version);
  await row.getByRole('button', { name: 'Use version', exact: true }).click();
  await expect(row.getByText(english.settings.compact.saved, { exact: true })).toBeVisible();
}

async function capture(page: Page, info: TestInfo, name: string) {
  const path = info.outputPath(`${name}.png`);
  await page.screenshot({ path });
  await info.attach(name, { path, contentType: 'image/png' });
}

async function expectDialogFits(page: Page, width: number) {
  const bounds = await page.getByRole('dialog').evaluate((element) => {
    const rect = element.getBoundingClientRect();
    return { left: rect.left, right: rect.right };
  });
  expect(bounds.left).toBeGreaterThanOrEqual(0);
  expect(bounds.right).toBeLessThanOrEqual(width);
}

for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) test(`operator version workflows (${theme}/${width})`, async ({ page }, info) => {
  await scenario(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
  await signInForManagement(page);
  const state = await fixture(page);
  await page.goto('/admin/harnesses');
  const directory = page.getByRole('table', { name: english.programList.title, exact: true });
  await expect(directory).toBeVisible();
  await expect(directory).toContainText('0.162.1');
  await expect(directory).toContainText('0.164.0');
  const directoryBounds = await directory.evaluate((element) => ({ left: element.getBoundingClientRect().left, right: element.getBoundingClientRect().right }));
  expect(directoryBounds.left).toBeGreaterThanOrEqual(0);
  expect(directoryBounds.right).toBeLessThanOrEqual(width);
  await capture(page, info, `programs-${width}-${theme}`);
  await directory.getByRole('link', { name: 'Codex', exact: true }).click();
  await page.getByRole('tab', { name: english.programList.capabilities, exact: true }).click();
  for (const group of ['Conversation', 'Tools', 'Sessions', 'Usage']) await expect(page.getByRole('heading', { name: group, exact: true })).toBeVisible();
  await expect(page.locator('[data-program-capability="steer"]')).toContainText('Steering');
  const capabilityOverflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(capabilityOverflow).toBeLessThanOrEqual(1);
  await capture(page, info, `capabilities-${width}-${theme}`);
  await page.getByRole('tab', { name: english.admin.harnesses.buildsTitle, exact: true }).click();
  const heading = page.locator('#admin-builds');
  await expect(heading.getByRole('button', { name: english.help.hint.label.replace('{{term}}', english.programVersions.title), exact: true })).toBeVisible();
  await expect(page.getByRole('table', { name: english.admin.harnesses.buildsTitle, exact: true })).toBeVisible();
  await capture(page, info, `builds-${width}-${theme}`);
  await page.getByRole('tab', { name: english.programList.settings, exact: true }).click();
  const program = page.locator('[data-harness-version="codex"]');
  const longCount=program.getByRole('button',{name:`${LONG_VERSION} · 1 environment · 0 runtimes`,exact:true});
  await expect(longCount).toBeVisible();
  const edges=await longCount.evaluate((element)=>({right:element.getBoundingClientRect().right,parent:element.closest('[data-harness-version]')!.getBoundingClientRect().right}));
  // CSS layout dimensions round to whole pixels at the screen edge.
  expect(edges.right).toBeLessThanOrEqual(edges.parent+1);
  await program.getByRole('combobox', { name: 'Codex', exact: true }).click();
  await page.getByRole('option', { name: 'Newest checked', exact: true }).click();
  await expect.poll(() => state.writes.some((write) => write.scope === 'system')).toBeTruthy();
  await program.getByRole('button', { name: 'Checked', exact: true }).click();
  const results=page.getByRole('dialog',{name:english.admin.harnesses.checkResults,exact:true});
  await expect(results).toContainText(`${english.programVersions.checks.toolResults} · Checked`);
  await expect(results).toContainText(`${english.programVersions.checks.nativeImport} · Checked`);
  await expect(results).not.toContainText('native_import');
  await expect(results).not.toContainText('tools.result_form');
  await capture(page, info, `check-results-${width}-${theme}`);
  await page.keyboard.press('Escape');
  await capture(page, info, `policy-${width}-${theme}`);
  expect(state.requests.filter((request) => request === 'GET /admin/harnesses' || request === 'GET /harnesses/builds')).toEqual([]);
  await program.getByRole('button', { name: 'Update 1 environment…', exact: true }).click();
  await expect(page.getByText('Codex 0.162.1 → 0.164.0 · newer', { exact: true })).toBeVisible();
  await expectDialogFits(page, width);
  await page.getByRole('button', { name: 'Update 1 environment', exact: true }).click();
  await expect.poll(() => state.starts.length).toBe(1);
  expect(state.starts[0]?.options).toMatchObject({ active_turn: 'wait' });
  await page.getByRole('button', { name: 'Hide', exact: true }).click();

  await page.goto(`/environments/${state.environment.id}`.replace('/environments/', '/resources/environments/') + '?tab=settings&setting=agents.harness_versions');
  await exact(page, '0.162.1');
  await expect.poll(() => state.writes.some((write) => write.scope === 'environment')).toBeTruthy();
  await page.goto(`/resources/environments/${state.environment.id}`);
  await expect(page.getByText('Codex 0.164.0 → 0.162.1 · older', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Update…', exact: true }).first().click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText(english.programVersions.offInBuild.replace('{{feature}}', english.programVersions.disabledFeatures.nativeImport));
  await expect(dialog).toContainText(english.programVersions.fallback.neutralCarry);
  await expect(dialog).not.toContainText('Paginated native archive unsupported');
  await expect(dialog.locator('[data-plan-list="if-fails"]')).toContainText(english.change.lists.ifFails);
  await expect(dialog.locator('[data-plan-list="if-fails"]')).toContainText(english.change.env.plan.rollback.unit.replace('{{name}}', state.environment.name));
  await expect(dialog).toContainText(english.change.env.lock_one);
  await expect(dialog.locator('[data-plan-time]')).toContainText('min');
  await dialog.getByRole('button', { name: english.programVersions.offInBuild.replace('{{feature}}', english.programVersions.disabledFeatures.nativeImport), exact: true }).click();
  const historySupport = page.getByRole('dialog', { name: english.admin.harnesses.checkResults, exact: true });
  await expect(historySupport).toContainText(`${english.programVersions.checks.nativeImport} · Unavailable`);
  await expect(historySupport.locator('[data-build-case="history.native_import"] .text-text-muted')).toContainText('Paginated native archive unsupported');
  await expect(historySupport).not.toContainText('import_native');
  await expect(historySupport).not.toContainText('history.native_import');
  await capture(page, info, `history-support-${width}-${theme}`);
  await page.keyboard.press('Escape');
  await expect(historySupport).toBeHidden();
  await expectDialogFits(page, width);
  await dialog.getByRole('radio', { name: english.changeCompact.turn.interrupt, exact: true }).click();
  await expect(dialog.getByText('Stops 1 turn', { exact: true })).toBeVisible();
  await dialog.getByRole('button',{name:'Notify 1 person',exact:true}).click();
  await expect(page.getByText(english.ui.avatarStack.andMore_one.replace('{{count}}','1'),{exact:true})).toBeVisible();
  await page.keyboard.press('Escape');
  await capture(page, info, `interrupt-${width}-${theme}`);
  await dialog.locator('[data-plan-time]').scrollIntoViewIfNeeded();
  await expect(dialog.locator('[data-plan-list="if-fails"]')).toBeVisible();
  await expect(dialog.locator('[data-plan-time]')).toBeVisible();
  await capture(page, info, `recovery-${width}-${theme}`);
  await dialog.locator('[data-confirm]').click();
  await expect.poll(() => state.starts.length).toBe(2);
  expect(state.starts[1]?.options).toMatchObject({ active_turn: 'interrupt' });
  await page.getByRole('button', { name: 'Hide', exact: true }).click();

  await page.goto(`/project/${ACME.web}/settings?setting=agents.harness_versions`);
  await exact(page, '0.162.1');
  await expect.poll(() => state.writes.some((write) => write.scope === 'project')).toBeTruthy();
  await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(overflow).toBeLessThanOrEqual(1);
  await expect(page.locator(`[data-setting="${KEY}"]:visible`).first()).toContainText('Codex: Exact version 0.162.1');
  await capture(page, info, `project-pin-${width}-${theme}`);
  const requestsPath = info.outputPath('version-requests.json');
  await writeFile(requestsPath, JSON.stringify(state.requests, null, 2));
  await info.attach('version-requests', { path: requestsPath, contentType: 'application/json' });
});

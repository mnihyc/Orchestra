/** Host data consent, named waits and permanent loss use the same shipped plan at both widths. */
import { expect, test, type Page, type WebSocketRoute } from '@playwright/test';
import type { Host, HostDataFate, HostDeletePlan, HostDeletePreflightBody, Job, KeptRecord, LossPlan } from '../../src/generated/api-types';
import { caption } from '../../e2e/capture/open';
import { screenReady } from '../../e2e/layout-rules-helpers';
import { replySent, scenario, signIn } from './support';

const JOB = '019a0000-0000-7000-8000-000000000001';
const MOUNT = '019a0000-0000-7000-8000-000000000002';
const ENVIRONMENT = '019a0000-0000-7000-8000-000000000003';
const THREAD = '019a0000-0000-7000-8000-000000000004';
const PLAN = '019a0000-0000-7000-8000-000000000005';
const UNTIL = '2026-11-03T12:00:00.000Z';
const OBSERVED = '2026-10-27T12:00:00.000Z';
const FINISHED = '2026-10-27T12:00:05.000Z';

async function setup(page: Page, width: number, theme: 'light' | 'dark', locale: 'en' | 'zh-CN') {
  await scenario();
  await page.clock.setFixedTime(FINISHED);
  await page.setViewportSize({ width, height: 900 });
  await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
  const signedIn = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname.endsWith('/auth/login')).then(replySent);
  await signIn(page);
  const auth = await signedIn;
  if (typeof auth !== 'object' || auth === null || !('access_token' in auth) || typeof auth.access_token !== 'string') throw new Error('The disposable sign-in has no access token.');
  const headers = { Authorization: `Bearer ${auth.access_token}` };
  expect((await page.request.patch('/api/v1/me', { headers, data: { theme, locale } })).ok()).toBe(true);
  const response = await page.request.get('/api/v1/hosts', { headers });
  expect(response.ok()).toBe(true);
  const hosts = await response.json() as Host[];
  const host = hosts.find((row) => row.access.permissions.includes('host.manage'));
  if (host === undefined) throw new Error('The disposable person needs a managed host.');
  return host;
}

function object(host: Host): KeptRecord {
  return { id: 'kept-volume', accounting_key: 'kept-volume', kind: 'volume', name: 'Leftover volume', resource: { kind: 'host', id: host.id }, place: null,
    host: { id: host.id, name: host.name, address: '' }, machine: null, owner: null, bytes: 65536, included_in: null, why: 'unknown', since: null, until: null,
    facts: null, observed_at: OBSERVED, availability: 'ready', can_clean: true, blocker: null, operator_command: null };
}

function job(host: Host): Job {
  return { id: JOB, kind: 'host_delete', node_id: null, environment_id: null, provider_id: null, batch_id: null, state: 'running', result: null, error: null, log: null,
    created_by: null, created_at: OBSERVED, started_at: OBSERVED, finished_at: null,
    progress: { phase: 'cascades', phases: [{ name: 'cascades', state: 'running', weight: 80 }, { name: 'mounts', state: 'pending', weight: 15 }, { name: 'delete', state: 'pending', weight: 5 }],
      percent: 0, files_done: 0, files_total: null, bytes_done: 0, bytes_total: null, rate_bps: null, eta_seconds: null, message: host.name,
      activity: { waiting_for: { type: 'host_environments', count: 1 }, updated_at: OBSERVED, stall_after_seconds: 0, directories_done: null, stopping: false } } };
}

async function live(page: Page, host: Host, offline: boolean) {
  const currentHost = { ...host, machine: { ...host.machine, online: !offline } };
  let current = job(host);
  const publish = new Set<() => void>();
  await page.routeWebSocket(/\/api\/v1\/ws(?:\?|$)/u, (ws: WebSocketRoute) => {
    const server = ws.connectToServer();
    const subscriptions = new Map<number, string>();
    let closed = false;
    const send = () => {
      if (closed) return;
      for (const [id, feed] of subscriptions) ws.send(JSON.stringify({ op: 'event', id, feed, data: feed.startsWith('job:') ? { job: current } : { row: currentHost } }));
    };
    publish.add(send);
    ws.onMessage((raw) => {
      const value = JSON.parse(String(raw)) as { op: string; id?: number; feed?: string };
      if (value.op === 'sub' && value.id !== undefined && (value.feed === `job:${JOB}` || value.feed === `resource:host:${host.id}`)) {
        subscriptions.set(value.id, value.feed);
        ws.send(JSON.stringify({ op: 'subscribed', id: value.id, feed: value.feed, seq: null, mode: 'ephemeral' }));
        send();
      } else if (value.op === 'unsub' && value.id !== undefined && subscriptions.has(value.id)) {
        const feed = subscriptions.get(value.id); subscriptions.delete(value.id);
        ws.send(JSON.stringify({ op: 'closed', id: value.id, feed, reason: 'unsubscribed' }));
      } else server.send(raw);
    });
    ws.onClose((code, reason) => { closed = true; publish.delete(send); void server.close({ ...(code === undefined ? {} : { code }), ...(reason === undefined ? {} : { reason }) }); });
  });
  await page.route(new RegExp(`/api/v1/hosts/${host.id}(?:\\?|$)`, 'u'), (route) => route.fulfill({ status: 200, json: currentHost }));
  return { read: () => current, send: (next: Job) => { current = next; for (const send of publish) send(); } };
}

function plan(host: Host, request: HostDeletePreflightBody): HostDeletePlan {
  const fate = (id: string, fallback: HostDataFate) => request.data.find((row) => row.id === id)?.fate ?? fallback;
  return { job: null, notification: { people: 2, default_on: true }, admin_grants: [], preflight_id: PLAN, expires_at: UNTIL, name: host.name, online: true,
    rows: [{ kind: 'environments', count: 1, blockers: [] }, { kind: 'mount_locations', count: 1, blockers: [] }, { kind: 'mcp_servers', count: 1, blockers: [] }, { kind: 'provider_runtimes', count: 0, blockers: [] }],
    recovery: [], others: 1, volumes: [], kept_until: UNTIL, executable: true, objects: [], erase: request.erase ?? true, erase_blockers: [], inventory_measurement: null,
    data: [{ id: 'mount-data', resource: { id: MOUNT, name: 'shared-data' }, label: null, kind: 'mount', bytes: 8192, until: null, selected: fate('mount-data', request.mounts === 'delete_data' ? 'delete' : 'leave'), choices: ['leave', 'move', 'delete'], host: null, copy: null, blockers: [] },
      { id: 'kept-files', resource: { id: ENVIRONMENT, name: 'Kept workspace' }, label: null, kind: 'environment', bytes: null, until: UNTIL, selected: fate('kept-files', 'wait'), choices: ['wait', 'purge'], host: null, copy: null, blockers: [] }] };
}

for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`host data, waits and leftovers ${width}px ${theme} ${locale}`, async ({ page }, info) => {
    const host = await setup(page, width, theme, locale);
    const source = await live(page, host, false);
    let accepted = 0;
    await page.route(new RegExp(`/api/v1/hosts/${host.id}/delete(?:/preflight)?$`, 'u'), async (route) => {
      const request = route.request();
      if (new URL(request.url()).pathname.endsWith('/preflight')) await route.fulfill({ status: 200, json: plan(host, request.postDataJSON() as HostDeletePreflightBody) });
      else {
        expect(request.postDataJSON()).toEqual({ preflight_id: PLAN, confirm: host.name, notify: false, volumes: [], data_confirmations: [{ id: 'mount-data', confirm: 'shared-data' }] });
        accepted += 1; await route.fulfill({ status: 202, json: source.read() });
      }
    });
    await page.goto(`/resources/hosts/${host.id}`); await screenReady(page, locale, theme);
    await page.getByRole('button', { name: caption('@resources.act.menuOf', locale).replace('{{name}}', host.name), exact: true }).click();
    await page.getByRole('menuitem', { name: caption('@resources.act.delete', locale), exact: true }).click();
    const title = caption('@hosts.remove.title', locale).replace('{{name}}', host.name);
    const dialog = page.getByRole('dialog', { name: title, exact: true });
    const mount = dialog.getByRole('combobox', { name: caption('@hostCustody.fateFor', locale).replace('{{name}}', 'shared-data'), exact: true });
    await expect(mount).toHaveText(caption('@hostCustody.fates.leave', locale));
    await expect(dialog.getByRole('checkbox', { name: caption('@hostCustody.erase', locale), exact: true })).toBeChecked();
    await expect(dialog.getByRole('combobox', { name: caption('@hostCascade.mcp_servers', locale), exact: true })).toHaveCount(0);
    await expect(dialog.getByRole('combobox', { name: caption('@hostCascade.environments', locale), exact: true })).toHaveCount(0);
    await expect(dialog.getByRole('combobox', { name: caption('@hostCascade.host', locale), exact: true })).toHaveCount(0);
    const notify = dialog.getByRole('checkbox', { name: locale === 'en' ? /^Notify /u : /^通知/u });
    await expect(notify).toHaveAccessibleName(/2/u); await notify.uncheck();
    await mount.click(); await page.getByRole('option', { name: caption('@hostCustody.fates.delete', locale), exact: true }).click();
    const typed = dialog.getByRole('textbox', { name: caption('@hostCascade.typeMount', locale).replace('{{name}}', 'shared-data'), exact: true });
    await dialog.getByRole('textbox', { name: caption('@hosts.remove.type', locale).replace('{{name}}', host.name), exact: true }).fill(host.name);
    const submit = dialog.getByRole('button', { name: caption('@hosts.remove.confirm', locale).replace('{{name}}', host.name), exact: true });
    await typed.fill('Shared-data'); await expect(submit).toBeDisabled(); expect(accepted).toBe(0);
    await typed.fill('shared-data'); await expect(submit).toBeEnabled();
    expect(await dialog.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath('host-data-plan.png'), fullPage: true });
    await submit.click();
    await expect(dialog.locator('[data-job-line]')).toContainText(locale === 'en' ? 'environment of others' : '他人的');
    source.send({ ...source.read(), progress: { ...source.read().progress, activity: { ...source.read().progress.activity!, waiting_for: { type: 'retained_data', until: UNTIL } } } });
    await expect(dialog.locator('[data-job-line]')).toContainText(locale === 'en' ? 'Waiting for kept data until' : '等待保留数据到期');
    await page.screenshot({ path: info.outputPath('host-data-wait.png'), fullPage: true });
    source.send({ ...source.read(), state: 'succeeded', finished_at: FINISHED, progress: { ...source.read().progress, phase: 'delete', phases: source.read().progress.phases.map((phase)=>({...phase,state:'done' as const})), percent: 100 }, result: { host_id: host.id, deleted_at: FINISHED, left_confirmed: true, left: [object(host)], left_data: [], uninstall_command: null } });
    const leftovers = dialog.getByRole('table', { name: caption('@hostCustody.left', locale), exact: true });
    await expect(leftovers.getByText('Leftover volume', { exact: true })).toBeVisible();
    await expect(dialog.getByText(caption('@machineInstallation.uninstallUnknown', locale), { exact: true })).toBeVisible();
    await expect(dialog.locator('pre')).toHaveCount(0); expect(accepted).toBe(1);
    await expect(dialog.locator('[data-phase-state="running"]')).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath('host-data-result.png'), fullPage: true });
  });

  test(`permanent loss plan and result ${width}px ${theme} ${locale}`, async ({ page }, info) => {
    const host = await setup(page, width, theme, locale); await live(page, host, true);
    const loss: LossPlan = { target: { type: 'host', id: host.id }, name: host.name, preflight_id: PLAN, expires_at: UNTIL, admin_grants: [], notification: { people: 0, default_on: true },
      environments: [{ environment: { id: ENVIRONMENT, name: 'Shared workspace' }, threads: [{ id: THREAD, kind: 'thread', name: 'Visible work', redacted: false }], threads_hidden: 2, kept_bytes: null }],
      mounts: [], providers: [], jobs: 1, kept_bytes: null, provider_threads: [], provider_threads_hidden: 0 };
    let declarations = 0;
    await page.route(/\/api\/v1\/loss(?:\/preflight)?$/u, async (route) => {
      if (!new URL(route.request().url()).pathname.endsWith('/preflight')) { declarations += 1; expect(route.request().postDataJSON()).toEqual({ preflight_id: PLAN, confirm: host.name, notify: true }); }
      await route.fulfill({ status: 200, json: loss });
    });
    await page.goto(`/resources/hosts/${host.id}`); await screenReady(page, locale, theme);
    await page.getByRole('button', { name: caption('@resources.act.menuOf', locale).replace('{{name}}', host.name), exact: true }).click();
    await page.getByRole('menuitem', { name: caption('@loss.declareMenu', locale), exact: true }).click();
    const dialog = page.getByRole('dialog', { name: caption('@loss.declareTitle', locale).replace('{{name}}', host.name), exact: true });
    const input = dialog.getByRole('textbox', { name: caption('@loss.typeName', locale).replace('{{name}}', host.name), exact: true });
    const submit = dialog.getByRole('button', { name: caption('@loss.declare', locale), exact: true });
    await input.fill('Wrong name'); await expect(submit).toBeDisabled(); expect(declarations).toBe(0);
    await input.fill(host.name); await expect(submit).toBeEnabled();
    await page.screenshot({ path: info.outputPath('permanent-loss-plan.png'), fullPage: true });
    await submit.click();
    await expect(dialog.getByText(caption('@hostCustody.declared', locale).replace('{{name}}', host.name), { exact: true })).toBeVisible();
    await expect(dialog.getByRole('textbox')).toHaveCount(0); expect(declarations).toBe(1);
    expect(await dialog.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath('permanent-loss-result.png'), fullPage: true });
    await dialog.locator('footer').getByRole('button', { name: caption('@common.close', locale), exact: true }).click();
    await expect(dialog).toHaveCount(0);
  });
}

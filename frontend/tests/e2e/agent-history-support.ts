/** Native wire fixtures for the managed-history setting and build-check presentation. */
import type { Page } from '@playwright/test';
import type { HarnessBuild, MachineRef, ResolvedSetting, SettingDef, SettingReach } from '../../src/generated/api-types.ts';
import { RUNS_ON } from '../doubles/fixtures.ts';
import { ACME } from './support.ts';

export const HISTORY_KEY = 'retention.agent_history_idle_days';
export const PROGRAM_EXIT_REASON = 'Codex app-server exited with code 7: final diagnostic while a managed turn was running; the recorded conversation remains available';

/** Native HTTP reads and live snapshots share this single fake-server observation. */
export async function programExitProvider() {
  const setState = async (state: 'none' | 'error' | 'online' | 'unknown') => {
    const address = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
    const response = await fetch(`${address}/__double/resources/provider-program-report`, { method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ id: RUNS_ON.codexSubscription, state, reason: PROGRAM_EXIT_REASON }) });
    if (!response.ok) throw new Error(`The native program observation was refused: ${response.status}`);
    return await response.json() as { source: MachineRef };

  };
  const { source } = await setState('online');
  return { setState, source, path: `/resources/providers/${RUNS_ON.codexSubscription}?tab=settings` };
}

export async function historySetting(page: Page) {
  const def: SettingDef = { key: HISTORY_KEY, title: 'Agent history kept', description: "Remove a thread's agent history after this many days without a turn; 0 never does.",
    group: 'retention', class: 'policy', scopes: ['system', 'org', 'project', 'topic', 'thread'], merge: 'replace',
    schema: { type: 'integer', minimum: 0, 'x-unit': 'days' }, default: 7, example: 7, display: { unit: 'days', zero: 'off' },
    text_key: 'day100.builtin.retention.agent_history_idle_days', keywords: [] };
  let current: ResolvedSetting = { key: HISTORY_KEY, value: 7, own: null, source: { scope: 'default', node: null }, locked_by: null, writable: true, parts: [] };
  const reach: SettingReach = { applies: { orgs: 0, projects: 0, topics: 0, threads: 1 }, overrides: { items: [], next_cursor: null } };
  const writes: unknown[] = [];
  await page.route('**/api/v1/settings/registry', (route) => route.fulfill({ json: [def] }));
  await page.route(`**/api/v1/nodes/${ACME.research}/settings`, (route) => route.fulfill({ json: [current] }));
  const settingPath = `/api/v1/nodes/${ACME.research}/settings/${HISTORY_KEY}`;
  await page.route((url) => url.pathname === settingPath || url.pathname === `${settingPath}/reach`, async (route) => {
    if (new URL(route.request().url()).pathname.endsWith('/reach')) { await route.fulfill({ json: reach }); return; }
    const body: unknown = route.request().postDataJSON(); writes.push(body);
    if (typeof body !== 'object' || body === null || !('value' in body) || typeof body.value !== 'number') throw new Error('The real numeric control must submit a number');
    current = { ...current, value: body.value, own: { value: body.value, locked: false, revision: 1 }, source: { scope: 'org', node: { id: ACME.research, kind: 'org', name: 'Research', redacted: false } } };
    await route.fulfill({ json: current });
  });
  return writes;
}

export async function historyBuilds(page: Page) {
  await page.route('**/api/v1/admin/harnesses', async (route) => {
    const response = await route.fetch();
    const builds = await response.json() as HarnessBuild[];
    await route.fulfill({ response, json: builds.map((build) => build.kind !== 'codex' ? build : { ...build, status: 'checked',
      check: { at: '2026-10-27T12:00:00Z', machine: 'host-1', missing: ['thread/delete', 'thread/loaded/list'], off: ['history_delete', 'history_delete_fallback'], error: null, stage: 'complete', cases: [] } }) });
  });
}

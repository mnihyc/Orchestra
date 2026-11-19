/** Details reads the native reach only on intent, retains its counts and pages overrides without guessing descendants. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import { PAGE_SIZE } from '@/api/nodes';
import { SettingReachDetails } from '@/features/settings/Reach';
import { SettingRow } from '@/features/settings/SettingRow';
import type { KeyDef } from '@/features/settings/settingsModel';
import type { ResolvedSetting, SettingReach } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

interface ReadOptions {
  params: { path: { id?: string; key: string }; query: { limit: number; cursor?: string } };
  signal: AbortSignal;
}
const calls = vi.hoisted(() => ({ read: vi.fn<(path: string, options: ReadOptions) => Promise<{ data: SettingReach; response: Response }>>() }));
vi.mock('@/api/client', async (load) => ({ ...await load<object>(), api: { GET: calls.read } }));
vi.mock('@/features/chat/open', () => ({ useOpenPath: () => vi.fn() }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

const def: KeyDef = { key: 'agent.web_search', title: 'Web search', description: 'Let the agent search the web.',
  group: 'agent', class: 'parameter', scopes: ['org', 'project', 'topic'], merge: 'replace', schema: { type: 'boolean' },
  default: false, example: true, text_key: null, keywords: [] };
const target = { kind: 'node', id: 'research' } as const;
const initial: ResolvedSetting = { key: def.key, value: true, own: null, writable: false, locked_by: null,
  source: { scope: 'default', node: null }, parts: [] };
const first: SettingReach = { applies: { orgs: 0, projects: 1, topics: 1, threads: 7 }, overrides: {
  items: [{ id: 'override-one', kind: 'topic', name: 'login', redacted: false }], next_cursor: 'page-two',
} };
const second: SettingReach = { ...first, overrides: { items: [{ id: 'override-two', kind: 'project', name: 'Web', redacted: false }], next_cursor: null } };
const reply = (data: SettingReach) => ({ data, response: new Response(null, { status: 200 }) });
function expectRead(index: number, path: string, params: ReadOptions['params']) {
  const call = calls.read.mock.calls[index];
  if (call === undefined) throw new Error(`Reach request ${index + 1} was not made`);
  const [actualPath, { signal, ...options }] = call;
  expect(actualPath).toBe(path);
  expect(options).toEqual({ params });
  expect(signal).toBeInstanceOf(AbortSignal);
}
beforeEach(async () => {
  await setLocale('en'); vi.clearAllMocks(); calls.read.mockResolvedValue(reply(first));
});
afterEach(async () => { await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('loads from the row itself and gives native applicability and overrides together without a gear in %s', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  show(<ul><SettingRow def={def} setting={initial} target={target} place={{ name: 'Research', kind: 'org', owner: null, access: { permissions: ['org.view'] } }} /></ul>);
  expect(calls.read).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('settings.reach.openFor', { name: def.title }) }));
  const region = await screen.findByRole('region', { name: i18n.t('settings.reach.openFor', { name: def.title }) });
  expect(within(region).getByText(locale === 'en' ? 'Applies to 1 project, 1 topic, and 7 threads' : '适用于1 个项目、1 个主题和7 个线程')).toBeVisible();
  expect(within(region).getByRole('link', { name: 'login' })).toHaveAttribute('href', '/topic/override-one?tab=settings&section=policies&setting=agent.web_search');
  expect(calls.read).toHaveBeenCalledOnce();
  expectRead(0, '/nodes/{id}/settings/{key}/reach', {
    path: { id: target.id, key: def.key }, query: { limit: PAGE_SIZE },
  });
  expect(region).not.toHaveTextContent('override-one');
  expect(region.closest('[data-setting-row]')).not.toBeNull();
  expect(region).toHaveTextContent('·');
  expect(screen.queryByRole('menu')).not.toBeInTheDocument();
  expect(screen.getAllByText(i18n.t('settings.editor.from.default'), { exact: true })).toHaveLength(1);
  await user.click(within(region).getByRole('button', { name: i18n.t('settings.reach.closeFor', { name: def.title }) }));
  await user.click(screen.getByRole('button', { name: i18n.t('settings.reach.openFor', { name: def.title }) }));
  expect(await screen.findByRole('region', { name: i18n.t('settings.reach.openFor', { name: def.title }) })).toBeVisible();
  expect(calls.read).toHaveBeenCalledTimes(1);
});

it('hides reach for a changed native value without a repair GET, and reads that value only when opened again', async () => {
  const user = userEvent.setup();
  const row = (setting: ResolvedSetting) => <ul><SettingRow def={def} setting={setting} target={target} place={{ name: 'Research', kind: 'org', owner: null, access: { permissions: ['org.view'] } }} /></ul>;
  const view = show(row(initial));
  await user.click(screen.getByRole('button', { name: i18n.t('settings.reach.openFor', { name: def.title }) }));
  await screen.findByRole('link', { name: 'login' });
  expect(calls.read).toHaveBeenCalledTimes(1);
  view.rerender(row({ ...initial, value: false }));
  expect(screen.queryByRole('link', { name: 'login' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: i18n.t('settings.reach.openFor', { name: def.title }) })).toBeVisible();
  expect(calls.read).toHaveBeenCalledTimes(1);
  calls.read.mockResolvedValue(reply({ applies: { orgs: 0, projects: 0, topics: 0, threads: 0 }, overrides: { items: [], next_cursor: null } }));
  await user.click(screen.getByRole('button', { name: i18n.t('settings.reach.openFor', { name: def.title }) }));
  expect(await screen.findByText(i18n.t('settings.reach.none'))).toBeVisible();
  expect(screen.queryByRole('link', { name: 'login' })).not.toBeInTheDocument();
  expect(calls.read).toHaveBeenCalledTimes(2);
});

it('loads the next cursor on request, keeps all served counts, and Refresh makes only one first-page read', async () => {
  const user = userEvent.setup();
  calls.read.mockResolvedValueOnce(reply(first)).mockResolvedValueOnce(reply(second)).mockResolvedValueOnce(reply(first));
  show(<SettingReachDetails target={target} def={def} snapshot="confirmed-value" />);
  await screen.findByRole('link', { name: 'login' });
  await user.click(screen.getByRole('button', { name: 'More overrides' }));
  expect(await screen.findByRole('link', { name: 'Web' })).toHaveAttribute('href', '/project/override-two/settings?section=policies&setting=agent.web_search');
  expect(screen.getByRole('link', { name: 'login' })).toBeVisible();
  expect(screen.getByText('Applies to 1 project, 1 topic, and 7 threads')).toBeVisible();
  expectRead(1, '/nodes/{id}/settings/{key}/reach', {
    path: { id: target.id, key: def.key }, query: { limit: PAGE_SIZE, cursor: 'page-two' },
  });
  expect(screen.queryByRole('button', { name: 'More overrides' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Refresh where this value applies' }));
  await waitFor(() => expect(calls.read).toHaveBeenCalledTimes(3));
  expectRead(2, '/nodes/{id}/settings/{key}/reach', {
    path: { id: target.id, key: def.key }, query: { limit: PAGE_SIZE },
  });
  expect(screen.queryByRole('link', { name: 'Web' })).not.toBeInTheDocument();
});

it('reads the system’s own native route and says visible zero without a completion or global count claim', async () => {
  calls.read.mockResolvedValue(reply({ applies: { orgs: 0, projects: 0, topics: 0, threads: 0 }, overrides: { items: [], next_cursor: null } }));
  show(<SettingReachDetails target={{ kind: 'system' }} def={def} snapshot="system-value" />);
  expect(await screen.findByText('No other visible places use this value.')).toBeVisible();
  expect(calls.read).toHaveBeenCalledOnce();
  expectRead(0, '/admin/settings/{key}/reach', {
    path: { key: def.key }, query: { limit: PAGE_SIZE },
  });
  expect(screen.queryByText('Overridden in')).not.toBeInTheDocument();
});

it('removes previously visible reach and override names when a new read is refused', async () => {
  const user = userEvent.setup();
  show(<SettingReachDetails target={target} def={def} snapshot="readable-value" />);
  await screen.findByRole('link', { name: 'login' });
  calls.read.mockRejectedValueOnce(new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Access denied', detail: 'This place is private now.', status: 403,
    request_id: 'reach-refused', missing: [], override: false, requestable: false }, null));
  await user.click(screen.getByRole('button', { name: 'Refresh where this value applies' }));
  await waitFor(() => expect(screen.queryByRole('link', { name: 'login' })).not.toBeInTheDocument());
  expect(screen.queryByText('Applies to 1 project, 1 topic, and 7 threads')).not.toBeInTheDocument();
  expect(screen.getByRole('alert')).toBeVisible();
});

/** An environment source keeps its native name and is never mistaken for a recorded snapshot. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as EnvironmentsApi from '@/api/environments';
import type * as SourceApi from '@/api/source';
import type * as SearchApi from '@/api/search';
import type * as RecentsApi from '@/api/pickerRecents';
import { client } from '@/api/client';
import { NetworkError } from '@/lib/requests';
import { snapshotQuery } from '@/api/source';
import { StartingFiles } from '@/features/start/StartingFiles';
import type { Environment, EnvironmentRef, Snapshot, SnapshotPage, NameHit, NameSearchPage } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { me, RUNS_ON, userRef } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { show } from './support';

const reads = vi.hoisted(() => ({ pages: vi.fn<(id:string,cursor:string|null)=>Promise<SnapshotPage>>(), names: vi.fn<(search: SearchApi.NameSearch, cursor: string | null) => Promise<NameSearchPage>>(), record: vi.fn<typeof RecentsApi.recordPickerChoice>(), environment: vi.fn<(id: string) => Promise<Environment>>() }));
vi.mock('@/api/environments', async (load) => {
  const actual = await load<typeof EnvironmentsApi>();
  return { ...actual, environmentQuery: (id: string) => ({ ...actual.environmentQuery(id), initialData: undefined, queryFn: () => reads.environment(id) }) };
});
vi.mock('@/api/source', async (load) => {
  const actual = await load<typeof SourceApi>();
  return { ...actual, snapshotQuery: vi.fn((id: string) => actual.snapshotQuery(id)), threadSnapshotsQuery: (id: string) => ({ ...actual.threadSnapshotsQuery(id), queryFn: ({ pageParam }: { pageParam: string | null }) => reads.pages(id, pageParam) }) };
});
vi.mock('@/api/search', async (load) => {
  const actual = await load<typeof SearchApi>();
  return { ...actual, pickerNamesQuery: (search: SearchApi.NameSearch) => ({ ...actual.pickerNamesQuery(search), queryFn: ({ pageParam }: { pageParam: string | null }) => reads.names(search, pageParam) }) };
});
vi.mock('@/api/pickerRecents', async (load) => {
  const actual = await load<typeof RecentsApi>();
  return { ...actual, pickerRecentsQuery: (person: string) => ({ ...actual.pickerRecentsQuery(person), queryFn: () => Promise.resolve({ count: 5, choices: { thread: ['late-thread'] } }) }), recordPickerChoice: reads.record };
});
vi.mock('@/features/people/ThreadIdentity', () => ({ ThreadIdentity: () => null }));
const session = client.session.getState();
const hit = (id: string, name: string): NameHit => ({ id, name, kind: 'thread', path: [], home_path: [], address: null, archived: false, avatar: null,
  color: null, custom_color: null, owner: null, recent: false, tag: null, thread_attention: null, username: null });
beforeEach(() => { reads.pages.mockReset();reads.names.mockReset().mockImplementation((search, cursor) => Promise.resolve({
  items: search.q === 'f' ? [hit('late-thread', 'Files from later')] : cursor === null ? [hit('first-thread', 'First files'), hit('late-thread', 'Files from later')] : [hit('more-thread', 'More files')],
  partial: cursor === null, next_cursor: cursor === null ? 'next' : null,
})); reads.record.mockReset().mockResolvedValue({ count: 5, remembered: true }); });
afterEach(async () => { cleanup(); client.session.setState(session); vi.clearAllMocks(); delete document.documentElement.dataset.theme; await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`shows and clears the native environment file source without a snapshot read ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const world = new World(), ada = world.people.get('ada');
    if (ada === undefined) throw new Error('The ordinary reader fixture is required.');
    const resource = world.resources.inventoryView(ada, 'environment', RUNS_ON.workspace);
    if (resource?.kind !== 'environment') throw new Error('The native environment fixture is required.');
    reads.environment.mockResolvedValue(resource.value);
    const change = vi.fn();
    show(<StartingFiles value={{ from_environment: resource.value.id, selection: { mode: 'none' } }} onChange={change} />);
    expect(await screen.findByRole('button', { name: resource.value.name })).toBeVisible();
    await waitFor(() => expect(reads.environment).toHaveBeenCalledWith(resource.value.id));
    expect(snapshotQuery).not.toHaveBeenCalled(); expect(change).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('start.where.clearFiles') }));
    expect(change).toHaveBeenCalledOnce(); expect(change).toHaveBeenCalledWith(undefined);
  });
}

it.each(['en', 'zh-CN'] as const)('chooses recent source threads with one-letter native search and keeps the chosen source while paging in %s', async (locale) => {
  await setLocale(locale); client.session.setState({ status: 'signed_in', userId: 'files-reader', readerVersion: 821 });
  const user = userEvent.setup(), change = vi.fn(); show(<StartingFiles onChange={change} />);
  expect(reads.names).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('start.where.filesFrom') }));
  const input = screen.getByRole('combobox', { name: i18n.t('start.where.fileThread') }); await user.click(input);
  expect(await screen.findByRole('option', { name: 'Files from later' })).toBeVisible();
  expect(screen.getAllByRole('option')[0]).toHaveAccessibleName('Files from later');
  await user.type(input, 'f'); await waitFor(() => expect(reads.names).toHaveBeenCalledWith(expect.objectContaining({ q: 'f', kinds: ['thread'] }), null));
  await user.keyboard('{Enter}'); await waitFor(() => expect(reads.record).toHaveBeenCalledWith({ kind: 'thread', key: 'late-thread' }));
  expect(change).not.toHaveBeenCalled(); await user.click(input);
  await screen.findByRole('option', { name: 'First files' });
  await user.click(await screen.findByRole('button', { name: i18n.t('ui.pagination.more') }));
  await screen.findByRole('option', { name: 'More files' });
  await user.keyboard('{Escape}'); await user.click(screen.getByRole('button', { name: i18n.t('start.where.useFiles') }));
  expect(change).toHaveBeenCalledExactlyOnceWith({ from_thread: 'late-thread', selection: { mode: 'all' } });
  expect(snapshotQuery).not.toHaveBeenCalled();
});

it('keeps a failed native source lookup explicitly retryable without changing any files', async () => {
  reads.names.mockRejectedValueOnce(new NetworkError('failed'));
  const change = vi.fn(), user = userEvent.setup(); show(<StartingFiles onChange={change} />);
  await user.click(screen.getByRole('button', { name: i18n.t('start.where.filesFrom') }));
  await user.click(screen.getByRole('combobox', { name: i18n.t('start.where.fileThread') }));
  await user.click(await screen.findByRole('button', { name: i18n.t('common.tryAgain') }));
  expect(await screen.findByRole('option', { name: 'First files' })).toBeVisible();
  expect(change).not.toHaveBeenCalled(); expect(reads.names).toHaveBeenCalledTimes(2);
});

it.each(['en', 'zh-CN'] as const)('finds a recorded source after another page without changing its native snapshot identity in %s', async (locale) => {
  await setLocale(locale);
  const owner = userRef(me(1, 'ada', 'Ada'));
  const environment: EnvironmentRef = { id: 'source-environment', name: 'Source workspace', address: 'acme.source', placement: 'managed', status: 'running', owner, ports_exposed: false };
  const snapshot = (id: string, label: string): Snapshot => ({ id, label, state: 'complete', kind: 'manual', storage: 'blobs', environment, directory: 'web',
    env_generation: 1, created_at: '2026-11-01T10:00:00Z', completed_at: '2026-11-01T10:00:01Z', created_by: owner, expires_at: null,
    file_count: 1, missing_count: 0, repository_head: null, selection: { include: [], exclude: [] }, thread_id: 'source-thread', total_bytes: 128, unavailable_reason: null });
  reads.names.mockResolvedValue({partial:false,next_cursor:null,items:[{ id: 'source-thread', name: 'Source thread', kind: 'thread', address: null, archived: false, avatar: null, color: null, custom_color: null,
    home_path: [], owner, path: [], recent: true, tag: null, thread_attention: null, username: null }]});
  reads.pages.mockImplementation((_id, cursor) => Promise.resolve(cursor === null
    ? { items: Array.from({ length: 12 }, (_, at) => snapshot(`record-${at}`, `Recorded source ${at}`)), next_cursor: 'older' }
    : { items: [snapshot('stable-source', 'Stable baseline')], next_cursor: null }));
  const chosen = vi.fn();
  const user = userEvent.setup();
  show(<StartingFiles onChange={chosen} />);
  await user.click(screen.getByRole('button', { name: i18n.t('start.where.filesFrom') }));
  await user.click(screen.getByRole('combobox',{name:i18n.t('start.where.fileThread')}));
  await user.click(await screen.findByRole('option',{name:'Source thread'}));
  await user.click(screen.getByRole('radio', { name: i18n.t('start.where.recordedFiles') }));
  await user.click(await screen.findByRole('button', { name: i18n.t('ui.pagination.more') }));
  await waitFor(() => expect(reads.pages).toHaveBeenCalledWith('source-thread', 'older'));
  const picker = screen.getByRole('combobox', { name: i18n.t('start.where.snapshot') });
  await user.click(picker);
  await user.type(picker, 'Stable baseline');
  expect(screen.queryByRole('option', { name: 'Recorded source 11' })).not.toBeInTheDocument();
  expect(chosen).not.toHaveBeenCalled();
  await user.click(await screen.findByRole('option', { name: 'Stable baseline' }));
  await user.click(screen.getByRole('button', { name: i18n.t('start.where.useFiles') }));
  expect(chosen).toHaveBeenCalledExactlyOnceWith({ snapshot_id: 'stable-source' });
});

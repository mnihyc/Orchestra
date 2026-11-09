/** New placement keeps explicit file sources and management choices, with no eager snapshot reads. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Search from '@/api/search';
import type * as Source from '@/api/source';
import { Where } from '@/features/start/Where';
import type { PlaceRef } from '@/features/navigation/places';
import type { NameHit, NewEnvironment, Snapshot, SnapshotPage } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { environment, options } from '../unit/start-fixtures';
import { show } from './support';

const io = vi.hoisted(() => ({ names: vi.fn<() => Promise<NameHit[]>>(), snapshots: vi.fn<(id: string, cursor: string | null) => Promise<SnapshotPage>>() }));
vi.mock('@/api/search', async (load) => ({ ...await load<typeof Search>(), namesQuery: (search: Search.NameSearch) => ({ queryKey: ['file-source-names', search], queryFn: io.names }) }));
vi.mock('@/api/source', async (load) => ({ ...await load<typeof Source>(), snapshotQuery: (id: string) => ({ queryKey: ['chosen-file-state', id], queryFn: () => Promise.resolve(state(id)) }), threadSnapshotsQuery: (id: string) => ({ queryKey: ['file-source-states', id], initialPageParam: null as string | null,
  queryFn: ({ pageParam }: { pageParam: string | null }) => io.snapshots(id, pageParam), getNextPageParam: (page: SnapshotPage) => page.next_cursor ?? undefined }) }));
vi.mock('@/features/people/NamedChip', () => ({ NamedChip: ({ name, id }: { name?: string; id: string }) => <span>{name ?? id}</span> }));
const place: PlaceRef = { id: 'topic', kind: 'topic', name: 'Work', parentId: null, archived: false, status: null, folder: false };
const hit: NameHit = { home_path: [], id: 'source-thread', kind: 'thread', name: 'Source work', address: null, archived: false, avatar: null, color: null, custom_color: null, owner: null, path: [], recent: true, tag: null, thread_attention: null, username: null };
function state(id: string, good = true): Snapshot {
  return { id, label: `State ${id}`, state: good ? 'complete' : 'partial', completed_at: '2026-10-14T10:00:00Z', created_at: '2026-10-14T10:00:00Z', created_by: null,
    directory: 'work', env_generation: 1, environment: environment('source').environment, expires_at: null, file_count: 2, kind: 'manual', missing_count: good ? 0 : 1,
    repository_head: null, selection: { include: [], exclude: [] }, storage: 'blobs', thread_id: hit.id, total_bytes: 20, unavailable_reason: null };
}
function NewPlacement() {
  const [value, setValue] = useState<Pick<NewEnvironment, 'files' | 'access'>>({});
  return <><Where compact place={place} options={options({ environments: [], environment_default: 'fresh' })} value="new" onChange={() => undefined} machines={[]} homeId="org" requesting={null} onRequestingChange={() => undefined}
    newEnvironment={value} onNewEnvironmentChange={setValue} /><output aria-label="Creation choices">{JSON.stringify(value)}</output></>;
}
beforeEach(() => { io.names.mockReset().mockResolvedValue([hit]); io.snapshots.mockReset().mockImplementation((_id, cursor) => Promise.resolve({ items: cursor === null ? [state('bad', false)] : [state('good')], next_cursor: cursor === null ? 'next' : null })); });
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it('chooses a thread’s selected files and who manages the new environment in '+locale, async () => {
    await setLocale(locale); const user = userEvent.setup(); show(<NewPlacement />);
    expect(io.names).not.toHaveBeenCalled(); expect(io.snapshots).not.toHaveBeenCalled();
    const managers = screen.getByRole('radiogroup', { name: i18n.t('start.where.managers') });
    await user.click(within(managers).getByRole('radio', { name: i18n.t('choiceCatalog.resourceAccess.only_me.name') }));
    await user.click(screen.getByRole('button', { name: i18n.t('start.where.filesFrom') }));
    const row = await screen.findByRole('treeitem', { name: 'Source work' }); row.focus(); await user.keyboard('{Enter}');
    await user.click(screen.getByRole('radio', { name: i18n.t('changeCompact.files.selection') }));
    await user.type(screen.getByLabelText(i18n.t('fork.files.include')), 'src/**');
    await user.click(screen.getByRole('button', { name: i18n.t('start.where.useFiles') }));
    expect(screen.getByLabelText('Creation choices')).toHaveTextContent(JSON.stringify({ access: 'only_me', files: { from_thread: hit.id, selection: { mode: 'selection', include: ['src/**'] } } }));
    expect(io.snapshots).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: i18n.t('start.where.filesFrom') }));
    await user.click(screen.getByRole('button', { name: i18n.t('common.cancel') }));
    expect(screen.getByLabelText('Creation choices')).toHaveTextContent('src/**');
    await user.click(screen.getByRole('button', { name: i18n.t('start.where.clearFiles') }));
    expect(screen.getByLabelText('Creation choices')).toHaveTextContent('{"access":"only_me"}');
  });

  it('pages recorded states and refuses an incomplete one in '+locale, async () => {
    await setLocale(locale); const user = userEvent.setup(); show(<NewPlacement />);
    await user.click(screen.getByRole('button', { name: i18n.t('start.where.filesFrom') }));
    await user.click(await screen.findByRole('treeitem', { name: 'Source work' }));
    await user.click(screen.getByRole('radio', { name: i18n.t('start.where.recordedFiles') }));
    await waitFor(() => expect(io.snapshots).toHaveBeenCalledWith(hit.id, null));
    const choose = screen.getByRole('button', { name: i18n.t('start.where.useFiles') }); expect(choose).toBeDisabled();
    await user.click(await screen.findByRole('combobox', { name: i18n.t('start.where.snapshot') }));
    expect(screen.getByRole('option', { name: 'State bad' })).toHaveAttribute('data-disabled');
    await user.keyboard('{Escape}');
    await user.click(screen.getByRole('button', { name: i18n.t('ui.pagination.more') }));
    await waitFor(() => expect(io.snapshots).toHaveBeenCalledWith(hit.id, 'next'));
    await user.click(screen.getByRole('combobox', { name: i18n.t('start.where.snapshot') }));
    await user.click(await screen.findByRole('option', { name: 'State good' }));
    await waitFor(() => expect(screen.getByRole('button', { name: i18n.t('start.where.useFiles') })).toBeEnabled());
    await user.click(screen.getByRole('button', { name: i18n.t('start.where.useFiles') }));
    expect(screen.getByLabelText('Creation choices')).toHaveTextContent('{"files":{"snapshot_id":"good"}}');
  });
}

/** An environment source keeps its native name and is never mistaken for a recorded snapshot. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as EnvironmentsApi from '@/api/environments';
import type * as SourceApi from '@/api/source';
import { snapshotQuery } from '@/api/source';
import { StartingFiles } from '@/features/start/StartingFiles';
import type { Environment } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { RUNS_ON } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { show } from './support';

const reads = vi.hoisted(() => ({ environment: vi.fn<(id: string) => Promise<Environment>>() }));
vi.mock('@/api/environments', async (load) => {
  const actual = await load<typeof EnvironmentsApi>();
  return { ...actual, environmentQuery: (id: string) => ({ ...actual.environmentQuery(id), initialData: undefined, queryFn: () => reads.environment(id) }) };
});
vi.mock('@/api/source', async (load) => {
  const actual = await load<typeof SourceApi>();
  return { ...actual, snapshotQuery: vi.fn((id: string) => actual.snapshotQuery(id)) };
});
afterEach(async () => { cleanup(); vi.clearAllMocks(); delete document.documentElement.dataset.theme; await setLocale('en'); });

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

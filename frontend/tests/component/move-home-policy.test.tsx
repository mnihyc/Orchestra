/** A legacy directory's undeclared personal home is left out unless the person selects it. */
import { waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';

import { planEnvironmentMove } from '@/api/changes';
import { EnvironmentChange } from '@/features/change/EnvironmentChange';

import { show } from './support';

const legacy = vi.hoisted((): { sources: null | undefined } => ({ sources: null }));

vi.mock('@/api/environments', async (original) => ({ ...await original<object>(), environmentQuery: () => ({
  queryKey: ['legacy-move-environment'],
  queryFn: () => Promise.resolve({ name: 'Legacy directory', template: null, data_sources: legacy.sources }),
}) }));
vi.mock('@/api/resources', async (original) => ({ ...await original<object>(), templatesQuery: () => ({
  queryKey: ['legacy-move-templates'], queryFn: () => Promise.resolve([]),
}) }));
vi.mock('@/api/changes', async (original) => ({
  ...await original<object>(),
  environmentMovePlacesQuery: () => ({ queryKey: ['legacy-move-places'], queryFn: () => Promise.resolve([]) }),
  planEnvironmentMove: vi.fn(),
}));

beforeEach(() => {
  vi.clearAllMocks();
  // A refused destination still exposes the requested selection without starting a move.
  vi.mocked(planEnvironmentMove).mockRejectedValue(new Error('No destination is available'));
});

for (const sources of [null, undefined]) it(`leaves an undeclared home out when data_sources is ${String(sources)}`, async () => {
  legacy.sources = sources;
  show(<EnvironmentChange environmentId="legacy" kind="move" job={null} open onClose={vi.fn()} />);
  await waitFor(() => expect(planEnvironmentMove).toHaveBeenCalled());
  expect(vi.mocked(planEnvironmentMove).mock.calls[0]?.[1]).toMatchObject({ home: false });
});

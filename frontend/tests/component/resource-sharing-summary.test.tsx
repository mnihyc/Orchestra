/** A group describes the real home defaults, never the current reader's permission flags. */
import { cleanup, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Access from '@/api/access';
import { ListSharing, ResourceSharingSummary, resourceSharingLines } from '@/features/resources/SharingSummary';
import { CATALOGUE, pageFor } from '../unit/general-access-fixtures';
import { detailHome } from './resource-detail-fixtures';
import { show } from './support';

const values = vi.hoisted(() => ({ failed: false, reads: vi.fn() }));
vi.mock('@/api/access', async (load) => ({ ...await load<typeof Access>(),
  defaultsQuery: () => ({ queryKey: ['group-defaults'], initialData: pageFor('template'), queryFn: () => { values.reads(); return values.failed ? Promise.reject(new Error('No longer readable')) : Promise.resolve(pageFor('template')); }, retry: false }),
  permissionsQuery: { queryKey: ['group-permissions'], initialData: () => CATALOGUE.reply, queryFn: () => Promise.resolve(CATALOGUE.reply) },
}));
afterEach(() => { cleanup(); values.failed = false; values.reads.mockClear(); });

it('describes complete default roles with the existing catalogue projection', () => {
  const result = resourceSharingLines(pageFor('template'), CATALOGUE, ['template']);
  expect(result).not.toBeNull();
  expect(result?.some((line) => line.verb === 'interact' && line.holder.kind === 'role')).toBe(true);
  show(<ResourceSharingSummary home={detailHome} kinds={['template']} />);
  expect(screen.getByText('can use')).toBeVisible();
});

it('does not turn a profile-specific policy or different kinds into a universal audience', () => {
  const page = pageFor('environment');
  page.rows = page.rows.map((row) => ({ ...row, profile: 'connected' }));
  expect(resourceSharingLines(page, CATALOGUE, ['environment'])).toBeNull();
  expect(resourceSharingLines(pageFor('template'), CATALOGUE, ['template', 'mount'])).toBeNull();
});

it('withdraws a cached audience when its native defaults reader is refused', async () => {
  values.failed = true; const view = show(<ResourceSharingSummary home={detailHome} kinds={['template']} />);
  await vi.waitFor(() => expect(view.container.querySelector('[data-resource-sharing]')).toBeNull());
});

it('draws the sharing summary carried by the list without reading each home again', async () => {
  const defaults = pageFor('template');
  const homes = new Map([[detailHome.id, { home_id: detailHome.id, rows: defaults.rows, problem: null }]]);
  show(<ListSharing value={{ homes, retry: vi.fn() }}><ResourceSharingSummary home={detailHome} kinds={['template']} /></ListSharing>);
  expect(await screen.findByText('can use')).toBeVisible();
  expect(values.reads).not.toHaveBeenCalled();
});

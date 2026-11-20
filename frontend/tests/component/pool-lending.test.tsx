/** Consent rows are searchable by currently readable targets and never retain a failed name projection. */
import { queryOptions, QueryClient } from '@tanstack/react-query';
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { PoolLending } from '@/features/resources/pools/Lending';
import type { LendingMatcher, NameHit, PoolLendingScope, PoolLendingView } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { show } from './support';

const world = vi.hoisted(() => ({ hide: false, put: vi.fn() }));
const scope: PoolLendingScope = { kind: 'member', object: { kind: 'provider', id: 'provider' } };
const row = (id: string): PoolLendingView => ({ id: `consent-${id}`, matcher: { kind: 'pool', pool_id: id }, options: {}, show_names: false, revision: 1, can_revoke: true, expires_at: null, giver: null });
vi.mock('@/api/pools', async (original) => ({ ...await original<object>(), putPoolLending: world.put,
  poolLendingQuery: () => queryOptions({ queryKey: ['consents'], queryFn: () => ({ items: [row('cheap'), row('other')], next: null }) }),
  poolConsentNamesQuery: (matcher: LendingMatcher) => queryOptions({ queryKey: ['consent-names', matcher], queryFn: () => {
    if (world.hide) throw new Error('The name is unavailable');
    return [{ id: 'pool', kind: 'pool', name: matcher.kind === 'pool' && matcher.pool_id === 'cheap' ? 'Cheap models' : 'Other models' } as NameHit];
  } }),
}));

it('finds a member consent by its readable pool name without changing it', async () => {
  world.hide = false; world.put.mockReset();
  show(<PoolLending scope={scope} />);
  await userEvent.click(screen.getByText(i18n.t('pools.through')));
  expect(await screen.findByText('Cheap models')).toBeInTheDocument();
  await userEvent.type(screen.getByRole('textbox', { name: i18n.t('pools.searchConsents') }), 'cheap');
  expect(screen.getByText('Cheap models')).toBeInTheDocument();
  expect(screen.queryByText('Other models')).not.toBeInTheDocument();
  expect(world.put).not.toHaveBeenCalled();
});

it('removes a cached target name when its current lookup fails', async () => {
  world.hide = false;
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  show(<PoolLending scope={scope} />, cache);
  await userEvent.click(screen.getByText(i18n.t('pools.through')));
  await screen.findByText('Cheap models');
  world.hide = true;
  await cache.invalidateQueries({ queryKey: ['consent-names'], exact: false });
  await waitFor(() => expect(screen.queryByText('Cheap models')).not.toBeInTheDocument());
});

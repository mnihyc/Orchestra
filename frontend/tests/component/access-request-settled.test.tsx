/** A successful local decision stays readable after the pending list refreshes. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as AccessApi from '@/api/access';
import type * as ApprovalsApi from '@/api/approvals';
import { RequestReview } from '@/features/access/RequestCard';
import type { Approval, UserRef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ADA, BEA, CATALOGUE, TOPIC } from '../unit/general-access-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({
  read: vi.fn<() => Promise<Approval[]>>(), decide: vi.fn<typeof ApprovalsApi.decideApproval>(),
}));
vi.mock('@/api/access', async (load) => ({ ...await load<typeof AccessApi>(),
  waitingRequestsQuery: (id: string) => ({ queryKey: ['settled-requests', id], queryFn: calls.read, retry: false }),
  permissionsQuery: { queryKey: ['settled-permissions'], queryFn: () => Promise.resolve(CATALOGUE.reply) },
}));
vi.mock('@/api/approvals', async (load) => ({ ...await load<typeof ApprovalsApi>(), decideApproval: calls.decide }));
vi.mock('@/realtime/live', async (load) => ({ ...await load(), useSocketStatus: () => ({ phase: 'ready' }) }));
vi.mock('@/ui/UserChip', () => ({ UserChip: ({ user }: { user: UserRef }) => <span>{user.display_name}</span> }));

const pending: Extract<Approval, { kind: 'access' }> = {
  id: 'request', batch_id: 'batch', kind: 'access', state: 'pending', title: 'Review Release',
  object: { kind: 'thread', id: 'release', name: 'Release' }, node: TOPIC, requester: { user: BEA },
  request: { permissions: ['doc.view'], reason: 'Read the release.', holder: { kind: 'user', user: BEA }, options: null, for_thread: null },
  required_permission: 'access.manage', can_decide: true, approver_scope: null, created_at: '2026-10-15T21:31:00Z',
  decided_at: null, decided_automatically: false, decided_by: null, decision: null, expires_at: null, item_id: null, thread_id: null,
};
const decided: Approval = { ...pending, state: 'approved', can_decide: false, decided_by: ADA,
  decided_at: '2026-11-03T08:00:00Z', decision: { approve: true } };
let cache: QueryClient;
beforeEach(async () => {
  await setLocale('en');
  cache = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  calls.read.mockReset().mockResolvedValue([pending]);
  calls.decide.mockReset();
});
afterEach(() => { cleanup(); cache.clear(); });

it('retains its own decided row when refresh removes it before the decision callback', async () => {
  let finish: (approval: Approval) => void = () => undefined;
  calls.decide.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  const user = userEvent.setup();
  show(<QueryClientProvider client={cache}><RequestReview nodeId="topic" /></QueryClientProvider>);
  await user.click(screen.getByRole('button', { name: i18n.t('access.waiting.review') }));
  await user.click(await screen.findByRole('button', { name: 'Other…' }));
  await user.click(screen.getByRole('button', { name: /^Approve\b/ }));
  await waitFor(() => expect(calls.decide).toHaveBeenCalledOnce());
  act(() => { cache.setQueryData(['settled-requests', 'topic'], []); });
  expect(await screen.findByText(i18n.t('access.waiting.empty'))).toBeVisible();
  act(() => { finish(decided); });
  const region = await screen.findByRole('region', { name: i18n.t('access.waiting.title') });
  expect(within(region).getByRole('status')).toHaveTextContent(i18n.t('approvals.quick.given'));
  expect(region).toHaveTextContent(ADA.display_name);
  expect(within(region).getByRole('status').querySelector('time')).toHaveAttribute('datetime', decided.decided_at);
  await user.keyboard('{Escape}');
  calls.read.mockResolvedValue([]);
  await user.click(screen.getByRole('button', { name: i18n.t('access.waiting.review') }));
  expect(await screen.findByText(i18n.t('access.waiting.empty'))).toBeVisible();
  expect(screen.queryByRole('region', { name: i18n.t('access.waiting.title') })).not.toBeInTheDocument();
});

it('does not retain requests removed by somebody else', async () => {
  const user = userEvent.setup();
  show(<QueryClientProvider client={cache}><RequestReview nodeId="topic" /></QueryClientProvider>);
  await user.click(screen.getByRole('button', { name: i18n.t('access.waiting.review') }));
  await screen.findByRole('region', { name: i18n.t('access.waiting.title') });
  act(() => { cache.setQueryData(['settled-requests', 'topic'], []); });
  expect(await screen.findByText(i18n.t('access.waiting.empty'))).toBeVisible();
  expect(screen.queryByRole('region', { name: i18n.t('access.waiting.title') })).not.toBeInTheDocument();
  expect(calls.decide).not.toHaveBeenCalled();
});

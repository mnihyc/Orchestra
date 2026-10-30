/** The before-Orch band keeps native failure words, offers Read again only to a manager, and waits for no offline or unsupported reader. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { BeforeOrchHeading } from '@/features/timeline/items/BeforeOrch';
import type { AdoptHistoryStatus } from '@/generated/api-types';
import type * as ThreadStore from '@/realtime/stores/thread';
import { show } from './support';

const calls = vi.hoisted(() => ({ status: null as AdoptHistoryStatus | null }));
vi.mock('@/features/timeline/context', () => ({ useTimelineContext: () => ({ threadId: 'thread-1', inspection: undefined }) }));
vi.mock('@/api/adoption', () => ({ adoptedHistoryQuery: (thread: string) => ({ queryKey: ['test-history', thread], queryFn: () => Promise.resolve(calls.status) }), retryAdoptedHistory: vi.fn() }));
vi.mock('@/realtime/stores/thread', async (original) => ({ ...await original<typeof ThreadStore>(), useThreadRow: () => ({ access: { permissions: ['thread.manage'] } }) }));
function draw() { show(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><BeforeOrchHeading /></QueryClientProvider>); }
beforeEach(() => { calls.status = { state: 'unreadable', reason: 'The native record is still locked by its writer', can_read_again: true }; });

describe('the original history of an adopted session', () => {
  it('shows the harness words and the supported read act', async () => { draw(); expect(await screen.findByText('The native record is still locked by its writer')).toBeVisible(); expect(screen.getByRole('button', { name: 'Read again' })).toBeVisible(); });
  it('gives a viewer the same reason without an act', async () => { calls.status!.can_read_again = false; draw(); await screen.findByText('The native record is still locked by its writer'); expect(screen.queryByRole('button')).not.toBeInTheDocument(); });
  it('says that the machine is offline and offers no queued retry', async () => { calls.status = { state: 'offline', reason: null, can_read_again: false }; draw(); expect(await screen.findByText(/machine is offline/)).toBeVisible(); expect(screen.queryByRole('button')).not.toBeInTheDocument(); });
  it('says once that the adapter cannot read sessions and offers nothing', async () => { calls.status = { state: 'unsupported', reason: 'No session reader in this extension', can_read_again: false }; draw(); expect(await screen.findByText(/adapter cannot read/)).toBeVisible(); expect(screen.getByText('No session reader in this extension')).toBeVisible(); expect(screen.queryByRole('button')).not.toBeInTheDocument(); });
  it('keeps a successfully imported record read-only with no retry act', async () => { calls.status = { state: 'read', reason: null, can_read_again: false }; draw(); expect(await screen.findByText(/not sent to the agent again/)).toBeVisible(); expect(screen.queryByRole('button')).not.toBeInTheDocument(); });
});

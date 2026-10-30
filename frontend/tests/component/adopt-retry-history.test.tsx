/** A failed adoption's original line offers a history read retry to its manager and reports another failure. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { RetryAdoptedHistory } from '@/features/resources/adopt/RetryHistory';
import type { Thread } from '@/generated/api-types';
import type * as ThreadStore from '@/realtime/stores/thread';
import { show } from './support';

const calls = vi.hoisted(() => ({ manages: true, retry: vi.fn<(thread: string, key: string) => Promise<Thread>>() }));
vi.mock('@/api/adoption', () => ({ retryAdoptedHistory: calls.retry }));
vi.mock('@/realtime/stores/thread', async (original) => ({ ...await original<typeof ThreadStore>(), useThreadRow: () => ({ access: { permissions: calls.manages ? ['thread.manage'] : ['thread.view'] } }) }));
function draw() { show(<QueryClientProvider client={new QueryClient({ defaultOptions: { mutations: { retry: false } } })}><RetryAdoptedHistory threadId="thread-1" /></QueryClientProvider>); }
beforeEach(() => { calls.manages = true; calls.retry.mockReset().mockResolvedValue({ id: 'thread-1' } as unknown as Thread); });

describe('reading adopted history again', () => {
  it('requests a history read with its own repeatable identity', async () => {
    const user = userEvent.setup(); draw();
    await user.click(screen.getByRole('button', { name: 'Read again' }));
    await waitFor(() => expect(calls.retry).toHaveBeenCalledTimes(1));
    expect(calls.retry).toHaveBeenCalledWith('thread-1', expect.any(String));
  });
  it('leaves a viewer without a retry act', () => { calls.manages = false; draw(); expect(screen.queryByRole('button')).not.toBeInTheDocument(); });
  it('shows another failed read with the original line still present', async () => {
    calls.retry.mockRejectedValue(new Error('The record is still unavailable'));
    const user = userEvent.setup(); draw();
    await user.click(screen.getByRole('button', { name: 'Read again' }));
    expect(await screen.findByRole('alert')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Read again' })).toBeEnabled();
  });
});

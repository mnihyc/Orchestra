/** Group read receipts preserve the original cut/key and use only the returned Undo ticket after resync. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { readGroup } from '@/api/away';
import { ApiError } from '@/api/client';
import { ReadReceipt, useGroupReads } from '@/features/inbox/away/Reads';
import type { PlaceGroup } from '@/features/inbox/away/model';
import { TooltipProvider } from '@/ui/Tooltip';

vi.setConfig({ testTimeout: 30_000 });
vi.mock('@/api/away', () => ({ readGroup: vi.fn(), groupedKey: ['away', 'groups'] }));
afterEach(() => vi.clearAllMocks());

const group: PlaceGroup = { key: 'supplied-group', place: null, unread: 2, waiting_for_me: 0, through_seq: 12, rows: [] };
const result = { unread: 0, changed: 2, undo_token: 'server-ticket', undo_expires_at: '2099-01-01T00:00:00.000Z' };
function Harness({ through = 12 }: { through?: number }) {
  const reads = useGroupReads();
  return <TooltipProvider><button onClick={() => reads.read({ ...group, through_seq: through }, 'Visible place')}>Mark group</button>{reads.acts.map((act) => <ReadReceipt key={act.key} act={act} onRetry={() => reads.retry(act)} onUndo={() => reads.undo(act)} />)}</TooltipProvider>;
}
function draw() {
  const client = new QueryClient();
  const view = render(<QueryClientProvider client={client}><Harness /></QueryClientProvider>);
  return { ...view, client };
}

describe('one group action survives uncertain replies', () => {
  it('keeps its cut and key when a newer arrival or resync precedes Retry', async () => {
    const { rerender, client } = draw();
    vi.mocked(readGroup).mockRejectedValueOnce(new Error('Reply lost')).mockResolvedValueOnce(result);
    await userEvent.click(screen.getByRole('button', { name: 'Mark group' }));
    await screen.findByText('Not confirmed');
    rerender(<QueryClientProvider client={client}><Harness through={20} /></QueryClientProvider>);
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
    await screen.findByRole('button', { name: 'Undo' });
    const [first, again] = vi.mocked(readGroup).mock.calls;
    expect(first?.[0]).toEqual({ group_key: 'supplied-group', through_seq: 12 });
    expect(again).toEqual(first);
  });
  it('a second press pending creates no second read', async () => {
    draw();
    let finish: (value: typeof result) => void = () => undefined;
    vi.mocked(readGroup).mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
    await userEvent.click(screen.getByRole('button', { name: 'Mark group' }));
    await userEvent.click(screen.getByRole('button', { name: 'Mark group' }));
    expect(readGroup).toHaveBeenCalledTimes(1);
    finish(result);
    await screen.findByRole('button', { name: 'Undo' });
  });
  it('Undo sends the returned ticket alone and an expired ticket never creates another read', async () => {
    draw();
    vi.mocked(readGroup).mockResolvedValueOnce(result).mockRejectedValueOnce(new ApiError({ type: 'orch:error:gone', code: 'gone', title: 'Expired', status: 410, detail: 'Expired', request_id: 'undo-expired' }, null));
    await userEvent.click(screen.getByRole('button', { name: 'Mark group' }));
    await userEvent.click(await screen.findByRole('button', { name: 'Undo' }));
    await waitFor(() => expect(screen.getByText('Its undo time ended')).toBeVisible());
    expect(vi.mocked(readGroup).mock.calls[1]?.[0]).toEqual({ undo_token: 'server-ticket' });
    expect(readGroup).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole('button', { name: 'Undo' })).not.toBeInTheDocument();
  });
});

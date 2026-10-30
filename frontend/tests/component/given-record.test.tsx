/** A long model input is read and copied whole, while its bounded preview remains readable. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { GivenRecord } from '@/features/inspector/Given';
import type { ModelInput, ModelTextBlock } from '@/generated/api-types';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<() => Promise<string>>(), copied: vi.fn<(text: string) => Promise<boolean>>() }));
vi.mock('@/api/modelInputs', () => ({
  modelInputsQuery: vi.fn(),
  modelInputText: (_record: ModelInput, block: ModelTextBlock) => block.text.blob === null ? Promise.resolve(block.text.inline) : calls.read(),
}));
vi.mock('@/ui/clipboard', () => ({ copyText: calls.copied }));

const record: ModelInput = { id: 'record-1', node_id: 'thread-1', segment_id: 'segment-1', turn_id: null, input_id: null, item_id: 'marker-1', purpose: 'session', reference: 'opening-1', state: 'sent', created_at: '2026-10-14T10:00:00Z', sent_at: '2026-10-14T10:00:01Z', seq_created: 1, seq_updated: 2, blocks: [
  { key: 'preamble', source: 'orch', text: { inline: 'Orch introduction', blob: null, truncated: false, total_len: 17 } },
  { key: 'standing', source: 'standing', text: { inline: 'The bounded tail', blob: 'hash-1', truncated: true, total_len: 50_000 } },
] };
function draw() { show(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><ol><GivenRecord record={record} search="" first /></ol></QueryClientProvider>); }
beforeEach(() => { calls.read.mockReset().mockResolvedValue('Beginning\nMiddle\nThe bounded tail'); calls.copied.mockReset().mockResolvedValue(true); });

describe('the exact text given to a model', () => {
  it('does not fetch a large block until asked, then shows its whole text without another scroll frame', async () => {
    const user = userEvent.setup(); draw();
    expect(screen.getByText('The bounded tail')).toBeVisible();
    expect(screen.getByText('Session opening')).toBeVisible();
    expect(screen.getByText('Sent')).toBeVisible();
    expect(screen.getByText(/characters in the preview/)).toBeVisible();
    expect(calls.read).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Read the full text' }));
    const full = await screen.findByText(/Beginning.*Middle.*The bounded tail/s);
    expect(full).not.toHaveClass('overflow-auto');
    expect(calls.read).toHaveBeenCalledTimes(1);
  });
  it('copies every full block in its original order, including text outside the preview', async () => {
    const user = userEvent.setup(); draw();
    await user.click(screen.getByRole('button', { name: 'Copy all that was given' }));
    await waitFor(() => expect(calls.copied).toHaveBeenCalledWith('Orch introduction\n\nBeginning\nMiddle\nThe bounded tail'));
  });
  it('shows a failed full read without claiming the preview was the whole text', async () => {
    calls.read.mockRejectedValue(new Error('The text cannot be read now'));
    const user = userEvent.setup(); draw();
    await user.click(screen.getByRole('button', { name: 'Read the full text' }));
    expect(await screen.findByRole('alert')).toBeVisible();
    expect(screen.getByText('The bounded tail')).toBeVisible();
  });
});

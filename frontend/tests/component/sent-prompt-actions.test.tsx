/** The delivered-message UI preserves an unknown edit's replay key and obeys actual author availability. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import type * as Continuations from '@/api/continuations';
import { captureThreadRead } from '@/api/threadRead';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { UserMessage } from '@/features/timeline/items/UserMessage';
import { NetworkError } from '@/lib/requests';
import { threadWriter } from '@/realtime/stores/thread';
import { userMessage } from '../doubles/thread-rows';
import { A, B, snapshot, summary, viewedPrompt } from '../unit/continuation-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ create: vi.fn<typeof Continuations.createPromptVersion>(), adjacent: vi.fn<typeof Continuations.adjacentPromptVersion>() }));
vi.mock('@/api/continuations', async (load) => ({ ...await load<typeof Continuations>(), createPromptVersion: calls.create, adjacentPromptVersion: calls.adjacent }));

function message({ mine = true, unavailable = false } = {}) {
  const sync = snapshot(); const author = sync.thread.owner;
  const item = { ...userMessage('sent-message', 'Original message', { author, inputId: 'sent-input' }), continuation_id: A };
  if (item.kind !== 'user_message') throw new Error('A delivered human message is required');
  const prompt = { ...summary('sent-input'), author, total: 1, edit: unavailable
    ? { available: false, reason: 'native_boundary_unsupported' as const, detail: null }
    : { available: true, reason: null, detail: null } };
  threadWriter.reset(sync.thread.id, { ...sync, thread: { ...sync.thread, current_turn_id: null, queue: { ...sync.thread.queue, revision: 8 } },
    items: [item], prompt_summaries: [prompt] }, false, captureThreadRead(sync.thread.id));
  const select = vi.fn();
  const context: TimelineContext = { threadId: sync.thread.id, threadName: sync.thread.name, detail: 'steps', recent: new Set(),
    layout: 'one', setLayout: () => undefined, meId: mine ? author.id : 'another-person', canInteract: true, canManage: true,
    live: true, lit: null, selection: NOT_PICKING, onVersionSelect: select };
  show(<TimelineContextValue.Provider value={context}><UserMessage item={item} /></TimelineContextValue.Provider>);
  return { select, threadId: sync.thread.id };
}

beforeEach(() => { threadWriter.clear(); calls.create.mockReset(); calls.adjacent.mockReset(); });

it.each([['lost connection', new NetworkError('failed')], ['server error', new ApiError({ type: 'about:blank', title: 'Internal error', status: 500, code: 'internal', detail: 'The commit result was not confirmed.', request_id: 'synthetic' }, null)]] as const)('retries an unknown edit with the exact original body (%s)', async (_name, error) => {
  const user = userEvent.setup();
  calls.create.mockRejectedValueOnce(error).mockResolvedValueOnce({ continuation_id: B, input_id: 'new-input',
    operation_id: 'operation', seq: 11, job: viewedPrompt('preparing').job! });
  const { select, threadId } = message();
  await user.click(screen.getByRole('button', { name: 'Edit message' }));
  const editor = screen.getByRole('textbox', { name: 'Edit message' });
  await user.clear(editor); await user.type(editor, 'Revised message');
  await user.click(screen.getByRole('button', { name: 'Send edit' }));
  const retry = await screen.findByRole('button', { name: 'Try again' });
  expect(editor).toBeDisabled(); expect(select).not.toHaveBeenCalled();
  await user.click(retry);
  expect(calls.create).toHaveBeenCalledTimes(2);
  expect(calls.create.mock.calls[1]).toEqual(calls.create.mock.calls[0]);
  expect(calls.create.mock.calls[0]?.[0]).toMatchObject({ threadId, key: { kind: 'thread', owner: threadId } });
  expect(calls.create.mock.calls[0]?.slice(1)).toEqual(['sent-input', expect.objectContaining({
    content: [{ type: 'text', text: 'Revised message' }], source_continuation_id: A,
    expected_active_continuation_id: A, expected_execution_revision: 1, expected_queue_revision: 8,
  })]);
  expect(calls.create.mock.calls[0]?.[2].client_id).toMatch(/^[a-f0-9-]{36}$/);
  expect(select).toHaveBeenCalledExactlyOnceWith(B, { inputId: 'new-input', itemId: null, groupId: 'synthetic-group' });
});

it('does not advertise Edit for another person’s message', () => {
  message({ mine: false });
  expect(screen.queryByRole('button', { name: 'Edit message' })).not.toBeInTheDocument();
  expect(calls.create).not.toHaveBeenCalled(); expect(calls.adjacent).not.toHaveBeenCalled();
});

it('explains a native boundary refusal without opening an editor or issuing work', async () => {
  const user = userEvent.setup(); message({ unavailable: true });
  const button = screen.getByRole('button', { name: 'Edit message' });
  expect(button).toHaveAttribute('aria-disabled', 'true');
  await user.click(button);
  expect(screen.queryByRole('textbox', { name: 'Edit message' })).not.toBeInTheDocument();
  expect(calls.create).not.toHaveBeenCalled();
});

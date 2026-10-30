/** Preparation status and actual activation jobs survive reload; retry never creates another prompt. */
import { act, cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import type * as Continuations from '@/api/continuations';
import { captureThreadRead, selectThreadContinuation } from '@/api/threadRead';
import { ContinuationNotice } from '@/features/thread/ContinuationNotice';
import type { ThreadPromptVersion } from '@/generated/api-types';
import { NetworkError } from '@/lib/requests';
import { threadWriter, useThreadView } from '@/realtime/stores/thread';
import { A, B, projection, snapshot, viewedPrompt } from '../unit/continuation-fixtures';
import { show } from './support';

const call = vi.hoisted(() => vi.fn<typeof Continuations.activateContinuation>());
vi.mock('@/api/continuations', async (load) => ({ ...await load<typeof Continuations>(), activateContinuation: call }));
beforeEach(() => { threadWriter.clear(); call.mockReset(); });
afterEach(() => { cleanup(); threadWriter.clear(); });
function mount(prompt: ThreadPromptVersion, allowed = true) {
  const cut = snapshot(projection(B, A, 4, false)), id = cut.thread.id;
  const sync = { ...cut, thread: { ...cut.thread, current_turn_id: null,
    input_access: { ...cut.thread.input_access, allowed }, queue: { ...cut.thread.queue, revision: 17 } }, viewed_prompt: prompt };
  selectThreadContinuation(id, B); threadWriter.reset(id, sync, false, captureThreadRead(id));
  const select = vi.fn();
  const Harness = () => <ContinuationNotice threadId={id} view={useThreadView(id)} onSelect={select} />;
  show(<Harness />);
  return { id, select, publish: (next: ThreadPromptVersion, seq = sync.seq) => act(() => {
    threadWriter.apply(id, { seq, viewed_prompt: next }, captureThreadRead(id));
  }) };
}
const accepted = (jobId: string) => ({ continuation_id: B, input_id: 'edited-input', operation_id: jobId,
  seq: 21, job: { ...viewedPrompt('preparing').job!, id: jobId } });

it('retries a failed version after reload and again after an actual failed activation using fresh operation identities', async () => {
  const user = userEvent.setup();
  call.mockResolvedValueOnce(accepted('retry-one')).mockResolvedValueOnce(accepted('retry-two'));
  const { id, select, publish } = mount(viewedPrompt('failed'));
  await user.click(screen.getByRole('button', { name: 'Try again' }));
  expect(call).toHaveBeenCalledTimes(1); expect(select).not.toHaveBeenCalled();
  const body = call.mock.calls[0]?.[2];
  expect(body?.client_id).toMatch(/^[a-f0-9-]{36}$/);
  expect(body).toEqual({ client_id: body?.client_id, expected_active_continuation_id: A, expected_execution_revision: 4, expected_queue_revision: 17 });
  expect(call.mock.calls[0]?.[0]).toMatchObject({ threadId: id }); expect(call.mock.calls[0]?.[1]).toBe(B);
  const failed = viewedPrompt('failed');
  publish({ ...failed, job: { ...failed.job!, id: 'retry-one' } });
  await user.click(await screen.findByRole('button', { name: 'Try again' }));
  expect(call).toHaveBeenCalledTimes(2);
  expect(call.mock.calls[1]?.[2].client_id).not.toBe(body?.client_id);
  expect(call.mock.calls[1]?.[2]).not.toHaveProperty('content'); expect(select).not.toHaveBeenCalled();
});

it.each([['lost connection', new NetworkError('failed')], ['server error', new ApiError({ type: 'about:blank', title: 'Internal error', status: 500, code: 'internal', detail: 'The commit result was not confirmed.', request_id: 'synthetic' }, null)]] as const)('retries an uncertain activation with exactly the same body (%s)', async (_name, error) => {
  const user = userEvent.setup(); call.mockRejectedValueOnce(error).mockResolvedValueOnce(accepted('one'));
  mount(viewedPrompt('ready'));
  await user.click(screen.getByRole('button', { name: 'Continue from this version' }));
  await user.click(await screen.findByRole('button', { name: 'Try again' }));
  expect(call).toHaveBeenCalledTimes(2); expect(call.mock.calls[1]).toEqual(call.mock.calls[0]);
});

it('a ready native version still shows its actual running activation and cannot be activated twice', async () => {
  const ready = viewedPrompt('ready');
  mount({ ...ready, job: { ...ready.job!, state: 'running', percent: 30 } });
  const button = screen.getByRole('button', { name: 'Continue from this version' });
  expect(button).toHaveAttribute('aria-disabled', 'true');
  await userEvent.click(button);
  expect(call).not.toHaveBeenCalled();
});

it('does not offer native retry without current input/provider authority', () => {
  mount(viewedPrompt('failed'), false);
  expect(screen.queryByRole('button', { name: 'Try again' })).not.toBeInTheDocument();
  expect(call).not.toHaveBeenCalled();
});


it('a newer authoritative terminal job cannot strand the earlier accepted retry in a busy state', async () => {
  const user = userEvent.setup(); call.mockResolvedValueOnce(accepted('earlier-accepted')).mockResolvedValueOnce(accepted('latest-retry'));
  const { publish } = mount(viewedPrompt('failed'));
  await user.click(screen.getByRole('button', { name: 'Try again' }));
  const failed = viewedPrompt('failed');
  expect(screen.getByRole('button', { name: 'Try again' })).toHaveAttribute('aria-disabled', 'true');
  publish({ ...failed, job: { ...failed.job!, id: 'older-terminal-operation' } }, 20);
  expect(screen.getByRole('button', { name: 'Try again' })).toHaveAttribute('aria-disabled', 'true');
  publish({ ...failed, job: { ...failed.job!, id: 'newer-terminal-operation' } }, 22);
  await user.click(await screen.findByRole('button', { name: 'Try again' }));
  expect(call).toHaveBeenCalledTimes(2);
  expect(call.mock.calls[1]?.[2].client_id).not.toBe(call.mock.calls[0]?.[2].client_id);
  expect(call.mock.calls[1]?.[2]).not.toHaveProperty('content');
});

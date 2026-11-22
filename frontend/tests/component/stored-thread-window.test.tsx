/** A retained renderer has its own actual-ID window and never follows or marks the live thread. */
import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { StoredTimeline } from '@/features/timeline/Timeline';
import { threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { A, B, answer, projection, snapshot } from '../unit/continuation-fixtures';
import { me, userRef } from '../doubles/fixtures';
import { command } from '../doubles/thread-rows';
import { show } from './support';

afterEach(() => { cleanup(); threadWriter.clear(); vi.unstubAllGlobals(); });

it('renders stored text and stored output without replacing the same thread’s live window or issuing live reads', async () => {
  const user = userEvent.setup();
  const live = snapshot(); const id = live.thread.id;
  threadWriter.reset(id, { ...live, items: [answer('live', 'The live route stays here', A)] }, false);
  const before = threadViewOf(id);
  const stored = snapshot(projection(B, A, 1, false));
  const tool = command('stored-command', { command: 'pwd', output: 'output', exit: 0 });
  if (tool.kind !== 'tool_call' || tool.payload.tool.kind !== 'command') throw new Error('Expected stored command');
  const complete = 'Stored complete output';
  const retained = 'b'.repeat(64);
  tool.payload.tool.output.total_len = complete.length;
  tool.payload.tool.output.blob = retained;
  tool.payload.tool.output.retained = { offset: 0, len: complete.length };
  const reads: string[] = [];
  vi.stubGlobal('fetch', (input: RequestInfo | URL) => {
    const request = input instanceof Request ? input : new Request(input);
    reads.push(request.url);
    if (new URL(request.url).pathname === '/api/v1/me/state') {
      return Promise.resolve(Response.json({ timeline: { work_detail: 'normal', diff_layout: 'one' } }));
    }
    return Promise.resolve(new Response(complete, { status: 200, headers: { 'content-type': 'text/plain' } }));
  });
  const shown = show(<StoredTimeline snapshot={{ ...stored, items: [answer('stored', 'Frozen retained message', A), tool] }} />);
  expect(await screen.findByText('Frozen retained message')).toBeVisible();
  expect(screen.queryByText('The live route stays here')).not.toBeInTheDocument();
  expect(threadViewOf(id)).toBe(before);
  expect(reads.map(url => new URL(url).pathname)).toEqual(['/api/v1/me/state']);
  await user.click(screen.getByRole('button', { name: /pwd/ }));
  await user.click(screen.getByRole('button', { name: /^Show all/ }));
  expect(await screen.findByText('Stored complete output')).toBeVisible();
  expect(reads).toHaveLength(2);
  const path = new URL(reads[1] ?? 'http://invalid');
  expect(path.pathname).toBe(`/api/v1/threads/${id}/items/stored-command/output`);
  expect(path.searchParams.get('inspect_deleted')).toBe('true');
  expect(path.searchParams.get('continuation_id')).toBe(B);
  expect(path.searchParams.get('expected_blob')).toBe(retained);
  expect(threadViewOf(id)).toBe(before);
  shown.unmount(); expect(threadViewOf(id)).toBe(before);
});

it('shows a cancelled author’s words with the recorded reason for losing access', async () => {
  const data = snapshot();
  const message = { id: 'queued', continuation_id: A, after_item: null, author: userRef(me(1, 'ada', 'Ada Lovelace')),
    parts: [{ type: 'text' as const, text: 'Keep my explanation here' }], submitted_at: '2026-11-01T12:00:00Z',
    outcome: { cause: 'access_changed' as const, at: '2026-11-01T12:05:00Z', by: null }, seq_created: 3, seq_updated: 9 };
  show(<StoredTimeline snapshot={{ ...data, items: [], retained_inputs: [message] }} />);
  expect(await screen.findByText('Keep my explanation here')).toBeVisible();
  expect(screen.getByText('Ada Lovelace')).toBeVisible();
  expect(screen.getByText('Cancelled · Access changed')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Copy' })).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: 'Details' }));
  const reason = await screen.findByText('Can no longer send to this thread');
  expect(reason).toBeVisible();
  expect(reason.closest('dl')?.querySelector('time')).toHaveAttribute('datetime', message.outcome.at);
  expect(screen.queryByRole('button', { name: /retry/i })).toBeNull();
});

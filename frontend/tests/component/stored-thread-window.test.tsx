/** A retained renderer has its own actual-ID window and never follows or marks the live thread. */
import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { StoredTimeline } from '@/features/timeline/Timeline';
import { threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { A, B, answer, projection, snapshot } from '../unit/continuation-fixtures';
import { command } from '../doubles/thread-rows';
import { show } from './support';

afterEach(() => { cleanup(); threadWriter.clear(); vi.unstubAllGlobals(); });

it('renders stored text and stored output without replacing the same thread’s live window or issuing live reads', async () => {
  const user = userEvent.setup();
  const live = snapshot(); const id = live.thread.id;
  threadWriter.reset(id, { ...live, items: [answer('live', 'The live route stays here', A)] }, false);
  const before = threadViewOf(id);
  const stored = snapshot(projection(B, A, 1, false));
  const tool = command('stored-command', { command: 'pwd', output: 'Stored tail', exit: 0 });
  if (tool.kind !== 'tool_call' || tool.payload.tool.kind !== 'command') throw new Error('Expected stored command');
  tool.payload.tool.output.total_len = 200;
  const reads: string[] = [];
  vi.stubGlobal('fetch', (input: RequestInfo | URL) => {
    const request = input instanceof Request ? input : new Request(input);
    reads.push(request.url);
    if (new URL(request.url).pathname === '/api/v1/me/state') {
      return Promise.resolve(Response.json({ timeline: { work_detail: 'steps', diff_layout: 'one' } }));
    }
    return Promise.resolve(new Response('Stored complete output', { status: 200, headers: { 'content-type': 'text/plain' } }));
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
  expect(threadViewOf(id)).toBe(before);
  shown.unmount(); expect(threadViewOf(id)).toBe(before);
});

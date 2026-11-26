/** A conversation on screen is never replaced by loading shapes because it is read again: a full resync, a reconnect that asks
 * for a reload, or a slow answer that then fails for the line. It stays, read-only, and the fresh answer replaces it. */
import { useImperativeHandle } from 'react';
import { act, cleanup, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Personal from '@/api/personal';
import type * as Live from '@/realtime/live';
import { Timeline } from '@/features/timeline/Timeline';
import { ThreadSummaryInspector } from '@/features/summaries/ThreadSummaryInspector';
import { client } from '@/api/client';
import { captureThreadRead } from '@/api/threadRead';
import { i18n } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { threadReadState, threadViewOf, threadWriter } from '@/realtime/stores/thread';
import type { VirtualListProps } from '@/ui/VirtualList';
import { agentMessage, turn, userMessage } from '../doubles/thread-rows';
import { A, snapshot } from '../unit/continuation-fixtures';
import { show } from './support';

vi.mock('@/api/personal', async (original) => ({ ...await original<typeof Personal>(),
  meStateQuery: { queryKey: ['me-state'], queryFn: () => Promise.resolve({}), staleTime: Infinity },
  setMeState: () => Promise.resolve(),
}));
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof Live>(), useSocketStatus: () => ({ phase: 'ready' }) }));
vi.mock('@/ui/VirtualList', () => ({ VirtualList: (props: VirtualListProps) => {
  useImperativeHandle(props.ref, () => ({ scrollToEnd: () => undefined, scrollToKey: () => true, isAtEnd: () => true, element: () => null }));
  return <ol aria-label={props.label}>{Array.from({ length: props.count }, (_, index) => <li key={props.keyOf(index)}>{props.render(index)}</li>)}</ol>;
} }));
vi.mock('@/features/timeline/StatusLine', () => ({ StatusLine: () => null }));
vi.mock('@/features/summaries/ResourceSummary', () => ({ ResourceSummary: () => <p>Summary: the login redirect</p> }));

const session = client.session.getState();
const ids: string[] = [];
afterEach(() => {
  cleanup(); vi.restoreAllMocks();
  for (const id of ids.splice(0)) threadWriter.evict(id);
  client.session.setState(session);
});

function rows(id: string, answerText: string) {
  const done = turn('kept-turn', { ordinal: 1, state: 'completed', inputId: 'kept-question' });
  const source = snapshot();
  return { ...source, turns: [done], items: [
    userMessage('kept-question', 'Where is the redirect?', { turn: done.id, author: source.thread.owner }),
    agentMessage('kept-answer', answerText, { turn: done.id }),
  ].map((item) => ({ ...item, thread_id: id, continuation_id: A })) };
}

function held() {
  client.session.setState({ status: 'signed_in', userId: 'kept-reader', readerVersion: 1 });
  const id = snapshot().thread.id;
  ids.push(id);
  threadWriter.reset(id, rows(id, 'In the login handler.'), false, captureThreadRead(id));
  return id;
}

/** What the read again looks like to the store, as the feed drives it. */
const readAgain = {
  /** A full resync of a changed authority or history: the read is retired, then opened again. */
  resync: (id: string) => { threadWriter.invalidate(id, false); threadWriter.beginRead(id, 1); },
  /** A reconnect the server answers with a reload: the same read is opened again. */
  reconnect: (id: string) => { threadWriter.beginRead(id, 1); },
  /** A slow answer that then failed for the line. */
  'failure of the line': (id: string) => { threadWriter.invalidate(id, false); threadWriter.beginRead(id, 1); threadWriter.failed(id, new NetworkError('failed')); },
};

it.each(Object.keys(readAgain) as (keyof typeof readAgain)[])('keeps the conversation and its summary through a %s and swaps in the fresh answer', (how) => {
  const id = held();
  const view = show(<><ThreadSummaryInspector threadId={id} /><Timeline threadId={id} item={null} empty={null} /></>);
  expect(screen.getByText('In the login handler.')).toBeVisible();
  expect(screen.getByText('Summary: the login redirect')).toBeVisible();
  act(() => readAgain[how](id));
  expect(threadReadState(threadViewOf(id))).toMatchObject({ readOnly: true, kept: true });
  expect(screen.getByText('Where is the redirect?')).toBeVisible();
  expect(screen.getByText('In the login handler.')).toBeVisible();
  expect(screen.getByText('Summary: the login redirect')).toBeVisible();
  expect(screen.queryByText(i18n.t('summaryBox.reading'))).not.toBeInTheDocument();
  expect(view.container.querySelector('[aria-busy="true"], .animate-skeleton')).toBeNull();
  if (how === 'failure of the line') {
    act(() => { threadWriter.failed(id, null); });
    act(() => { threadWriter.beginRead(id, 1); });
  }
  act(() => { threadWriter.reset(id, rows(id, 'In the session middleware.'), false, captureThreadRead(id)); });
  expect(screen.getByText('In the session middleware.')).toBeVisible();
  expect(screen.queryByText('In the login handler.')).not.toBeInTheDocument();
  expect(threadReadState(threadViewOf(id))).toMatchObject({ readiness: 'ready', kept: false });
});

it('shows the loading shapes only for a first opening, with nothing to keep', () => {
  client.session.setState({ status: 'signed_in', userId: 'kept-reader', readerVersion: 1 });
  const id = snapshot().thread.id; ids.push(id);
  threadWriter.beginRead(id, 1);
  const view = show(<Timeline threadId={id} item={null} empty={null} />);
  expect(view.container.querySelector('[aria-busy="true"]')).not.toBeNull();
  act(() => { threadWriter.reset(id, rows(id, 'In the login handler.'), false, captureThreadRead(id)); });
  expect(screen.getByText('In the login handler.')).toBeVisible();
  expect(view.container.querySelector('[aria-busy="true"]')).toBeNull();
});

/** The Outline shows conversation points in the timeline's words and follows live items without refetching. */
import { act, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type * as Router from '@tanstack/react-router';
import type * as ThreadsApi from '@/api/threads';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { OutlinePanel } from '@/features/inspector/Outline';
import type { ThreadOutlinePage, Thread } from '@/generated/api-types';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import { agentMessage, errorItem, turn, userMessage } from '../doubles/thread-rows';
import { me, userRef } from '../doubles/fixtures';
import { show } from './support';

const server = vi.hoisted(() => ({ read: vi.fn<() => Promise<ThreadOutlinePage>>() }));
vi.mock('@/api/threads', async (load) => ({ ...await load<typeof ThreadsApi>(), outlineQuery: (id: string) => ({ queryKey: ['outline', id], queryFn: server.read, initialPageParam: null, getNextPageParam: (page: ThreadOutlinePage) => page.next_before ?? undefined }) }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(), Link: ({ params, search, children, className }: { params: { threadId: string }; search: { item: string }; children: ReactNode; className?: string }) => <a className={className} href={`/thread/${params.threadId}?item=${search.item}`}>{children}</a> }));
const THREAD = 'outline-thread';
const AT = '2026-10-15T00:00:00Z';
const thread = { id: THREAD, seq: 12, state: 'idle', lock: null } as Thread;
let locale = currentLocale();
let theme: string | null = null;
beforeEach(() => {
  locale = currentLocale(); theme = document.documentElement.getAttribute('data-theme'); server.read.mockReset();
  server.read.mockResolvedValue({ entries: [
    { item_id: 'internal', seq: 1, at: AT, title: 'orch context', kind: 'marker' },
    { item_id: 'u1', seq: 2, at: AT, title: '', kind: 'turn', turn_ordinal: 1 },
    { item_id: 'u1', seq: 2, at: AT, title: 'Check the redirect', kind: 'user_message' },
    { item_id: 'milestone', seq: 10, at: AT, title: 'Redirect fixed', kind: 'milestone' },
    { item_id: 'approval', seq: 11, at: AT, title: 'Choose scope', kind: 'approval' },
  ], next_before: null, seq: 12 });
  const items = [userMessage('u1', 'Check the redirect', { author: userRef(me(1, 'ada', 'Ada')), turn: 't1' }), agentMessage('a1', 'Initial finding', { turn: 't1' })]
    .map((item, index) => ({ ...item, thread_id: THREAD, seq_created: index + 2, seq_updated: index + 2 }));
  threadWriter.reset(THREAD, { thread, items, turns: [turn('t1', { ordinal: 1, state: 'completed', inputId: 'in-1' })] }, false);
});
afterEach(async () => { vi.useRealTimers(); threadWriter.evict(THREAD); await setLocale(locale); if (theme === null) document.documentElement.removeAttribute('data-theme'); else document.documentElement.setAttribute('data-theme', theme); });

describe.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('outline %s / %s', (language, look) => {
  beforeEach(async () => { await setLocale(language); document.documentElement.setAttribute('data-theme', look); });
  it.each(['stored', 'live', 'legacy'] as const)('uses goal words for a %s input without exposing its internal prompt', async (mode) => {
    const condition = 'npm test exits 0';
    const message = userMessage('goal', '<orch>\nInternal instructions\n</orch>', { author: userRef(me(1, 'ada', 'Ada')), turn: 'goal-turn' });
    if (message.kind !== 'user_message') throw new Error('The fixture must be a user message');
    const facts = { goal_id: 'saved-goal', condition, turn: 7, max_turns: 30, tokens: 0, not_yet: null, plan: false };
    const item = { ...message, thread_id: THREAD, seq_created: 13, seq_updated: 13, payload: { ...message.payload, source: 'goal' as const, goal: mode === 'legacy' ? null : facts } };
    const stored = { item_id: 'goal', seq: 13, at: AT, title: condition, kind: 'user_message' as const, goal_condition: condition };
    server.read.mockResolvedValue({ entries: mode === 'stored' ? [stored] : [], next_before: null, seq: 13 });
    threadWriter.reset(THREAD, { thread, items: mode === 'stored' ? [] : [item], turns: [] }, false);
    show(<OutlinePanel access={{ thread }} />);
    const expected = mode === 'legacy' ? i18n.t('goals.line.label') : i18n.t('goals.turn.headline', { condition });
    const link = await screen.findByRole('link', { name: new RegExp(expected.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')) });
    expect(link).toHaveAttribute('href', `/thread/${THREAD}?item=goal`);
    expect(link).toHaveTextContent(expected);
    expect(screen.queryByText(/<orch>|Internal instructions/)).not.toBeInTheDocument();
    expect(server.read).toHaveBeenCalledTimes(1);
    if (mode === 'live') {
      act(() => { threadWriter.apply(THREAD, { items: [{ ...item, seq_updated: 14, payload: { ...item.payload, goal: { ...facts, condition: 'The new redirect is covered' } } }] }); });
      expect(await screen.findByText(i18n.t('goals.turn.headline', { condition: 'The new redirect is covered' }))).toBeVisible();
      expect(server.read).toHaveBeenCalledTimes(1);
    }
  });

  it('names the turn, both speakers, milestone and approval once and updates text without another list request', async () => {
    show(<OutlinePanel access={{ thread }} />);
    await screen.findByText('Redirect fixed');
    const list = screen.getByRole('list', { name: i18n.t('inspector.outline.list') });
    expect(within(list).getByText(i18n.t('inspector.outline.turn', { ordinal: 1 }))).toBeVisible();
    expect(within(list).getByText('Check the redirect')).toBeVisible();
    expect(within(list).getByText('Initial finding')).toBeVisible();
    expect(within(list).getByText('Choose scope')).toBeVisible();
    expect(within(list).queryByText('orch context')).not.toBeInTheDocument();
    expect(within(list).getAllByRole('link')).toHaveLength(5);
    expect(within(list).getByText('Check the redirect').closest('a')).toHaveAttribute('href', `/thread/${THREAD}?item=u1`);
    const updated = { ...agentMessage('a1', 'Revised finding', { turn: 't1' }), thread_id: THREAD, seq_created: 3, seq_updated: 13 };
    vi.useFakeTimers();
    await act(async () => { threadWriter.apply(THREAD, { thread: { ...thread, seq: 13 }, items: [updated] }); await vi.advanceTimersByTimeAsync(1500); });
    expect(within(list).getByText('Revised finding')).toBeVisible();
    expect(within(list).queryByText('Initial finding')).not.toBeInTheDocument();
    expect(server.read).toHaveBeenCalledTimes(1);
    vi.useRealTimers();
    await userEvent.click(screen.getByRole('checkbox', { name: i18n.t('inspector.outline.kind.user_message') }));
    expect(within(list).queryByText('Revised finding')).not.toBeInTheDocument();
    expect(within(list).queryByText('Check the redirect')).not.toBeInTheDocument();
    expect(within(list).getByText('Redirect fixed')).toBeVisible();
  });
});

it('loads an older outline page only when asked, retaining the live entries and both lines of a turn', async () => {
  const newest = await server.read();
  server.read.mockClear();
  threadWriter.apply(THREAD, { turns: [{ ...turn('t1', { ordinal: 2, state: 'completed', inputId: 'in-1' }), seq_updated: 13 }] });
  server.read.mockResolvedValueOnce({ ...newest, entries: newest.entries.map((entry) => entry.kind === 'turn' ? { ...entry, turn_ordinal: 2 } : entry), next_before: 2 }).mockResolvedValueOnce({ entries: [
    { item_id: 'older', seq: 1, at: AT, title: '', kind: 'turn', turn_ordinal: 1 },
    { item_id: 'older', seq: 1, at: AT, title: 'Earlier decision', kind: 'user_message' },
  ], next_before: null, seq: 12 });
  show(<OutlinePanel access={{ thread }} />);
  await screen.findByText('Redirect fixed');
  expect(server.read).toHaveBeenCalledTimes(1);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('ui.pagination.more') }));
  expect(await screen.findByText('Earlier decision')).toBeVisible();
  expect(screen.getByText(i18n.t('inspector.outline.turn', { ordinal: 1 }))).toBeVisible();
  expect(screen.getByText('Redirect fixed')).toBeVisible();
  expect(server.read).toHaveBeenCalledTimes(2);
});

it('keeps a reported error reachable and filterable as live errors arrive without another list read', async () => {
  show(<OutlinePanel access={{ thread }} />);
  await screen.findByText('Redirect fixed');
  act(() => { threadWriter.apply(THREAD, { items: [{ ...errorItem('failed', 'The model refused the request.'), thread_id: THREAD, seq_created: 13, seq_updated: 13 }] }); });
  const link = await screen.findByRole('link', { name: /The model refused the request/ });
  expect(link).toHaveAttribute('href', `/thread/${THREAD}?item=failed`);
  await userEvent.click(screen.getByRole('checkbox', { name: i18n.t('inspector.outline.kind.error') }));
  expect(screen.queryByRole('link', { name: /The model refused the request/ })).not.toBeInTheDocument();
  expect(screen.getByText('Redirect fixed')).toBeVisible();
  expect(server.read).toHaveBeenCalledTimes(1);
});

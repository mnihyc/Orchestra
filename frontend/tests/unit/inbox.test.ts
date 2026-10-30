/** What the Inbox holds as rows and counters arrive: a row is replaced by a higher number only, a read mark shows at once and is put back when the server refuses, rows beyond the loaded pages wait for their page, days are the person's own, and the three filters keep what they name. */
import { beforeEach, describe, expect, it } from 'vitest';

import { dayKey, filterRows, groupByDay, previousDay, writeDay } from '@/features/inbox/model';
import { bodyOf, unreadIn } from '@/features/inbox/read';
import type { Notification } from '@/generated/api-types';
import { userWriter, useUserStore, withinPages } from '@/realtime/stores/user';

/** A row written at minute `n` of an hour, with the Inbox number `n`. */
function row(n: number, more: Partial<Notification> = {}): Notification {
  return {
    id: `id-${String(n).padStart(3, '0')}`,
    kind: 'turn.completed',
    title: `Row ${n}`,
    body: '',
    node: null,
    thread_id: null,
    approval_id: null,
    chat_id: null,
    url: '/inbox',
    data: {},
    created_at: new Date(Date.UTC(2026, 9, 1, 12, n)).toISOString(),
    read_at: null,
    inbox_seq: n,
    group: null,
    waiting_for_me: false,
    ...more,
  };
}

const state = () => useUserStore.getState();
const held = (id: string) => state().rows.get(id);

beforeEach(() => userWriter.clear());

describe('the Inbox as rows arrive', () => {
  it('replaces a row by one with a higher Inbox number and never by one with an equal or lower', () => {
    userWriter.reset([row(1)], { unread: 1, approvalsPending: 0 }, 1, null);
    userWriter.apply([row(1, { read_at: '2026-10-01T13:00:00.000Z', inbox_seq: 5 })], null, 5);
    expect(held('id-001')?.read_at).not.toBeNull();
    userWriter.apply([row(1, { inbox_seq: 3 })], null, 5);
    expect(held('id-001')?.read_at).not.toBeNull();
    userWriter.apply([row(1, { title: 'same number', inbox_seq: 5 })], null, 5);
    expect(held('id-001')?.title).toBe('Row 1');
  });

  it('takes the counters that come with a frame, and leaves them alone where a frame brings none', () => {
    userWriter.reset([row(1)], { unread: 1, approvalsPending: 2 }, 1, null);
    userWriter.apply([row(2)], { unread: 2, approvalsPending: 3 }, 2);
    expect(state().counters).toEqual({ unread: 2, approvalsPending: 3 });
    userWriter.apply([row(3)], null, 3);
    expect(state().counters).toEqual({ unread: 2, approvalsPending: 3 });
    expect(state().lastSeq).toBe(3);
  });

  it('does not hold a row that lies beyond the pages loaded from the newest, and holds one that is newer', () => {
    userWriter.reset([row(10), row(9), row(8)], { unread: 3, approvalsPending: 0 }, 10, 'more');
    userWriter.apply([row(2, { inbox_seq: 11 })], null, 11);
    expect(held('id-002')).toBeUndefined();
    userWriter.apply([row(12, { inbox_seq: 12 })], null, 12);
    expect(held('id-012')).toBeDefined();
    expect(withinPages(state(), row(8))).toBe(true);
    expect(withinPages(state(), row(7))).toBe(false);
  });

  it('shows a row found beyond the pages under Unread alone, until the pages reach it', () => {
    userWriter.reset([row(10), row(9)], { unread: 3, approvalsPending: 0 }, 10, 'more');
    userWriter.found([row(2)]);
    const rows = [...state().rows.values()].sort((a, b) => b.inbox_seq - a.inbox_seq);
    const ids = (filter: 'all' | 'unread' | 'mentions') => filterRows(rows, filter, state()).map((each) => each.id);
    expect(ids('all')).toEqual(['id-010', 'id-009']);
    expect(ids('unread')).toEqual(['id-010', 'id-009', 'id-002']);
    userWriter.older([row(8), row(2)], null);
    expect(state().reachedEnd).toBe(true);
    expect(filterRows([...state().rows.values()].sort((a, b) => b.inbox_seq - a.inbox_seq), 'all', state()).map((each) => each.id)).toContain('id-002');
  });

  it('keeps for Mentions only the rows in which somebody named the person', () => {
    const rows = [row(4, { kind: 'mention' }), row(3, { kind: 'chat.message' }), row(2, { kind: 'doc.mention' }), row(1, { kind: 'doc.comment' })];
    userWriter.reset(rows, { unread: 4, approvalsPending: 0 }, 4, null);
    expect(filterRows(rows, 'mentions', state()).map((each) => each.kind)).toEqual(['mention', 'doc.mention']);
  });
});

describe('a read mark', () => {
  it('shows at once and lowers the count by what it changed', () => {
    userWriter.reset([row(1), row(2), row(3, { read_at: '2026-10-01T13:00:00.000Z' })], { unread: 2, approvalsPending: 0 }, 3, null);
    const before = userWriter.markRead(['id-001', 'id-003'], '2026-10-01T14:00:00.000Z');
    // The row that was read already is not counted twice.
    expect(before.map((each) => each.id)).toEqual(['id-001']);
    expect(held('id-001')?.read_at).toBe('2026-10-01T14:00:00.000Z');
    expect(state().counters.unread).toBe(1);
  });

  it('is put back where the server refused it, unless the server has said better since', () => {
    userWriter.reset([row(1), row(2)], { unread: 2, approvalsPending: 0 }, 2, null);
    const before = userWriter.markRead(['id-001', 'id-002'], '2026-10-01T14:00:00.000Z');
    // The server's own row for the second one arrived meanwhile, with a higher number.
    userWriter.apply([row(2, { read_at: '2026-10-01T14:00:01.000Z', inbox_seq: 9 })], { unread: 1, approvalsPending: 0 }, 9);
    userWriter.unmarkRead(before);
    expect(held('id-001')?.read_at).toBeNull();
    expect(held('id-002')?.read_at).toBe('2026-10-01T14:00:01.000Z');
    expect(state().counters.unread).toBe(2);
  });
});

describe('what a read selection names', () => {
  const rows = [
    row(1, { thread_id: 'thread-a' }),
    row(2, { node: { id: 'topic-b', kind: 'topic', name: 'B', redacted: false } }),
    row(3, { chat_id: 'chat-c' }),
    row(4, { thread_id: 'thread-a', read_at: '2026-10-01T13:00:00.000Z' }),
  ];

  it('is the unread rows of a place, whether the row names the place as its node or as its thread', () => {
    expect(unreadIn(rows, { place: 'thread-a' }).map((each) => each.id)).toEqual(['id-001']);
    expect(unreadIn(rows, { place: 'topic-b' }).map((each) => each.id)).toEqual(['id-002']);
  });

  it('is the unread rows of a chat, the rows named, or all of them', () => {
    expect(unreadIn(rows, { chat: 'chat-c' }).map((each) => each.id)).toEqual(['id-003']);
    expect(unreadIn(rows, { ids: ['id-002', 'id-004'] }).map((each) => each.id)).toEqual(['id-002']);
    expect(unreadIn(rows, { all: true })).toHaveLength(3);
  });

  it('is said to the server as the member it stands for; "all" counts up to now or to the newest row held, whichever is later', () => {
    expect(bodyOf({ ids: ['a'] }, null, 0)).toEqual({ ids: ['a'] });
    expect(bodyOf({ place: 'p' }, null, 0)).toEqual({ node_id: 'p' });
    expect(bodyOf({ chat: 'c' }, null, 0)).toEqual({ chat_id: 'c' });
    const now = Date.UTC(2026, 9, 1, 12, 0);
    expect(bodyOf({ all: true }, null, now)).toEqual({ before: '2026-10-01T12:00:00.000Z' });
    // A machine whose clock is behind the server's must not leave out the newest row it holds.
    expect(bodyOf({ all: true }, '2026-10-01T12:05:00.000Z', now)).toEqual({ before: '2026-10-01T12:05:00.000Z' });
  });
});

describe('the days of the Inbox', () => {
  it('are the person’s days: the same moment falls on different days in different zones', () => {
    const late = '2026-10-01T17:30:00.000Z';
    expect(dayKey(late, 'UTC')).toBe('2026-10-01');
    expect(dayKey(late, 'Asia/Shanghai')).toBe('2026-10-02');
    expect(dayKey(late, 'America/Los_Angeles')).toBe('2026-10-01');
  });

  it('is told apart from yesterday by the calendar, across a month and a year', () => {
    expect(previousDay('2026-03-01')).toBe('2026-02-28');
    expect(previousDay('2026-01-01')).toBe('2025-12-31');
    expect(previousDay('2024-03-01')).toBe('2024-02-29');
  });

  it('group the rows, newest first, at the midnight of the person', () => {
    const rows = [row(30, { created_at: '2026-10-02T00:30:00.000Z' }), row(20, { created_at: '2026-10-01T23:30:00.000Z' }), row(10, { created_at: '2026-10-01T10:00:00.000Z' })];
    expect(groupByDay(rows, 'UTC').map((group) => [group.key, group.rows.length])).toEqual([
      ['2026-10-02', 1],
      ['2026-10-01', 2],
    ]);
    // In Shanghai the first two are on the 2nd and the third, at six in the evening, on the 1st.
    expect(groupByDay(rows, 'Asia/Shanghai').map((group) => [group.key, group.rows.length])).toEqual([
      ['2026-10-02', 2],
      ['2026-10-01', 1],
    ]);
  });

  it('are written with the year only when it is not the present one', () => {
    expect(writeDay('2026-10-01', '2026-10-12', 'en-US')).toMatch(/October/);
    expect(writeDay('2026-10-01', '2026-10-12', 'en-US')).not.toMatch(/2026/);
    expect(writeDay('2025-12-31', '2026-10-12', 'en-US')).toMatch(/2025/);
  });
});

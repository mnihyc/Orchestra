/** Recent calendar boundaries and predicates agree for the bar and the wide table. */
import { expect, it } from 'vitest';
import { groupedRecents, navigationFilter, recentAt, recentGroup } from '@/features/navigation/recent';
import { DEFAULT_VIEW, everythingSearch, initialView, validView, viewFromAddress } from '@/features/everything/view';
import { writeViewChoice } from '@/lib/viewChoices';

it('groups by the reader’s calendar across midnight, daylight saving and the preceding week', () => {
  const now = Date.parse('2026-11-02T05:30:00Z');
  const zone = 'America/New_York';
  expect(recentGroup('2026-11-02T05:01:00Z', now, zone)).toBe('today');
  expect(recentGroup('2026-11-02T04:30:00Z', now, zone)).toBe('yesterday');
  expect(recentGroup('2026-10-31T12:00:00Z', now, zone)).toBe('week');
  expect(recentGroup('2026-10-26T12:00:00Z', now, zone)).toBe('week');
  expect(recentGroup('2026-10-25T12:00:00Z', now, zone)).toBe('earlier');
  expect(recentGroup(null, now, zone)).toBe('earlier');
  expect(recentGroup('invalid', now, zone)).toBe('earlier');
});

it('orders Opened by personal visits and Active by activity without mixing the clocks', () => {
  const row = { last_opened: '2026-10-25T12:00:00Z', last_activity: '2026-10-27T12:00:00Z' };
  expect(recentAt(row, 'opened')).toBe(row.last_opened);
  expect(recentAt(row, 'active')).toBe(row.last_activity);
  expect(recentAt({ last_activity: row.last_activity }, 'opened')).toBeNull();
});

it('keeps calendar groups together while preserving the chosen column order inside each group', () => {
  const now = Date.parse('2026-10-27T12:00:00Z');
  const rows = [
    { name: 'A', last_activity: '2026-10-24T09:00:00Z' },
    { name: 'B', last_activity: '2026-10-27T09:00:00Z' },
    { name: 'C', last_activity: '2026-10-24T08:00:00Z' },
    { name: 'D', last_activity: '2026-10-26T09:00:00Z' },
  ];
  expect(groupedRecents(rows, 'active', now, 'UTC').map((row) => row.name)).toEqual(['B', 'D', 'A', 'C']);
  expect(rows.map((row) => row.name)).toEqual(['A', 'B', 'C', 'D']);
});

it('sends the same kinds and done/archive/focus predicates before the server page limit', () => {
  const filter = navigationFilter('active', 'notes', { threads: false, documents: true, chats: true }, false, true, true, 'current-thread');
  expect(filter).toMatchObject({ kinds: ['org', 'project', 'topic', 'document', 'chat'], recent: 'active', sort: 'last_activity', direction: 'desc', navigation: true, showArchived: false, hideDone: true, participating: true, keep: 'current-thread', q: 'notes' });
  expect(navigationFilter('opened', '', { threads: true, documents: false, chats: false }, true, false, false).kinds).toEqual(['org', 'project', 'topic', 'thread']);
});

it('keeps each wide list’s previous view while an explicit link begins from defaults', () => {
  const saved = { ...DEFAULT_VIEW, arrangement: 'recent', order: 'active', sort: 'last_activity', direction: 'desc', kinds: ['document'], hideDone: true };
  writeViewChoice('reader', 'everything', 'view', JSON.stringify(saved), 'mine:one');
  expect(initialView('reader', 'mine:one')).toMatchObject(saved);
  expect(initialView('reader', 'mine:two')).toEqual(DEFAULT_VIEW);
  expect(initialView('another', 'mine:one')).toEqual(DEFAULT_VIEW);
  const address = everythingSearch({ parent: 'one', arrangement: 'recent', order: 'opened', kinds: 'thread,document', navigation: 'true', showArchived: 'false' });
  expect(initialView('reader', 'mine:one', viewFromAddress(address))).toMatchObject({ arrangement: 'recent', order: 'opened', sort: 'opened', direction: 'desc', hideDone: false, navigation: true, showArchived: false, kinds: ['thread', 'document'] });
  expect(validView({ arrangement: 'retired', order: 'retired', sort: 'retired' })).toEqual(DEFAULT_VIEW);
});

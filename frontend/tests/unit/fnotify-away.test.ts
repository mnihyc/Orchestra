/** Explicit zoned pauses, current-device readiness, and server-supplied Inbox priority/page counts. */
import { describe, expect, it } from 'vitest';

import { mergePlacePages, placeUrl } from '@/features/inbox/away/model';
import { deviceState, type DeviceFacts } from '@/features/settings/personal/away/device';
import { activePause, futureInZone } from '@/features/settings/personal/away/time';
import type { AwayNotification, NotificationGroup, PushDevice } from '@/features/settings/personal/away/types';

const now = Date.parse('2026-01-01T00:00:00Z');
const facts: DeviceFacts = { secure: true, worker: true, push: true, notifications: true, permission: 'granted' };
const device: PushDevice = { id: 'mine', label: 'Own browser', endpoint_hash: 'own-hash', last_ok_at: null };
const group = (key: string, unread: number, waiting = 0): NotificationGroup => ({ key, place: null, unread, waiting_for_me: waiting, through_seq: 100 });
const row = (id: string, metadata: NotificationGroup, more: Partial<AwayNotification> = {}): AwayNotification => ({ id, kind: 'turn.completed', title: id, body: 'An answer that is not an Inbox excerpt', node: null, thread_id: null, approval_id: null, chat_id: null, url: '/inbox', data: {}, created_at: '2026-10-14T00:00:00Z', read_at: null, inbox_seq: 1, group: metadata, waiting_for_me: false, ...more });

describe('an explicit pause in the shown zone', () => {
  it('shows only a valid future pause, including the exact expiry boundary', () => {
    expect(activePause(null, now)).toBeNull();
    expect(activePause('invalid old time', now)).toBeNull();
    expect(activePause('2026-01-01T00:00:00Z', now)).toBeNull();
    expect(activePause('2025-12-31T23:59:59Z', now)).toBeNull();
    expect(activePause('2026-01-01T00:00:01Z', now)).toBe('2026-01-01T00:00:01Z');
  });
  it('asks for both fields without guessing a time', () => expect(futureInZone('', '', 'UTC', now)).toEqual({ error: 'missing' }));
  it('converts a positive and a negative offset to the selected UTC instant', () => {
    expect(futureInZone('2026-10-15', '12:30', 'Asia/Singapore', now)).toEqual({ instant: '2026-10-15T04:30:00.000Z' });
    expect(futureInZone('2026-10-15', '12:30', 'America/New_York', now)).toEqual({ instant: '2026-10-15T16:30:00.000Z' });
  });
  it('refuses a past instant and an invalid calendar date', () => {
    expect(futureInZone('2026-01-01', '00:00', 'UTC', now)).toEqual({ error: 'future' });
    expect(futureInZone('2026-02-30', '12:00', 'UTC', now)).toEqual({ error: 'invalid' });
  });
  it('refuses a missing or repeated clock hour rather than picking either side for the person', () => {
    expect(futureInZone('2026-03-29', '01:30', 'Europe/London', now)).toEqual({ error: 'invalid' });
    expect(futureInZone('2026-10-25', '01:30', 'Europe/London', now)).toEqual({ error: 'ambiguous' });
  });
});

describe('this device has its own readiness', () => {
  it('never treats another device as this one', () => {
    expect(deviceState(facts, 'local-hash', [device])).toBe('missing');
    expect(deviceState(facts, 'own-hash', [device])).toBe('registered');
  });
  it('keeps permission, context and registration distinct', () => {
    expect(deviceState({ ...facts, secure: false }, 'own-hash', [device])).toBe('insecure');
    expect(deviceState({ ...facts, push: false }, 'own-hash', [device])).toBe('unavailable');
    expect(deviceState({ ...facts, permission: 'denied' }, 'own-hash', [device])).toBe('blocked');
    expect(deviceState({ ...facts, permission: 'default' }, 'own-hash', [device])).toBe('default');
  });
});

describe('current readable place groups', () => {
  it('merges paged ids once and keeps the supplied count once', () => {
    const metadata = group('project', 3);
    const groups = mergePlacePages([{ items: [row('one', metadata), row('two', metadata)], next_cursor: 'next' }, { items: [row('two', metadata), row('three', metadata)], next_cursor: null }]);
    expect(groups).toHaveLength(1);
    expect(groups[0]?.rows).toHaveLength(3);
    expect(groups[0]?.unread).toBe(3);
  });
  it('puts only current supplied actionability first, not a notification kind', () => {
    const groups = mergePlacePages([{ items: [row('old-request', group('old', 1), { kind: 'approval.requested' }), row('can-answer', group('action', 1, 1), { waiting_for_me: true, created_at: '2026-10-13T00:00:00Z' })], next_cursor: null }]);
    expect(groups.map((one) => one.key)).toEqual(['action', 'old']);
  });
  it('a current resync removes old ids and never constructs hidden headings', () => {
    const fresh = mergePlacePages([{ items: [row('visible', group('current', 1)), row('without-group', group('ignored', 1), { group: null })], next_cursor: null }]);
    expect(fresh.map((one) => one.key)).toEqual(['current']);
    expect(placeUrl({ id: 'hidden', kind: 'project', name: null, redacted: true })).toBeNull();
    expect(placeUrl({ id: 'web', kind: 'project', name: 'Web', redacted: false })).toBe('/project/web');
  });
});

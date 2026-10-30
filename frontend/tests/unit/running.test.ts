/** A reviewed native set keeps later work out, and unavailable or incomplete facts never promise End all. */
import { describe, expect, it, vi } from 'vitest';
import type { BackgroundCommand, ThreadBackground } from '@/generated/api-types';
import { fetchBackground } from '@/api/background';
import { canEndAll, endResults, loadReview, reviewedRequest } from '@/features/running/review';
vi.mock('@/api/background', () => ({ fetchBackground: vi.fn() }));
const command = (id: string): BackgroundCommand => ({ id, segment_id: 'segment', command: 'sleep 300', command_truncated: false, cwd: null, cwd_redacted: false, place: 'environment', since: null, turn_id: null, item_id: null, cpu_percent: null, memory_bytes: null });
const roster = (more: Partial<ThreadBackground> = {}): ThreadBackground => ({ supported: true, as_of: null, state: 'live', reason: null, items: [command('one')], next_cursor: null, total: 1, list_ref: 'review', expires_at: '2026-10-14T10:00:30Z', can_end: true, ...more });

describe('reviewing native commands', () => {
  it('captures the exact reviewed set before a later command begins', () => {
    const seen = roster(); const request = reviewedRequest(seen, 'all');
    seen.items.push(command('later')); seen.total = 2;
    expect(request).toEqual({ list_ref: 'review', all: true, count: 1 });
    expect(reviewedRequest(roster(), ['another'])).toBeNull();
    expect(reviewedRequest(roster(), ['one', 'one'])).toBeNull();
  });
  it('offers no All for unavailable, View-only or partial facts', () => {
    expect(canEndAll(roster({ state: 'offline', total: null }))).toBe(false);
    expect(canEndAll(roster({ supported: false, state: 'unavailable', total: null, can_end: false }))).toBe(false);
    expect(canEndAll(roster({ can_end: false }))).toBe(false);
    expect(canEndAll(roster({ total: null }))).toBe(false);
    expect(canEndAll(roster({ total: 250, next_cursor: 'page' }))).toBe(false);
  });
  it('reads every page and keeps the final idle deadline before confirming more than two hundred', async () => {
    const items = Array.from({ length: 250 }, (_, index) => command(String(index)));
    vi.mocked(fetchBackground).mockResolvedValueOnce(roster({ items: items.slice(0, 50), total: 250, next_cursor: 'next' })).mockResolvedValueOnce(roster({ items: items.slice(50), total: 250, expires_at: '2026-10-14T10:01:00Z' }));
    const reviewed = await loadReview('thread', new AbortController().signal);
    expect(reviewed.items).toHaveLength(250);
    expect(reviewed.expires_at).toBe('2026-10-14T10:01:00Z');
    expect(reviewedRequest(reviewed, 'all')).toEqual({ list_ref: 'review', all: true, count: 250 });
    expect(reviewedRequest(reviewed, items.map((item) => item.id))).toBeNull();
  });
  it('shows an already-ended or unknown target from a job without treating its result as a live roster', () => {
    const results = [{ id: 'one', state: 'already_ended', reason: null, item_id: null }, { id: 'two', state: 'unknown', reason: 'permission lost', item_id: null }];
    expect(endResults({ results })).toEqual(results);
    expect(endResults(roster())).toEqual([]);
  });
});

/** A reviewed native set keeps later work out, and unavailable or incomplete facts never promise End all. */
import { describe, expect, it, vi } from 'vitest';
import type { BackgroundCommand, ThreadBackground } from '@/generated/api-types';
import { backgroundWake, backgroundWork } from '../doubles/background-wake';
import { ApiError } from '@/api/client';
import { fetchBackground } from '@/api/background';
import { canEndAll, endResults, loadReview, reviewedRequest, reviewedTargetRequest } from '@/features/running/review';
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
  it('keeps the native end reason through paging and refuses targets it closes', async () => {
    vi.mocked(fetchBackground).mockReset();
    vi.mocked(fetchBackground).mockResolvedValueOnce(roster({ total: 2, next_cursor: 'next' }))
      .mockResolvedValueOnce(roster({ total: 2, items: [command('two')], can_end: false, end_reason: 'interact_required' }));
    const review = await loadReview('thread', new AbortController().signal);
    expect(review.end_reason).toBe('interact_required');
    expect(reviewedRequest(review, ['one'])).toBeNull();
    const mixed = roster({ total: 2, items: [{ ...command('one'), end_reason: 'no_current_target' }, command('two')] });
    expect(canEndAll(mixed)).toBe(false);
    expect(reviewedRequest(mixed, ['one'])).toBeNull();
    expect(reviewedRequest(mixed, ['two'])).toEqual({ list_ref: 'review', command_ids: ['two'] });
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

it('maps a recorded task to only the fresh review end identity, not its recorded id', () => {
  const task = backgroundWork({ end_id: null });
  const review = roster({ items: [command('fresh-end')], wake: backgroundWake({ tasks: [{ ...task, end_id: 'fresh-end' }] }) });
  expect(reviewedTargetRequest(review, { task })).toEqual({ list_ref: 'review', command_ids: ['fresh-end'] });
  for (const changed of [backgroundWork({ end_id: 'fresh-end', item_id: 'another-item' }), backgroundWork({ end_id: 'fresh-end', turn_id: 'another-turn' }), backgroundWork({ end_id: 'fresh-end', state: 'lost' }), backgroundWork({ end_id: null }), backgroundWork({ end_id: 'fresh-end', can_end: false })]) {
    expect(reviewedTargetRequest({ ...review, wake: backgroundWake({ tasks: [changed] }) }, { task })).toBeNull();
  }
  expect(reviewedTargetRequest({ ...review, can_end: false }, { task })).toBeNull();
});

it('keeps reviewed task end identities from every page of the same captured list', async () => {
  vi.mocked(fetchBackground).mockReset();
  const task = backgroundWork();
  vi.mocked(fetchBackground).mockResolvedValueOnce(roster({ items: [command('one')], total: 2, next_cursor: 'second', wake: backgroundWake({ tasks: [task] }) }))
    .mockResolvedValueOnce(roster({ items: [command('fresh-end')], total: 2, wake: backgroundWake({ tasks: [{ ...task, end_id: 'fresh-end' }] }) }));
  const review = await loadReview('thread', new AbortController().signal);
  expect(reviewedTargetRequest(review, { task })).toEqual({ list_ref: 'review', command_ids: ['fresh-end'] });
  expect(review.items.map((item) => item.id)).toEqual(['one', 'fresh-end']);
});

it('refuses a task source identity change inside one native review transfer', async () => {
  vi.mocked(fetchBackground).mockReset();
  vi.mocked(fetchBackground).mockResolvedValueOnce(roster({ total: 2, next_cursor: 'second', wake: backgroundWake({ tasks: [backgroundWork({ end_id: 'one' })] }) }))
    .mockResolvedValueOnce(roster({ items: [command('two')], total: 2, wake: backgroundWake({ tasks: [backgroundWork({ item_id: 'later-source', end_id: 'two' })] }) }));
  await expect(loadReview('thread', new AbortController().signal)).rejects.toThrow('the native task changed during transfer');
});

it('renews an expired review during transfer and confirms only the complete replacement set', async () => {
  vi.mocked(fetchBackground).mockReset();
  vi.mocked(fetchBackground).mockResolvedValueOnce(roster({ items: [command('old')], total: 2, next_cursor: 'expired' }))
    .mockRejectedValueOnce(new ApiError({ type: 'about:blank', title: 'Expired', detail: 'The snapshot expired', request_id: 'fixture', status: 409, code: 'conflict.stale_revision' }, null))
    .mockResolvedValueOnce(roster({ list_ref: 'fresh', items: [command('fresh-one')], total: 2, next_cursor: 'fresh-next' }))
    .mockResolvedValueOnce(roster({ list_ref: 'fresh', items: [command('fresh-two')], total: 2 }));
  const review = await loadReview('thread', new AbortController().signal);
  expect(review.items.map((item) => item.id)).toEqual(['fresh-one', 'fresh-two']);
  expect(reviewedRequest(review, 'all')).toEqual({ list_ref: 'fresh', all: true, count: 2 });
});

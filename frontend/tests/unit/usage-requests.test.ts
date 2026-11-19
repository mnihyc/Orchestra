/** Live request logs preserve the reader's position and apply every metadata filter. */
import { describe, expect, it } from 'vitest';
import { emptyLog, matchesRequest, newestRequests, requestLog } from '@/features/usage/requests';
import { filtersOf, patchSearch, readSearch } from '@/features/usage/search';
import { filterValue, isRest, keyLabel } from '@/features/usage/keys';
import { i18n } from '@/i18n';
import { requestRow } from '../doubles/request-row';

describe('a paused request log', () => {
  it('keeps visible rows and their order while new arrivals wait, then inserts them newest first', () => {
    const old = requestRow();
    const newer = requestRow({ id: 'new', at: '2026-10-15T04:01:00.000Z' });
    const original = requestLog(emptyLog(), { type: 'history', rows: [old] });
    let state = requestLog(original, { type: 'pause' });
    state = requestLog(state, { type: 'live', rows: [old, newer] });
    expect(state.rows).toBe(original.rows);
    expect(newestRequests(state.rows).map((row) => row.id)).toEqual([old.id]);
    expect([...state.waiting.keys()].filter((id) => !state.rows.has(id))).toEqual(['new']);
    state = requestLog(state, { type: 'resume' });
    expect(newestRequests(state.rows).map((row) => row.id)).toEqual(['new', old.id]);
    expect(state.waiting.size).toBe(0);
  });

  it('replaces a running row on resume and never resurrects it from an older snapshot', () => {
    const running = requestRow({ running: true });
    let state = requestLog(emptyLog(), { type: 'history', rows: [running] });
    state = requestLog(state, { type: 'pause' });
    state = requestLog(state, { type: 'live', rows: [requestRow({ running: false })] });
    expect(state.rows.get(running.id)?.running).toBe(true);
    state = requestLog(state, { type: 'resume' });
    state = requestLog(state, { type: 'live', rows: [running] });
    expect(state.rows.get(running.id)?.running).toBe(false);
  });

  it('queues a refreshed first page while an explicitly loaded older page may extend the bottom', () => {
    const row = requestRow();
    let state = requestLog(emptyLog(), { type: 'history', rows: [row] });
    state = requestLog(state, { type: 'pause' });
    state = requestLog(state, { type: 'history', rows: [row, requestRow({ id: 'new', at: '2026-10-15T05:00:00.000Z' })] });
    expect(state.rows.size).toBe(1);
    expect(state.waiting.has('new')).toBe(true);
    state = requestLog(state, { type: 'history', rows: [requestRow({ id: 'old', at: '2026-10-14T04:00:00.000Z' })] });
    expect(state.rows.has('old')).toBe(true);
    expect(state.rows.has('new')).toBe(false);
  });
});

it('filters requests by person, place, provider, harness, model, effort, tier, programme and running outcome', () => {
  const row = requestRow({ running: true });
  expect(matchesRequest(row, { user_id: row.user?.id, project_id: row.project.id, topic_id: row.topic.id, thread_id: row.thread?.id, provider_id: row.provider.id, harness_kind: 'codex', environment_kind: 'managed', model: 'model-a', tier: 'fast', effort: 'high', program: 'research', outcome: 'running' })).toBe(true);
  for (const filters of [{ model: 'model-b' }, { program: 'none' }, { tier: 'none' }, { effort: 'low' }, { provider_id: 'other' }, { user_id: 'other' }, { outcome: 'done' }] as const) expect(matchesRequest(row, filters)).toBe(false);
});

it('retains view and programme filters in a shared usage address and distinguishes no programme', () => {
  const search = readSearch({ view: 'requests', program: 'research', effort: 'high', outcome: 'running' });
  expect(filtersOf(search)).toMatchObject({ program: 'research', effort: 'high', outcome: 'running' });
  expect(patchSearch(search, { view: 'performance' }).program).toBe('research');
  expect(isRest({ program: null })).toBe(false);
  expect(filterValue('program', { program: null })).toBe('none');
  expect(keyLabel('program', { program: null }, i18n.t)).toBe('No programme');
});

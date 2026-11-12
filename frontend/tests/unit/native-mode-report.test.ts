/** Streamed text cannot make the composer revisit history or hide a corrected current native report. */
import { expect, it, vi } from 'vitest';
import { reportedMode } from '@/features/composer/nativeModes';
import type { Item } from '@/generated/api-types';
import { marker, reasoning } from '../doubles/thread-rows';

function report(id: string, value: string, segment = 'session'): Item {
  return { ...marker(id, { marker: 'harness_changed', data: { program: 'Codex', change: 'sandbox_mode', previous: { state: 'unsupported' },
    value: { type: value }, expected: { state: 'told', value: { type: 'workspaceWrite' } }, changed: true, diverged: value !== 'workspaceWrite', next_turn: true, turn_id: null, reported_at: '2026-11-07T20:00:00Z' } }), segment_id: segment };
}
it('indexes a structural order once, reads corrected values live and does not visit history on streaming', () => {
  const rows = Array.from({ length: 1_000 }, (_, index) => reasoning(`old-${index}`, 'Earlier thinking'));
  const native = report('current-report', 'readOnly'); rows.push(native);
  const order = rows.map((item) => item.id); const items = new Map(rows.map((item) => [item.id, item]));
  expect(reportedMode({ order, items }, 'session', 'sandbox')?.data.value).toEqual({ type: 'readOnly' });
  const changed = new Map(items); changed.set(native.id, report(native.id, 'workspaceWrite'));
  changed.set('old-999', reasoning('old-999', 'Earlier thinking with more streamed text'));
  const read = vi.spyOn(changed, 'get');
  expect(reportedMode({ order, items: changed }, 'session', 'sandbox')?.data.diverged).toBe(false);
  expect(read).toHaveBeenCalledExactlyOnceWith(native.id);
  read.mockClear(); expect(reportedMode({ order, items: changed }, 'session', 'approval')).toBeNull(); expect(read).not.toHaveBeenCalled();
  expect(reportedMode({ order, items: changed }, 'another-session', 'sandbox')).toBeNull();
});
it('reindexes an admitted later observation while excluding a child stream and another session', () => {
  const first = report('first', 'readOnly'); const later = report('later', 'workspaceWrite');
  const child = { ...report('child', 'dangerFullAccess'), stream: 'sub:child' }; const oldSession = report('old-session', 'dangerFullAccess', 'old-session');
  const rows = [first, later, child, oldSession]; const items = new Map(rows.map((item) => [item.id, item]));
  expect(reportedMode({ order: [first.id], items }, 'session', 'sandbox')?.data.diverged).toBe(true);
  expect(reportedMode({ order: rows.map((item) => item.id), items }, 'session', 'sandbox')?.data.value).toEqual({ type: 'workspaceWrite' });
});

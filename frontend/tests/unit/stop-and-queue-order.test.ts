/** A message put first goes before the one put first earlier, however the places are written, and a thread that is being stopped says since when. */
import { expect, it } from 'vitest';
import type { Thread, Turn } from '@/generated/api-types';
import { stoppingSince as ofTurn } from '@/features/composer/StopControl';
import { stoppingSince, stoppingWords } from '@/features/timeline/waiting';
import { i18n } from '@/i18n';
import { comparePositions } from '@/realtime/stores/thread';
import { failedStart } from '../component/failed-start-fixtures';

const t = i18n.t.bind(i18n);

it('puts a place below zero before every place above it, whatever its length', () => {
  const places = ['2', '0', '-1', '10', '-12', '1', '-2'];
  expect([...places].sort(comparePositions)).toEqual(['-12', '-2', '-1', '0', '1', '2', '10']);
  expect(comparePositions('-1', '1')).toBeLessThan(0);
  expect(comparePositions('-1', '-1')).toBe(0);
  expect(comparePositions('-0.5', '0.5')).toBeLessThan(0);
  expect(comparePositions('-2.5', '-2.25')).toBeLessThan(0);
});

it('says since when a thread is being stopped, from the thread when it names it and from the turn when the turn carries the stop', () => {
  const thread: Thread = { ...failedStart().thread, stopping_since: '2026-11-14T10:00:00.000Z' };
  expect(stoppingSince(thread)).toBe('2026-11-14T10:00:00.000Z');
  expect(stoppingWords(thread, t)).toBe(t('thread.stopping.line'));
  const quiet: Thread = { ...thread, stopping_since: null };
  expect(stoppingSince(quiet)).toBeNull();
  expect(stoppingWords(quiet, t)).toBeNull();
  expect(stoppingWords(null, t)).toBeNull();
  const stopped = { interrupted_by: { id: 'u', name: 'Ada' }, interrupted_at: '2026-11-14T10:00:05.000Z' } as unknown as Turn;
  expect(ofTurn(stopped, '2026-11-14T10:00:00.000Z')).toBe('2026-11-14T10:00:05.000Z');
  expect(ofTurn({ ...stopped, interrupted_by: null }, null)).toBeNull();
  expect(ofTurn(null, '2026-11-14T10:00:00.000Z')).toBe('2026-11-14T10:00:00.000Z');
});

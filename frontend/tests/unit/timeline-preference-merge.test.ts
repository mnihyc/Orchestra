/** Refused ways of reading must not roll back link-sharing consent or another thread's newer preference. */
import { expect, it } from 'vitest';
import { changedTimeline, rollbackTimeline } from '@/features/timeline/preferences';

it('restores a failed global choice while preserving a newer link opt-out and opaque fields', () => {
  const before = { work_detail: 'normal', share_on_link: true, opaque: { retained: true } };
  const change = { value: { work_detail: 'verbose' as const } };
  const current = { ...changedTimeline(before, change), share_on_link: false, later: 'kept' };
  expect(rollbackTimeline(current, before, change)).toEqual({ ...before, share_on_link: false, later: 'kept' });
});

it('restores only the failed thread fields and keeps other thread and namespace changes', () => {
  const before = { share_on_link: false, threads: { a: { work_detail: 'normal', extra: 'mine' }, b: { work_detail: 'normal' } } };
  const change = { thread: 'a', value: { work_detail: 'verbose' as const } };
  const current = changedTimeline(changedTimeline(before, change), { thread: 'b', value: { work_detail: 'diagnostics' } });
  expect(rollbackTimeline(current, before, change)).toEqual({ share_on_link: false, threads: { a: { work_detail: 'normal', extra: 'mine' }, b: { work_detail: 'diagnostics' } } });
});

it('keeps a newer selection of the same field instead of undoing it', () => {
  const before = { threads: { a: { work_detail: 'normal' } } };
  const change = { thread: 'a', value: { work_detail: 'verbose' as const } };
  const current = changedTimeline(before, { thread: 'a', value: { work_detail: 'diagnostics' } });
  expect(rollbackTimeline(current, before, change)).toEqual(current);
});

it('retires a failed new thread override without leaving a false own value', () => {
  const before = { share_on_link: false, extra: 'kept' };
  const change = { thread: 'a', value: { work_detail: 'verbose' as const } };
  expect(rollbackTimeline(changedTimeline(before, change), before, change)).toEqual(before);
});

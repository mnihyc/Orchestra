/** Refused detail choices must not roll back link-sharing consent or another thread's newer preference. */
import { expect, it } from 'vitest';
import { changedTimeline, rollbackTimeline } from '@/features/timeline/preferences';

it('restores a failed global choice while preserving a newer link opt-out and opaque fields', () => {
  const before = { work_detail: 'steps', share_on_link: true, opaque: { retained: true } };
  const change = { value: { work_detail: 'summary' as const } };
  const current = { ...changedTimeline(before, change), share_on_link: false, later: 'kept' };
  expect(rollbackTimeline(current, before, change)).toEqual({ ...before, share_on_link: false, later: 'kept' });
});

it('restores only the failed thread fields and keeps other thread and namespace changes', () => {
  const before = { share_on_link: false, threads: { a: { work_detail: 'steps', extra: 'mine' }, b: { live_tools: false } } };
  const change = { thread: 'a', value: { work_detail: 'summary' as const, live_tools: false } };
  const current = changedTimeline(changedTimeline(before, change), { thread: 'b', value: { live_tools: true } });
  expect(rollbackTimeline(current, before, change)).toEqual({ share_on_link: false, threads: { a: { work_detail: 'steps', extra: 'mine' }, b: { live_tools: true } } });
});

it('keeps a newer selection of the same field instead of undoing it', () => {
  const before = { threads: { a: { work_detail: 'steps' } } };
  const change = { thread: 'a', value: { work_detail: 'summary' as const } };
  const current = changedTimeline(before, { thread: 'a', value: { work_detail: 'everything' } });
  expect(rollbackTimeline(current, before, change)).toEqual(current);
});

it('retires a failed new thread override without leaving a false own value', () => {
  const before = { share_on_link: false, extra: 'kept' };
  const change = { thread: 'a', value: { work_detail: 'summary' as const, live_tools: false } };
  expect(rollbackTimeline(changedTimeline(before, change), before, change)).toEqual(before);
});

/** A start that waits says what it waits for and what holds it up, tells when the step began, and a source that could not be prepared says why and what to do. */
import { expect, it } from 'vitest';
import type { Thread } from '@/generated/api-types';
import { waitingSince, waitingWords } from '@/features/timeline/waiting';
import { i18n, setLocale } from '@/i18n';
import { failedStart } from '../component/failed-start-fixtures';

const t = i18n.t.bind(i18n);

function starting(change: Partial<Thread>): Thread {
  return { ...failedStart().thread, state: 'starting', blocked: null, lock: null, starting_step: 'source', starting_since: null, starting_work: null, ...change };
}

it('names the recording of the files as its own step', () => {
  const thread = starting({ starting_step: 'recording' });
  expect(waitingWords(thread, t)).toBe(t('thread.starting.recording'));
  expect(waitingWords(thread, t)).not.toBe(t('thread.starting.files'));
});

it('says which command a step waits for once the server names it, and when the step began', () => {
  const thread = starting({ starting_work: { command: 'git.prepare', behind: null }, starting_since: '2026-11-14T10:00:00.000Z' });
  expect(waitingWords(thread, t)).toBe(t('thread.starting.work.gitPrepare'));
  expect(waitingSince(thread)).toBe('2026-11-14T10:00:00.000Z');
});

it('says what holds a command up when other work stands in the line of its environment', () => {
  const thread = starting({ starting_work: { command: 'fs.mkdir', behind: 'fs.record' } });
  expect(waitingWords(thread, t)).toBe(t('thread.starting.behind', { what: t('thread.starting.work.fsMkdir'), other: t('thread.starting.other.fsRecord') }));
  const unknown = starting({ starting_work: { command: 'fs.mkdir', behind: 'future.kind' } });
  expect(waitingWords(unknown, t)).toContain(t('thread.starting.other.work'));
});

it('has no time to show for a thread that is not starting or waits for nothing in particular', () => {
  expect(waitingSince(null)).toBeNull();
  expect(waitingSince(starting({}))).toBeNull();
  expect(waitingSince({ ...starting({ starting_since: '2026-11-14T10:00:00.000Z' }), state: 'idle' })).toBeNull();
});

it.each(['en', 'zh-CN'] as const)('says why a source could not be prepared in time and what to do, in %s', async (locale) => {
  await setLocale(locale);
  try {
    const failed: Thread = { ...failedStart().thread, state: 'blocked', starting_step: null,
      blocked: { reason: 'error', detail: 'Source preparation failed (timeout). Choose Prepare again to retry.', sandbox: null } };
    const words = waitingWords(failed, i18n.t.bind(i18n)) ?? '';
    expect(words).toContain(i18n.t('source.prepare.reasons.timeout'));
    expect(words).not.toContain('Source preparation failed');
    if (locale === 'zh-CN') expect(words).toMatch(/\p{Script=Han}/u);
  } finally { await setLocale('en'); }
});

/** Live trees explain why, and bounded Git reads point the person to their shell. */
import { describe, expect, it } from 'vitest';

import { liveIndexReasonKey } from '@/features/inspector/Files';
import { gitReadTooSlow } from '@/features/inspector/Git';
import { indexingProgress, liveIndexReason } from '@/features/files/indexWords';
import { recordingIndexingWords, recordingReason } from '@/features/source/RecordingNotice';
import { turnChanges, turnFilesLabel } from '@/features/timeline/TurnFiles';
import { wholeUnavailableReason } from '@/features/source/words';
import { i18n, setLocale } from '@/i18n';
import { change, fileChange } from '../doubles/thread-rows';

describe('a large workspace', () => {
  it('says the reported reason for live listing and explains explicit selections', () => {
    for (const reason of ['chosen', 'entries', 'watches', 'scan_time'] as const) {
      const words = i18n.t(liveIndexReasonKey(reason));
      expect(words).not.toBe(liveIndexReasonKey(reason));
      expect(words).toMatch(/live|index|watch|Scanning/);
    }
    expect(i18n.t(liveIndexReasonKey(null))).toContain('has not reported why');
    expect(i18n.t('largeTree.mountSuggestion')).toContain('Choose explicit paths');
  });

  it('says how far a pass over a whole tree has come, only while there is one', () => {
    expect(indexingProgress({ indexing: { done: 80_000, total: 200_000 } })).toEqual({ total: 200_000, percent: 40 });
    expect(indexingProgress({ indexing: { done: 300, total: 200 } })).toEqual({ total: 200, percent: 100 });
    for (const facts of [{ indexing: { done: 5, total: 0 } }, { indexing: 'now' }, {}, null, undefined]) expect(indexingProgress(facts)).toBeNull();
    expect(i18n.t('largeTree.indexing', { files: '200,000', percent: 40 })).toBe('Indexing 200,000 files, 40 %');
  });

  it('offers a shell when a bounded Git read runs out and does not confuse other errors with that', () => {
    expect(gitReadTooSlow(new Error("git status is too slow here; use the environment's shell"))).toBe(true);
    expect(gitReadTooSlow(new Error('git status is too large; use the shell'))).toBe(true);
    expect(gitReadTooSlow(new Error('the connector is offline'))).toBe(false);
    expect(i18n.t('largeTree.gitCut')).toContain('cut at its output limit');
    expect(i18n.t('largeTree.openShell')).toBe('Open shell');
  });
});

it.each(['en', 'zh-CN'] as const)('explains every Live cause, its reported limit and restore cost in %s', async (locale) => {
  await setLocale(locale);
  try {
    for (const reason of ['chosen', 'entries', 'watches', 'scan_time'] as const) {
      const words = liveIndexReason(i18n.t, reason, { index_entry_limit: 12345 });
      expect(words).not.toBe(`largeTree.reasons.${reason}`);
      if (reason === 'entries') expect(words).toContain('12345');
      if (locale === 'zh-CN') expect(words).toMatch(/\p{Script=Han}/u);
    }
    expect(liveIndexReason(i18n.t, 'entries', {})).toBe(i18n.t('largeTree.reasons.entries'));
    expect(liveIndexReason(i18n.t, null, {})).toBe(i18n.t('largeTree.reasons.unknown'));
    expect(wholeUnavailableReason(i18n.t, 'entries:12345')).toContain('12345');
    expect(i18n.t('largeTree.liveCost')).not.toBe('largeTree.liveCost');
    if (locale === 'en') expect(i18n.t('largeTree.liveCost')).toMatch(/disk.*requested.*Whole-directory restore/);
    else expect(i18n.t('largeTree.liveCost')).toMatch(/磁盘.*无法还原整个目录/u);
  } finally { await setLocale('en'); }
});

it('lists journal paths even when no file tool supplied a patch', () => {
  const changes = turnChanges(new Map(), [], 'large-turn', ['shell.txt', 'code.txt', 'shell.txt']);
  expect(changes.files.map(({ change }) => change.path)).toEqual(['shell.txt', 'code.txt']);
  expect(changes.partial).toBe(true);
});

it.each(['en', 'zh-CN'] as const)('explains large recording and Git ignore coverage in one line in %s', async (locale) => {
  await setLocale(locale);
  try {
    expect(recordingReason({ recording: { state: 'unavailable', reason: 'too_large' } })).toBe('too_large');
    expect(recordingReason({ recording: { state: 'complete', reason: null } })).toBeNull();
    for (const facts of [null, {}, { recording: 'too_large' }, { recording: { reason: 'unreadable' } }]) expect(recordingReason(facts)).toBeNull();
    for (const key of ['largeTree.recordingTooLarge', 'largeTree.recordingLive', 'largeTree.recordingSetting', 'source.restore.gitignore']) {
      const text = i18n.t(key);
      expect(text).not.toBe(key);
      expect(text).not.toContain('\n');
      if (locale === 'zh-CN') expect(text).toMatch(/\p{Script=Han}/u);
    }
    expect(i18n.t('source.restore.gitignore')).toContain('.gitignore');
  } finally { await setLocale('en'); }
});

it.each(['en', 'zh-CN'] as const)('names indexing and per-file restore at the first affected turn in %s', async (locale) => {
  await setLocale(locale);
  try {
    const words = recordingIndexingWords(i18n.t, { done: 80_000, total: 200_000 });
    expect(words).toContain('200,000');
    expect(words).toContain('40');
    expect(words).not.toContain('largeTree.');
    expect(recordingIndexingWords(i18n.t, null)).not.toContain('largeTree.');
    if (locale === 'zh-CN') expect(words).toMatch(/\p{Script=Han}/u);
  } finally { await setLocale('en'); }
});

it('uses observed end coverage instead of counting reverted tool edits as directory changes', () => {
  const edit = fileChange('reported-edit', { turn: 'covered-turn', changes: [change('reverted.txt', null)] });
  const items = new Map([[edit.id, edit]]);
  const known = turnChanges(items, [edit.id], 'covered-turn', ['shell.txt']);
  expect(known.files.map(({ change: file }) => file.path)).toEqual(['shell.txt']);
  expect(known.coverage).toBe('observed');
  const unchanged = turnChanges(items, [edit.id], 'covered-turn', []);
  expect(unchanged.files).toEqual([]);
  expect(unchanged.reported).toBe(1);
});

it.each(['en', 'zh-CN'] as const)('qualifies tool-only file figures when end coverage is unknown in %s', async (locale) => {
  await setLocale(locale);
  try {
    const edit = fileChange('tool-only', { turn: 'unknown-turn', changes: [change('code.txt', null)] });
    const unknown = turnChanges(new Map([[edit.id, edit]]), [edit.id], 'unknown-turn');
    expect(unknown.coverage).toBe('tools');
    expect(turnFilesLabel(i18n.t, unknown)).toBe(i18n.t('thread.turnFiles.reported', { count: 1 }));
    expect(i18n.t('thread.turnFiles.reported', { count: 1 })).not.toContain('thread.turnFiles.');
    if (locale === 'zh-CN') expect(turnFilesLabel(i18n.t, unknown)).toMatch(/\p{Script=Han}/u);
  } finally { await setLocale('en'); }
});

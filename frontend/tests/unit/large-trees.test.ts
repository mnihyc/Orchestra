/** Live trees explain why, and bounded Git reads point the person to their shell. */
import { describe, expect, it } from 'vitest';

import { liveIndexReasonKey } from '@/features/inspector/Files';
import { gitReadTooSlow } from '@/features/inspector/Git';
import { indexingProgress, liveIndexReason } from '@/features/files/indexWords';
import { wholeUnavailableReason } from '@/features/source/words';
import { i18n, setLocale } from '@/i18n';

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

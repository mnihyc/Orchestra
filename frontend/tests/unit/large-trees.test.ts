/** Live trees explain why, and bounded Git reads point the person to their shell. */
import { describe, expect, it } from 'vitest';

import { liveIndexReasonKey } from '@/features/inspector/Files';
import { gitReadTooSlow } from '@/features/inspector/Git';
import { i18n } from '@/i18n';

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

  it('offers a shell when a bounded Git read runs out and does not confuse other errors with that', () => {
    expect(gitReadTooSlow(new Error("git status is too slow here; use the environment's shell"))).toBe(true);
    expect(gitReadTooSlow(new Error('git status is too large; use the shell'))).toBe(true);
    expect(gitReadTooSlow(new Error('the connector is offline'))).toBe(false);
    expect(i18n.t('largeTree.gitCut')).toContain('cut at its output limit');
    expect(i18n.t('largeTree.openShell')).toBe('Open shell');
  });
});

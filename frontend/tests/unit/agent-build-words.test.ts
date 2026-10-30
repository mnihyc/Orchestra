/** Reported program compatibility reads as words in either language, while version identifiers stay exact. */
import { describe, expect, it } from 'vitest';
import { harnessSupportWords, harnessVersionWords } from '@/features/resources/agents/descriptor';
import { i18n, loadSearchCatalogues } from '@/i18n';

describe('an agent program’s reported compatibility', () => {
  it('distinguishes supported, untested and unsupported, and explains an unknown status', () => {
    const t = i18n.getFixedT('en');
    expect(harnessSupportWords(t, 'supported')).toBe('Supported version');
    expect(harnessSupportWords(t, 'untested')).toBe('Compatibility not verified');
    expect(harnessSupportWords(t, 'unsupported')).toBe('This version is not supported');
    expect(harnessSupportWords(t, 'unknown')).toBe('Support has not been reported');
  });
  it('uses Chinese words and keeps real version identifiers unchanged', async () => {
    await loadSearchCatalogues();
    const t = i18n.getFixedT('zh-CN');
    expect(harnessSupportWords(t, 'supported')).toBe('支持的版本');
    expect(harnessSupportWords(t, 'untested')).toBe('兼容性尚未验证');
    expect(harnessSupportWords(t, 'unsupported')).toBe('不支持此版本');
    expect(harnessSupportWords(t, 'new_support_code')).toBe('尚未报告支持情况');
    expect(harnessVersionWords(t, 'unknown')).toBe('尚未报告版本');
    expect(harnessVersionWords(t, null)).toBe('尚未报告版本');
    expect(harnessVersionWords(t, '1.0.0+world')).toBe('1.0.0+world');
  });
});

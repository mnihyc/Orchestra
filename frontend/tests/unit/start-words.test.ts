/** A reader of Chinese waits for the Chinese catalogue alone; English, the fallback, comes when the browser is idle. */
import { expect, it, vi } from 'vitest';

import { i18n, setLocale, startI18n } from '@/i18n';

it('starts with the reader\'s own words, and fetches English afterwards', { timeout: 60_000 }, async () => {
  try {
    // Starting again begins from no words at all, as a new page does.
    await startI18n('zh-CN');
    expect(i18n.hasResourceBundle('zh-CN', 'translation')).toBe(true);
    expect(i18n.hasResourceBundle('en', 'translation')).toBe(false);
    await vi.waitFor(() => { expect(i18n.hasResourceBundle('en', 'translation')).toBe(true); }, { timeout: 10_000 });
  } finally { await setLocale('en'); }
});

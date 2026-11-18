/** What the files view says when a file cannot be had, in both languages: a place that relays keeps no copy, a place that caches may still have one. */
import { screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { Unavailable } from '@/features/files/Unavailable';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

describe('a file that cannot be had', () => {
  it.each(['en', 'zh-CN'] as const)('says that an offline environment that relays keeps no copy, and that the file opens when it is back, in %s', async (locale) => {
    await setLocale(locale);
    const view = show(<Unavailable reason="offline" />);
    try {
      expect(screen.getByText(i18n.t('files.unavailable.offlineRelayed'))).toBeVisible();
      expect(screen.getByText(i18n.t('files.unavailable.offlineRelayedHint'))).toBeVisible();
      expect(screen.queryByText(i18n.t('files.unavailable.offlineHint'))).not.toBeInTheDocument();
    } finally { view.unmount(); await setLocale('en'); }
  });

  it('keeps the words of a place that caches: a file opened before stays readable', () => {
    show(<Unavailable reason="offline_not_cached" />);
    expect(screen.getByText(i18n.t('files.unavailable.offline'))).toBeVisible();
    expect(screen.getByText(i18n.t('files.unavailable.offlineHint'))).toBeVisible();
  });
});

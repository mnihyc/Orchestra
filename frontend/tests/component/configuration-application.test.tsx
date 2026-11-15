/** Accepted session facts remain independent of newer requests and missing observations. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it } from 'vitest';
import { ConfigurationFacts } from '@/features/thread/ConfigurationFacts';
import { i18n, setLocale } from '@/i18n';
import { show, skipOverflowFallback } from './support';
skipOverflowFallback();
afterEach(async () => { await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) it(`keeps accepted setup separate while native work defers newer instructions (${locale})`, async () => {
  await setLocale(locale);
  const acceptedAt = '2026-11-01T10:00:00.000Z';
  const value = { accepted_session_revision: 7, accepted_at: acceptedAt, pending_kinds: ['instructions' as const, 'tools' as const],
    deferred_by_background: true, reload_policy: 'each_turn' as const, observed_at: '2026-11-15T10:00:00.000Z' };
  const view = show(<ConfigurationFacts application={value} />);
  expect(view.container).toHaveTextContent(locale === 'en' ? 'Background work defers refresh' : '后台任务暂缓会话刷新');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
  const body = await screen.findByRole('dialog', { name: i18n.t('common.details') });
  expect(body.querySelector(`time[datetime="${acceptedAt}"]`)).not.toBeNull();
  expect(body).toHaveTextContent(locale === 'en' ? 'Reopens each turn' : '每轮重新打开会话');
  const refreshed = { ...value, accepted_session_revision: 8, accepted_at: value.observed_at, pending_kinds: [], deferred_by_background: false };
  view.rerender(<ConfigurationFacts application={refreshed} />);
  await waitFor(() => expect(body.querySelector(`time[datetime="${acceptedAt}"]`)).toBeNull());
  expect(view.container).not.toHaveTextContent(locale === 'en' ? 'Background work defers refresh' : '后台任务暂缓会话刷新');
  view.rerender(<ConfigurationFacts application={null} />);
  expect(body).toHaveTextContent(locale === 'en' ? 'Not reported' : '尚未报告');
  expect(body.querySelector('time')).toBeNull();
});

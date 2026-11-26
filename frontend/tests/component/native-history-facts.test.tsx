/** Physical native cache observations never claim the readable conversation was erased. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it } from 'vitest';
import { NativeHistoryFacts } from '@/features/timeline/NativeHistoryFacts';
import { i18n, setLocale } from '@/i18n';
import { show, skipOverflowFallback } from './support';
skipOverflowFallback();
afterEach(async () => { await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) it(`shows only a positive native removal and retires an older observation (${locale})`, async () => {
  await setLocale(locale);
  const at = '2026-11-15T10:00:00.000Z';
  const retained = { state: 'retained' as const, idle_eligible_at: { state: 'not_applicable' as const },
    policy_days: 0, policy_source: 'topic' as const, policy_observed_at: at, removed_at: null, observed_at: at };
  const view = show(<NativeHistoryFacts lifetime={retained} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
  const body = await screen.findByRole('dialog', { name: i18n.t('common.details') });
  expect(body).toHaveTextContent(locale === 'en' ? 'Session files kept' : '会话文件已保留');
  view.rerender(<NativeHistoryFacts lifetime={{ ...retained, state: 'removed', removed_at: at }} />);
  expect(body).toHaveTextContent(locale === 'en' ? 'Session files removed' : '会话文件已移除');
  expect(body.querySelector(`time[datetime="${at}"]`)).not.toBeNull();
  expect(body).not.toHaveTextContent(locale === 'en' ? 'Conversation removed' : '对话已删除');
  view.rerender(<NativeHistoryFacts lifetime={undefined} />);
  expect(body).toHaveTextContent(locale === 'en' ? 'Not reported' : '尚未报告');
  expect(body.querySelector('time')).toBeNull();
});

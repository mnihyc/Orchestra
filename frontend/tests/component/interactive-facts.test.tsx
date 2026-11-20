/** A withdrawn native observation cannot lend its policy, deadline or retirement to the next body. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it } from 'vitest';
import { InteractiveFacts } from '@/features/browser/InteractiveFacts';
import type { Browser } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show, skipOverflowFallback } from './support';
skipOverflowFallback();
afterEach(async () => { await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) it(`withdraws native timers instead of inferring a lease from a ticket (${locale})`, async () => {
  await setLocale(locale);
  const at = '2026-11-15T10:00:00.000Z';
  const browser: Browser = { id: 'subject', environment: { id: 'environment', name: 'Work' } as Browser['environment'],
    profile: null, origin: 'started', opened_by: null, thread: null, preset: 'headless', display: null, endpoint: null,
    program: null, note: null, tabs: null, watching: 0, driving: [], state: 'open', close_reason: null,
    opened_at: at, last_used_at: at, closed_at: null, lifecycle: { gate: null,
      idle_close_eligible_at: { state: 'not_applicable' }, closed_entry_expires_at: { state: 'unreported' },
      driving_idle_expires_at: { state: 'not_applicable' }, retirement_reason: null, idle_minutes: 0, policy_source: 'topic', evaluated_at: at } };
  const view = show(<InteractiveFacts subject={browser} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
  const facts = await screen.findByRole('dialog', { name: i18n.t('common.details') });
  expect(facts).toHaveTextContent(locale === 'en' ? 'Automatic idle close off' : '闲置自动关闭已停用');
  expect(facts).toHaveTextContent(locale === 'en' ? 'Topic policy' : '主题策略');
  expect(facts.querySelector(`time[datetime="${at}"]`)).not.toBeNull();
  view.rerender(<InteractiveFacts subject={{ ...browser, lifecycle: undefined }} />);
  expect(facts).toHaveTextContent(locale === 'en' ? 'Not reported' : '尚未报告');
  expect(facts).not.toHaveTextContent(locale === 'en' ? 'Automatic idle close off' : '闲置自动关闭已停用');
  expect(facts.querySelector('time')).toBeNull();
});

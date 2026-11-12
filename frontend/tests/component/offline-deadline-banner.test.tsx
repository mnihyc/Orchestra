/** The existing composer banner shows only the current native turn's retained deadline. */
import { cleanup, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { BlockedBanner } from '@/features/thread/BlockedBanner';
import type { Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { formatDateTime } from '@/lib/time';
import { snapshot } from '../unit/continuation-fixtures';
import { executionDisplay } from '../doubles/thread-display';
import { show } from './support';

vi.mock('@/features/change/target', () => ({ openChange: vi.fn() }));
afterEach(async () => { cleanup(); useAppearance.setState({ locale: 'en' }); await setLocale('en'); });
const turn = '00000000-0000-0000-0000-000000000201';
const since = '2026-11-08T00:00:00Z';
const at = '2026-11-08T00:10:00Z';
function thread(): Thread {
  return { ...snapshot().thread, state: 'blocked', display: executionDisplay('blocked'), lock: null, current_turn_id: turn,
    blocked: { reason: 'agent_offline', detail: null, sandbox: null,
      offline_deadline: { turn_id: turn, since, at, after_seconds: 600 } } };
}
for (const locale of ['en', 'zh-CN'] as const) {
  it(`shows the native absolute offline deadline in the existing banner (${locale})`, async () => {
    await setLocale(locale); useAppearance.setState({ locale }); const native = thread();
    show(<BlockedBanner thread={native} onProceed={vi.fn()} />);
    const context = { locale, timeZone: useAppearance.getState().timeZone };
    expect(screen.getByText(i18n.t('thread.blocked.offlineDeadline', { when: formatDateTime(at, context) }))).toBeVisible();
  });
}
it('keeps the default banner without a deadline and suppresses a stale turn deadline', () => {
  const native = thread(); if (native.blocked === null) throw new Error('blocked fixture required');
  native.blocked = { reason: 'agent_offline', detail: null, sandbox: null };
  const view = show(<BlockedBanner thread={native} onProceed={vi.fn()} />);
  expect(view.container.querySelector('[data-offline-deadline]')).toBeNull();
  const newer = { ...thread(), current_turn_id: '00000000-0000-0000-0000-000000000202' };
  view.rerender(<BlockedBanner thread={newer} onProceed={vi.fn()} />);
  expect(view.container.querySelector('[data-offline-deadline]')).toBeNull();
});
it('describes an unrepresentable calendar deadline without imposing a UI policy cap', () => {
  const native = thread(); if (!native.blocked?.offline_deadline) throw new Error('deadline fixture required');
  native.blocked.offline_deadline.at = null;
  native.blocked.offline_deadline.after_seconds = Number.MAX_SAFE_INTEGER + 1;
  const context = { locale: 'en', timeZone: useAppearance.getState().timeZone };
  show(<BlockedBanner thread={native} onProceed={vi.fn()} />);
  expect(screen.getByText(i18n.t('thread.blocked.offlineDeadlineBeyondCalendar', { since: formatDateTime(since, context) }))).toBeVisible();
});

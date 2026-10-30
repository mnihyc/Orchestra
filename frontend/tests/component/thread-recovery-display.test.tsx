/** Recovery icons explain automatic waits and guard stops without turning them into human blockers. */
import { act, cleanup, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ThreadStateIcon, threadStateLabel } from '@/ui/ThreadState';
import { useAppearance } from '@/lib/appearance';
import { formatSpan } from '@/lib/time';
import { i18n, setLocale } from '@/i18n';
import { display, recoveryDisplay } from '../doubles/thread-display';
import { show } from './support';

const appearance = useAppearance.getState();
const NOW = Date.parse('2026-10-28T00:00:00.000Z');
afterEach(async () => { cleanup(); vi.useRealTimers(); useAppearance.setState(appearance); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  it(`shows the retry countdown and stopped cause at ${width}/${theme}/${locale}`, async () => {
    await setLocale(locale);
    useAppearance.setState({ locale, theme, resolvedTheme: theme });
    vi.useFakeTimers(); vi.setSystemTime(NOW);
    const queued = display('queued', { blocked_reason: 'error', recovery: recoveryDisplay() });
    const label = (seconds: number) => `${i18n.t('threadStates.retryIn', { time: formatSpan(seconds * 1_000, { locale }, 'short') })} · ${i18n.t('calls.attempt', { attempt: '2', total: '3' })}`;
    const view = show(<div data-theme={theme} style={{ width }}><ThreadStateIcon display={queued} /></div>);
    expect(screen.getByRole('img', { name: label(55) })).toBeVisible();
    expect(view.container.querySelector('svg.lucide-clock-3')).not.toBeNull();
    expect(view.container.querySelector('[class~="motion-safe:animate-spin"]')).toBeNull();
    await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
    expect(screen.getByRole('img', { name: label(54) })).toBeVisible();
    view.rerender(<ThreadStateIcon display={display('running', { recovery: recoveryDisplay() })} />);
    expect(screen.getByRole('img', { name: i18n.t('state.running') })).toBeVisible();
    expect(view.container.querySelector('svg.lucide-loader-circle')).not.toBeNull();
    const stopped = display('failed', { blocked_reason: 'error', recovery: recoveryDisplay({ stop_reason: 'repeat_limit', count: 4, window_secs: 300 }) });
    view.rerender(<ThreadStateIcon display={stopped} />);
    const reason = locale === 'en' ? 'Provider busy · 4 times in 5 min' : i18n.t('calls.repeatStopped', { kind: i18n.t('calls.kinds.provider_busy'), count: 4, occurrences: '4', window: formatSpan(300_000, { locale }, 'short') });
    expect(screen.getByRole('img', { name: reason })).toBeVisible();
    expect(view.container.querySelector('svg.lucide-octagon-alert')).not.toBeNull();
    expect(view.container.querySelector('[class~="motion-safe:animate-spin"]')).toBeNull();
  });
}

it('uses the reserved continuation and configured denominator rather than guessing three attempts', () => {
  const continuing = display('queued', { recovery: recoveryDisplay({ continuing: true, attempts: 7 }) });
  expect(threadStateLabel(continuing, i18n.t, { locale: 'en' }, NOW)).toBe(`Continuing in ${formatSpan(55_000, { locale: 'en' }, 'short')} · attempt 2 of 7`);
  const retry = display('queued', { recovery: recoveryDisplay({ step: 'continue', continuing: false }) });
  expect(threadStateLabel(retry, i18n.t, { locale: 'en' }, NOW)).toBe(`Retrying in ${formatSpan(55_000, { locale: 'en' }, 'short')} · attempt 2 of 3`);
  const stopped = display('failed', { recovery: recoveryDisplay({ stop_reason: 'author_lacks_use' }) });
  expect(threadStateLabel(stopped, i18n.t, { locale: 'en' }, NOW)).toBe(i18n.t('calls.details.author_lacks_use'));
});

it('shares its clocks across a thousand icons and releases them when those icons leave', async () => {
  vi.useFakeTimers(); vi.setSystemTime(NOW);
  const queued = display('queued', { recovery: recoveryDisplay() });
  const icons = (count: number) => <>{Array.from({ length: count }, (_, index) => <ThreadStateIcon key={index} display={queued} />)}</>;
  const view = show(icons(1));
  await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
  const oneIconTimers = vi.getTimerCount();
  const network = vi.spyOn(globalThis, 'fetch');
  view.rerender(icons(1_000));
  await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
  expect(screen.getAllByRole('img')).toHaveLength(1_000);
  expect(vi.getTimerCount()).toBe(oneIconTimers);
  expect(network).not.toHaveBeenCalled();
  view.unmount();
  expect(vi.getTimerCount()).toBe(0);
  network.mockRestore();
});

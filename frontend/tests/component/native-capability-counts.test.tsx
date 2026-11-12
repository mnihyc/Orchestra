/** Native aggregate quantities are visible once, preserving zero and an unknown unit without request facts. */
import { cleanup, screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { RequestFacts, nativeCounterRows } from '@/features/usage/RequestFacts';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { requestRow } from '../doubles/request-row';
import { nativeTiming, nativeTokens } from '../fixtures/native-counts';
import { show } from './support';

afterEach(async () => { cleanup(); await setLocale('en'); useAppearance.getState().set({ locale: 'en', theme: 'system' }); });
for (const language of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`labels aggregate counts without a request, attempt or duration ${language}/${theme}`, async () => {
    await setLocale(language); useAppearance.getState().set({ locale: language, theme });
    const event = requestRow({ scope: 'turn', origin: 'harness', timing: nativeTiming, tokens: nativeTokens, request_ref: null });
    const { container } = show(<RequestFacts event={event} />);
    expect(screen.getByText(i18n.t('usageRequest.nativeAggregate'), { exact: true })).toBeVisible();
    expect(screen.getByText(i18n.t('capabilityRoutes.names.web_search'), { exact: true })).toBeVisible();
    expect(screen.getByText('2', { exact: true })).toBeVisible();
    expect(screen.getByText(i18n.t('capabilityRoutes.names.page_fetch'), { exact: true })).toBeVisible();
    expect(screen.getByText('0', { exact: true })).toBeVisible();
    expect(screen.getByText('extra_capacity', { exact: true })).toBeVisible();
    expect(screen.getByText(i18n.t('usagePerf.dash'), { exact: true })).toBeVisible();
    expect(container).not.toHaveTextContent(i18n.t('usageRequest.attempt'));
    expect(container).not.toHaveTextContent(i18n.t('usagePerf.outcomes.done'));
    expect(nativeCounterRows({ ...event, scope: 'request' })).toEqual([]);
    expect(nativeCounterRows({ ...event, origin: 'forwarder' })).toEqual([]);
  });
}
it('ignores invalid and unsupported quantities without turning them into zero', () => {
  const timing = { ...nativeTiming, declared_counters: { wrong: { state: 'told' as const, value: -1 }, unsafe: { state: 'told' as const, value: Number.MAX_SAFE_INTEGER + 1 }, unavailable: { state: 'unsupported' as const } } };
  expect(nativeCounterRows(requestRow({ scope: 'turn', timing }))).toEqual([{ capability: 'unsafe', quantity: null }]);
});

/** A paused service's recovery read stays visible without inventing a resumed service. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as CallsApi from '@/api/calls';
import { callRecoveryQuery } from '@/api/calls';
import { ApiError } from '@/api/client';
import { ThreadCallRecovery } from '@/features/thread/CallRecovery';
import type { CallRecovery, ThreadCallRecoveryReply } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { apiErrorSentence } from '@/i18n/errors';
import { useAppearance } from '@/lib/appearance';
import { failedStart } from './failed-start-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<() => Promise<ThreadCallRecoveryReply>>(), control: vi.fn<typeof CallsApi.controlCallRecovery>() }));
vi.mock('@/api/calls', async (load) => {
  const api = await load<typeof CallsApi>();
  return { ...api, callRecoveryQuery: (id: string) => ({ ...api.callRecoveryQuery(id), queryFn: () => calls.read() }), controlCallRecovery: calls.control };
});
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

const appearance = useAppearance.getState();
beforeEach(() => { calls.read.mockReset(); calls.control.mockReset().mockResolvedValue({ recovery: null }); });
afterEach(async () => { cleanup(); useAppearance.setState(appearance); await setLocale('en'); });

function paused(): CallRecovery {
  return { service: 'web_search', state: 'waiting', failure: { code: 'unavailable', message: 'Native service unavailable', retryable: true,
    window: null, said: null, resets_at: null, incomplete: false }, rule_id: 'pause-search', step: { type: 'wait', until: 'person' },
    attempt: 1, due_at: null, detail: null, count: null, window_secs: null };
}

// EN: Paused service → checking recovery → read failure → Try again → Resume service explicitly.
// 中文：暂停的服务 → 检查恢复状态 → 读取失败 → 重试 → 明确点击“恢复服务”。
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440] as const) {
  it(`keeps the recovery inspector line and retries only its read (${locale}/${theme}/${width})`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme, resolvedTheme: theme });
    const { thread } = failedStart();
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const gate: { reject?: (error: unknown) => void; resolve?: (value: ThreadCallRecoveryReply) => void } = {};
    const pending = new Promise<ThreadCallRecoveryReply>((resolve, reject) => { gate.resolve = resolve; gate.reject = reject; });
    calls.read.mockReturnValueOnce(pending).mockResolvedValue({ recovery: paused() });
    const view = show(<QueryClientProvider client={cache}><div style={{ width }}><ThreadCallRecovery thread={thread} serviceOnly /></div></QueryClientProvider>);
    try {
      expect(screen.getByText(i18n.t('calls.checkingRecovery'))).toBeVisible();
      expect(screen.queryByRole('button', { name: i18n.t('calls.resumeService') })).not.toBeInTheDocument();
      expect(calls.control).not.toHaveBeenCalled();
      const error = new ApiError({ type: 'orch:error:internal', code: 'internal', status: 503, title: '', detail: 'Hidden recovery backend', request_id: 'service-read' }, null);
      await act(async () => { gate.reject?.(error); await pending.catch(() => undefined); });
      const failure = await screen.findByRole('alert');
      expect(failure).toHaveTextContent(i18n.t('calls.recoveryUnavailable'));
      expect(failure).toHaveTextContent(apiErrorSentence('internal', 503, i18n.t, locale));
      expect(view.container).not.toHaveTextContent('Hidden recovery backend');
      expect(calls.control).not.toHaveBeenCalled();
      await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
      const resume = await screen.findByRole('button', { name: i18n.t('calls.resumeService') });
      expect(calls.read).toHaveBeenCalledTimes(2);
      expect(calls.control).not.toHaveBeenCalled();
      expect(view.container.querySelector('[data-call-recovery="waiting"]')).toBeVisible();
      for (const key of ['retry', 'continue', 'switch'] as const) expect(screen.queryByRole('button', { name: i18n.t(`calls.${key}`) })).not.toBeInTheDocument();
      await userEvent.click(resume);
      await waitFor(() => expect(calls.control).toHaveBeenCalledOnce());
      expect(calls.control.mock.calls[0]?.[1]).toBe('retry_now');
    } finally { gate.resolve?.({ recovery: null }); view.unmount(); cache.clear(); }
  });
}

for (const locale of ['en', 'zh-CN'] as const) {
  for (const problem of [
    { code: 'authz.denied', status: 403 }, { code: 'not_found', status: 404 }, { code: 'capability.unsupported', status: 409 },
  ] as const) it(`explains a ${problem.code} recovery read without private details or stale controls (${locale})`, async () => {
    await setLocale(locale); useAppearance.setState({ locale });
    const { thread } = failedStart();
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    cache.setQueryData(callRecoveryQuery(thread.id).queryKey, { recovery: paused() });
    calls.read.mockRejectedValue(new ApiError({ ...problem, type: `orch:error:${problem.code}`, title: '', detail: 'Hidden provider, grant and audience', request_id: 'private-service-read' }, null));
    const view = show(<QueryClientProvider client={cache}><ThreadCallRecovery thread={thread} serviceOnly /></QueryClientProvider>);
    try {
      const failure = await screen.findByRole('alert');
      expect(failure).toHaveTextContent(i18n.t('calls.recoveryUnavailable'));
      expect(failure).toHaveTextContent(apiErrorSentence(problem.code, problem.status, i18n.t, locale));
      expect(screen.getByRole('button', { name: i18n.t('common.tryAgain') })).toBeEnabled();
      expect(screen.queryByRole('button', { name: i18n.t('calls.resumeService') })).not.toBeInTheDocument();
      expect(view.container.querySelector('[data-call-recovery]')).toBeNull();
      expect(view.container).not.toHaveTextContent('Hidden provider, grant and audience');
      expect(calls.control).not.toHaveBeenCalled();
    } finally { view.unmount(); cache.clear(); }
  });

  for (const service of [null, 'primary'] as const) it(`does not invent a paused service from a ${service ?? 'missing'} recovery (${locale})`, async () => {
    await setLocale(locale);
    const { thread } = failedStart();
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const { service: _service, ...primary } = paused();
    const reply: ThreadCallRecoveryReply = { recovery: service === null ? null : primary };
    calls.read.mockResolvedValue(reply);
    const view = show(<QueryClientProvider client={cache}><ThreadCallRecovery thread={thread} serviceOnly /></QueryClientProvider>);
    try {
      await waitFor(() => expect(cache.getQueryData(callRecoveryQuery(thread.id).queryKey)).toEqual(reply));
      expect(screen.queryByText(i18n.t('calls.checkingRecovery'))).not.toBeInTheDocument();
      expect(screen.queryByRole('alert')).not.toBeInTheDocument();
      expect(screen.queryByRole('button', { name: i18n.t('calls.resumeService') })).not.toBeInTheDocument();
      expect(view.container.querySelector('[data-call-recovery]')).toBeNull();
      expect(calls.control).not.toHaveBeenCalled();
    } finally { view.unmount(); cache.clear(); }
  });
}

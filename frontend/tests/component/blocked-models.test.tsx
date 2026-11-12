/** A routing name never supplies an unreported vendor-family classification. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as ThreadsApi from '@/api/threads';
import type * as RunsApi from '@/features/composer/runsWith';
import { ApiError } from '@/api/client';
import { parametersQuery } from '@/api/threads';
import { settingsQuery } from '@/api/settings';
import { providerQuery } from '@/api/resources';
import { BlockedBanner } from '@/features/thread/BlockedBanner';
import type { ModelOption, ThreadParameters, Turn } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { apiErrorSentence } from '@/i18n/errors';
import { useAppearance } from '@/lib/appearance';
import { threadWriter } from '@/realtime/stores/thread';
import { ACME } from '../doubles/fixtures';
import { turn } from '../doubles/thread-rows';
import { World } from '../doubles/world';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<() => Promise<ThreadParameters>>(), change: vi.fn<typeof RunsApi.changeModelChoice>() }));
vi.mock('@/api/threads', async (load) => {
  const api = await load<typeof ThreadsApi>();
  return { ...api, parametersQuery: (id: string) => ({ ...api.parametersQuery(id), queryFn: () => calls.read() }) };
});
vi.mock('@/features/composer/runsWith', async (load) => ({ ...await load<typeof RunsApi>(), changeModelChoice: calls.change }));
const appearance = useAppearance.getState();
beforeEach(() => { calls.read.mockReset(); calls.change.mockReset().mockResolvedValue({ dropped: [], option: model('portable-fable-proxy', 'Portable model') }); });
afterEach(async () => { cleanup(); threadWriter.evict(ACME.mainThread); useAppearance.setState(appearance); await setLocale('en'); });

const model = (model_name: string, display_name: string, allowed = true): ModelOption => ({
  model_name, display_name, allowed, reason: allowed ? null : 'excluded_by_entry', parameters: {}, token_estimate: { ascii_prose_x100: 400, ascii_structured_x100: 300, other_x100: 100, image_tokens: 1000, measured: false },
});

function modelControls() {
  const surface = document.querySelector('[data-family-limit]');
  if (!(surface instanceof HTMLElement)) throw new Error('The existing model entrance must remain present');
  return within(surface);
}

function familyLimit(family: string | null, known = true) {
  const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('The model-choice fixture needs its native thread');
  const thread = { ...world.threads.thread(person, row), state: 'blocked' as const, model: 'current-model', blocked: { reason: 'provider_exhausted' as const, detail: 'The vendor reported a family limit' } };
  if (thread.binding === null) throw new Error('The model-choice fixture needs a provider binding');
  const failed: Turn = { ...turn('limited-turn', { ordinal: 1, state: 'failed', inputId: 'earlier', model: thread.model }), error: { code: 'turn.limit', message: 'Family unavailable', retryable: true, said: null, resets_at: null, window: null, incomplete: false,
    limit: { scope: 'model_family', model_family: family, retry_after_secs: null, window_minutes: null } } };
  threadWriter.reset(thread.id, { thread, turns: [failed] }, false);
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  const parameters: ThreadParameters = { approval_mode: 'on_request', approval_reviewer: null, approval_modes: ['on_request'], model: { value: thread.model, writable: true, source: { scope: 'thread', node: null }, options: [
    model(thread.model, 'Current choice'), model('portable-fable-proxy', 'Portable model'), model('different-route', 'Another model'), model('not-permitted', 'Unavailable model', false),
  ] }, effort: { value: null, writable: true, supported: false, source: { scope: 'default', node: null } }, parameters: [] };
  if (known) cache.setQueryData(parametersQuery(thread.id).queryKey, parameters);
  cache.setQueryData(settingsQuery(thread.id).queryKey, []);
  cache.setQueryData(providerQuery(thread.binding.provider.id).queryKey, planProvider({ access: { permissions: [], administers: false, through: [], admin_only: [], participating: false } }));
  return { thread, cache, parameters };
}

for (const locale of ['en', 'zh-CN'] as const) it.each(['Fable', null])('offers every other allowed model when vendor membership is absent in '+locale+' (%s)', async (family) => {
  await setLocale(locale);
  const { thread, cache } = familyLimit(family);
  const view = show(<QueryClientProvider client={cache}><BlockedBanner thread={thread} onProceed={() => undefined} /></QueryClientProvider>);
  try {
    await userEvent.click(screen.getByRole('combobox', { name: i18n.t('thread.blocked.familyChoose') }));
    expect(screen.getByRole('option', { name: 'Portable model' })).toBeVisible();
    expect(screen.getByRole('option', { name: 'Another model' })).toBeVisible();
    expect(screen.queryByRole('option', { name: 'Current choice' })).not.toBeInTheDocument();
    expect(screen.queryByRole('option', { name: 'Unavailable model' })).not.toBeInTheDocument();
  } finally { view.unmount(); cache.clear(); }
});

// EN: Family limit → checking model picker → read failure → Try again → choose another model.
// 中文：模型族限额 → 检查中的模型选择器 → 读取失败 → 重试 → 选择其他模型。
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440] as const) {
  it(`keeps the model entrance through checking, failure and an explicit read retry (${locale}/${theme}/${width})`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme, resolvedTheme: theme });
    const { thread, cache, parameters } = familyLimit('Fable', false);
    const gate: { reject?: (error: unknown) => void; resolve?: (value: ThreadParameters) => void } = {};
    const pending = new Promise<ThreadParameters>((resolve, reject) => { gate.resolve = resolve; gate.reject = reject; });
    calls.read.mockReturnValueOnce(pending).mockResolvedValue(parameters);
    const view = show(<QueryClientProvider client={cache}><div style={{ width }}><BlockedBanner thread={thread} onProceed={() => undefined} /></div></QueryClientProvider>);
    try {
      const controls = modelControls();
      const picker = screen.getByRole('combobox', { name: i18n.t('thread.blocked.familyChoose') });
      expect(picker).toBeDisabled();
      expect(picker).toHaveTextContent(i18n.t('thread.blocked.familyChecking'));
      expect(screen.queryByText(i18n.t('thread.blocked.familyNoOther'))).not.toBeInTheDocument();
      const error = new ApiError({ type: 'orch:error:internal', code: 'internal', status: 503, title: '', detail: 'Private service implementation', request_id: 'model-read' }, null);
      await act(async () => { gate.reject?.(error); await pending.catch(() => undefined); });
      expect(await controls.findByRole('alert')).toHaveTextContent(apiErrorSentence('internal', 503, i18n.t, locale));
      expect(picker).toBeDisabled();
      expect(picker).toHaveTextContent(i18n.t('thread.blocked.familyReadFailed'));
      expect(view.container).not.toHaveTextContent('Private service implementation');
      expect(calls.change).not.toHaveBeenCalled();
      await userEvent.click(controls.getByRole('button', { name: i18n.t('common.tryAgain') }));
      await waitFor(() => expect(picker).toBeEnabled());
      expect(calls.read).toHaveBeenCalledTimes(2);
      expect(calls.change).not.toHaveBeenCalled();
      await userEvent.click(picker);
      await userEvent.click(screen.getByRole('option', { name: 'Portable model' }));
      await waitFor(() => expect(calls.change).toHaveBeenCalledOnce());
      expect(calls.change.mock.calls[0]?.[1]).toMatchObject({ model: 'portable-fable-proxy', effort: null });
    } finally { gate.resolve?.(parameters); view.unmount(); cache.clear(); }
  });
}

for (const locale of ['en', 'zh-CN'] as const) {
  for (const problem of [
    { code: 'authz.denied', status: 403 }, { code: 'not_found', status: 404 }, { code: 'capability.unsupported', status: 409 },
  ] as const) it(`keeps a ${problem.code} read distinct from an empty model list and hides old choices (${locale})`, async () => {
    await setLocale(locale); useAppearance.setState({ locale });
    const { thread, cache, parameters } = familyLimit('Fable');
    cache.setQueryData(parametersQuery(thread.id).queryKey, { ...parameters, model: { ...parameters.model, value: 'portable-fable-proxy' } });
    calls.read.mockRejectedValue(new ApiError({ ...problem, type: `orch:error:${problem.code}`, title: '', detail: 'Hidden provider and model', request_id: 'refused-model-read' }, null));
    const view = show(<QueryClientProvider client={cache}><BlockedBanner thread={thread} onProceed={() => undefined} /></QueryClientProvider>);
    try {
      const picker = screen.getByRole('combobox', { name: i18n.t('thread.blocked.familyChoose') });
      expect(picker).toHaveTextContent('Portable model');
      await userEvent.click(picker);
      expect(screen.getByRole('option', { name: 'Portable model' })).toBeVisible();
      await act(async () => { await cache.invalidateQueries({ queryKey: parametersQuery(thread.id).queryKey, exact: true }); });
      expect(await modelControls().findByRole('alert')).toHaveTextContent(apiErrorSentence(problem.code, problem.status, i18n.t, locale));
      expect(picker).toBeDisabled();
      expect(picker).toHaveTextContent(i18n.t('thread.blocked.familyReadFailed'));
      expect(picker).not.toHaveTextContent('Portable model');
      expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
      expect(view.container).not.toHaveTextContent('Hidden provider and model');
      expect(view.container).not.toHaveTextContent(i18n.t('thread.blocked.familyNoOther'));
      await userEvent.click(picker);
      expect(screen.queryByRole('option')).not.toBeInTheDocument();
      expect(calls.change).not.toHaveBeenCalled();
    } finally { view.unmount(); cache.clear(); }
  });

  it(`keeps the model entrance with a known empty-list reason (${locale})`, async () => {
    await setLocale(locale);
    const { thread, cache, parameters } = familyLimit('Fable');
    cache.setQueryData(parametersQuery(thread.id).queryKey, { ...parameters, model: { ...parameters.model, options: [model(thread.model, 'Current choice')] } });
    const view = show(<QueryClientProvider client={cache}><BlockedBanner thread={thread} onProceed={() => undefined} /></QueryClientProvider>);
    try {
      const picker = screen.getByRole('combobox', { name: i18n.t('thread.blocked.familyChoose') });
      expect(picker).toBeDisabled();
      expect(picker).toHaveTextContent(i18n.t('thread.blocked.familyNoOther'));
      expect(modelControls().queryByRole('alert')).not.toBeInTheDocument();
      expect(calls.read).not.toHaveBeenCalled(); expect(calls.change).not.toHaveBeenCalled();
    } finally { view.unmount(); cache.clear(); }
  });
}

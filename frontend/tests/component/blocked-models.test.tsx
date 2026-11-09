/** A routing name never supplies an unreported vendor-family classification. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it } from 'vitest';
import { parametersQuery } from '@/api/threads';
import { settingsQuery } from '@/api/settings';
import { providerQuery } from '@/api/resources';
import { BlockedBanner } from '@/features/thread/BlockedBanner';
import type { ModelOption, ThreadParameters, Turn } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import { ACME } from '../doubles/fixtures';
import { turn } from '../doubles/thread-rows';
import { World } from '../doubles/world';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

afterEach(async () => { cleanup(); threadWriter.evict(ACME.mainThread); await setLocale('en'); });

const model = (model_name: string, display_name: string, allowed = true): ModelOption => ({
  model_name, display_name, allowed, reason: allowed ? null : 'excluded_by_entry', parameters: {}, token_estimate: { ascii_prose_x100: 400, ascii_structured_x100: 300, other_x100: 100, image_tokens: 1000, measured: false },
});

for (const locale of ['en', 'zh-CN'] as const) it.each(['Fable', null])('offers every other allowed model when vendor membership is absent in '+locale+' (%s)', async (family) => {
  await setLocale(locale);
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
  cache.setQueryData(parametersQuery(thread.id).queryKey, parameters);
  cache.setQueryData(settingsQuery(thread.id).queryKey, []);
  cache.setQueryData(providerQuery(thread.binding.provider.id).queryKey, planProvider({ access: { permissions: [], administers: false, through: [], admin_only: [], participating: false } }));
  const view = show(<QueryClientProvider client={cache}><BlockedBanner thread={thread} onProceed={() => undefined} /></QueryClientProvider>);
  try {
    await userEvent.click(screen.getByRole('combobox', { name: i18n.t('thread.blocked.familyChoose') }));
    expect(screen.getByRole('option', { name: 'Portable model' })).toBeVisible();
    expect(screen.getByRole('option', { name: 'Another model' })).toBeVisible();
    expect(screen.queryByRole('option', { name: 'Current choice' })).not.toBeInTheDocument();
    expect(screen.queryByRole('option', { name: 'Unavailable model' })).not.toBeInTheDocument();
  } finally { view.unmount(); cache.clear(); }
});

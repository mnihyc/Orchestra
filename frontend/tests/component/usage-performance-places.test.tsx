/** Picker hints need measured history and a provider status part appears only during real degradation. */
import { act, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import type { UsageAnalytics, UsageDegradation } from '@/generated/api-types';
import { ApiError } from '@/api/client';
import { ProviderDegradedPart, useChoicePerformance, useProviderHealth } from '@/features/usage/PerformancePlaces';
import { performanceOf, timingFixture } from '../doubles/performance';
import { requestRow } from '../doubles/request-row';
import { show } from './support';

const fake = vi.hoisted(() => ({ answer: undefined as UsageAnalytics | undefined, navigate: vi.fn(), params: [] as unknown[], healthError: null as Error | null, healthCalls: 0, healthLoaded: true }));
vi.mock('@/api/usage', () => ({ usageQuery: (params: unknown) => { fake.params.push(params); return { queryKey: ['performance-places', params], initialData: fake.answer, staleTime: Number.POSITIVE_INFINITY, queryFn: () => Promise.resolve(fake.answer) }; }, healthQuery: (providerId: string) => ({ queryKey: ['test-health', providerId], initialData: fake.healthLoaded ? fake.answer?.degradation ?? undefined : undefined, queryFn: (): Promise<UsageDegradation> => { fake.healthCalls += 1; if (fake.healthError !== null) return Promise.reject(fake.healthError); const health = fake.answer?.degradation; if (health === undefined || health === null) return Promise.reject(new Error('the fixture supplies health')); return Promise.resolve(health); } }) }));
vi.mock('@tanstack/react-router', () => ({ useNavigate: () => fake.navigate }));

function Hint() { const hint = useChoicePerformance({ topic_id: requestRow().topic.id, binding: { provider: requestRow().provider } }); return <output>{hint('Swift', 'high', 'fast', null)}</output>; }

beforeEach(() => {
  fake.navigate.mockClear();fake.params=[];fake.healthError=null;fake.healthCalls=0;fake.healthLoaded=true;
  const performance = { ...performanceOf([timingFixture()]), requests: 20, speed_samples: 20, first_token_samples: 20 };
  fake.answer = { scope: 'me', depth: 'own', from: '2026-10-09', to: '2026-10-15', bucket: 'day', totals: { turns: 1, credits: '0', estimated_credits: '0', unrated_tokens: 0, tokens: requestRow().tokens, performance }, me: null, breakdowns: [], series: [], comparison: [{ model: 'Swift', effort: 'high', tier: 'fast', program: null, performance }], degradation: { since: '2026-10-15T04:00:00Z', recent: performance, baseline: performance, errors: false, first_output: false } };
});

it('shows a muted picker hint only after twenty measured requests for the exact choice', async () => {
  show(<Hint />);
  expect(await screen.findByText('About 61 tok/s, first output in 0.5 s')).toBeInTheDocument();
  expect(fake.params).toContainEqual(expect.objectContaining({ filters: { provider_id: requestRow().provider.id } }));
});

it('leaves a choice without enough measurements unnamed by a fabricated speed', () => {
  const row = fake.answer?.comparison?.[0];
  if (row === undefined) throw new Error('the fixture includes a choice');
  row.performance.speed_samples = 19;
  show(<Hint />);
  expect(screen.getByRole('status')).toBeEmptyDOMElement();
});

it('opens provider failures from a degraded-only status part', async () => {
  const health = fake.answer?.degradation;
  if (health === undefined || health === null) throw new Error('the fixture includes provider health');
  health.errors = true; health.recent = { ...health.recent, requests: 10, errors: { provider_busy: 2 } };
  const provider = requestRow().provider;
  show(<ProviderDegradedPart thread={{ binding: { provider } }} />);
  const part = await screen.findByRole('button', { name: /Team models: 20.0% errors/ });
  await userEvent.setup().click(part);
  const destination: unknown = fake.navigate.mock.lastCall?.[0];
  expect(destination).toMatchObject({ to: '/usage', search: { scope: `provider:${provider.id}`, view: 'requests', outcome: 'errors' } });
});

it('has no status part while the provider is healthy', () => {
  show(<ProviderDegradedPart thread={{ binding: { provider: requestRow().provider } }} />);
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
});

it('does not fetch provider health merely to draw a status bar before its panel opens', async () => {
  fake.healthLoaded = false;
  show(<ProviderDegradedPart thread={{ binding: { provider: requestRow().provider } }} />);
  await act(async () => { await Promise.resolve(); });
  expect(fake.healthCalls).toBe(0); expect(fake.params).toEqual([]);
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
});

it('stops health polling after the server refuses provider access', async () => {
  vi.useFakeTimers();
  try {
    fake.healthError = new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Forbidden', status: 403, detail: 'Provider access is required.', request_id: 'request-1' }, null);
    function OpenPanel() { useProviderHealth(requestRow().provider.id); return <ProviderDegradedPart thread={{ binding: { provider: requestRow().provider } }} />; }
    show(<OpenPanel />);
    await act(async () => { await vi.advanceTimersByTimeAsync(1); });
    expect(fake.healthCalls).toBe(1);
    await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
    expect(fake.healthCalls).toBe(1);
    expect(fake.params).toEqual([]);
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  } finally { vi.useRealTimers(); }
});

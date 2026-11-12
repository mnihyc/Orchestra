/** A real provider batch receipt reaches the existing dialog without full jobs or automatic resend. */
import { act, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { client } from '@/api/client';
import { UpdateDialog } from '@/features/fleet/UpdateDialog';
import type { ProviderBatchResponse, ProviderRuntimeUpdateBatchPreflightReply } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { failedStart } from './failed-start-fixtures';
import { detailEnvironment, detailUnitProvider } from './resource-detail-fixtures';
import { show } from './support';

const io = vi.hoisted(() => ({
  send: vi.fn<(request: Request) => Promise<Response>>(),
  job: vi.fn<(id: string) => void>(),
}));
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  const client = actual.createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch: io.send });
  return { ...actual, client, api: client.api };
});
vi.mock('@/api/forks', async (load) => ({ ...await load<object>(), jobQuery: (id: string) => {
  io.job(id);
  return { queryKey: ['batch-receipt-job', id], queryFn: () => Promise.resolve({ ...failedStart().job, id }) };
} }));
vi.mock('@/features/resource-facts/ResourceFacts', () => ({ ResourceChip: ({ subject }: { subject: { provider?: { name: string } } }) => <span>{subject.provider?.name}</span> }));
vi.mock('@/features/change/TargetBuilds', () => ({ TargetBuilds: () => null }));
vi.mock('@/features/change/Progress', () => ({ JobProgress: () => <p>Job progress</p>, committed: () => false }));

afterEach(() => {
  vi.clearAllMocks();
  act(() => client.session.setState({ status: 'signed_out', userId: null }));
});

it('acknowledges one provider confirmation without job polling, plan refresh or automatic resend', async () => {
  client.session.setState({ status: 'signed_in', userId: 'batch-operator' });
  const preflight = '00000000-0000-4000-8202-000000000001';
  const provider = { ...detailUnitProvider, id: '00000000-0000-4000-8202-000000000004' };
  const options = { active_turn: 'wait' as const, wait_minutes: 10, verify_timeout_secs: 120, keep_previous: false, stop_processes: true };
  const estimate = { seconds_low: 1, seconds_high: 2, basis: 'assumed' as const };
  const full: ProviderRuntimeUpdateBatchPreflightReply = {
    preflight_id: preflight, expires_at: '2030-01-01T00:00:00Z', executable: 1,
    options, estimate, notify: { people: [], hidden: 0 }, skipped: [],
    plans: [{ recovery: false, preflight_id: preflight, expires_at: '2030-01-01T00:00:00Z',
      provider, reason: 'update', updates: [], kept: ['Sign-in'], lost: [],
      rollback: 'unit', network: detailEnvironment.network, threads: [], blockers: [], options, estimate }],
  };
  const receipt: ProviderBatchResponse = {
    projection: 'receipt', batch_id: '00000000-0000-4000-8202-000000000002',
    jobs: [{ provider_id: provider.id, job_id: '00000000-0000-4000-8202-000000000003' }],
  };
  const sent: Request[] = [];
  io.send.mockImplementation((request) => {
    sent.push(request.clone());
    const path = new URL(request.url).pathname;
    if (path === '/api/v1/providers/runtime/updates/preflight') {
      return Promise.resolve(new Response(JSON.stringify(full), { status: 200, headers: { 'Content-Type': 'application/json' } }));
    }
    if (path === '/api/v1/providers/runtime/updates') {
      return Promise.resolve(new Response(JSON.stringify(receipt), {
        status: 202, headers: { 'Content-Type': 'application/json', 'X-Request-Id': 'batch-confirmed-once' },
      }));
    }
    return Promise.reject(new Error('Unexpected batch followup request'));
  });
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const invalidation = vi.spyOn(cache, 'invalidateQueries');
  const targets = { environments: [], providers: [provider.id] };
  const content = () => <QueryClientProvider client={cache}><UpdateDialog targets={targets} onClose={vi.fn()} /></QueryClientProvider>;
  const view = show(content());
  try {
    const label = i18n.t('programVersions.updateCount', { units: i18n.t('programVersions.runtimeCount', { count: 1, number: '1' }) });
    const confirm = await screen.findByRole('button', { name: label });
    await waitFor(() => expect(confirm).toBeEnabled());
    await userEvent.click(confirm);
    const acknowledged = await screen.findByText(i18n.t('ui.receipt.accepted'));
    expect(acknowledged).toBeVisible();
    expect(acknowledged.closest('[data-state-line]')).toHaveAttribute('role', 'status');
    expect(acknowledged.closest('[data-state-line]')).toHaveAttribute('data-state-tone', 'neutral');
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(io.job).not.toHaveBeenCalled();
    expect(invalidation).not.toHaveBeenCalled();
    // Keep the established explicit-submit convention; acknowledgement never presses this button.
    await waitFor(() => expect(confirm).toBeEnabled());
    view.rerender(content());
    await act(async () => { await Promise.resolve(); });
    expect(sent.map((request) => `${request.method} ${new URL(request.url).pathname}`)).toEqual([
      'POST /api/v1/providers/runtime/updates/preflight', 'POST /api/v1/providers/runtime/updates',
    ]);
    expect(io.send).toHaveBeenCalledTimes(2);
    expect(io.job).not.toHaveBeenCalled();
    const request = sent[1];
    if (request === undefined) throw new Error('Expected the single provider confirmation');
    const body: unknown = await request.json();
    expect(body).toMatchObject({ preflight_id: preflight, options: { active_turn: 'wait', wait_minutes: 10 } });
    expect(request.headers.get('Idempotency-Key')).toEqual(expect.any(String));
    expect(request.headers.get('Idempotency-Key')).not.toBe('');
  } finally {
    view.unmount();
    cache.clear();
  }
});

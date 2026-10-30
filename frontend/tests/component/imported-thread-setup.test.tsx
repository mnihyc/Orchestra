/** An imported conversation gets local resources on its own identity, preserving a successful placement on retry. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { SetUpThread } from '@/features/start/SetUpThread';
import { ApiError } from '@/api/client';
import { resolve, type Picks } from '@/features/start/choices';
import type { Thread, ThreadBindingSetBody, ThreadEnvironmentSetBody } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { snapshot } from '../unit/continuation-fixtures';
import { environment, options } from '../unit/start-fixtures';
import { show } from './support';

const io = vi.hoisted(() => ({
  place: vi.fn<(id: string, body: ThreadEnvironmentSetBody) => Promise<Thread>>(),
  bind: vi.fn<(id: string, body: ThreadBindingSetBody) => Promise<Thread>>(),
  create: vi.fn(), fresh: false, loading: false, asking: false,
}));
vi.mock('@/api/environments', async (original) => ({ ...await original<object>(), createEnvironment: io.create }));
vi.mock('@/api/threadSetup', () => ({ placeUnboundThread: io.place, bindUnboundThread: io.bind }));
vi.mock('@/api/start', async (original) => ({ ...await original<object>(), startOptionsQuery: (_topic: string, id: string | null) => ({
  queryKey: ['import-access', id], queryFn: () => id === 'e2' ? Promise.reject(new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', status: 403, title: 'Access denied', detail: 'Use is needed', request_id: 'use-required', missing: [{ object: { kind: 'environment', id, name: 'Env e2', owner: null }, permissions: ['environment.use'], ask: [], ask_more: 0, closed: null, options: null, override: false }] }, null)) : Promise.resolve(options()),
}) }));
vi.mock('@/features/start/options', () => ({ useStartOptions: (_topic: string, picks: Picks) => {
  const offered = options(io.fresh ? { environments: [], environment_default: 'fresh' } : io.asking ? {
    environments: [environment('e1', { isDefault: true }), { ...environment('e2', { usable: false }), reason: 'environment_use_required' }],
  } : {});
  return { options: io.loading ? null : offered, resolved: io.loading ? null : resolve(offered, picks, { suggest: true }),
    loading: io.loading, failed: null, refreshing: false, retry: vi.fn() };
} }));

const imported = (): Thread => ({ ...snapshot().thread, binding: null, environment: null });
beforeEach(() => {
  vi.clearAllMocks(); io.fresh = false; io.loading = false; io.asking = false;
  io.place.mockResolvedValue({ ...imported(), environment: environment('placed').environment });
  io.bind.mockResolvedValue(snapshot().thread);
  io.create.mockResolvedValue({ ...environment('made').environment, name: 'Imported work' });
});
afterEach(async () => { await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`sets up the imported thread with the native provider, agent, model and environment choices (${locale})`, async () => {
  await setLocale(locale);
  const thread = imported(), close = vi.fn();
  show(<SetUpThread thread={thread} onClose={close} />);
  expect(screen.getByRole('dialog')).toHaveTextContent('Provider p1');
  expect(screen.getByRole('dialog')).toHaveTextContent('Agent a1');
  await userEvent.click(screen.getByRole('button', { name: locale === 'en' ? 'Set up' : '设置' }));
  await waitFor(() => expect(close).toHaveBeenCalledOnce());
  expect(io.place).toHaveBeenCalledExactlyOnceWith(thread.id, { use: 'e1' });
  expect(io.bind).toHaveBeenCalledExactlyOnceWith(thread.id, {
    provider_id: 'p1', agent_id: 'a1', arrangement: 'separated', model: 'm1', effort: 'low',
  });
  expect(io.place.mock.invocationCallOrder[0]).toBeLessThan(io.bind.mock.invocationCallOrder[0] ?? 0);
});

it('keeps a newly accepted environment when binding is refused and retries without creating another', async () => {
  io.fresh = true;
  io.bind.mockRejectedValueOnce(new Error('The chosen provider changed'));
  const close = vi.fn(), thread = imported();
  show(<SetUpThread thread={thread} onClose={close} />);
  await userEvent.click(screen.getByRole('button', { name: 'Set up' }));
  await screen.findByRole('alert');
  expect(close).not.toHaveBeenCalled();
  expect(io.place).toHaveBeenCalledExactlyOnceWith(thread.id, { use: 'made' });
  expect(io.create).toHaveBeenCalledExactlyOnceWith({ home_node_id: thread.topic_id }, expect.any(String));
  await userEvent.click(screen.getByRole('button', { name: 'Set up' }));
  await waitFor(() => expect(close).toHaveBeenCalledOnce());
  expect(io.bind).toHaveBeenCalledTimes(2);
  expect(io.place).toHaveBeenCalledTimes(1);
  expect(io.create).toHaveBeenCalledTimes(1);
});

it('waits for a current placement answer before admitting setup', () => {
  io.loading = true;
  show(<SetUpThread thread={imported()} onClose={vi.fn()} />);
  expect(screen.getByRole('button', { name: 'Set up' })).toBeDisabled();
  expect(io.place).not.toHaveBeenCalled();
  expect(io.bind).not.toHaveBeenCalled();
});

it('does not place an imported thread in the old environment while asking for another', async () => {
  io.asking = true;
  const user = userEvent.setup();
  show(<SetUpThread thread={imported()} onClose={vi.fn()} />);
  await user.click(screen.getByRole('combobox', { name: i18n.t('start.where.title') }));
  await user.click(screen.getByRole('option', { name: 'Env e2' }));
  const save = screen.getByRole('button', { name: 'Set up' });
  expect(save).toBeDisabled();
  await user.click(save);
  expect(io.place).not.toHaveBeenCalled();
  expect(io.bind).not.toHaveBeenCalled();
});

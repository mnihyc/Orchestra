/** Enable runs without reviewing earlier activity; running the waiting messages stays an explicit choice. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ResourceEnableDialog, ResourceEnableReview } from '@/features/storage/Enable';
import type { FactTarget } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const read = vi.hoisted(() => vi.fn<(target: FactTarget) => Promise<unknown>>());
vi.mock('@/api/resource-impact', () => ({ resourceImpactQuery: (target: FactTarget) => ({ queryKey: ['enable-review', target], queryFn: () => read(target), retry: false }) }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));

const target: FactTarget = { object: { kind: 'provider', id: '00000000-0000-4000-8000-000000000001' }, model_id: null };
afterEach(async () => { cleanup(); vi.resetAllMocks(); await setLocale('en'); });

it.each(['provider', 'model', 'agent', 'environment'] as const)('Enable of a %s reads nothing first and leaves waiting messages off until selected', async (kind) => {
  const user = userEvent.setup({ delay: null });
  const selected: FactTarget = { object: { ...target.object, kind: kind === 'model' ? 'provider' : kind }, model_id: kind === 'model' ? '00000000-0000-4000-8000-000000000003' : null };
  const enable = vi.fn<(runWaiting: boolean) => Promise<unknown>>().mockResolvedValue(undefined);
  show(<ResourceEnableDialog resource={{ target: selected, name: 'Team provider', enable }} onClose={vi.fn()} />);
  const waiting = screen.getByRole('checkbox', { name: i18n.t('resourceImpact.runWaiting') });
  expect(waiting).not.toBeChecked();
  expect(screen.getByRole('button', { name: i18n.t('resources.act.enable') })).toBeEnabled();
  await user.click(waiting);
  await user.click(screen.getByRole('button', { name: i18n.t('resources.act.enable') }));
  await waitFor(() => expect(enable).toHaveBeenCalledExactlyOnceWith(true));
  expect(read).not.toHaveBeenCalled();
});

it('the default Enable passes false and never releases a waiting queue implicitly', async () => {
  const user = userEvent.setup({ delay: null });
  const enable = vi.fn<(runWaiting: boolean) => Promise<unknown>>().mockResolvedValue(undefined);
  show(<ResourceEnableDialog resource={{ target, name: 'Team provider', enable }} onClose={vi.fn()} />);
  await user.click(screen.getByRole('button', { name: i18n.t('resources.act.enable') }));
  await waitFor(() => expect(enable).toHaveBeenCalledExactlyOnceWith(false));
});

it('a refusal while an earlier stop finishes stays in the dialog and Enable can be tried again', async () => {
  const user = userEvent.setup({ delay: null });
  const enable = vi.fn<(runWaiting: boolean) => Promise<unknown>>().mockRejectedValueOnce(new Error('The accepted stop has not finished')).mockResolvedValue(undefined);
  const close = vi.fn();
  show(<ResourceEnableDialog resource={{ target, name: 'Team provider', enable }} onClose={close} />);
  await user.click(screen.getByRole('button', { name: i18n.t('resources.act.enable') }));
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(close).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('resources.act.enable') }));
  await waitFor(() => expect(close).toHaveBeenCalledOnce());
  expect(enable).toHaveBeenCalledTimes(2);
});

it('a bulk Enable makes no read per selection and keeps its one waiting choice off', async () => {
  const user = userEvent.setup({ delay: null });
  const first = vi.fn<(runWaiting: boolean) => Promise<unknown>>().mockResolvedValue(undefined);
  const second = vi.fn<(runWaiting: boolean) => Promise<unknown>>().mockResolvedValue(undefined);
  const other: FactTarget = { object: { kind: 'agent', id: '00000000-0000-4000-8000-000000000004' }, model_id: null };
  show(<ResourceEnableReview resources={[{ target, name: 'Team provider', enable: first }, { target: other, name: 'Build profile', enable: second }]} onClose={vi.fn()} />);
  const waiting = screen.getByRole('checkbox', { name: i18n.t('resourceImpact.runWaiting') });
  expect(waiting).not.toBeChecked();
  expect(screen.getAllByRole('checkbox')).toHaveLength(1);
  await user.click(screen.getByRole('button', { name: i18n.t('resources.act.enable') }));
  await waitFor(() => expect(first).toHaveBeenCalledExactlyOnceWith(false));
  await waitFor(() => expect(second).toHaveBeenCalledExactlyOnceWith(false));
  expect(read).not.toHaveBeenCalled();
});

it('a partial bulk Enable retains its result and retries only what failed', async () => {
  const user = userEvent.setup({ delay: null });
  const first = vi.fn<(runWaiting: boolean) => Promise<unknown>>().mockResolvedValue(undefined);
  const second = vi.fn<(runWaiting: boolean) => Promise<unknown>>().mockRejectedValueOnce(new Error('A resource changed')).mockResolvedValue(undefined);
  const close = vi.fn();
  show(<ResourceEnableReview resources={[{ target, name: 'Team provider', enable: first }, { target: { ...target, object: { ...target.object, id: '00000000-0000-4000-8000-000000000004' } }, name: 'Backup provider', enable: second }]} onClose={close} />);
  const confirm = screen.getByRole('button', { name: i18n.t('resources.act.enable') });
  await user.click(confirm);
  expect(await screen.findByText(i18n.t('source.job.succeeded'))).toBeVisible();
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(close).not.toHaveBeenCalled();
  expect(first).toHaveBeenCalledExactlyOnceWith(false);
  expect(second).toHaveBeenCalledExactlyOnceWith(false);
  await user.click(confirm);
  await waitFor(() => expect(close).toHaveBeenCalledOnce());
  expect(first).toHaveBeenCalledOnce();
  expect(second).toHaveBeenCalledTimes(2);
});

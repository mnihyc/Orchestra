/** Enable reviews authored waiting work and requires an explicit choice before running it. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ResourceEnableDialog, ResourceEnableReview } from '@/features/storage/Enable';
import type { FactTarget, ResourceImpact } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const read = vi.hoisted(() => vi.fn<(target: FactTarget) => Promise<ResourceImpact>>());
vi.mock('@/api/resource-impact', () => ({ resourceImpactQuery: (target: FactTarget) => ({ queryKey: ['enable-review', target], queryFn: () => read(target), retry: false }) }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

const target: FactTarget = { object: { kind: 'provider', id: '00000000-0000-4000-8000-000000000001' }, model_id: null };
function facts(): ResourceImpact {
  const empty = { count: 0, threads: 0, visible: [], hidden: 0, since: null };
  return { target, name: 'Team provider', stopping: false,
    facts: { subject: { kind: 'resource', object: target.object }, as_of: '2026-10-28T00:00:00Z', dependents: [], dependents_complete: true, uses: null },
    activity: { running: empty, stopped: empty, waiting: { count: 6, threads: 4, hidden: 3, since: '2026-03-03T00:00:00Z', visible: [{ object: { kind: 'thread', id: '00000000-0000-4000-8000-000000000002' }, name: 'Repair redirects', relation: 'binding', until: null }] } },
  };
}
beforeEach(() => read.mockResolvedValue(facts()));
afterEach(async () => { cleanup(); vi.resetAllMocks(); await setLocale('en'); });

it.each(['provider', 'model', 'agent', 'environment'] as const)('Enable of a %s leaves waiting messages off until selected', async (kind) => {
  const user = userEvent.setup({ delay: null });
  const selected: FactTarget = { object: { ...target.object, kind: kind === 'model' ? 'provider' : kind }, model_id: kind === 'model' ? '00000000-0000-4000-8000-000000000003' : null };
  const enable = vi.fn<(runWaiting: boolean) => Promise<unknown>>().mockResolvedValue(undefined);
  show(<ResourceEnableDialog resource={{ target: selected, name: 'Team provider', enable }} onClose={vi.fn()} />);
  const waiting = await screen.findByRole('checkbox', { name: i18n.t('resourceImpact.runWaiting') });
  expect(waiting).not.toBeChecked();
  expect(read).toHaveBeenCalledWith(selected);
  const count = screen.getByRole('button', { name: '6 messages in 4 threads' });
  await user.click(count);
  expect(await screen.findByText('Repair redirects')).toBeVisible();
  expect(screen.getByText('and 3 more')).toBeVisible();
  expect(enable).not.toHaveBeenCalled();
  await user.keyboard('{Escape}');
  await user.click(waiting);
  await user.click(screen.getByRole('button', { name: i18n.t('resources.act.enable') }));
  await waitFor(() => expect(enable).toHaveBeenCalledExactlyOnceWith(true));
});

it('the default confirmation passes false and never releases a waiting queue implicitly', async () => {
  const user = userEvent.setup({ delay: null });
  const enable = vi.fn<(runWaiting: boolean) => Promise<unknown>>().mockResolvedValue(undefined);
  show(<ResourceEnableDialog resource={{ target, name: 'Team provider', enable }} onClose={vi.fn()} />);
  await screen.findByRole('checkbox', { name: i18n.t('resourceImpact.runWaiting') });
  await user.click(screen.getByRole('button', { name: i18n.t('resources.act.enable') }));
  await waitFor(() => expect(enable).toHaveBeenCalledExactlyOnceWith(false));
});

it('an unfinished native stop names the wait and blocks Enable', async () => {
  read.mockResolvedValue({ ...facts(), stopping: true });
  const enable = vi.fn<(runWaiting: boolean) => Promise<unknown>>().mockResolvedValue(undefined);
  show(<ResourceEnableDialog resource={{ target, name: 'Team provider', enable }} onClose={vi.fn()} />);
  expect(await screen.findByText(i18n.t('resourceImpact.stopping'))).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('resources.act.enable') })).toBeDisabled();
  expect(enable).not.toHaveBeenCalled();
});

it('a bulk Enable reviews every selected resource and keeps its one waiting choice off', async () => {
  const user = userEvent.setup({ delay: null });
  const first = vi.fn<(runWaiting: boolean) => Promise<unknown>>().mockResolvedValue(undefined);
  const second = vi.fn<(runWaiting: boolean) => Promise<unknown>>().mockResolvedValue(undefined);
  const other: FactTarget = { object: { kind: 'agent', id: '00000000-0000-4000-8000-000000000004' }, model_id: null };
  show(<ResourceEnableReview resources={[{ target, name: 'Team provider', enable: first }, { target: other, name: 'Build profile', enable: second }]} onClose={vi.fn()} />);
  const waiting = await screen.findByRole('checkbox', { name: i18n.t('resourceImpact.runWaiting') });
  expect(waiting).not.toBeChecked();
  expect(screen.getAllByRole('checkbox')).toHaveLength(1);
  expect(read).toHaveBeenCalledWith(target);
  expect(read).toHaveBeenCalledWith(other);
  expect(first).not.toHaveBeenCalled();
  expect(second).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('resources.act.enable') }));
  await waitFor(() => expect(first).toHaveBeenCalledExactlyOnceWith(false));
  await waitFor(() => expect(second).toHaveBeenCalledExactlyOnceWith(false));
});

it('a partial bulk Enable retains its result and retries only what failed', async () => {
  const user = userEvent.setup({ delay: null });
  const first = vi.fn<(runWaiting: boolean) => Promise<unknown>>().mockResolvedValue(undefined);
  const second = vi.fn<(runWaiting: boolean) => Promise<unknown>>().mockRejectedValueOnce(new Error('A reviewed resource changed')).mockResolvedValue(undefined);
  const close = vi.fn();
  show(<ResourceEnableReview resources={[{ target, name: 'Team provider', enable: first }, { target: { ...target, object: { ...target.object, id: '00000000-0000-4000-8000-000000000004' } }, name: 'Backup provider', enable: second }]} onClose={close} />);
  await screen.findByRole('checkbox', { name: i18n.t('resourceImpact.runWaiting') });
  const confirm = screen.getByRole('button', { name: i18n.t('resources.act.enable') });
  await user.click(confirm);
  expect(await screen.findByText(i18n.t('source.job.succeeded'))).toBeVisible();
  expect(close).not.toHaveBeenCalled();
  expect(first).toHaveBeenCalledExactlyOnceWith(false);
  expect(second).toHaveBeenCalledExactlyOnceWith(false);
  await user.click(confirm);
  await waitFor(() => expect(close).toHaveBeenCalledOnce());
  expect(first).toHaveBeenCalledOnce();
  expect(second).toHaveBeenCalledTimes(2);
});

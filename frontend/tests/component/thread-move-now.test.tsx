/** A thread moves to another topic of its tree at once: no review is drawn, a refusal is said, the notice offers Undo. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as NodeApi from '@/api/nodes';
import { MoveToTopic } from '@/features/thread-move/MoveToTopic';
import type { PickedPlace } from '@/features/settings/PlacePicker';
import type { Thread, ThreadMovePlan } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const calls = vi.hoisted(() => ({ preview: vi.fn(), move: vi.fn(), apply: vi.fn(), change: vi.fn() }));
vi.mock('@/api/thread-move', () => ({ previewThreadMove: calls.preview, moveThread: calls.move }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof NodeApi>(),
  nodeQuery: (id: string) => ({ queryKey: ['thread-move-now', id], queryFn: () => ({ id, kind: 'topic', name: id === 'docs' ? 'Docs' : 'Landing', access: { permissions: ['topic.start_threads'] } }) }) }));
vi.mock('@/realtime/stores/thread', async (original) => ({ ...await original<object>(), threadWriter: { apply: calls.apply } }));
vi.mock('@/features/change/target', async (original) => ({ ...await original<object>(), openChange: calls.change }));
vi.mock('@/features/duplicate/CopyPlacePicker', () => ({ CopyPlacePicker: ({ onChange }: { onChange: (place: PickedPlace) => void }) => <>
  <button type="button" onClick={() => onChange({ id: 'docs', kind: 'topic', color: null, folder: false, path: 'Acme / Docs', rootOrgId: 'acme' })}>docs</button>
  <button type="button" onClick={() => onChange({ id: 'away', kind: 'topic', color: null, folder: false, path: 'Other / Away', rootOrgId: 'other' })}>away</button>
</> }));

const thread = { id: 'thread', kind: 'thread', name: 'Login', org_id: 'acme', topic_id: 'landing', revision: 2 } as unknown as Thread;
function plan(to: string, losing = 0, blockers: ThreadMovePlan['blockers'] = []): ThreadMovePlan {
  return { to: { id: to, kind: 'topic', name: to === 'docs' ? 'Docs' : 'Landing', redacted: false }, expected_revision: 2, preflight_id: `review-${to}`, blockers,
    access: { pages: 1, orgs: null, gaining: [], losing: Array.from({ length: losing }, (_, index) => ({ user: { id: `p${index}` }, permissions: [], pages: 1 })), more: 0 },
    settings: [], unchanged: { owner: { id: 'owner', display_name: 'Ada' }, environment: null, directory: '/' } } as unknown as ThreadMovePlan;
}
function Host({ onClose }: { onClose: () => void }) {
  const [open, setOpen] = useState(true);
  return open ? <MoveToTopic thread={thread} close={() => { setOpen(false); onClose(); }} /> : null;
}

beforeEach(() => {
  calls.preview.mockImplementation((_id: string, to: string) => Promise.resolve(plan(to, to === 'docs' ? 1 : 0)));
  calls.move.mockImplementation(() => Promise.resolve({ ...thread, revision: 3 }));
});
afterEach(async () => { cleanup(); vi.resetAllMocks(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`moves at once without drawing a review and offers Undo (${locale})`, async () => {
    await setLocale(locale);
    const user = userEvent.setup();
    const close = vi.fn();
    show(<Host onClose={close} />);
    await user.click(screen.getByRole('button', { name: 'docs' }));
    const move = await screen.findByRole('button', { name: i18n.t('threadMove.confirm', { topic: 'Docs' }) });
    await waitFor(() => expect(move).toBeEnabled());
    expect(calls.preview).not.toHaveBeenCalled();
    // Nothing the move leaves as it was is narrated: no owner, environment or directory line.
    expect(screen.queryByText(/Ada/)).not.toBeInTheDocument();
    await user.click(move);
    await waitFor(() => expect(calls.move).toHaveBeenCalledExactlyOnceWith('thread', plan('docs', 1), expect.any(String)));
    await waitFor(() => expect(close).toHaveBeenCalledOnce());
    expect(await screen.findByText(i18n.t('threadMove.done', { topic: 'Docs' }))).toBeVisible();
    expect(screen.getByText(i18n.t('places.move.lost', { count: 1 }))).toBeVisible();
    await user.click((await screen.findAllByRole('button', { name: i18n.t('places.toast.undo') }))[0]!);
    await waitFor(() => expect(calls.preview).toHaveBeenLastCalledWith('thread', 'landing'));
    await waitFor(() => expect(calls.move).toHaveBeenCalledTimes(2));
  });
}

it('a refusal in the stored plan is said in the dialog and nothing moves', async () => {
  const user = userEvent.setup();
  calls.preview.mockResolvedValue(plan('docs', 0, [{ code: 'conflict.locked', message: 'Wait for the change job to finish', missing: [] }]));
  const close = vi.fn();
  show(<Host onClose={close} />);
  await user.click(screen.getByRole('button', { name: 'docs' }));
  const move = await screen.findByRole('button', { name: i18n.t('threadMove.confirm', { topic: 'Docs' }) });
  await waitFor(() => expect(move).toBeEnabled());
  await user.click(move);
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(calls.move).not.toHaveBeenCalled();
  expect(close).not.toHaveBeenCalled();
});

it('a topic of another tree still opens the move to another organization', async () => {
  const user = userEvent.setup();
  show(<Host onClose={vi.fn()} />);
  await user.click(screen.getByRole('button', { name: 'away' }));
  expect(calls.change).toHaveBeenCalledWith({ kind: 'thread', id: 'thread', name: 'Login' }, expect.objectContaining({ kind: 'move-out' }));
});

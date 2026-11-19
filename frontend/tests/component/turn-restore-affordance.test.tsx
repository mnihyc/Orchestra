/** Only a reader who can restore a recorded completed turn is offered its file action; ordinary turns add no unavailable-copy paragraph or metadata request. */
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Source from '@/api/source';
import type * as OpenView from '@/features/workbench/openView';
import { RestoreFromTurn } from '@/features/source/Restore';
import type { RestorePlan, Snapshot, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import { ACME } from '../doubles/fixtures';
import { turn } from '../doubles/thread-rows';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<() => Promise<Snapshot>>(), plan: vi.fn<() => Promise<RestorePlan>>(), panel: vi.fn() }));
vi.mock('@/api/source', async (load) => ({ ...await load<typeof Source>(), snapshotQuery: (id: string) => ({ queryKey: ['restore-affordance', id], queryFn: calls.read }), planRestore: calls.plan }));
vi.mock('@/features/workbench/openView', async (load) => ({ ...await load<typeof OpenView>(), revealView: calls.panel }));
let native: Thread;
let recorded: Snapshot;
beforeEach(() => {
  calls.read.mockReset(); calls.plan.mockReset(); calls.panel.mockReset();
  const world = new World(); const person = world.people.get('ada');
  const node = world.places.rowOf(ACME.mainThread);
  if (person === undefined || node === undefined) throw new Error('The restore test needs its native thread');
  native = { ...world.threads.thread(person, node), environment_permissions: ['files.view', 'files.manage'] };
  if (native.environment === null) throw new Error('The recorded files belong to an environment');
  recorded = { id: 'before-this-turn', kind: 'turn', state: 'complete', directory: native.directory, environment: native.environment, env_generation: 1,
    file_count: 1, total_bytes: 10, missing_count: 0, storage: 'environment', thread_id: native.id, label: '', repository_head: null,
    created_at: '2026-10-15T10:00:00Z', completed_at: '2026-10-15T10:00:01Z', created_by: null, expires_at: null,
    selection: { include: [], exclude: [] }, unavailable_reason: null };
  calls.read.mockResolvedValue(recorded);
  calls.plan.mockResolvedValue({ blockers: [], directory: native.directory, environment: native.environment, expires_at: '2026-10-15T10:15:00Z', generation: 1,
    ignore_fingerprint: 'ignore', ignored: [], namespace_fingerprint: 'namespace', options: {
      active_turn: 'wait', keep_previous: false, stop_processes: false, verify_timeout_secs: 60, wait_minutes: 10,
    },
    paths: [{ action: 'change', expected: 'previous-content', later_change: false, later_writers: [], path: 'src/login.ts' }],
    preflight_id: 'restore-plan', snapshot: recorded, undo_available: true, writers: [] });
});
afterEach(async () => { threadWriter.evict(native.id); await setLocale('en'); document.documentElement.removeAttribute('data-theme'); });

it.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('keeps ordinary and read-only turns quiet, then offers a real recorded restore without permanent warning prose in %s/%s', async (locale, look) => {
  await setLocale(locale); document.documentElement.setAttribute('data-theme', look);
  threadWriter.reset(native.id, { thread: native }, false);
  const finished = { ...turn('finished', { ordinal: 1, state: 'completed', inputId: 'input', completedAt: recorded.completed_at! }), before_snapshot_id: null };
  const view = show(<RestoreFromTurn threadId={native.id} turn={finished} />);
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
  expect(screen.queryByText(/^(?:Recorded state unknown|(?:已)?记录(?:状态|的状态)(?:未知|不明|不详))$/u)).not.toBeInTheDocument();
  expect(calls.read).not.toHaveBeenCalled();
  act(() => threadWriter.apply(native.id, { thread: { ...native, environment: null, seq: native.seq + 1 } }));
  view.rerender(<RestoreFromTurn threadId={native.id} turn={{ ...finished, before_snapshot_id: recorded.id }} />);
  expect(screen.queryByRole('button')).not.toBeInTheDocument(); expect(calls.read).not.toHaveBeenCalled();
  act(() => threadWriter.apply(native.id, { thread: { ...native, environment_permissions: ['files.view'], seq: native.seq + 2 } }));
  expect(screen.queryByRole('button')).not.toBeInTheDocument(); expect(calls.read).not.toHaveBeenCalled();
  view.rerender(<RestoreFromTurn threadId={native.id} turn={{ ...finished, before_snapshot_id: recorded.id, state: 'running', completed_at: null }} />);
  act(() => threadWriter.apply(native.id, { thread: { ...native, seq: native.seq + 3 } }));
  expect(screen.queryByRole('button')).not.toBeInTheDocument(); expect(calls.read).not.toHaveBeenCalled();
  view.rerender(<RestoreFromTurn threadId={native.id} turn={{ ...finished, before_snapshot_id: recorded.id }} />);
  const restore = screen.getByRole('button', { name: i18n.t('source.restore.open') });
  expect(restore).toBeEnabled();
  expect(calls.read).not.toHaveBeenCalled(); expect(calls.plan).not.toHaveBeenCalled();
  await userEvent.click(restore);
  await waitFor(() => expect(calls.read).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(calls.plan).toHaveBeenCalledTimes(1));
  expect(screen.queryByText(/^(?:Recorded state unknown|(?:已)?记录(?:状态|的状态)(?:未知|不明|不详))$/u)).not.toBeInTheDocument();
});

it('offers one explicit Retry for a failed recorded-copy read, then restores that turn’s action from the returned metadata', async () => {
  calls.read.mockRejectedValueOnce(new Error('The copy could not be read'));
  threadWriter.reset(native.id, { thread: native }, false);
  const finished = { ...turn('finished', { ordinal: 1, state: 'completed', inputId: 'input', completedAt: '2026-10-15T10:00:01Z' }), before_snapshot_id: recorded.id };
  show(<RestoreFromTurn threadId={native.id} turn={finished} />);
  expect(calls.read).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('source.restore.open') }));
  const retry = await screen.findByRole('button', { name: i18n.t('common.tryAgain') });
  expect(screen.queryByText(/^(?:Recorded state unknown|(?:已)?记录(?:状态|的状态)(?:未知|不明|不详))$/u)).not.toBeInTheDocument();
  await userEvent.click(retry);
  expect(await screen.findByRole('button', { name: i18n.t('source.restore.confirm') })).toBeEnabled();
  expect(calls.read).toHaveBeenCalledTimes(2);
});

it.each(['en', 'zh-CN'] as const)('reads an unavailable copy only on opening and shows its reason with the keyboard-reachable Review changes action in %s', async (locale) => {
  await setLocale(locale); calls.read.mockResolvedValue({ ...recorded, state: 'unavailable', unavailable_reason: 'store_unavailable' });
  threadWriter.reset(native.id, { thread: native }, false);
  const finished = { ...turn('finished', { ordinal: 1, state: 'completed', inputId: 'input', completedAt: '2026-10-15T10:00:01Z' }), before_snapshot_id: recorded.id };
  show(<RestoreFromTurn threadId={native.id} turn={finished} />);
  expect(calls.read).not.toHaveBeenCalled();
  expect(screen.queryByText(i18n.t('source.restore.why.storeUnavailable'))).not.toBeInTheDocument();
  const user = userEvent.setup(); await user.click(screen.getByRole('button', { name: i18n.t('source.restore.open') }));
  const review = await screen.findByRole('button', { name: i18n.t('source.restore.review') });
  expect(screen.getByText(i18n.t('source.restore.why.storeUnavailable'))).toBeVisible();
  expect(calls.read).toHaveBeenCalledTimes(1); expect(calls.plan).not.toHaveBeenCalled();
  act(() => review.focus()); expect(review).toHaveFocus(); await user.keyboard('{Enter}');
  expect(calls.panel).toHaveBeenCalledExactlyOnceWith('changes');
});

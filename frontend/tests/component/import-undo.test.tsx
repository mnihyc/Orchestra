/** Undo shows what would be removed and asks for review again when its effects change. */
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { ImportUndo } from '@/features/people/ImportUndo';
import type { ImportCounts, ImportUndoPlan, Job } from '@/generated/api-types';
import { NetworkError } from '@/lib/requests';
import { show } from './support';

const calls = vi.hoisted(() => ({ plan: vi.fn(), start: vi.fn() }));
vi.mock('@/api/imports', async (original) => ({ ...await original<object>(), planImportUndo: calls.plan, startImportUndo: calls.start }));

const none: ImportCounts = { places: 0, folders: 0, documents: 0, chats: 0, threads: 0, files: 0, messages: 0, turns: 0, comments: 0, versions: 0 };
const plan: ImportUndoPlan = { import: 'import', counts: { ...none, places: 1 }, merged: none, added_since: 0, edited_since: 0, restore_until: '2099-01-01T00:00:00Z', blockers: [] };
const empty: ImportUndoPlan = { ...plan, counts: none };
const job = { id: 'job' } as Job;

beforeEach(() => { calls.plan.mockReset(); calls.start.mockReset(); });

it('lists what is removed and kept and refuses to start from an outdated plan', async () => {
  const updated = { ...plan, counts: { ...none, places: 2 } };
  calls.plan.mockResolvedValueOnce(empty).mockResolvedValueOnce(updated);
  calls.start.mockResolvedValue(job);
  const onStarted = vi.fn(), onClose = vi.fn(), user = userEvent.setup();
  show(<ImportUndo id="import" onClose={onClose} onStarted={onStarted} />);
  const button = await screen.findByRole('button', { name: 'Undo import' });
  await waitFor(() => expect(button).toBeEnabled());
  expect(screen.getByText('Nothing to remove')).toBeVisible();
  expect(screen.getByText('Replaced, skipped and unchanged objects stay')).toBeVisible();
  await user.click(button);
  expect(await screen.findByText('The plan changed; review it again')).toBeVisible();
  expect(screen.getByText('Will remove: 2 places')).toBeVisible();
  expect(calls.start).not.toHaveBeenCalled();
  expect(button).toBeEnabled();
  expect(onStarted).not.toHaveBeenCalled();
  expect(onClose).not.toHaveBeenCalled();
});

it.each([
  { ...empty, counts: { ...none, places: 1 } },
  { ...empty, counts: { ...none, threads: 1 } },
  { ...empty, counts: { ...none, chats: 1 } },
  { ...empty, counts: { ...none, folders: 1 } },
  { ...empty, counts: { ...none, documents: 2 }, edited_since: 1 },
  { ...empty, merged: { ...none, messages: 1 } },
  { ...empty, merged: { ...none, versions: 1 } },
  { ...empty, added_since: 2 },
])('allows removal after the current preflight checks return no blockers', async (checked) => {
  calls.plan.mockResolvedValue(checked);
  show(<ImportUndo id="import" onClose={vi.fn()} onStarted={vi.fn()} />);
  const button = await screen.findByRole('button', { name: 'Undo import' });
  await waitFor(() => expect(button).toBeEnabled());
  if (checked.counts.places > 0) expect(screen.getByText('Will remove: 1 place')).toBeVisible();
  if (checked.merged.messages > 0) expect(screen.getByText('Will remove: 1 message')).toBeVisible();
  if (checked.merged.versions > 0) expect(screen.getByText('Will remove: 1 version')).toBeVisible();
  if (checked.edited_since > 0) expect(screen.getByText('1 imported document was edited later')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Undo import' })).toBeEnabled();
  expect(calls.start).not.toHaveBeenCalled();
});

it('does not offer confirmation when the server reports a blocker', async () => {
  calls.plan.mockResolvedValue({ ...plan, blockers: [{ code: 'import.forbidden', message: 'Server words' }] });
  show(<ImportUndo id="import" onClose={vi.fn()} onStarted={vi.fn()} />);
  expect(await screen.findByText('You may not make this here')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Undo import' })).toBeDisabled();
});

it('confirms a removing plan after a matching clean recheck', async () => {
  calls.plan.mockResolvedValue(plan);
  calls.start.mockResolvedValue(job);
  const onStarted = vi.fn(), onClose = vi.fn();
  show(<ImportUndo id="import" onClose={onClose} onStarted={onStarted} />);
  const button = await screen.findByRole('button', { name: 'Undo import' });
  await waitFor(() => expect(button).toBeEnabled());
  await userEvent.click(button);
  await waitFor(() => expect(onStarted).toHaveBeenCalledExactlyOnceWith(job));
  expect(calls.plan).toHaveBeenCalledTimes(2);
  expect(calls.start).toHaveBeenCalledExactlyOnceWith('import', expect.any(String));
  expect(onClose).toHaveBeenCalledOnce();
});

it('retries an unconfirmed undo with the same act identity', async () => {
  calls.plan.mockResolvedValue(empty);
  calls.start.mockRejectedValueOnce(new NetworkError('failed')).mockResolvedValueOnce(job);
  const onStarted = vi.fn(), user = userEvent.setup();
  show(<ImportUndo id="import" onClose={vi.fn()} onStarted={onStarted} />);
  const button = await screen.findByRole('button', { name: 'Undo import' });
  await waitFor(() => expect(button).toBeEnabled());
  await user.click(button);
  const retry = await screen.findByRole('button', { name: 'Retry' });
  expect(screen.getByText('Not confirmed. Check Your imports or retry this request.')).toBeVisible();
  await user.click(retry);
  await waitFor(() => expect(onStarted).toHaveBeenCalledExactlyOnceWith(job));
  expect(calls.start.mock.calls[0]).toEqual(calls.start.mock.calls[1]);
  expect(calls.plan).toHaveBeenCalledTimes(2);
});

it('does not send an undo as a different reader after the recheck', async () => {
  const before = client.session.getState();
  client.session.setState({ ...before, userId: 'first', readerVersion: 1, status: 'signed_in' });
  try {
    let finish: (value: ImportUndoPlan) => void = () => { throw new Error('Undo was not rechecked'); };
    calls.plan.mockResolvedValueOnce(empty).mockImplementationOnce(() => new Promise<ImportUndoPlan>((resolve) => { finish = resolve; }));
    const user = userEvent.setup();
    show(<ImportUndo id="import" onClose={vi.fn()} onStarted={vi.fn()} />);
    const button = await screen.findByRole('button', { name: 'Undo import' });
    await waitFor(() => expect(button).toBeEnabled());
    await user.click(button);
    await waitFor(() => expect(calls.plan).toHaveBeenCalledTimes(2));
    act(() => { client.session.setState({ userId: 'second', readerVersion: 2, status: 'signed_in' }); });
    await act(async () => { finish(empty); await Promise.resolve(); });
    expect(calls.start).not.toHaveBeenCalled();
  } finally { client.session.setState(before); }
});

it('does not hand a late undo job to a different reader', async () => {
  const before = client.session.getState();
  client.session.setState({ ...before, userId: 'first', readerVersion: 1, status: 'signed_in' });
  try {
    let finish: (value: Job) => void = () => { throw new Error('Undo was not sent'); };
    calls.plan.mockResolvedValue(empty);
    calls.start.mockImplementation(() => new Promise<Job>((resolve) => { finish = resolve; }));
    const onStarted = vi.fn(), user = userEvent.setup();
    show(<ImportUndo id="import" onClose={vi.fn()} onStarted={onStarted} />);
    const button = await screen.findByRole('button', { name: 'Undo import' });
    await waitFor(() => expect(button).toBeEnabled());
    await user.click(button);
    await waitFor(() => expect(calls.start).toHaveBeenCalledOnce());
    act(() => { client.session.setState({ userId: 'second', readerVersion: 2, status: 'signed_in' }); });
    await act(async () => { finish(job); await Promise.resolve(); });
    expect(onStarted).not.toHaveBeenCalled();
  } finally { client.session.setState(before); }
});

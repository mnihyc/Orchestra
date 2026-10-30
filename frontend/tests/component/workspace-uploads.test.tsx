/** Workspace uploads share one conflict choice, carry large files by blob and stop cancelled transfers. */
import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import type * as Workspace from '@/api/workspace';
import { ApiError, client } from '@/api/client';
import { queryClient } from '@/api/query';
import { useWorkspaceUploads } from '@/features/files/Uploads';
import type { FileStat, Job } from '@/generated/api-types';
import { show } from './support';

const calls = vi.hoisted(() => ({ write: vi.fn<typeof Workspace.writeUploadedFile>(), finish: vi.fn<typeof Workspace.finishWorkspaceUpload>(), refresh: vi.fn<(...paths: string[]) => Promise<void>>(), cancel: vi.fn(), slow: false, started: vi.fn() }));
vi.mock('@/api/workspace', async (load) => ({ ...await load<typeof Workspace>(), writeUploadedFile: calls.write, finishWorkspaceUpload: calls.finish, makeDirectory: vi.fn(() => Promise.resolve({})) }));
vi.mock('@/lib/uploads', () => ({ UploadTask: class {
  readonly uploadId = '10000000-0000-0000-0000-000000000001';
  constructor(readonly file: File) {}
  run(signal: AbortSignal, progress: (received: number) => void) {
    calls.started(); progress(calls.slow ? 7 : this.file.size);
    if (calls.slow) return new Promise((_resolve, reject) => { signal.addEventListener('abort', () => reject(new DOMException('Upload cancelled', 'AbortError')), { once: true }); });
    return Promise.resolve({ blob: this.file.name.padEnd(64, 'a'), size: this.file.size, media_type: this.file.type, name: this.file.name });
  }
  cancel() { calls.cancel(); return Promise.resolve(); }
} }));

const door = { environmentId: 'workspace', threadId: null };
function Probe({ files, blocked = false }: { files: File[]; blocked?: string | false }) {
  const uploads = useWorkspaceUploads(door, blocked, calls.refresh);
  return <><button onClick={() => uploads.add('assets', files)}>Upload files</button>{uploads.progress}{uploads.dialog}</>;
}
const exists = () => new ApiError({ type: 'orch:error:conflict.stale_revision', code: 'conflict.stale_revision', status: 409, title: 'Exists', detail: 'Exists', request_id: 'test' }, null);
beforeEach(() => { calls.write.mockReset(); calls.finish.mockReset(); calls.finish.mockResolvedValue(undefined); calls.refresh.mockReset(); calls.refresh.mockResolvedValue(undefined); calls.cancel.mockClear(); calls.started.mockClear(); calls.slow = false; });

function stat(path: string, size = 3): FileStat { return { type: 'file', path, name: path.slice(path.lastIndexOf('/') + 1), size, mtime: '2026-10-25T00:00:00Z', mode: 0o644, link_target: null, cached: false, hash: null, media_type: 'text/plain', is_text: true }; }

it('asks once for the whole batch and keeps both names even across final-write races', async () => {
  calls.write.mockImplementation((_door, path: string) => path.includes(' (2)') ? Promise.resolve(stat(path)) : Promise.reject(exists()));
  show(<Probe files={[new File(['one'], 'a.txt'), new File(['two'], 'b.txt')]} />);
  fireEvent.click(screen.getByRole('button', { name: 'Upload files' }));
  expect(await screen.findByRole('dialog')).toHaveTextContent('Files already exist');
  fireEvent.click(screen.getByRole('button', { name: 'Keep both' }));
  await waitFor(() => expect(calls.finish).toHaveBeenCalledTimes(1));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(calls.write.mock.calls.map((call) => call[1]).sort()).toEqual(['assets/a (2).txt', 'assets/a.txt', 'assets/b (2).txt', 'assets/b.txt']);
  expect(calls.refresh).toHaveBeenCalledTimes(2);
  expect(calls.refresh).toHaveBeenCalledWith('assets');
});

it('uploads beyond the old raw-body limit without buffering another copy of the file', async () => {
  const file = new File([new Uint8Array(9 * 1024 * 1024)], 'large.bin');
  calls.write.mockResolvedValue(stat('assets/large.bin', file.size));
  const buffer = vi.spyOn(file, 'arrayBuffer');
  show(<Probe files={[file]} />); fireEvent.click(screen.getByRole('button', { name: 'Upload files' }));
  await waitFor(() => expect(calls.finish).toHaveBeenCalledTimes(1));
  const attempt = calls.write.mock.calls[0]?.[7];
  expect(attempt?.id).toEqual(expect.any(String));
  expect(calls.write).toHaveBeenCalledWith(door, 'assets/large.bin', expect.any(String), 'none', expect.any(AbortSignal), expect.any(String), expect.any(Function), { id: attempt?.id });
  expect(buffer).not.toHaveBeenCalled();
});

it('cancel stops the transfer, writes no file and records its receipt as cancelled', async () => {
  calls.slow = true;
  show(<Probe files={[new File(['a'.repeat(100)], 'partial.txt')]} />);
  fireEvent.click(screen.getByRole('button', { name: 'Upload files' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Cancel partial.txt' }));
  await waitFor(() => expect(calls.finish).toHaveBeenCalledTimes(1));
  expect(calls.cancel).toHaveBeenCalledOnce();
  expect(calls.write).not.toHaveBeenCalled();
  expect(calls.finish.mock.calls[0]?.[2]).toMatchObject({ folder: 'assets', outcome: 'cancelled', files: [{ name: 'partial.txt', upload_id: '10000000-0000-0000-0000-000000000001', outcome: 'cancelled' }] });
});

it('keeps an already dispatched native write stopping until its separate receipt even when HTTP has ended', async () => {
  let acknowledge: () => void = () => undefined;
  const proof = new Promise<void>((resolve) => { acknowledge = resolve; });
  const stop = vi.fn(() => proof);
  calls.write.mockImplementation((_door, _path, _blob, _expected, signal, _batch, report) => {
    report?.({ stop });
    return new Promise((_resolve, reject) => { signal.addEventListener('abort', () => reject(new DOMException('HTTP ended', 'AbortError')), { once: true }); });
  });
  show(<Probe files={[new File(['already at the machine'], 'native.txt')]} />);
  try {
    fireEvent.click(screen.getByRole('button', { name: 'Upload files' }));
    await waitFor(() => expect(calls.write).toHaveBeenCalledOnce());
    fireEvent.click(screen.getByRole('button', { name: 'Cancel native.txt' }));
    await waitFor(() => expect(stop).toHaveBeenCalledOnce());
    expect(screen.getByText('Stopping…')).toBeVisible();
    expect(calls.cancel).not.toHaveBeenCalled();
    expect(calls.finish).not.toHaveBeenCalled();
    await act(async () => { acknowledge(); await proof; });
    await waitFor(() => expect(calls.finish).toHaveBeenCalledOnce());
    expect(calls.cancel).toHaveBeenCalledOnce();
    expect(calls.finish.mock.calls[0]?.[2]).toMatchObject({ outcome: 'cancelled', files: [{ name: 'native.txt', outcome: 'cancelled' }] });
    expect(calls.refresh).not.toHaveBeenCalled();
  } finally { acknowledge(); }
});

it('retains an unconfirmed native stop and retries stopping without another write or premature history', async () => {
  const stop = vi.fn<() => Promise<void>>().mockRejectedValueOnce(new Error('Lost connection')).mockResolvedValue(undefined);
  calls.write.mockImplementation((_door, _path, _blob, _expected, signal, _batch, report) => {
    report?.({ stop });
    return new Promise((_resolve, reject) => { signal.addEventListener('abort', () => reject(new DOMException('HTTP ended', 'AbortError')), { once: true }); });
  });
  show(<Probe files={[new File(['machine copy'], 'native.txt')]} />);
  fireEvent.click(screen.getByRole('button', { name: 'Upload files' }));
  await waitFor(() => expect(calls.write).toHaveBeenCalledOnce());
  fireEvent.click(screen.getByRole('button', { name: 'Cancel native.txt' }));
  await screen.findByRole('alert');
  expect(screen.getByText('Stopping…')).toBeVisible();
  expect(calls.finish).not.toHaveBeenCalled();
  expect(calls.cancel).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
  await waitFor(() => expect(calls.finish).toHaveBeenCalledOnce());
  expect(stop).toHaveBeenCalledTimes(2);
  expect(calls.write).toHaveBeenCalledOnce();
  expect(calls.cancel).toHaveBeenCalledOnce();
});

it('refreshes a file saved before Cancel only after its late native receipt and keeps it out of incomplete attempts', async () => {
  let acknowledge: () => void = () => undefined;
  let written: (value: FileStat) => void = () => undefined;
  const proof = new Promise<void>((resolve) => { acknowledge = resolve; });
  const receipt = new Promise<FileStat>((resolve) => { written = resolve; });
  const stop = vi.fn(() => proof);
  calls.write.mockImplementation((_door, _path, _blob, _expected, _signal, _batch, report) => {
    report?.({ stop });
    return receipt;
  });
  show(<Probe files={[new File(['already saved'], 'saved.txt')]} />);
  try {
    fireEvent.click(screen.getByRole('button', { name: 'Upload files' }));
    await waitFor(() => expect(calls.write).toHaveBeenCalledOnce());
    fireEvent.click(screen.getByRole('button', { name: 'Cancel saved.txt' }));
    await waitFor(() => expect(stop).toHaveBeenCalledOnce());
    expect(screen.getByText('Stopping…')).toBeVisible();
    expect(calls.refresh).not.toHaveBeenCalled();
    expect(calls.finish).not.toHaveBeenCalled();
    await act(async () => { acknowledge(); written(stat('assets/saved.txt')); await proof; await receipt; });
    await waitFor(() => expect(calls.finish).toHaveBeenCalledOnce());
    expect(calls.refresh).toHaveBeenCalledExactlyOnceWith('assets');
    expect(calls.write).toHaveBeenCalledOnce();
    expect(calls.finish.mock.calls[0]?.[2]).toMatchObject({ outcome: 'complete', files: [] });
  } finally { acknowledge(); written(stat('assets/saved.txt')); }
});

it('keeps one admitted write when the saved receipt reply is lost and retried', async () => {
  const actual = await vi.importActual<typeof Workspace>('@/api/workspace');
  calls.write.mockImplementation(actual.writeUploadedFile);
  const before = client.session.getState();
  const jobs = new Map<string, Job>();
  const admissions: string[] = [];
  let reads = 0;
  const file = new File(['saved before the reply was lost'], 'receipt.txt');
  const receipt = stat('assets/receipt.txt', file.size);
  vi.stubGlobal('fetch', vi.fn((request: Request): Promise<Response> => {
    const path = new URL(request.url).pathname;
    const id = path.split('/').at(-2) ?? '';
    if (request.method === 'POST' && path.endsWith('/write')) {
      admissions.push(id);
      if (!jobs.has(id)) {
        const now = new Date().toISOString();
        jobs.set(id, { id, kind: 'delivery', state: 'running', node_id: null, environment_id: door.environmentId,
          provider_id: null, batch_id: null, result: null, error: null, log: null, created_by: null,
          created_at: now, started_at: now, finished_at: null,
          progress: { phase: 'files', phases: [{ name: 'files', state: 'running', weight: 100 }], percent: 0,
            files_done: 0, files_total: 1, bytes_done: 0, bytes_total: file.size, rate_bps: null, eta_seconds: null, message: null } });
      }
      return Promise.resolve(Response.json(jobs.get(id), { status: 202 }));
    }
    if (request.method === 'GET' && path.endsWith('/write')) {
      expect(jobs.get(id)?.state).toBe('succeeded');
      reads += 1;
      if (reads === 1) return Promise.reject(new TypeError('The saved receipt reply was lost'));
      return Promise.resolve(Response.json(receipt));
    }
    return Promise.reject(new Error(`Unexpected ${request.method} ${path}`));
  }));
  show(<Probe files={[file]} />);
  try {
    fireEvent.click(screen.getByRole('button', { name: 'Upload files' }));
    await waitFor(() => expect(admissions).toHaveLength(1));
    const id = admissions[0]!;
    await waitFor(() => expect(queryClient.getQueryCache().find({ queryKey: ['job', id], exact: true })?.getObserversCount()).toBe(1));
    const running = jobs.get(id)!;
    const completed: Job = { ...running, state: 'succeeded', result: { ...receipt }, finished_at: new Date().toISOString(),
      progress: { ...running.progress, percent: 100, bytes_done: file.size, files_done: 1,
        phases: [{ name: 'files', state: 'done', weight: 100 }] } };
    jobs.set(id, completed);
    queryClient.setQueryData(['job', id], completed);
    const retry = await screen.findByRole('button', { name: /Retry.*receipt\.txt/ });
    expect(calls.finish).not.toHaveBeenCalled();
    fireEvent.click(retry);
    await waitFor(() => expect(admissions).toHaveLength(2));
    expect(admissions[1]).toBe(id);
    expect(jobs.size).toBe(1);
    await waitFor(() => expect(calls.finish).toHaveBeenCalledOnce());
    expect(calls.refresh).toHaveBeenCalledExactlyOnceWith('assets');
    expect(reads).toBe(2);
  } finally {
    await act(async () => { client.session.setState({ readerVersion: (before.readerVersion ?? 0) + 1 }); await Promise.resolve(); });
    queryClient.clear();
    client.session.setState({ ...before, readerVersion: (before.readerVersion ?? 0) + 2 });
    vi.unstubAllGlobals();
  }
});

it('retrying failed stop cleanup preserves a file already confirmed saved instead of recording it cancelled', async () => {
  let saved: (file: FileStat) => void = () => undefined;
  const receipt = new Promise<FileStat>((resolve) => { saved = resolve; });
  const stop = vi.fn<() => Promise<void>>().mockRejectedValueOnce(new Error('The stop reply was lost')).mockResolvedValue(undefined);
  calls.write.mockImplementation((_door, _path, _blob, _expected, _signal, _batch, report) => {
    report?.({ stop });
    return receipt;
  });
  show(<Probe files={[new File(['saved before stop cleanup'], 'saved.txt')]} />);
  try {
    fireEvent.click(screen.getByRole('button', { name: 'Upload files' }));
    await waitFor(() => expect(calls.write).toHaveBeenCalledOnce());
    fireEvent.click(screen.getByRole('button', { name: 'Cancel saved.txt' }));
    await screen.findByRole('alert');
    expect(calls.finish).not.toHaveBeenCalled();
    await act(async () => { saved(stat('assets/saved.txt')); await receipt; });
    await waitFor(() => expect(calls.refresh).toHaveBeenCalledExactlyOnceWith('assets'));
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(calls.finish).toHaveBeenCalledOnce());
    expect(calls.finish.mock.calls[0]?.[2]).toMatchObject({ outcome: 'complete', files: [] });
    expect(calls.write).toHaveBeenCalledOnce();
    expect(stop).toHaveBeenCalledTimes(2);
    expect(calls.cancel).toHaveBeenCalledOnce();
  } finally { saved(stat('assets/saved.txt')); }
});

it.each(['Read only', 'Machine is offline'])('starts no upload when %s', (blocked) => {
  show(<Probe blocked={blocked} files={[new File(['a'], 'a.txt')]} />);
  fireEvent.click(screen.getByRole('button', { name: 'Upload files' }));
  expect(calls.started).not.toHaveBeenCalled(); expect(calls.write).not.toHaveBeenCalled();
});

it('does not finish an old private batch as the person who signs in next', async () => {
  const before = client.session.getState();
  calls.slow = true;
  show(<Probe files={[new File(['private unfinished bytes'], 'private-name.txt')]} />);
  fireEvent.click(screen.getByRole('button', { name: 'Upload files' }));
  await waitFor(() => expect(calls.started).toHaveBeenCalledOnce());
  try {
    await act(async () => {
      client.session.setState({ readerVersion: (before.readerVersion ?? 0) + 1, userId: 'another-person' });
      await Promise.resolve();
    });
    await waitFor(() => expect(calls.cancel).toHaveBeenCalled());
    expect(calls.finish).not.toHaveBeenCalled(); expect(calls.write).not.toHaveBeenCalled();
    expect(screen.queryByText('private-name.txt')).not.toBeInTheDocument();
  } finally {
    act(() => client.session.setState({ ...before, readerVersion: (client.session.getState().readerVersion ?? 0) + 1 }));
  }
});

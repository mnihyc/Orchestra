/** A queue row owns its dispatched attachment until the native operation has actually stopped. */
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { useUploads } from '@/lib/useUploads';

const calls = vi.hoisted(() => ({ upload: vi.fn(), release: vi.fn<() => Promise<void>>(), publish: vi.fn(), stopped: vi.fn() }));
vi.mock('@/features/workbench/viewOpening', () => ({ useViewWork: () => undefined }));
vi.mock('@/lib/uploads', () => ({ UploadTask: class {
  readonly uploadId = 'private-upload';
  constructor(readonly file: File) {}
  run(_signal: AbortSignal, progress: (received: number) => void) {
    calls.upload(this.file.name);
    progress(this.file.size);
    return Promise.resolve({ blob: 'a'.repeat(64), size: this.file.size, media_type: 'text/plain', name: this.file.name });
  }
  cancel() { return calls.release(); }
} }));

function deferred() {
  let resolve: () => void = () => undefined;
  const promise = new Promise<void>((done) => { resolve = done; });
  return { promise, resolve };
}

function Probe({ proofs, settlesOnAbort = false }: { proofs: Map<string, ReturnType<typeof deferred>>; settlesOnAbort?: boolean }) {
  const queue = useUploads('workspace-stop-proof', () => Promise.reject(new Error('no destination')), calls.publish);
  const add = () => queue.add([...proofs.keys()].map((name) => new File(['content'], name)), undefined, async (_blob, signal, file, report) => {
    const proof = proofs.get(file.name);
    if (proof === undefined) throw new Error('missing native fixture');
    signal.addEventListener('abort', () => { calls.stopped(file.name); }, { once: true });
    if (settlesOnAbort && file.name === 'first.txt') {
      report({ stop: () => proof.promise });
      await new Promise<void>((_resolve, reject) => { signal.addEventListener('abort', () => reject(new DOMException('HTTP request ended', 'AbortError')), { once: true }); });
    }
    await proof.promise;
    signal.throwIfAborted();
    return file.name;
  });
  return <><button onClick={add}>Upload</button>{queue.rows.map((row) => <div key={row.id} data-testid={row.file.name}>
    <span>{row.phase}</span><button onClick={() => { void queue.remove(row.id); }}>Cancel {row.file.name}</button>
    {row.error !== undefined && <span>cleanup failed</span>}
  </div>)}</>;
}

beforeEach(() => { calls.upload.mockClear(); calls.release.mockReset(); calls.release.mockResolvedValue(undefined); calls.publish.mockClear(); calls.stopped.mockClear(); });

it('keeps a dispatched attachment visible and pinned until its native stopped acknowledgement', async () => {
  const proof = deferred();
  render(<Probe proofs={new Map([['first.txt', proof]])} />);
  fireEvent.click(screen.getByText('Upload'));
  await waitFor(() => expect(calls.upload).toHaveBeenCalledWith('first.txt'));
  // The task has entered attach, rather than merely completed its private upload.
  await act(async () => { await Promise.resolve(); });
  fireEvent.click(screen.getByText('Cancel first.txt'));
  await waitFor(() => expect(calls.stopped).toHaveBeenCalledWith('first.txt'));
  expect(screen.getByTestId('first.txt')).toHaveTextContent('stopping');
  expect(calls.release).not.toHaveBeenCalled();
  expect(calls.publish).not.toHaveBeenCalled();
  await act(async () => { proof.resolve(); await proof.promise; });
  await waitFor(() => expect(screen.queryByTestId('first.txt')).not.toBeInTheDocument());
  expect(calls.release).toHaveBeenCalledOnce();
  expect(calls.publish).not.toHaveBeenCalled();
});

it('a settled aborted HTTP request is not the native stop receipt', async () => {
  const proof = deferred();
  render(<Probe proofs={new Map([['first.txt', proof]])} settlesOnAbort />);
  fireEvent.click(screen.getByText('Upload'));
  await act(async () => { await Promise.resolve(); });
  fireEvent.click(screen.getByText('Cancel first.txt'));
  await waitFor(() => expect(calls.stopped).toHaveBeenCalledWith('first.txt'));
  await act(async () => { await Promise.resolve(); });
  expect(screen.getByTestId('first.txt')).toHaveTextContent('stopping');
  expect(calls.release).not.toHaveBeenCalled();
  expect(calls.publish).not.toHaveBeenCalled();
  await act(async () => { proof.resolve(); await proof.promise; });
  await waitFor(() => expect(screen.queryByTestId('first.txt')).not.toBeInTheDocument());
  expect(calls.release).toHaveBeenCalledOnce();
  expect(calls.publish).not.toHaveBeenCalled();
});

it('starts no next transfer while the cancelled native attachment still owns its operation', async () => {
  const first = deferred();
  const second = deferred();
  render(<Probe proofs={new Map([['first.txt', first], ['second.txt', second]])} settlesOnAbort />);
  fireEvent.click(screen.getByText('Upload'));
  await act(async () => { await Promise.resolve(); });
  fireEvent.click(screen.getByText('Cancel first.txt'));
  await waitFor(() => expect(calls.stopped).toHaveBeenCalledWith('first.txt'));
  expect(calls.upload.mock.calls).toEqual([['first.txt']]);
  expect(screen.getByTestId('second.txt')).toHaveTextContent('waiting');
  await act(async () => { first.resolve(); await first.promise; });
  await waitFor(() => expect(calls.upload).toHaveBeenCalledWith('second.txt'));
  await act(async () => { second.resolve(); await second.promise; });
  await waitFor(() => expect(calls.publish).toHaveBeenCalledWith('second.txt'));
});

it('keeps a failed cleanup stopping and lets a second cancel retry it without publishing content', async () => {
  const proof = deferred();
  calls.release.mockRejectedValueOnce(new Error('lost connection'));
  render(<Probe proofs={new Map([['first.txt', proof]])} />);
  fireEvent.click(screen.getByText('Upload'));
  await act(async () => { await Promise.resolve(); });
  fireEvent.click(screen.getByText('Cancel first.txt'));
  await waitFor(() => expect(calls.stopped).toHaveBeenCalledWith('first.txt'));
  await act(async () => { proof.resolve(); await proof.promise; });
  await waitFor(() => expect(screen.getByTestId('first.txt')).toHaveTextContent('cleanup failed'));
  expect(screen.getByTestId('first.txt')).toHaveTextContent('stopping');
  fireEvent.click(screen.getByText('Cancel first.txt'));
  await waitFor(() => expect(screen.queryByTestId('first.txt')).not.toBeInTheDocument());
  expect(calls.release).toHaveBeenCalledTimes(2);
  expect(calls.publish).not.toHaveBeenCalled();
});

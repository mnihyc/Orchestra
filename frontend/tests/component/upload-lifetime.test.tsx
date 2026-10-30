/** An upload continues in its original kept view while hidden and publishes only its completed file. */
import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { ViewOpening, clearViewOpenings, showViewOpening, useViewShown } from '@/features/workbench/viewOpening';
import { useUploads } from '@/lib/useUploads';
import { show } from './support';

const calls = vi.hoisted(() => ({ cancel: vi.fn(), publish: vi.fn(), finish: null as (() => void) | null, jobs: [] as (() => void)[] }));
vi.mock('@/lib/uploads', () => ({ UploadTask: class {
  readonly uploadId = null;
  constructor(readonly file: File) {}
  run(_signal: AbortSignal, progress: (received: number) => void) {
    progress(1);
    return new Promise((resolve) => { calls.finish = () => resolve({ blob: 'a'.repeat(64), name: this.file.name, size: this.file.size, media_type: 'text/plain' }); calls.jobs.push(calls.finish); });
  }
  cancel() { calls.cancel(); return Promise.resolve(); }
} }));

function Probe() {
  const shown = useViewShown();
  const queue = useUploads('kept-upload-lifetime', (blob) => Promise.resolve(blob.name), calls.publish);
  return <><button onClick={() => queue.add([new File(['abc'], 'a.txt')])}>Attach file</button><button onClick={() => queue.add([new File(['def'], 'b.txt')])}>Attach another file</button><span>{shown ? 'Shown' : 'Hidden'}</span><span>{queue.rows.length} pending</span><output aria-label="File progress">{queue.progress.completed}/{queue.progress.total} files · {queue.progress.received}/{queue.progress.size} bytes</output></>;
}
afterEach(() => { clearViewOpenings(); calls.cancel.mockClear(); calls.publish.mockClear(); calls.finish = null; calls.jobs = []; });

it('hiding the view does not cancel its transfer or leave an uploading row without work', async () => {
  const stop = showViewOpening('upload-proof', 'route');
  show(<ViewOpening view="upload-proof" route="route"><Probe /></ViewOpening>);
  fireEvent.click(await screen.findByRole('button', { name: 'Attach file' }));
  await waitFor(() => expect(calls.finish).not.toBeNull());
  await act(async () => { stop(); await Promise.resolve(); });
  expect(screen.getByText('Hidden')).toBeInTheDocument();
  expect(screen.getByText('1 pending')).toBeInTheDocument();
  expect(calls.cancel).not.toHaveBeenCalled();
  await act(async () => { calls.finish?.(); await Promise.resolve(); });
  await waitFor(() => expect(calls.publish).toHaveBeenCalledWith('a.txt'));
  expect(calls.cancel).not.toHaveBeenCalled();
});

it('keeps completed files and bytes in the progress while the next file uploads', async () => {
  show(<Probe />);
  fireEvent.click(screen.getByRole('button', { name: 'Attach file' }));
  fireEvent.click(screen.getByRole('button', { name: 'Attach another file' }));
  expect(screen.getByLabelText('File progress')).toHaveTextContent('0/2 files · 1/6 bytes');
  await act(async () => { calls.jobs[0]?.(); await Promise.resolve(); });
  await waitFor(() => expect(calls.jobs).toHaveLength(2));
  expect(screen.getByLabelText('File progress')).toHaveTextContent('1/2 files · 4/6 bytes');
  expect(calls.publish).toHaveBeenCalledExactlyOnceWith('a.txt');
  await act(async () => { calls.jobs[1]?.(); await Promise.resolve(); });
  expect(screen.getByLabelText('File progress')).toHaveTextContent('2/2 files · 6/6 bytes');
});

it('never publishes a finished old upload after the reader changed in the same task', async () => {
  const before = client.session.getState();
  show(<Probe />);
  fireEvent.click(screen.getByRole('button', { name: 'Attach file' }));
  await waitFor(() => expect(calls.finish).not.toBeNull());
  try {
    await act(async () => {
      client.session.setState({ readerVersion: (before.readerVersion ?? 0) + 1, userId: 'another-person' });
      calls.finish?.();
      await Promise.resolve();
    });
    expect(calls.publish).not.toHaveBeenCalled();
    expect(screen.getByText('0 pending')).toBeInTheDocument();
  } finally {
    act(() => client.session.setState({ ...before, readerVersion: (client.session.getState().readerVersion ?? 0) + 1 }));
  }
});

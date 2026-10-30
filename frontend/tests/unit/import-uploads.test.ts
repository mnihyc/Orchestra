/** Import uploads resume without resending completed files and stop when cancelled. */
import { describe, expect, it, vi } from 'vitest';
import { importSource } from '@/features/people/importSource';
import { ImportUploads, type ImportUploadProgress } from '@/features/people/importUploads';
import { UploadTask, type UploadTransport } from '@/lib/uploads';

function selected(files: File[]) {
  const source = importSource(files, false);
  if (source.fault !== null) throw new Error(`Invalid fixture: ${source.fault}`);
  return source;
}

function file(path: string, text: string): File {
  const result = new File([text], path.split('/').at(-1) ?? path);
  Object.defineProperty(result, 'webkitRelativePath', { value: path });
  return result;
}

describe('an import upload', () => {
  it('retries a failed file without sending the completed files again', async () => {
    const first = file('Notes/one.md', 'first');
    const second = file('Notes/two.md', 'second');
    let failure = true;
    const begin = vi.fn((source: File) => Promise.resolve({ upload_id: source.name, chunk_bytes: 1024, expires_at: '2999-01-01T00:00:00Z' }));
    const put = vi.fn((id: string, offset: number, bytes: Blob) => {
      if (id === 'two.md' && failure) { failure = false; return Promise.reject(new Error('Connection lost')); }
      return Promise.resolve(offset + bytes.size);
    });
    const complete = vi.fn((id: string) => Promise.resolve({ blob: id, name: id, media_type: 'text/plain', size: id === first.name ? first.size : second.size }));
    const transport: UploadTransport = { begin, put, complete };
    const batch = new ImportUploads(selected([first, second]), (one) => new UploadTask(one, transport));
    const progress: ImportUploadProgress[] = [];
    const signal = new AbortController().signal;

    await expect(batch.run(signal, (state) => progress.push(state))).rejects.toThrow('Connection lost');
    const uploaded = await batch.run(signal, (state) => progress.push(state));
    expect(uploaded.map((one) => [one.path, one.blob.blob])).toEqual([['Notes/one.md', 'one.md'], ['Notes/two.md', 'two.md']]);
    expect(begin.mock.calls.map(([one]) => one.name)).toEqual(['one.md', 'two.md']);
    expect(complete.mock.calls.map(([id]) => id)).toEqual(['one.md', 'two.md']);
    expect(progress.at(-1)).toEqual({ received: first.size + second.size, bytes: first.size + second.size, completed: 2, files: 2 });
  });

  it('aborts the current upload without revoking a completed shared grant', async () => {
    let begun: () => void = () => undefined;
    const started = new Promise<void>((resolve) => { begun = resolve; });
    const cancel = vi.fn(() => Promise.resolve());
    const release = vi.fn(() => Promise.resolve());
    const complete = vi.fn(() => Promise.resolve({ blob: 'blob', name: 'one.md', media_type: 'text/plain', size: 5 }));
    const transport: UploadTransport = {
      begin: (source) => Promise.resolve({ upload_id: source.name, chunk_bytes: 1024, expires_at: '2999-01-01T00:00:00Z' }),
      put: (id, offset, bytes, signal) => id === 'one.md' ? Promise.resolve(offset + bytes.size) : new Promise<number>((_resolve, reject) => {
          signal.addEventListener('abort', () => reject(signal.reason instanceof Error ? signal.reason : new DOMException('Upload cancelled', 'AbortError')), { once: true });
          begun();
        }),
      complete, cancel, release,
    };
    const batch = new ImportUploads(selected([file('Notes/one.md', 'first'), file('Notes/two.md', 'second')]), (one) => new UploadTask(one, transport));
    const running = batch.run(new AbortController().signal, () => undefined).catch((error: unknown) => error);
    await started;
    const stopping = batch.cancel();
    expect(batch.cancel()).toBe(stopping);
    await stopping;
    expect(await running).toMatchObject({ name: 'AbortError' });
    expect(cancel).toHaveBeenCalledExactlyOnceWith('two.md');
    expect(release).not.toHaveBeenCalled();
    expect(complete).toHaveBeenCalledExactlyOnceWith('one.md', expect.any(AbortSignal));
    await expect(batch.run(new AbortController().signal, () => undefined)).rejects.toMatchObject({ name: 'AbortError' });
  });

  it('reports a corrected received offset without counting the same bytes twice', async () => {
    const one = file('Notes/one.md', 'four');
    const batch = new ImportUploads(selected([one]), () => ({
      run: (_signal, progress) => {
        progress(3); progress(1); progress(4);
        return Promise.resolve({ blob: 'blob', name: one.name, media_type: 'text/plain', size: one.size });
      },
      cancel: () => Promise.resolve(),
    }));
    const progress: number[] = [];
    await batch.run(new AbortController().signal, (state) => progress.push(state.received));
    expect(progress).toEqual([0, 3, 1, 4, 4]);
  });

  it('retries failed cleanup without touching completed or untouched files', async () => {
    const files = [file('a.md', 'a'), file('b.md', 'b'), file('c.md', 'c')];
    const calls = new Map<string, number>();
    const batch = new ImportUploads(selected(files), (one) => ({
      run: () => one.name === 'a.md' ? Promise.resolve({ blob: 'a', name: one.name, media_type: 'text/plain', size: 1 }) : Promise.reject(new Error('Upload failed')),
      cancel: () => {
        const count = (calls.get(one.name) ?? 0) + 1;
        calls.set(one.name, count);
        return count === 1 ? Promise.reject(new Error('Connection lost')) : Promise.resolve();
      },
    }));
    await expect(batch.run(new AbortController().signal, () => undefined)).rejects.toThrow('Upload failed');
    const progress: number[] = [];
    await expect(batch.cancel((state) => progress.push(state.completed))).rejects.toThrow('Connection lost');
    expect([...calls.entries()]).toEqual([['b.md', 1]]);
    await batch.cancel((state) => progress.push(state.completed));
    expect([...calls.entries()]).toEqual([['b.md', 2]]);
    expect(progress.at(-1)).toBe(files.length);
    await batch.cancel();
    expect(calls.get('b.md')).toBe(2);
  });
});

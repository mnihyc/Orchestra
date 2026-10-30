/** A resumable upload recovers a lost piece answer without losing or repeating bytes and never completes a cancelled file. */
// @vitest-environment node
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import type { UploadedBlob } from '@/generated/api-types';
import { NetworkError } from '@/lib/requests';
import { UploadTask, type UploadTransport } from '@/lib/uploads';

const session = { upload_id: 'upload', chunk_bytes: 4, expires_at: '2099-01-01T00:00:00.000Z' };
const blob: UploadedBlob = { blob: 'a'.repeat(64), name: 'plan.txt', size: 10, media_type: 'text/plain' };
const offsetError = (received: number) => new ApiError({ type: 'orch:error:upload.offset', title: 'offset', detail: 'offset', code: 'upload.offset', status: 409, request_id: 'request', received }, null);

afterEach(() => { vi.useRealTimers(); });

describe('resumable uploads', () => {
  it('uses the received count after a lost answer and completes exactly the original bytes', async () => {
    vi.useFakeTimers();
    let stored = '';
    let lose = true;
    const begin = vi.fn(() => Promise.resolve(session));
    const requests: UploadTransport = {
      begin,
      put: async (_id, offset, bytes) => {
        if (offset !== stored.length) throw offsetError(stored.length);
        stored += await bytes.text();
        if (lose) { lose = false; throw new NetworkError('failed'); }
        return stored.length;
      },
      complete: () => { expect(stored).toBe('abcdefghij'); return Promise.resolve(blob); },
    };
    const progress: number[] = [];
    const done = new UploadTask(new File(['abcdefghij'], 'plan.txt'), requests).run(new AbortController().signal, (received) => progress.push(received));
    await vi.advanceTimersByTimeAsync(1_000);
    await expect(done).resolves.toEqual(blob);
    expect(begin).toHaveBeenCalledTimes(1);
    expect(progress.at(-1)).toBe(10);
  });

  it('keeps a failed session so Retry goes on from the last received bytes', async () => {
    vi.useFakeTimers();
    let stored = '';
    let offline = false;
    const begin = vi.fn(() => Promise.resolve(session));
    const requests: UploadTransport = {
      begin,
      put: async (_id, offset, bytes) => {
        if (offline) throw new NetworkError('offline');
        expect(offset).toBe(stored.length);
        stored += await bytes.text();
        if (stored.length === 4) offline = true;
        return stored.length;
      },
      complete: () => Promise.resolve(blob),
    };
    const task = new UploadTask(new File(['abcdefghij'], 'plan.txt'), requests);
    const failed = expect(task.run(new AbortController().signal)).rejects.toBeInstanceOf(NetworkError);
    await vi.advanceTimersByTimeAsync(7_000);
    await failed;
    offline = false;
    await expect(task.run(new AbortController().signal)).resolves.toEqual(blob);
    expect(stored).toBe('abcdefghij');
    expect(begin).toHaveBeenCalledTimes(1);
  });

  it('does not complete a file cancelled while a piece was in flight', async () => {
    const cancel = new AbortController();
    const complete = vi.fn(() => Promise.resolve(blob));
    const requests: UploadTransport = { begin: () => Promise.resolve(session), put: () => { cancel.abort(); return Promise.resolve(4); }, complete };
    await expect(new UploadTask(new File(['abcdefghij'], 'plan.txt'), requests).run(cancel.signal)).rejects.toBeDefined();
    expect(complete).not.toHaveBeenCalled();
  });

  it('finishes an empty file without sending any piece', async () => {
    const put = vi.fn(() => Promise.resolve(0));
    const empty = { ...blob, size: 0 };
    const requests: UploadTransport = { begin: () => Promise.resolve(session), put, complete: () => Promise.resolve(empty) };
    await expect(new UploadTask(new File([], 'plan.txt'), requests).run(new AbortController().signal)).resolves.toEqual(empty);
    expect(put).not.toHaveBeenCalled();
  });

  it('keeps a completed grant when cancellation races a completion needed by other work', async () => {
    let entered: () => void = () => undefined;
    let finish: (value: UploadedBlob) => void = () => undefined;
    const completing = new Promise<void>((resolve) => { entered = resolve; });
    const answer = new Promise<UploadedBlob>((resolve) => { finish = resolve; });
    const release = vi.fn(() => Promise.resolve());
    const cancel = vi.fn(() => Promise.resolve());
    const requests: UploadTransport = {
      begin: () => Promise.resolve(session),
      put: (_id, offset, bytes) => Promise.resolve(offset + bytes.size),
      complete: () => { entered(); return answer; },
      cancel, release,
    };
    const task = new UploadTask(new File(['abcdefghij'], 'plan.txt'), requests);
    const controller = new AbortController();
    const running = task.run(controller.signal).catch((error: unknown) => error);
    await completing;
    controller.abort();
    const stopping = task.cancel({ keepCompleted: true });
    finish(blob);
    await stopping;
    expect(await running).toMatchObject({ name: 'AbortError' });
    expect(release).not.toHaveBeenCalled();
    expect(cancel).not.toHaveBeenCalled();
  });

  it('releases a completed grant on ordinary cancellation', async () => {
    const release = vi.fn(() => Promise.resolve());
    const requests: UploadTransport = {
      begin: () => Promise.resolve(session),
      put: (_id, offset, bytes) => Promise.resolve(offset + bytes.size),
      complete: () => Promise.resolve(blob), release,
    };
    const task = new UploadTask(new File(['abcdefghij'], 'plan.txt'), requests);
    await task.run(new AbortController().signal);
    await task.cancel();
    expect(release).toHaveBeenCalledExactlyOnceWith(blob.blob);
  });
});

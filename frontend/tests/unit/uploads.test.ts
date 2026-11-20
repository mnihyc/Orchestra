/** A resumable upload recovers a lost piece answer without losing or repeating bytes and never completes a cancelled file. */
// @vitest-environment node
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import type { UploadedBlob } from '@/generated/api-types';
import { NetworkError } from '@/lib/requests';
import { UploadTask, type UploadTransport } from '@/lib/uploads';

const smallPolicy = { initial_piece_bytes: 4, initial_requests: 1, min_piece_bytes: 1, max_piece_bytes: 4, target_window_ms: 1000, window_rtts: 4, max_window_bytes: 16, requests: 1 };
const session = { upload_id: 'upload', policy: smallPolicy, chunk_bytes: 4, expires_at: '2099-01-01T00:00:00.000Z' };
const blob: UploadedBlob = { blob: 'a'.repeat(64), name: 'plan.txt', size: 10, media_type: 'text/plain' };
const offsetError = (received: number) => new ApiError({ type: 'orch:error:upload.offset', title: 'offset', detail: 'offset', code: 'upload.offset', status: 409, request_id: 'request', received }, null);

afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

describe('resumable uploads', () => {
  it('uses the received count after a lost answer and completes exactly the original bytes', async () => {
    vi.useFakeTimers();
    let stored = '';
    let lose = true;
    const begin = vi.fn(() => Promise.resolve(session));
    const requests: UploadTransport = {
      begin,
      status: () => Promise.resolve({ received: stored.length, ranges: stored.length === 0 ? [] : [{ start: 0, len: stored.length }], policy: smallPolicy }),
      put: async (_id, offset, bytes) => {
        if (offset !== stored.length) throw offsetError(stored.length);
        stored += await bytes.text();
        if (lose) { lose = false; throw new NetworkError('failed'); }
        return { received: stored.length, expires_at: session.expires_at };
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
      status: () => Promise.resolve({ received: stored.length, ranges: stored.length === 0 ? [] : [{ start: 0, len: stored.length }], policy: smallPolicy }),
      put: async (_id, offset, bytes) => {
        if (offline) throw new NetworkError('offline');
        expect(offset).toBe(stored.length);
        stored += await bytes.text();
        if (stored.length === 4) offline = true;
        return { received: stored.length, expires_at: session.expires_at };
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
    const requests: UploadTransport = { begin: () => Promise.resolve(session), status: () => Promise.resolve({ received: 0, ranges: [], policy: smallPolicy }), put: () => { cancel.abort(); return Promise.resolve({ received: 4, expires_at: session.expires_at }); }, complete };
    await expect(new UploadTask(new File(['abcdefghij'], 'plan.txt'), requests).run(cancel.signal)).rejects.toBeDefined();
    expect(complete).not.toHaveBeenCalled();
  });

  it('finishes an empty file without sending any piece', async () => {
    const put = vi.fn(() => Promise.resolve({ received: 0, expires_at: session.expires_at }));
    const empty = { ...blob, size: 0 };
    const requests: UploadTransport = { begin: () => Promise.resolve(session), status: () => Promise.resolve({ received: 0, ranges: [], policy: smallPolicy }), put, complete: () => Promise.resolve(empty) };
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
      begin: () => Promise.resolve(session), status: () => Promise.resolve({ received: 0, ranges: [], policy: smallPolicy }),
      put: (_id, offset, bytes) => Promise.resolve({ received: offset + bytes.size, expires_at: session.expires_at }),
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
      begin: () => Promise.resolve(session), status: () => Promise.resolve({ received: 0, ranges: [], policy: smallPolicy }),
      put: (_id, offset, bytes) => Promise.resolve({ received: offset + bytes.size, expires_at: session.expires_at }),
      complete: () => Promise.resolve(blob), release,
    };
    const task = new UploadTask(new File(['abcdefghij'], 'plan.txt'), requests);
    await task.run(new AbortController().signal);
    await task.cancel();
    expect(release).toHaveBeenCalledExactlyOnceWith(blob.blob);
  });
});


it('retries native verification from the same upload without resending acknowledged pieces', async () => {
  const begin = vi.fn(() => Promise.resolve(session));
  const put = vi.fn((_id: string, offset: number, bytes: Blob) => Promise.resolve({ received: offset + bytes.size, expires_at: session.expires_at }));
  const complete = vi.fn().mockRejectedValueOnce(new NetworkError('failed'))
    .mockResolvedValueOnce({ ...blob, relay: session.upload_id });
  const task = new UploadTask(new File(['abcdefghij'], 'plan.txt'), { begin, put, complete, status:()=>Promise.resolve({received:10,ranges:[{start:0,len:10}],policy:smallPolicy}) });
  await expect(task.run(new AbortController().signal)).rejects.toBeInstanceOf(NetworkError);
  const sent = put.mock.calls.length;
  await expect(task.run(new AbortController().signal)).resolves.toMatchObject({ relay: session.upload_id });
  expect(begin).toHaveBeenCalledTimes(1);
  expect(put).toHaveBeenCalledTimes(sent);
});

it('cancels the actual native session without releasing an unrelated retained upload of the same hash', async () => {
  const cancel = vi.fn(() => Promise.resolve());
  const release = vi.fn(() => Promise.resolve());
  const task = new UploadTask(new File([], 'plan.txt'), {
    begin: () => Promise.resolve(session), status:()=>Promise.resolve({received:0,ranges:[],policy:smallPolicy}), put: () => Promise.resolve({ received: 0, expires_at: session.expires_at }),
    complete: () => Promise.resolve({ ...blob, relay: session.upload_id, size: 0 }), cancel, release,
  });
  await task.run(new AbortController().signal);
  await task.cancel();
  expect(cancel).toHaveBeenCalledExactlyOnceWith(session.upload_id);
  expect(release).not.toHaveBeenCalled();
});

const MiB = 1024 * 1024;
const policy = { initial_piece_bytes: 64 * 1024, initial_requests: 4, min_piece_bytes: 64 * 1024, max_piece_bytes: 16 * MiB, target_window_ms: 1000, window_rtts: 4, max_window_bytes: 64 * MiB, requests: 5 };

/** One shared wire, rather than a separate bandwidth allowance for each request. */
function longLine(rate: number, loss: 'none' | 'piece' | 'answer' = 'none', size = 64 * MiB, beginMs = 700, responseBatchMs?: number) {
  const ranges: { start: number; len: number }[] = [];
  const offsets: number[] = []; const pieceSizes: number[] = [];
  let active = 0; let peak = 0; let wireFree = 0; let elapsed = 0; let lost: number | null = null;
  const started = performance.now(); let firstAck: number | undefined; let lastAck = 0;
  const wait = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));
  const status = vi.fn(() => Promise.resolve({ received: ranges.reduce((sum, range) => sum + range.len, 0), ranges: [...ranges], policy }));
  const requests = {
    begin: async () => { await wait(beginMs); return { ...session, chunk_bytes: policy.max_piece_bytes, policy }; },
    status,
    put: async (_id: string, offset: number, bytes: Blob) => {
      offsets.push(offset); pieceSizes.push(bytes.size); active += 1; peak = Math.max(peak, active);
      const nth = offsets.length;
      const ends = Math.max(wireFree, performance.now()) + bytes.size * 1000 / rate;
      wireFree = ends;
      const answerAt = responseBatchMs === undefined ? ends + 700 : Math.ceil((ends + 700) / responseBatchMs) * responseBatchMs;
      await wait(answerAt - performance.now());
      active -= 1;
      if (nth === 4 && loss === 'piece') { lost = offset; throw new NetworkError('failed'); }
      if (!ranges.some((range) => range.start === offset)) ranges.push({ start: offset, len: bytes.size });
      if (nth === 4 && loss === 'answer') { lost = offset; throw new NetworkError('failed'); }
      firstAck ??= performance.now(); lastAck = performance.now();
      return {received: ranges.reduce((sum,range)=>sum+range.len,0), expires_at:session.expires_at};
    },
    complete: async () => { await wait(700); elapsed = performance.now() - started; return { ...blob, size }; },
  };
  return { requests, offsets, pieceSizes, status, peak: () => peak, elapsed: () => elapsed, bulkElapsed: () => lastAck - (firstAck ?? lastAck), lost: () => lost };
}

describe('transfers over a long line', () => {
  it.each([8 * MiB, 32 * MiB, Number.POSITIVE_INFINITY])('keeps several pieces in flight for 64 MiB at 700 ms, bandwidth %s', async (rate) => {
    vi.useFakeTimers();
    const line = longLine(rate);
    const result = new UploadTask(new File([new Uint8Array(64 * MiB)], 'large.bin'), line.requests).run(new AbortController().signal);
    await vi.advanceTimersByTimeAsync(30_000);
    await expect(result).resolves.toMatchObject({ size: 64 * MiB });
    expect(line.peak()).toBeGreaterThan(1);
    expect(line.peak()).toBeLessThanOrEqual(policy.requests);
    expect(line.offsets.length).toBeLessThan(40);
    // Report setup/completion separately: their three RTTs cannot be amortized at arbitrary finite rates.
    const bulkRate = (64 * MiB - policy.initial_piece_bytes) / (line.bulkElapsed() / 1000);
    if (Number.isFinite(rate)) expect(bulkRate).toBeGreaterThanOrEqual(rate * 0.7);
    expect(64 * MiB / (line.elapsed() / 1000)).toBeGreaterThanOrEqual(Math.min(8 * MiB, rate * 0.7));
    process.stdout.write(`64 MiB at ${rate} B/s: ${line.offsets.length} data requests, peak ${line.peak()}, total ${line.elapsed()} ms, bulk ${line.bulkElapsed()} ms\n`);
  });

  it.each(['piece', 'answer'] as const)('consults durable ranges after a lost %s and resends only a missing piece', async (loss) => {
    vi.useFakeTimers();
    const line = longLine(32 * MiB, loss);
    const result = new UploadTask(new File([new Uint8Array(64 * MiB)], 'large.bin'), line.requests).run(new AbortController().signal);
    await vi.advanceTimersByTimeAsync(30_000);
    await expect(result).resolves.toMatchObject({ size: 64 * MiB });
    expect(line.status).toHaveBeenCalled();
    expect(line.lost()).not.toBeNull();
    for (const offset of new Set(line.offsets)) {
      expect(line.offsets.filter((at) => at === offset)).toHaveLength(offset === line.lost() && loss === 'piece' ? 2 : 1);
    }
  });
});

it('recovers a lost completion answer from its receipt without resending the file', async () => {
  const put = vi.fn((_id: string, offset: number, bytes: Blob) => Promise.resolve({received:offset+bytes.size,expires_at:session.expires_at}));
  const complete = vi.fn(() => Promise.reject(new NetworkError('failed')));
  const requests: UploadTransport = { begin: () => Promise.resolve(session), put, complete,
    status: () => Promise.resolve({ received: 10, ranges: [{ start: 0, len: 10 }], policy: smallPolicy, completed: blob }) };
  const task = new UploadTask(new File(['abcdefghij'], 'plan.txt'), requests);
  await expect(task.run(new AbortController().signal)).rejects.toBeInstanceOf(NetworkError);
  const sent = put.mock.calls.length;
  await expect(task.run(new AbortController().signal)).resolves.toEqual(blob);
  expect(put).toHaveBeenCalledTimes(sent);
  expect(complete).toHaveBeenCalledTimes(1);
});

it('shares the request and byte window between simultaneously uploading files', async () => {
  vi.useFakeTimers();
  const totals = new Map<string, number>();
  const seen = new Set<string>();
  let active = 0, bytes = 0, peakRequests = 0, peakBytes = 0, firstCompletedWith = 0;
  const wait = () => new Promise<void>((resolve) => setTimeout(resolve, 700));
  const requests: UploadTransport = {
    begin: async (file) => { await wait(); return { ...session, upload_id: file.name, policy, chunk_bytes: policy.max_piece_bytes }; },
    status: () => Promise.resolve({ received: 0, ranges: [], policy }),
    put: async (id, _offset, piece) => {
      active += 1; bytes += piece.size; peakRequests = Math.max(peakRequests, active); peakBytes = Math.max(peakBytes, bytes); seen.add(id);
      await wait(); active -= 1; bytes -= piece.size;
      const total = (totals.get(id) ?? 0) + piece.size; totals.set(id, total); return {received:total, expires_at:session.expires_at};
    },
    complete: (id) => { firstCompletedWith ||= seen.size; return Promise.resolve({ ...blob, name: id, size: 32 * MiB }); },
  };
  const done = Promise.all(['first.bin', 'second.bin'].map((name) => new UploadTask(new File([new Uint8Array(32 * MiB)], name), requests).run(new AbortController().signal)));
  await vi.advanceTimersByTimeAsync(10_000);
  await expect(done).resolves.toHaveLength(2);
  expect([...totals.values()]).toEqual([32 * MiB, 32 * MiB]);
  expect(firstCompletedWith).toBe(2);
  expect(peakRequests).toBeGreaterThan(1); expect(peakRequests).toBeLessThanOrEqual(policy.requests);
  expect(peakBytes).toBeLessThanOrEqual(policy.max_window_bytes);
});

it.each([true, false])('repeats a lost creation with the same idempotency key when randomUUID is available: %s', async (hasRandomUuid) => {
  if (!hasRandomUuid) vi.stubGlobal('crypto', { getRandomValues: crypto.getRandomValues.bind(crypto) });
  let lost = true;
  const begin = vi.fn((_file: File, _signal: AbortSignal, _key: string) => { if (lost) { lost = false; return Promise.reject(new NetworkError('failed')); } return Promise.resolve(session); });
  const requests: UploadTransport = { begin, status: () => Promise.resolve({ received: 0, ranges: [], policy: smallPolicy }), put: (_id, offset, bytes) => Promise.resolve({received:offset+bytes.size,expires_at:session.expires_at}), complete: () => Promise.resolve(blob) };
  const task = new UploadTask(new File(['abcdefghij'], 'plan.txt'), requests);
  await expect(task.run(new AbortController().signal)).rejects.toBeInstanceOf(NetworkError);
  await expect(task.run(new AbortController().signal)).resolves.toEqual(blob);
  expect(begin.mock.calls[0]?.[2]).toBe(begin.mock.calls[1]?.[2]);
  expect(begin.mock.calls[0]?.[2]).toMatch(/^[\da-f]{8}-[\da-f]{4}-4[\da-f]{3}-[89ab][\da-f]{3}-[\da-f]{12}$/u);
});

it('keeps pieces proportional to the line when connection setup inflated the creation round trip', async () => {
  vi.useFakeTimers();
  const rate = 300 * 1024;
  const line = longLine(rate, 'none', 8 * MiB, 2100);
  const done = new UploadTask(new File([new Uint8Array(8 * MiB)], 'thin.bin'), line.requests).run(new AbortController().signal);
  await vi.advanceTimersByTimeAsync(60_000);
  await expect(done).resolves.toMatchObject({ size: 8 * MiB });
  // Even the entire target window is a generous bound for a single piece on this line.
  expect(Math.max(...line.pieceSizes)).toBeLessThanOrEqual(rate * (policy.target_window_ms / 1000 + 2.1 * policy.window_rtts));
});

it('uses the completed receipt lifetime after the original upload lifetime has ended', async () => {
  vi.useFakeTimers();
  const begin = vi.fn(() => Promise.resolve({ ...session, expires_at: new Date(Date.now() + 1000).toISOString() }));
  const put = vi.fn((_id: string, offset: number, bytes: Blob) => Promise.resolve({received:offset+bytes.size,expires_at:session.expires_at}));
  const requests: UploadTransport = { begin, put, complete: () => Promise.reject(new NetworkError('failed')),
    status: () => Promise.resolve({ received: 10, ranges: [{ start: 0, len: 10 }], policy: smallPolicy, completed: blob }) };
  const task = new UploadTask(new File(['abcdefghij'], 'plan.txt'), requests);
  await expect(task.run(new AbortController().signal)).rejects.toBeInstanceOf(NetworkError);
  const sent = put.mock.calls.length;
  await vi.advanceTimersByTimeAsync(1001);
  await expect(task.run(new AbortController().signal)).resolves.toEqual(blob);
  expect(put).toHaveBeenCalledTimes(sent); expect(begin).toHaveBeenCalledTimes(1);
});

it('does not infer extra bandwidth from acknowledgements arriving together', async () => {
  vi.useFakeTimers();
  const rate = 300 * 1024;
  const line = longLine(rate, 'none', 8 * MiB, 700, 2000);
  const done = new UploadTask(new File([new Uint8Array(8 * MiB)], 'batched.bin'), line.requests).run(new AbortController().signal);
  await vi.advanceTimersByTimeAsync(90_000);
  await expect(done).resolves.toMatchObject({ size: 8 * MiB });
  expect(Math.max(...line.pieceSizes)).toBeLessThanOrEqual(rate * (policy.target_window_ms / 1000 + 0.7 * policy.window_rtts));
});

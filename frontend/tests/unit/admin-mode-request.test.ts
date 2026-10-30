// @vitest-environment node
/** A missing mode-write answer is reconciled from the session without another write. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { QueryObserver } from '@tanstack/react-query';
import type { AdminMode } from '@/generated/api-types';
import type { Change } from '@/api/acts';
import { NetworkError } from '@/lib/requests';
import { setAdminMode } from '@/api/adminMode';
import { queryClient } from '@/api/query';
import { ADMIN_QUERY } from '@/api/queryAuthority';

const calls = vi.hoisted(() => ({
  get: vi.fn<(path: string) => Promise<{ data: AdminMode }>>(),
  put: vi.fn<(path: string, options: { body: { as_admin: boolean } }) => Promise<{ data: AdminMode }>>(),
  reset: vi.fn<(changes: readonly Change[]) => Promise<void>>(),
}));
vi.mock('@/api/client', () => ({ api: { GET: calls.get, PUT: calls.put }, ok: async (reply: Promise<{ data: AdminMode }>) => (await reply).data }));
vi.mock('@/api/acts', () => ({ applyChanges: calls.reset }));
beforeEach(() => {
  vi.clearAllMocks();
  calls.reset.mockResolvedValue(undefined);
});
afterEach(() => queryClient.clear());

it('writes the session field, without a preference, grant or duplicate successful refresh', async () => {
  const state = { available: true, as_admin: true };
  calls.put.mockResolvedValue({ data: state });
  expect(await setAdminMode(true)).toEqual(state);
  expect(calls.put).toHaveBeenCalledExactlyOnceWith('/me/admin-mode', { body: { as_admin: true } });
  expect(calls.get).not.toHaveBeenCalled();
  expect(calls.reset).not.toHaveBeenCalled();
});

it('accepts the read-back session when the write answer was lost, and rebases through the registry', async () => {
  calls.put.mockRejectedValue(new NetworkError('failed'));
  calls.get.mockResolvedValue({ data: { available: true, as_admin: true } });
  expect(await setAdminMode(true)).toEqual({ available: true, as_admin: true });
  expect(calls.put).toHaveBeenCalledOnce();
  expect(calls.get).toHaveBeenCalledExactlyOnceWith('/me/admin-mode', {});
  expect(calls.reset).toHaveBeenCalledExactlyOnceWith([{ session: true, mode: { available: true, as_admin: true } }]);
});

it('keeps a partial refusal plain when the session read shows the requested state was not applied', async () => {
  const lost = new NetworkError('failed');
  calls.put.mockRejectedValue(lost);
  calls.get.mockResolvedValue({ data: { available: true, as_admin: false } });
  await expect(setAdminMode(true)).rejects.toBe(lost);
  expect(calls.put).toHaveBeenCalledOnce();
  expect(calls.get).toHaveBeenCalledExactlyOnceWith('/me/admin-mode', {});
  expect(calls.reset).toHaveBeenCalledExactlyOnceWith([{ session: true, mode: { available: true, as_admin: false } }]);
});

it('cancels privileged reads before asking the server to remove their reach, keeping ordinary reads alive', async () => {
  let privileged: AbortSignal | undefined;
  let ordinary: AbortSignal | undefined;
  const protectedRead = queryClient.query({ ...ADMIN_QUERY, queryKey: ['admin', 'in-flight'], queryFn: ({ signal }) => {
    privileged = signal; return new Promise<never>(() => undefined);
  } }).catch(() => undefined);
  const ordinaryRead = queryClient.query({ queryKey: ['ordinary', 'in-flight'], queryFn: ({ signal }) => {
    ordinary = signal; return new Promise<never>(() => undefined);
  } }).catch(() => undefined);
  calls.put.mockImplementation(() => {
    expect(privileged?.aborted).toBe(true);
    expect(ordinary?.aborted).toBe(false);
    return Promise.resolve({ data: { available: true, as_admin: false } });
  });
  try { expect(await setAdminMode(false)).toEqual({ available: true, as_admin: false }); }
  finally { await queryClient.cancelQueries(); await Promise.all([protectedRead, ordinaryRead]); }
});

it('restores a still-visible privileged read when the mode write is refused and its original authority remains', async () => {
  queryClient.setQueryData(['admin-mode'], { available: true, as_admin: true });
  let signal: AbortSignal | undefined;
  const read = vi.fn(({ signal: current }: { signal: AbortSignal }) => {
    signal = current;
    return read.mock.calls.length === 1 ? new Promise<string>(() => undefined) : Promise.resolve('Still permitted');
  });
  const observer = new QueryObserver(queryClient, { ...ADMIN_QUERY, queryKey: ['admin', 'visible'], queryFn: read });
  const leave = observer.subscribe(() => undefined);
  const refusal = new Error('Mode change refused');
  calls.put.mockImplementation(() => { expect(signal?.aborted).toBe(true); return Promise.reject(refusal); });
  try {
    await expect(setAdminMode(false)).rejects.toBe(refusal);
    expect(read).toHaveBeenCalledTimes(2);
    expect(queryClient.getQueryData(['admin', 'visible'])).toBe('Still permitted');
    expect(queryClient.getQueryData(['admin-mode'])).toEqual({ available: true, as_admin: true });
  } finally { leave(); }
});

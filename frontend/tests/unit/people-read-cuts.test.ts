// @vitest-environment node
/** Native reads after a received authority hint share that check; earlier, failed or replaced readers do not. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { beginPeopleRead, capturePeopleHint, repairPeople } from '@/api/peopleChanges';

const key = ['node-chat-opening', 'place'] as const;
const previous = client.session.getState();
beforeEach(() => { client.session.setState({ readerVersion: 7 }); });
afterEach(() => { client.session.setState(previous); });

function watch(cache: QueryClient, read: (signal: AbortSignal) => Promise<string>) {
  const options = { queryKey: key, staleTime: Infinity, retry: false, queryFn: async ({ signal }: { signal: AbortSignal }) => {
    const native = beginPeopleRead(cache, key);
    try { const value = await read(signal); native.accept(); return value; }
    catch (error) { native.cancel(); throw error; }
  } };
  const observer = new QueryObserver(cache, options);
  const leave = observer.subscribe(() => undefined);
  return { loaded: () => cache.query(options), close: () => { leave(); cache.clear(); } };
}
function cache() { return new QueryClient({ defaultOptions: { queries: { retry: false } } }); }

it('does not repeat a successful native check begun after the hint was received', async () => {
  const held = cache(), cut = capturePeopleHint(held), read = vi.fn(() => Promise.resolve('Current scopes'));
  const view = watch(held, read);
  try {
    await view.loaded(); await repairPeople(held, [], cut);
    expect(read).toHaveBeenCalledOnce(); expect(held.getQueryData(key)).toBe('Current scopes');
  } finally { view.close(); }
});

it('keeps an already-running post-hint native check rather than cancelling it for a duplicate', async () => {
  const held = cache(), cut = capturePeopleHint(held);
  let finish!: (value: string) => void, signal!: AbortSignal;
  const read = vi.fn((current: AbortSignal) => { signal = current; return new Promise<string>((resolve) => { finish = resolve; }); });
  const view = watch(held, read);
  try {
    const loaded = view.loaded(); await repairPeople(held, [], cut);
    expect(read).toHaveBeenCalledOnce(); expect(signal.aborted).toBe(false);
    finish('Admitted scopes'); expect(await loaded).toBe('Admitted scopes');
  } finally { finish('Admitted scopes'); view.close(); }
});

it('retakes an earlier read even when its answer arrives after the newer hint', async () => {
  const held = cache(); let finish!: (value: string) => void;
  const read = vi.fn<() => Promise<string>>().mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; })).mockResolvedValue('Current scopes');
  const view = watch(held, read);
  try {
    const loaded = view.loaded(), cut = capturePeopleHint(held);
    finish('Earlier scopes'); await loaded; await repairPeople(held, [], cut);
    expect(read).toHaveBeenCalledTimes(2); expect(held.getQueryData(key)).toBe('Current scopes');
  } finally { finish('Earlier scopes'); view.close(); }
});

it('does not reuse the check for a later hint or a replacement session reader', async () => {
  const held = cache(), first = capturePeopleHint(held), read = vi.fn(() => Promise.resolve('Admitted scopes'));
  const view = watch(held, read);
  try {
    await view.loaded(); await repairPeople(held, [], first); expect(read).toHaveBeenCalledOnce();
    const second = capturePeopleHint(held); await repairPeople(held, [], second); expect(read).toHaveBeenCalledTimes(2);
    client.session.setState({ readerVersion: 8 }); await repairPeople(held, [], second); expect(read).toHaveBeenCalledTimes(3);
  } finally { view.close(); }
});

it('does not treat a failed post-hint read as an accepted authority check', async () => {
  const held = cache(), read = vi.fn(() => Promise.resolve('Earlier scopes'));
  const view = watch(held, read);
  try {
    await view.loaded(); const cut = capturePeopleHint(held);
    read.mockRejectedValueOnce(new Error('The check failed'));
    await expect(held.refetchQueries({ queryKey: key, exact: true }, { throwOnError: true })).rejects.toThrow('The check failed');
    read.mockResolvedValue('Current scopes'); await repairPeople(held, [], cut);
    expect(read).toHaveBeenCalledTimes(3); expect(held.getQueryData(key)).toBe('Current scopes');
  } finally { view.close(); }
});

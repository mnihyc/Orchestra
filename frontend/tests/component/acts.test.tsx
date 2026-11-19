/** Every act leaves the views of what it changed showing the change, without a reload. For each entry of the registry of
 * acts: views that read each piece of data the act names are on screen, beside one that reads something else and one
 * that reads what the act removed; the server takes the act and answers differently afterwards, and the client tells
 * the registry of it as it does of every act that succeeds (that it does, before the act resolves, is the unit test's
 * `tests/unit/acts.test.ts`); once that has resolved, every view of what it changed shows the server's new answer, the
 * view of something else still shows the old one, and nothing is asked again of what was removed.
 * A session-authority change names every projection, including the otherwise unrelated view. */
import { QueryClientProvider, useQuery } from '@tanstack/react-query';
import { act, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { ACTS, actSucceeded, covers, findAct, followWith, type Stale } from '@/api/acts';
import type { Act } from '@/api/client';
import { queryClient } from '@/api/query';
import type { Provider } from '@/generated/api-types';

/** What the server answers now: every act it takes moves it on. */
let state = 0;
/** The feeds the registry asked for what changed. */
let fed: string[] = [];

/** One view of one piece of data: it says which, and what the server answered for it. */
function View({ queryKey }: { queryKey: readonly unknown[] }) {
  const read = useQuery({ queryKey, queryFn: () => Promise.resolve(state) });
  return <p>{`${JSON.stringify(queryKey)} → ${read.data === undefined ? '…' : String(read.data)}`}</p>;
}

/** The method and a path of an entry, each parameter named after itself. */
function request(entry: string): { method: string; path: string } {
  const space = entry.indexOf(' ');
  return { method: entry.slice(0, space), path: entry.slice(space + 1).replace(/\{(\w+)\}/g, '$1-1') };
}

/** A view's key for a change: the change's own key, with the one thing it names (or another thing of its kind). */
const viewOf = (change: Stale): readonly string[] => (change.exact === true ? change.key : [...change.key, change.id ?? 'any-one']);

beforeEach(() => {
  state = 0;
  fed = [];
  followWith({ trees: () => fed.push('trees'), thread: (id) => fed.push(`thread:${id}`), document: (id) => fed.push(`document:${id}`), counters: () => fed.push('counters') });
});

afterEach(() => queryClient.clear());

describe('after an act', () => {
  it('renders the returned provider in its detail and list without reading either again', async () => {
    // A mutation returns an authorized resource DTO. A name-only reference deliberately takes
    // the missing-projection path and is not a complete provider reply.
    const access: Provider['access'] = { permissions: ['provider.view', 'provider.manage'], administers: false, participating: true, admin_only: [], through: [] };
    const first = { id: 'p1', name: 'Before', revision: 1, access };
    const next = { ...first, name: 'Research', revision: 2 };
    const read = vi.fn(() => Promise.resolve([first]));
    const readDetail = vi.fn(() => Promise.resolve(first));
    queryClient.setQueryData(['providers'], [first]);
    queryClient.setQueryData(['provider', 'p1'], first);
    function Rows() {
      const list = useQuery({ queryKey: ['providers'], queryFn: read, staleTime: Infinity });
      const detail = useQuery({ queryKey: ['provider', 'p1'], queryFn: readDetail, staleTime: Infinity });
      return <><ul>{list.data?.map((one) => <li key={one.id}>{one.name}</li>)}</ul><output>{detail.data?.name}</output></>;
    }
    render(<QueryClientProvider client={queryClient}><Rows /></QueryClientProvider>);
    await act(async () => {
      await actSucceeded({ method: 'PATCH', path: '/providers/p1', answer: { provider: next, job: null } });
      expect(queryClient.getQueryData(['provider', 'p1'])).toEqual(next);
    });
    // Query observers receive the cache's batched notice after the act itself has settled.
    await waitFor(() => {
      expect(screen.getByRole('listitem')).toHaveTextContent('Research');
      expect(screen.getByRole('status')).toHaveTextContent('Research');
    });
    expect(read).not.toHaveBeenCalled();
    expect(readDetail).not.toHaveBeenCalled();
  });

  it.each(Object.keys(ACTS))('%s: every view of what it changed shows the change, and nothing else is asked again', async (entry) => {
    const { method, path } = request(entry);
    const changes = findAct(method, path)?.changes ?? [];
    // Session authority changes the projection of every query, including otherwise unrelated data.
    const sessionChanged = changes.some((change) => 'session' in change);
    const stale = changes.filter((change): change is Stale => 'key' in change);
    const removed = stale.filter((change) => change.gone === true);
    const views = new Map<string, readonly string[]>();
    for (const change of stale) views.set(JSON.stringify(viewOf(change)), viewOf(change));
    const elsewhere = ['data-no-act-names', 'one'];
    views.set(JSON.stringify(elsewhere), elsewhere);

    render(
      <QueryClientProvider client={queryClient}>
        {[...views.entries()].map(([name, key]) => (
          <View key={name} queryKey={key} />
        ))}
      </QueryClientProvider>,
    );
    for (const name of views.keys()) expect(await screen.findByText(`${name} → 0`)).toBeInTheDocument();

    // The server takes the act, and what it answers afterwards is different; then the client tells the registry.
    state += 1;
    let held = new Map<string, unknown>();
    await act(async () => {
      await actSucceeded({ method: method as Act['method'], path });
      held = new Map([...views].map(([name, key]) => [name, queryClient.getQueryData(key)]));
      // The views on screen hear of a new answer through the query library's batched notice, one macrotask later.
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    for (const [name, key] of views) {
      const gone = removed.some((change) => covers(change, key));
      const named = !gone && (sessionChanged || stale.some((change) => change.gone !== true && covers(change, key)));
      // What it changed is fresh in the cache by the time the act resolved, and on screen once the views heard of it;
      // what it removed and what it does not name are as they were. Session authority names every view.
      expect(held.get(name)).toBe(named ? 1 : 0);
      expect(screen.getByText(`${name} → ${named ? 1 : 0}`)).toBeInTheDocument();
    }
    for (const change of changes) {
      if ('feed' in change) expect(fed).toContain(change.id === undefined ? change.feed : `${change.feed}:${change.id}`);
    }
  });
});

/** The documents written beside the places that are open in the tree: read when a place is opened, a page at a time, and read again when the place says it holds another number of them.
 *
 * A row of the tree says how many documents stand beside its place, which is what lets the place be
 * opened for them. The documents themselves are asked for only once the place is open, by the same
 * request and under the same key as the list of documents on the place's own screen, so that a
 * document made, moved or deleted there shows here without being asked for twice. A place that is
 * closed again is no longer watched.
 */
import { InfiniteQueryObserver } from '@tanstack/react-query';
import { useEffect, useRef } from 'react';
import { create } from 'zustand';

import { type DocumentRef, documentsOfNodeQuery } from '@/api/documents';
import { queryClient } from '@/api/query';

/** What is known of the documents beside one place. */
export interface NodeDocs {
  /** Those read so far, as the server lists them: the most recently changed first. */
  docs: readonly DocumentRef[];
  /** Whether the place holds more than were read. */
  more: boolean;
  /** `loading` before the first page has come, `failed` when it could not be read. */
  state: 'loading' | 'ready' | 'failed';
  /** Whether the next page is on its way. */
  loadingMore: boolean;
}

export type DocsByNode = ReadonlyMap<string, NodeDocs>;

const useTreeDocs = create<{ byNode: DocsByNode }>(() => ({ byNode: new Map() }));

interface Watch {
  observer: InfiniteQueryObserver<{ items: DocumentRef[]; next_cursor: string | null }>;
  stop: () => void;
  /** How many documents the place's row said it holds when they were last asked for. */
  count: number;
  /** The stamp of the place's row then: a document made, renamed, moved or deleted moves it. */
  seq: number;
  /** How many views watch it. */
  users: number;
}

const watched = new Map<string, Watch>();

/** What changed since the store was last told, by place: `null` for a place that is no longer watched. */
const pending = new Map<string, NodeDocs | null>();

/**
 * Notes what is known of a place's documents and tells the store once for everything noted in the same
 * turn: a tree that opens with a hundred places open is laid out again once, not a hundred times.
 */
function put(nodeId: string, docs: NodeDocs | null): void {
  if (pending.size === 0) {
    queueMicrotask(() => {
      useTreeDocs.setState((state) => {
        const byNode = new Map(state.byNode);
        for (const [id, known] of pending) {
          if (known === null) byNode.delete(id);
          else byNode.set(id, known);
        }
        pending.clear();
        return { byNode };
      });
    });
  }
  pending.set(nodeId, docs);
}

function watch(nodeId: string, count: number, seq: number): void {
  const held = watched.get(nodeId);
  if (held !== undefined) {
    held.users += 1;
    return;
  }
  const observer = new InfiniteQueryObserver(queryClient, {
    ...documentsOfNodeQuery(nodeId, { batch: true }),
    // A tree may watch hundreds of places: none of them asks again because the window came to the front. The row of
    // a place says when the documents beside it changed, and the screens of documents say when they changed one.
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  });
  const publish = (result: ReturnType<typeof observer.getCurrentResult>) => {
    put(nodeId, {
      docs: result.data?.pages.flatMap((page) => page.items) ?? [],
      more: result.hasNextPage,
      state: result.data !== undefined ? 'ready' : result.isError ? 'failed' : 'loading',
      loadingMore: result.isFetchingNextPage,
    });
  };
  const stop = observer.subscribe(publish);
  // What is held already is shown at once; before the first answer the place shows that its documents are on their way.
  publish(observer.getCurrentResult());
  watched.set(nodeId, { observer: observer as unknown as Watch['observer'], stop, count, seq, users: 1 });
}

function unwatch(nodeId: string): void {
  const held = watched.get(nodeId);
  if (held === undefined) return;
  held.users -= 1;
  if (held.users > 0) return;
  held.stop();
  watched.delete(nodeId);
  put(nodeId, null);
}

/** The place says it holds another number of documents than when they were read, or its row was stamped since (a
 * document beside it was renamed): they are read again. */
function recount(nodeId: string, count: number, seq: number): void {
  const held = watched.get(nodeId);
  if (held === undefined || (held.count === count && held.seq === seq)) return;
  held.count = count;
  held.seq = seq;
  void held.observer.refetch();
}

/** Reads the next page of the documents beside a place. */
export function loadMoreDocs(nodeId: string): void {
  void watched.get(nodeId)?.observer.fetchNextPage();
}

/** Tries again to read the documents beside a place, after a failure. */
export function retryDocs(nodeId: string): void {
  void watched.get(nodeId)?.observer.refetch();
}

/**
 * Watches the documents beside the given places for as long as the calling view is shown. `wanted` names
 * each place with the number of documents its row says it holds and the stamp of that row, as `id:count:seq`,
 * sorted, so that the same places as they were are the same text from one drawing to the next.
 */
export function useTreeDocsOf(wanted: string): DocsByNode {
  const watching = useRef<readonly string[]>([]);
  useEffect(() => {
    const places =
      wanted === ''
        ? []
        : wanted.split(',').map((entry) => {
            const [nodeId = '', count = '0', seq = '0'] = entry.split(':');
            return [nodeId, Number(count), Number(seq)] as const;
          });
    // The places that stay are taken before the ones that went are let go, so that a place watched before and after
    // is never without a watcher in between and is not asked for again.
    for (const [nodeId, count, seq] of places) {
      watch(nodeId, count, seq);
      recount(nodeId, count, seq);
    }
    for (const nodeId of watching.current) unwatch(nodeId);
    watching.current = places.map(([nodeId]) => nodeId);
  }, [wanted]);
  useEffect(
    () => () => {
      for (const nodeId of watching.current) unwatch(nodeId);
      watching.current = [];
    },
    [],
  );
  return useTreeDocs((state) => state.byNode);
}

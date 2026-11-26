/** What stands beside the places that are open in the tree, the documents written there and the shortcuts kept there: read when a place is opened, a page at a time, and read again when the place says it holds another number of them.
 *
 * A row of the tree says how many documents and how many shortcuts stand beside its place, which is what lets the
 * place be opened for them. They are asked for only once the place is open, by the same request and under the same
 * key as the lists of them on the place's own screen, so that one made, moved or deleted there shows here without
 * being asked for twice. A place that is closed again is no longer watched.
 */
import { InfiniteQueryObserver } from '@tanstack/react-query';
import { useEffect, useRef } from 'react';
import { create } from 'zustand';

import { type DocumentRef, documentsOfNodeQuery } from '@/api/documents';
import { queryClient } from '@/api/query';
import { type Shortcut, shortcutsOfPlaceQuery } from '@/api/shortcuts';

/** What is known of one list beside one place. */
interface Beside<T> {
  /** Those read so far, as the server lists them. */
  items: readonly T[];
  /** Whether the place holds more than were read. */
  more: boolean;
  /** `loading` before the first page has come, `failed` when it could not be read. */
  state: 'loading' | 'ready' | 'failed';
  /** Whether the next page is on its way. */
  loadingMore: boolean;
}

/** What is known of the documents beside one place. */
export interface NodeDocs extends Omit<Beside<DocumentRef>, 'items'> {
  /** Those read so far, as the server lists them: the most recently changed first. */
  docs: readonly DocumentRef[];
}

export type DocsByNode = ReadonlyMap<string, NodeDocs>;

/** What is known of the shortcuts kept in one place. */
export type NodeShortcuts = Beside<Shortcut>;

export type ShortcutsByNode = ReadonlyMap<string, NodeShortcuts>;

interface Watch {
  observer: InfiniteQueryObserver<{ items: unknown[]; next_cursor: string | null }>;
  stop: () => void;
  /** How many the place's row said it holds when they were last asked for. */
  count: number;
  /** The place's stamp then: making, renaming, moving or deleting one moves it. */
  seq: number;
  /** How many views watch it. */
  users: number;
}

/**
 * One kind of list beside the places of the tree: a store of what is known of each watched place, told once for
 * everything noted in the same turn (a tree that opens with a hundred places open is laid out again once, not a hundred
 * times), and the watching, reading again and paging of each place's list.
 */
function besideLists<T, V>(
  query: (nodeId: string) => object,
  shape: (known: Beside<T>) => V,
) {
  const useStore = create<{ byNode: ReadonlyMap<string, V> }>(() => ({ byNode: new Map() }));
  const watched = new Map<string, Watch>();
  /** What changed since the store was last told, by place: `null` for a place that is no longer watched. */
  const pending = new Map<string, V | null>();

  const put = (nodeId: string, known: V | null): void => {
    if (pending.size === 0) {
      queueMicrotask(() => {
        useStore.setState((state) => {
          const byNode = new Map(state.byNode);
          for (const [id, value] of pending) {
            if (value === null) byNode.delete(id);
            else byNode.set(id, value);
          }
          pending.clear();
          return { byNode };
        });
      });
    }
    pending.set(nodeId, known);
  };

  const watch = (nodeId: string, count: number, seq: number): void => {
    const held = watched.get(nodeId);
    if (held !== undefined) {
      held.users += 1;
      return;
    }
    const observer = new InfiniteQueryObserver(queryClient, {
      ...(query(nodeId) as ConstructorParameters<typeof InfiniteQueryObserver>[1]),
      // A tree may watch hundreds of places: none of them asks again because the window came to the front. The row of
      // a place says when what stands beside it changed, and the screens say when they changed one.
      refetchOnWindowFocus: false,
      refetchOnReconnect: false,
    }) as unknown as Watch['observer'];
    const publish = (result: ReturnType<Watch['observer']['getCurrentResult']>) => {
      put(
        nodeId,
        shape({
          items: (result.data?.pages.flatMap((page) => page.items) ?? []) as T[],
          more: result.hasNextPage,
          state: result.data !== undefined ? 'ready' : result.isError ? 'failed' : 'loading',
          loadingMore: result.isFetchingNextPage,
        }),
      );
    };
    const stop = observer.subscribe(publish);
    // What is held already is shown at once; before the first answer the place shows that its list is on its way.
    publish(observer.getCurrentResult());
    watched.set(nodeId, { observer, stop, count, seq, users: 1 });
  };

  const unwatch = (nodeId: string): void => {
    const held = watched.get(nodeId);
    if (held === undefined) return;
    held.users -= 1;
    if (held.users > 0) return;
    held.stop();
    watched.delete(nodeId);
    put(nodeId, null);
  };

  /** The place says it holds another number than when they were read, or it was stamped since (one beside it was renamed): they are read again. */
  const recount = (nodeId: string, count: number, seq: number): void => {
    const held = watched.get(nodeId);
    if (held === undefined || (held.count === count && held.seq === seq)) return;
    held.count = count;
    held.seq = seq;
    void held.observer.refetch();
  };

  /**
   * Watches the lists beside the given places for as long as the calling view is shown. `wanted` names each place
   * with the number its row says it holds and its stamp, as `id:count:seq`, sorted, so that the same places as they
   * were are the same text from one drawing to the next.
   */
  const useOf = (wanted: string): ReadonlyMap<string, V> => {
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
    return useStore((state) => state.byNode);
  };

  return {
    useOf,
    loadMore: (nodeId: string): void => void watched.get(nodeId)?.observer.fetchNextPage(),
    retry: (nodeId: string): void => void watched.get(nodeId)?.observer.refetch(),
  };
}

const docs = besideLists<DocumentRef, NodeDocs>(
  (nodeId) => documentsOfNodeQuery(nodeId, { batch: true }),
  ({ items, ...rest }) => ({ docs: items, ...rest }),
);

const shortcuts = besideLists<Shortcut, NodeShortcuts>((nodeId) => shortcutsOfPlaceQuery(nodeId), (known) => known);

/** Reads the next page of the documents beside a place. */
export const loadMoreDocs = docs.loadMore;

/** Tries again to read the documents beside a place, after a failure. */
export const retryDocs = docs.retry;

/** Watches the documents beside the given places (`id:count:docs_seq`, sorted) for as long as the calling view is shown. */
export const useTreeDocsOf = (wanted: string): DocsByNode => docs.useOf(wanted);

/** Reads the next page of the shortcuts kept in a place. */
export const loadMoreShortcuts = shortcuts.loadMore;

/** Tries again to read the shortcuts kept in a place, after a failure. */
export const retryShortcuts = shortcuts.retry;

/** Watches the shortcuts kept in the given places (`id:count:0`, sorted) for as long as the calling view is shown. */
export const useTreeShortcutsOf = (wanted: string): ShortcutsByNode => shortcuts.useOf(wanted);

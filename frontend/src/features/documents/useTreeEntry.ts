/** Content-free entry metadata follows its own identity, including ancestor-only moves. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useMemo, useState } from 'react';
import { useStore } from 'zustand';
import { record } from '@/api/cache';
import { ApiError, client } from '@/api/client';
import { observeDocumentEntry, restartDocumentEntry } from '@/api/documentTreeChanges';
import { documentTreeEntryKey, documentTreeEntryQuery } from '@/api/documentTree';
import { useFeedLocator } from '@/features/documents/useFeedLocator';
import type { DocumentTreeItem, DocumentTreeRead } from '@/generated/api-types';
import { socket } from '@/realtime/live';
import type { FeedFrame, Subscription } from '@/realtime/socket';

function matches(value: unknown, item: DocumentTreeItem): value is DocumentTreeRead {
  if (!record(value) || !Array.isArray(value.entries) || !Array.isArray(value.ancestors)) return false;
  const entries: unknown[] = value.entries;
  return entries.length === 1 && entries.every((row) => record(row) && record(row.entry) && record(row.entry.item)
    && row.entry.item.kind === item.kind && row.entry.item.id === item.id);
}

export function useTreeEntry(item: DocumentTreeItem, retained = false, enabled = true) {
  const cache = useQueryClient();
  const reader = useStore(client.session, (state) => state.readerVersion);
  const { kind, id } = item;
  const selected = useMemo<DocumentTreeItem>(() => ({ kind, id }), [kind, id]);
  const key = useMemo(() => documentTreeEntryKey(selected, retained), [selected, retained]);
  const query = useQuery({ ...documentTreeEntryQuery(selected, cache, retained), enabled });
  const [attempt, setAttempt] = useState(0);
  const feed = useFeedLocator(JSON.stringify([reader, ...key]), query.data?.feed);
  useEffect(() => {
    if (!enabled) return;
    const name = feed;
    let active = true, generation = 0, refused = false;
    let subscription: Subscription | null = null;
    const stopObservation = observeDocumentEntry(cache, selected, () => {
      generation += 1; subscription?.unsubscribe(); setAttempt((value) => value + 1);
    });
    const current = (ticket: number) => active && ticket === generation && reader === client.session.getState().readerVersion;
    const receive = (ticket: number, frame: FeedFrame) => {
      if (!current(ticket) || refused) return;
      if (frame.op === 'event' || frame.op === 'sync') {
        const next = frame.data.entry;
        if (!matches(next, selected)) return;
        const held = cache.getQueryCache().find({ queryKey: key, exact: true });
        if (held !== undefined) void held.cancel({ revert: false, silent: true });
        cache.setQueryData(key, next);
      } else if (frame.op === 'resync') {
        generation += 1; subscription?.unsubscribe();
        if (frame.reload === true) {
          const held = cache.getQueryCache().find({ queryKey: key, exact: true });
          if (held !== undefined) {
            void held.cancel({ revert: false, silent: true });
            held.setState({ data: undefined, dataUpdatedAt: 0, error: null, status: 'pending', fetchStatus: 'idle' });
          }
        }
        subscribe();
      } else if (frame.op === 'error' && frame.code !== 'too_many_subscriptions' || frame.op === 'closed' && frame.reason !== 'unsubscribed') {
        const denied = frame.op === 'error' ? frame.code === 'forbidden' : frame.reason === 'revoked';
        const missing = frame.op === 'error' ? frame.code === 'not_found' : frame.reason === 'deleted';
        const code = denied ? 'authz.denied' : missing ? 'not_found' : 'internal';
        const error = new ApiError({ type: `orch:error:${code}`, code, status: denied ? 403 : missing ? 404 : 503, title: '', detail: '', request_id: '' }, null);
        refused = denied || missing;
        const held = cache.getQueryCache().find({ queryKey: key, exact: true });
        if (held !== undefined) {
          void held.cancel({ revert: false, silent: true });
          held.setState({ ...(refused ? { data: undefined, dataUpdatedAt: 0 } : {}), error, status: 'error', fetchStatus: 'idle' });
        }
      }
    };
    function subscribe() {
      if (name === undefined || name === null) return;
      const ticket = generation;
      subscription = socket.subscribe(name, { since: () => null, onFrame: (frame) => receive(ticket, frame) });
    }
    subscribe();
    return () => { active = false; stopObservation(); subscription?.unsubscribe(); };
  }, [attempt, cache, enabled, feed, key, reader, selected]);
  return { ...query, retry: () => { restartDocumentEntry(cache, selected); return query.refetch(); } };
}

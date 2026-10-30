/** One visible document-directory page follows its exact metadata stream, without a refresh loop. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useMemo, useState } from 'react';
import { useStore } from 'zustand';

import { record } from '@/api/cache';
import { ApiError, client } from '@/api/client';
import { documentDirectoryKey, documentDirectoryQuery, type DocumentDirectoryRequest } from '@/api/documentTree';
import { observeDocumentDirectory } from '@/api/documentTreeChanges';
import { adoptDocumentDirectory, canonicalDirectoryRequest, retireDocumentDirectory } from '@/api/documentTreeCache';
import { useViewShown } from '@/features/workbench/viewOpening';
import { useFeedLocator } from '@/features/documents/useFeedLocator';
import type { DocumentTreePage } from '@/generated/api-types';
import { socket } from '@/realtime/live';
import type { FeedFrame, Subscription } from '@/realtime/socket';

/** A projection for another directory can never replace the page currently being read. */
function isDirectoryPage(value: unknown, request: DocumentDirectoryRequest): value is DocumentTreePage {
  if (!record(value) || !record(value.directory) || !record(value.directory.at) || !record(value.directory.node)) return false;
  return value.directory.at.kind === request.at.kind && value.directory.at.id === request.at.id
    && typeof value.directory.node.id === 'string' && (request.at.kind !== 'node_docs' || value.directory.node.id === request.at.id)
    && typeof value.feed === 'string' && Array.isArray(value.items) && Array.isArray(value.ancestors)
    && (value.next_cursor === null || typeof value.next_cursor === 'string');
}

type LivePhase = 'connecting' | 'current' | 'waiting' | 'offline' | 'failed';

export function useDirectoryPage(request: Omit<DocumentDirectoryRequest, 'cursor'>) {
  const cache = useQueryClient();
  const shown = useViewShown();
  const reader = useStore(client.session, (state) => state.readerVersion);
  const { at, deleted = false } = request;
  const { kind, id } = at;
  const scope = JSON.stringify([reader, kind, id, deleted]);
  const [anchor, setAnchor] = useState<{ scope: string; node: string; from: string } | null>(null);
  const canonicalNode = anchor?.scope === scope && anchor.from === request.node ? anchor.node : canonicalDirectoryRequest(request).node;
  const [position, setPosition] = useState<{ scope: string; node: string; cursor: string | null }>({ scope, node: canonicalNode, cursor: null });
  const [retryVersion, setRetryVersion] = useState(0);
  const [live, setLive] = useState<{ key: string; phase: LivePhase; error: Error | null } | null>(null);
  const cursor = position.scope === scope && position.node === canonicalNode ? position.cursor : null;
  const selection = useMemo<DocumentDirectoryRequest>(() => ({ node: canonicalNode, at: { kind, id }, deleted, cursor }), [canonicalNode, kind, id, deleted, cursor]);
  const query = useQuery({ ...documentDirectoryQuery(selection, cache), enabled: shown });
  const stateKey = JSON.stringify([reader, ...documentDirectoryKey(selection)]);
  const feed = useFeedLocator(stateKey, query.data?.feed);

  useEffect(() => {
    if (!shown) return;
    const subscribedFeed = feed;
    let active = true;
    let generation = 0;
    let refused = false;
    let subscription: Subscription | null = null;
    const stopObservation = observeDocumentDirectory(cache, selection, (next) => {
      generation += 1; subscription?.unsubscribe();
      if (next.node !== selection.node) setAnchor({ scope, node: next.node, from: request.node });
      setPosition({ scope, node: next.node, cursor: null });
      setRetryVersion((version) => version + 1);
    });
    const current = (ticket: number) => active && ticket === generation && reader === client.session.getState().readerVersion;
    const phase = (phase: LivePhase, error: Error | null = null) => setLive({ key: stateKey, phase, error });
    const stopTransport = socket.status.subscribe((next, previous) => {
      if (next.phase !== previous.phase && current(generation)) phase(next.phase === 'ready' ? 'connecting' : 'offline');
    });
    const receive = (ticket: number, frame: FeedFrame) => {
      if (!current(ticket) || refused) return;
      if (frame.op === 'error' && frame.code === 'too_many_subscriptions') { phase('waiting'); return; }
      if (frame.op === 'subscribed') { phase('connecting'); return; }
      if (frame.op === 'event' || frame.op === 'sync') {
        const page = frame.data.directory;
        if (!isDirectoryPage(page, selection)) return;
        const adopted = adoptDocumentDirectory(cache, selection, page, reader, frame.data.restart === true);
        if (adopted === null) return;
        if (adopted.node !== selection.node) setAnchor({ scope, node: adopted.node, from: request.node });
        if ((adopted.cursor ?? null) !== cursor) setPosition({ scope, node: adopted.node, cursor: adopted.cursor ?? null });
        phase('current');
      } else if (frame.op === 'resync') {
        generation += 1;
        subscription?.unsubscribe();
        if (frame.reload === true) retireDocumentDirectory(cache, selection, reader);
        subscribe();
      } else if (frame.op === 'error' || frame.op === 'closed' && frame.reason !== 'unsubscribed') {
        const denied = frame.op === 'error' ? frame.code === 'forbidden' : frame.reason === 'revoked';
        const missing = frame.op === 'error' ? frame.code === 'not_found' : frame.reason === 'deleted';
        const code = denied ? 'authz.denied' : missing ? 'not_found' : 'internal';
        const error = new ApiError({ type: `orch:error:${code}`, code, title: '', detail: '', status: denied ? 403 : missing ? 404 : 503, request_id: '' }, null);
        phase('failed', error);
        if (denied || missing) { refused = true; retireDocumentDirectory(cache, selection, reader, error); }
      }
    };
    function subscribe() {
      if (subscribedFeed === undefined || subscribedFeed === null) return;
      const ticket = generation;
      subscription = socket.subscribe(subscribedFeed, { since: () => null, onFrame: (frame) => receive(ticket, frame) });
    }
    subscribe();
    return () => { active = false; stopObservation(); stopTransport(); subscription?.unsubscribe(); };
  }, [cache, cursor, feed, reader, retryVersion, scope, selection, shown, stateKey, request.node]);

  const staleCursor = query.error instanceof ApiError && query.error.is('conflict.stale_revision')
    && query.error.problem.restart === true && query.error.problem.reason === 'document_directory';
  const restart = async () => {
    retireDocumentDirectory(cache, selection, reader);
    setPosition({ scope, node: canonicalNode, cursor: null });
    setRetryVersion((version) => version + 1);
    await cache.query(documentDirectoryQuery({ ...selection, cursor: null }, cache));
  };
  const retry = async () => {
    if (staleCursor) { await restart(); return; }
    setRetryVersion((version) => version + 1);
    await query.refetch();
  };
  return { query, cursor, canonicalNode, staleCursor, restart, retry,
    live: live?.key === stateKey ? live : { phase: 'connecting' as const, error: null },
    go: (next: string | null) => setPosition({ scope, node: canonicalNode, cursor: next }) };
}

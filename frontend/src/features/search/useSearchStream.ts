/** Presentation of the server's streamed order; held while a result is read and fixed once the first screen settles. */
import { useCallback, useEffect, useReducer, useRef } from 'react';
import { useQuery, useQueryClient, type InfiniteData } from '@tanstack/react-query';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { keyOfHit } from '@/features/search/hits';
import { searchKey, streamSearch, type SearchFilters } from '@/api/search';
import type { SearchHit, SearchHitPage, SearchOrderEntry, SearchStreamEvent } from '@/generated/api-types';

/** The wire encodes an elapsed typing age as an unsigned 32-bit millisecond count. */
const MAX_TYPING_AGE_MS = 0xffff_ffff;

interface State {
  key: string;
  hits: Record<string, SearchHit>;
  membership: Record<string, SearchOrderEntry['found_by']>;
  fields: Record<string, string>;
  shown: string[];
  base: string[];
  held: string[] | null;
  settled: boolean;
  blocked: boolean;
  expanded: boolean;
  ended: boolean;
  pageSize: number;
  window: number;
  running: Record<string, boolean>;
  reasons: Record<string, string | null>;
  unavailable: Record<string, boolean>;
  cursor: string | null;
  error: unknown;
}
const initial = (key = ''): State => ({key, hits: {}, membership: {}, fields: {}, shown: [], base: [], held: null, settled: false, blocked: false,
  expanded: false, ended: false, pageSize: 0, window: 0, running: {words: true, meaning: true}, reasons: {}, unavailable: {}, cursor: null, error: null});
type Action = {type: 'event'; event: SearchStreamEvent} | {type: 'reset'; key: string} | {type: 'settle'} | {type: 'block'; value: boolean} | {type: 'reveal'} | {type: 'continue'} | {type: 'error'; error: unknown};
const append = (before: string[], next: string[]) => [...before, ...next.filter((id) => !before.includes(id))];
function order(state: State, ids: string[]): State {
  if (state.settled) return {...state, shown: append(state.shown, ids), held: null};
  if (state.blocked) return {...state, held: ids};
  return {...state, shown: append(state.base, ids), held: null};
}
function reducer(state: State, action: Action): State {
  switch (action.type) {
    case 'reset': return initial(action.key);
    case 'error': return {...state, error: action.error, ended: true, running: {words: false, meaning: false}};
    case 'block': {
      const next = {...state, blocked: action.value};
      return !action.value && state.held !== null ? order(next, state.held) : next;
    }
    case 'settle': {
      const next = {...state, settled: true};
      return state.held === null ? next : order(next, state.held);
    }
    case 'reveal': {
      const next = {...state, settled: true, expanded: true};
      return state.held === null ? next : order(next, state.held);
    }
    case 'continue': return {...state, base: state.shown, settled: true, ended: false, running: {words: false, meaning: false}, cursor: null};
    case 'event': {
      const event = action.event;
      switch (event.type) {
        case 'options': return {...state, pageSize: event.page_size, window: event.reorder_window_secs, settled: state.settled || event.reorder_window_secs === 0};
        case 'hit': return {...state, hits: {...state.hits, [event.id]: event.hit}, membership: {...state.membership, [event.id]: state.membership[event.id] ?? event.found_by}};
        case 'order': return order({...state,
          membership: {...state.membership, ...Object.fromEntries(event.items.map((item) => [item.id, item.found_by]))},
          fields: {...state.fields, ...Object.fromEntries(event.items.flatMap((item) => 'match_field' in item && typeof item.match_field === 'string' ? [[item.id, item.match_field]] : []))},
        }, event.items.map((item) => item.id));
        case 'source': return {...state, running: {...state.running, [event.source]: event.state === 'running'}, reasons: {...state.reasons, [event.source]: event.reason}, unavailable: {...state.unavailable, [event.source]: event.state === 'unavailable'}};
        case 'cursor': return {...state, cursor: event.cursor, ended: true};
      }
    }
  }
}

export function useSearchStream(filters: SearchFilters, enabled: boolean, immediate: boolean, submitted: number, typedAt: number, onUnavailable?: (error: unknown) => boolean) {
  const cache = useQueryClient();
  const reader = useStore(client.session, (session) => `${session.userId ?? ''}:${session.readerVersion ?? 0}`);
  const queryKey = [...searchKey(filters), 'stream', reader] as const;
  const key = JSON.stringify(queryKey);
  const [stored, dispatch] = useReducer(reducer, undefined, initial);
  // A new reader or selection never paints the preceding reader's parts, even for one frame.
  const state = stored.key === key ? stored : initial(key);
  const current = useRef(state);
  useEffect(() => {current.current = state;}, [state]);
  const startedAt = useRef(0);
  const request = useRef<AbortController | null>(null);
  const collect = useCallback(async (signal: AbortSignal, cursor?: string): Promise<InfiniteData<SearchHitPage>> => {
    request.current?.abort();
    const controller = new AbortController(); request.current = controller;
    const abort = () => controller.abort();
    if (signal.aborted) abort(); else signal.addEventListener('abort', abort, {once: true});
    if (cursor === undefined) {
      startedAt.current = Date.now(); dispatch({type: 'reset', key});
      cache.setQueryData(queryKey, {pages: [{items: [], next_cursor: null}], pageParams: [null]});
    }
    try {
      await streamSearch(filters, (event) => {
        if (controller.signal.aborted) return;
        if (event.type === 'hit') cache.setQueryData<InfiniteData<SearchHitPage>>(queryKey, (before) => {
          const items = before?.pages.flatMap((page) => page.items) ?? [];
          const exists = items.some((hit) => keyOfHit(hit) === keyOfHit(event.hit));
          return {pages: [{items: exists ? items : [...items, event.hit], next_cursor: null}], pageParams: [null]};
        });
        dispatch({type: 'event', event});
      }, controller.signal, cursor, immediate, immediate ? 0 : Math.max(0, Math.min(MAX_TYPING_AGE_MS, Math.round(Date.now() - typedAt))));
    } catch (error) {
      if (!controller.signal.aborted && !onUnavailable?.(error)) dispatch({type: 'error', error});
      throw error;
    } finally { signal.removeEventListener('abort', abort); }
    return cache.getQueryData<InfiniteData<SearchHitPage>>(queryKey) ?? {pages: [], pageParams: []};
    // The canonical key contains every selection and reader member.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, immediate, typedAt, cache, onUnavailable]);
  // The normal query owns cancellation, authority retirement and native reference patches.
  // Payloads are cached during a stream but its order belongs to the mounted view. Reopening must
  // start a new authorized stream even if an interrupted stream left cached payloads behind.
  const source = useQuery({queryKey, queryFn: ({signal}) => collect(signal), enabled, staleTime: Infinity, refetchOnMount: 'always', retry: false});
  useEffect(() => {if (!enabled) request.current?.abort();}, [enabled]);
  useEffect(() => () => request.current?.abort(), []);
  const {refetch}=source;
  const previous = useRef({key, submitted});
  useEffect(() => {
    const was = previous.current; previous.current = {key, submitted};
    if (enabled && was.key === key && was.submitted !== submitted) void refetch();
  }, [key, submitted, enabled, refetch]);
  useEffect(() => {
    if (state.window === 0 || !enabled) return;
    const timer = setTimeout(() => dispatch({type: 'settle'}), Math.max(0, state.window * 1000 - (Date.now() - startedAt.current)));
    return () => clearTimeout(timer);
  }, [state.window, key, enabled]);
  useEffect(() => {
    const scroll = (event: Event) => {
      const target = event.target;
      const top = target instanceof HTMLElement ? target.scrollTop : window.scrollY;
      const height = target instanceof HTMLElement ? target.clientHeight : window.innerHeight;
      if (height > 0 && top > height) dispatch({type: 'settle'});
    };
    document.addEventListener('scroll', scroll, true);
    return () => document.removeEventListener('scroll', scroll, true);
  }, []);
  const more = () => {
    const held = current.current;
    if (!held.expanded) dispatch({type: 'reveal'});
    else if (held.cursor !== null) {
      dispatch({type: 'continue'});
      const cursor = held.cursor;
      void cache.query({queryKey: [...queryKey, 'more', cursor], queryFn: ({signal}) => collect(signal, cursor), staleTime: 0, gcTime: 0, retry: false}).catch(() => undefined);
    }
  };
  const block = useCallback((value: boolean) => dispatch({type: 'block', value}), [dispatch]);
  const cached = new Map((source.data?.pages ?? []).flatMap((page) => page.items).map((hit) => [keyOfHit(hit), hit]));
  const hits = Object.fromEntries(Object.entries(state.hits).flatMap(([id, payload]) => {
    const hit = cached.get(keyOfHit(payload));
    return hit === undefined ? [] : [[id, hit]];
  }));
  const available = state.shown.filter((id) => hits[id] !== undefined);
  const ids = state.expanded ? available : available.slice(0, state.pageSize);
  return {...state, hits, ids, total: Object.keys(hits).length, unfinished: !state.ended || Object.values(state.running).some(Boolean),
    more, block, hasMore: !state.expanded && Object.keys(hits).length > state.pageSize || state.cursor !== null};
}

/** A list resumes its own last state on this device; an addressed state always wins. */
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type Dispatch, type SetStateAction } from 'react';

import { useStore } from 'zustand';
import { client } from '@/api/client';

import { usePageContext } from '@/features/workbench/pageContext';
import { useViewChoice } from '@/lib/viewChoices';
import { useViewInstance } from '@/features/workbench/instance';

type Search = Record<string, unknown>;
const listRoutes = new Set(['/', '/everything/', '/inbox/', '/approvals/', '/search/', '/usage/', '/storage/',
  '/admin/$section/', '/org/$slug/', '/org/$slug/messages/', '/project/$projectId/', '/topic/$topicId/',
  '/trace/$nodeId/', '/shared/', '/doc/$documentId/', '/org/$slug/settings/$section/', '/documents/$nodeId/', '/document-folder/$folderId/']);
export function isListRoute(route: string): boolean { return listRoutes.has(route) || /^\/resources\/[^/$]+\/$/.test(route); }

/** Search words and content locators are not the list's view, order or filters. */
const transient = new Set(['q','query','text','cursor','message','item','items','block','line','file','comment','version','setting','continuation','prompt','catchup','job','focus','highlight','selected']);
export function listSearch(search: Search, route = ''): Search {
  return Object.fromEntries(Object.entries(search).filter(([key, value]) => !transient.has(key) && value !== undefined && !(route==='/topic/$topicId/'&&key==='tab'&&value==='settings')));
}
export function storedListState(value: string): Search {
  try { const parsed: unknown = JSON.parse(value); return parsed !== null && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed as Search : {}; }
  catch { return {}; }
}

/** The page adapter validates both addressed and restored values with the same route schema. */
export function usePageListSearch(route: string, pathname: string, addressed: Search, validate: (value: Search) => Search): Search {
  const scope=route==='/topic/$topicId/'&&addressed.tab==='settings'?`${pathname}.settings`:pathname;
  const [kept, keep] = useViewChoice<string>(`page.${scope}`, 'state', '{}');
  const enabled = isListRoute(route);
  const explicit = Object.keys(listSearch(addressed,route)).length>0 || ['q','query','text'].some((key)=>Object.hasOwn(addressed,key));
  const search = useMemo(() => {
    const raw = enabled && !explicit ? { ...storedListState(kept), ...addressed } : addressed;
    const result = validate(raw);
    return { ...result, ...(raw.list !== undefined ? { list: raw.list } : {}), ...(enabled ? { _list: true } : {}) };
  }, [addressed, enabled, explicit, kept, validate]);
  const serialized = JSON.stringify(listSearch(search,route));
  useEffect(() => { if (enabled && serialized !== kept) keep(serialized); }, [enabled, serialized, kept, keep]);
  return search;
}

/** Local list controls join the page's URL without changing the options that control offers. */
export function useListChoice<T>(list: string, key: string, fallback: T): [T, Dispatch<SetStateAction<T>>] {
  const page = usePageContext();
  const user=useStore(client.session,(state)=>state.userId);
  const reader=useStore(client.session,(state)=>state.readerVersion);
  const instance = useViewInstance();
  const target=instance?.target;
  const pathname = page?.target.pathname ?? (target === null || target === undefined ? list : `/${target.kind}/${target.id}`);
  const [kept, keep] = useViewChoice(`page.${pathname}.${list}`, key, JSON.stringify(fallback));
  const addressed = page?.target.search.list;
  const choices = useMemo<Search>(()=>typeof addressed === 'object' && addressed !== null && !Array.isArray(addressed) ? addressed as Search : {},[addressed]);
  const name = `${list}.${key}`;
  const raw = Object.hasOwn(choices,name) ? choices[name] : (page?.target.search._list === true ? fallback : storedListState(`{"value":${kept}}`).value);
  // Enum callers validate membership against their live choices; malformed device data uses the default.
  // A null default carries no information about the selected value's non-null type.
  const restored = (raw !== undefined && (raw === null ? fallback === null : fallback === null || (Array.isArray(fallback) ? Array.isArray(raw) : typeof raw === typeof fallback)) ? raw : fallback) as T;
  const scope=`${reader}:${pathname}:${list}:${key}`;
  const [anonymous,setAnonymous]=useState<{scope:string;value:T}|null>(null);
  const value=user===null&&page===null&&anonymous?.scope===scope?anonymous.value:restored;
  const current = useRef(value);
  useLayoutEffect(()=>{current.current=value;},[value]);
  const choose = useCallback<Dispatch<SetStateAction<T>>>((next) => {
    const value = typeof next === 'function' ? (next as (before: T) => T)(current.current) : next;
    current.current = value;
    // Unsigned readers can use a control without creating another person's device choice.
    if(user===null&&page===null)setAnonymous({scope,value});
    keep(JSON.stringify(value));
    if (page !== null) void page.navigate({ search: (before) => ({ ...before, _list: true, list: {
      ...(typeof before.list === 'object' && before.list !== null ? before.list : {}), [name]: value,
    } }), replace: true });
  }, [keep, name, page, user, scope]);
  const serialized=JSON.stringify(value);
  const published=useRef<string|null>(null);
  useEffect(()=>{
    if(page!==null && choices[name]===undefined && published.current!==serialized) {
      published.current=serialized;choose(JSON.parse(serialized) as T);
    } else if(choices[name]!==undefined) published.current=null;
  },[page,choices,name,choose,serialized]);
  return [value, choose];
}

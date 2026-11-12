/**
 * A list resumes its own last state on this device; an addressed state always wins.
 *
 * What an address holds of lists, whoever writes it (a control's own choice, a page's navigation, the arrival of a bare address): the
 * choices of the controls on the screen the address names, each when the person chose it or the control is on screen, and always with the
 * marker (`_list=true`), which says that the address is the whole state and nothing is to be resumed beside it. The choices of another
 * screen never travel: not into a navigation to a screen that has none of those controls, and not from a hook that only reads a choice to ask
 * for what the screen shows. A writer takes its `list` from `navigatedList` and a control tells the page that it is on screen by `useListChoice`.
 */
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type Dispatch, type SetStateAction } from 'react';

import { useStore } from 'zustand';
import { client } from '@/api/client';

import { usePageContext } from '@/features/workbench/pageContext';
import { useViewChoice } from '@/lib/viewChoices';
import { useViewInstance } from '@/features/workbench/instance';
import { canonicalListSearch, objectOf } from '@/lib/listAddress';

type Search = Record<string, unknown>;

/** Which controls of each open page are on screen now, by the name they are chosen under (`trace.group`): what an address may hold of the lists. */
const onScreenControls = new Map<string, Map<string, number>>();
function onScreen(page: string, name: string): () => void {
  const names = onScreenControls.get(page) ?? new Map<string, number>();
  onScreenControls.set(page, names);
  names.set(name, (names.get(name) ?? 0) + 1);
  return () => {
    const left = (names.get(name) ?? 1) - 1;
    if (left > 0) names.set(name, left); else names.delete(name);
    if (names.size === 0 && onScreenControls.get(page) === names) onScreenControls.delete(page);
  };
}
/** The part of a list state that belongs to controls on this page's screen. */
export function listOnScreen(page: string, list: unknown): Search | undefined {
  const held = objectOf(list), names = onScreenControls.get(page);
  if (held === undefined || names === undefined) return undefined;
  const kept = Object.entries(held).filter(([name]) => names.has(name));
  return kept.length === 0 ? undefined : canonicalListSearch({ list: Object.fromEntries(kept) }).list as Search;
}
/** The keys of an address that say which screen of a page it names (a topic's tab and Settings section), apart from what is chosen on it. */
const screenKeys: Readonly<Record<string, readonly string[]>> = { '/topic/$topicId/': ['tab', 'section'], '/org/$slug/': ['tab'] };
export function sameScreen(route: string, one: Search, other: Search): boolean {
  return (screenKeys[route] ?? []).every((key) => one[key] === other[key]);
}
/**
 * The `list` of the address a page's navigation writes, from the state it leaves (`before`) and the search it asks for (`next`).
 * A list the navigation names itself, or the remembered list of the screen it goes to, is that screen's own. Where it names none, the
 * choices go on only to the same screen and only for the controls that are on it; a navigation to another screen takes none along.
 */
export function navigatedList(route: string, page: string, before: Search, next: Search): Search | undefined {
  if (next.list !== undefined && next.list !== before.list) return objectOf(next.list);
  return sameScreen(route, before, next) ? listOnScreen(page, before.list) : undefined;
}
const listRoutes = new Set(['/', '/everything/', '/inbox/', '/approvals/', '/search/', '/usage/', '/storage/',
  '/admin/$section/', '/org/$slug/', '/org/$slug/messages/', '/project/$projectId/', '/topic/$topicId/',
  '/trace/$nodeId/', '/shared/', '/doc/$documentId/', '/documents/$nodeId/', '/document-folder/$folderId/']);
export function isListRoute(route: string): boolean { return listRoutes.has(route) || /^\/resources\/[^/$]+\/$/.test(route); }

/** Resource views have readable shareable parameters; disclosure state never becomes an address. */
export function isResourceListRoute(route: string): boolean { return /^\/resources\/[^/$]+\/$/.test(route); }
const resourceKeys: Readonly<Record<string, string>> = { view: 'view', groupBy: 'group', sort: 'sort', direction: 'direction', participating: 'participating' };
function resourceChoices(search: Search): Search {
  return Object.fromEntries(['view', 'group', 'sort', 'direction', 'participating'].flatMap((key) => {
    const value = search[key];
    return typeof value === 'string' || typeof value === 'boolean' ? [[key, value]] : [];
  }));
}
export function resourceListSearch(search: Search): Search {
  return Object.fromEntries(Object.entries(search).filter(([key]) => key !== '_list' && key !== 'list'));
}

/** Search words and content locators are not the list's view, order or filters. */
const transient = new Set(['q','query','text','cursor','message','item','items','block','line','file','comment','version','setting','continuation','prompt','catchup','job','focus','highlight','selected']);
export function listSearch(search: Search, route = ''): Search {
  return Object.fromEntries(Object.entries(search).filter(([key, value]) => !transient.has(key) && value !== undefined && !(route==='/topic/$topicId/'&&key==='tab'&&value==='settings')));
}
/** What names the page itself, such as the scope of a usage report or the section of a topic's Settings, is where the page is and never a list's remembered state (an address without it means the first). */
const identityKeys: Readonly<Record<string, readonly string[]>> = { '/usage/': ['scope'], '/topic/$topicId/': ['section'] };
/** The list state a page may remember and resume: its list controls, without the keys that say which page it is. */
export function rememberedListSearch(search: Search, route = ''): Search {
  const identity = identityKeys[route] ?? [];
  return Object.fromEntries(Object.entries(listSearch(search, route)).filter(([key]) => !identity.includes(key)));
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
    // Apply the same content-state rules to older device data as to newly saved choices.
    const raw = enabled && !explicit ? { ...rememberedListSearch(storedListState(kept), route), ...addressed } : addressed;
    const result = validate(raw);
    if (isResourceListRoute(route)) return resourceListSearch({ ...result, ...resourceChoices(raw) });
    return canonicalListSearch({ ...result, ...(enabled && raw.list !== undefined ? { list: raw.list } : {}), ...(enabled ? { _list: true } : {}) });
  }, [addressed, enabled, explicit, kept, validate, route]);
  const serialized = JSON.stringify(rememberedListSearch(search,route));
  useEffect(() => { if (enabled && serialized !== kept) keep(serialized); }, [enabled, serialized, kept, keep]);
  return search;
}

/**
 * Addressed list controls join their list's URL; transient controls keep only device state. A control puts its state into the address
 * while it is on screen (`shown`): a control that is mounted but closed, such as a drawer, has not been looked at, and a default it
 * never showed is no choice of the person's to carry in an address that is copied.
 */
export function useListChoice<T>(list: string, key: string, fallback: T, persistence: 'address' | 'device' = 'address', shown = true): [T, Dispatch<SetStateAction<T>>] {
  const context = usePageContext();
  const resource = context !== null && isResourceListRoute(context.target.route);
  const queryKey = resourceKeys[key];
  const page = persistence === 'address' && context !== null && isListRoute(context.target.route) && (!resource || queryKey !== undefined) && (!list.startsWith('resources.') || resource) ? context : null;
  const user=useStore(client.session,(state)=>state.userId);
  const reader=useStore(client.session,(state)=>state.readerVersion);
  const instance = useViewInstance();
  const target=instance?.target;
  const pathname = context?.target.pathname ?? (target === null || target === undefined ? list : `/${target.kind}/${target.id}`);
  const [kept, keep] = useViewChoice(`page.${pathname}.${list}`, key, JSON.stringify(fallback));
  const addressed = page?.target.search.list;
  const choices = useMemo<Search>(()=>typeof addressed === 'object' && addressed !== null && !Array.isArray(addressed) ? addressed as Search : {},[addressed]);
  const name = `${list}.${key}`;
  const saved = storedListState(`{"value":${kept}}`).value;
  const raw = resource ? page !== null && queryKey !== undefined && Object.hasOwn(page.target.search, queryKey) ? page.target.search[queryKey] : saved
    : Object.hasOwn(choices,name) ? choices[name] : (page?.target.search._list === true ? fallback : saved);
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
    if (page !== null && resource && queryKey !== undefined) {
      if (typeof value !== 'string' && typeof value !== 'boolean') return;
      void page.navigate({ search: (before) => ({ ...resourceListSearch(before), [queryKey]: value }), replace: true });
      return;
    }
    if (page !== null) void page.navigate({ search: (before) => ({ ...before, _list: true, list: {
      ...listOnScreen(page.id, before.list), [name]: value,
    } }), replace: true });
  }, [keep, name, page, user, scope, resource, queryKey]);
  // The page is told that this control is on screen, and a choice or a navigation puts into the address only the controls that are.
  const pageId = page?.id;
  useLayoutEffect(() => (shown && !resource && pageId !== undefined ? onScreen(pageId, name) : undefined), [shown, resource, pageId, name]);
  const serialized=JSON.stringify(value);
  const published=useRef<string|null>(null);
  useEffect(()=>{
    if(shown && !resource && page!==null && choices[name]===undefined && published.current!==serialized) {
      published.current=serialized;choose(JSON.parse(serialized) as T);
    } else if(choices[name]!==undefined) published.current=null;
  },[page,choices,name,choose,serialized,resource,shown]);
  return [value, choose];
}

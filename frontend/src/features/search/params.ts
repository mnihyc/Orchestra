/** What the address of the search says: the words, and what to keep to; read safely from whatever the address holds, and written back as short as it can be.
 *
 * The page's whole state is its address (`/search?q=…&scope=…&kinds=…&author=…&from=…&to=…`), so that a search
 * can be sent to somebody, kept, and found again with the back button. A value that is not what it should be
 * is left out, never an error: an address may be written by anybody.
 */
import { ITEM_KINDS, type ItemKind, SEARCH_KINDS, type SearchFilters, type SearchKind } from '@/api/search';
import type { SearchSort } from '@/generated/api-types';

/** What the address holds, as the router hands it over. */
export interface SearchParams {
  tab?: SearchTab;
  regex?: string;
  document_id?: string;
  sort?: SearchSort;
  q?: string;
  scope?: string;
  /** A comma list of kinds. */
  kinds?: string;
  author?: string;
  created_by?: string;
  edited_by?: string;
  owner?: string;
  date_field?: 'created' | 'edited';
  title_only?: string;
  shared_with_me?: string;
  file_type?: string;
  from?: string;
  to?: string;
  chat_id?: string;
  with_file?: string;
  mentions_me?: string;
  participating?: string;
}

export const SEARCH_TABS = ['all', 'conversations', 'chats', 'documents', 'files'] as const;
export type SearchTab = (typeof SEARCH_TABS)[number];

const IDENTIFIER = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const MOMENT = /^\d{4}-\d{2}-\d{2}(?:T[\d:.]+(?:Z|[+-]\d{2}:\d{2})?)?$/;
const MEDIA_TYPE = /^(?:\*\/\*|[^/\s*;]+\/(?:[^/\s*;]+|\*))$/;

const text = (value: unknown): string | undefined => (typeof value === 'string' && value.trim() !== '' ? value : undefined);
const identifier = (value: unknown): string | undefined => (typeof value === 'string' && IDENTIFIER.test(value) ? value : undefined);
const moment = (value: unknown): string | undefined => (typeof value === 'string' && MOMENT.test(value) ? value : undefined);

/** The kinds a list of words names; the words that name none are left out. */
export function kindsOf(list: string | undefined): SearchKind[] {
  if (list === undefined) return [];
  const wanted = new Set(list.split(','));
  return SEARCH_KINDS.filter((kind) => wanted.has(kind));
}

/** Reads the address. */
export function readParams(search: Record<string, unknown>): SearchParams {
  const q = text(search.q);
  const scope = identifier(search.scope);
  const rawKinds = typeof search.kinds === 'string' ? search.kinds : undefined;
  const kinds = kindsOf(rawKinds).join(',');
  const author = identifier(search.author);
  const createdBy = identifier(search.created_by);
  const editedBy = identifier(search.edited_by);
  const owner = identifier(search.owner);
  const fileType = text(search.file_type);
  const from = moment(search.from);
  const to = moment(search.to);
  const chat = identifier(search.chat_id);
  const document = identifier(search.document_id);
  return {
    ...(typeof search.tab === 'string' && SEARCH_TABS.includes(search.tab as SearchTab) ? { tab: search.tab as SearchTab } : {}),
    ...(search.sort === 'relevance' || search.sort === 'newest' ? { sort: search.sort } : {}),
    ...(q === undefined ? {} : { q }),
    ...(scope === undefined ? {} : { scope }),
    ...(rawKinds?.trim() === '' ? { kinds: '' } : kinds === '' ? {} : { kinds }),
    ...(author === undefined ? {} : { author }),
    ...(createdBy === undefined ? {} : { created_by: createdBy }),
    ...(editedBy === undefined ? {} : { edited_by: editedBy }),
    ...(owner === undefined ? {} : { owner }),
    ...(search.date_field === 'created' || search.date_field === 'edited' ? { date_field: search.date_field } : {}),
    ...(search.title_only === true || search.title_only === 'true' ? { title_only: 'true' } : {}),
    ...(search.shared_with_me === true || search.shared_with_me === 'true' ? { shared_with_me: 'true' } : {}),
    ...(fileType !== undefined && MEDIA_TYPE.test(fileType) ? { file_type: fileType } : {}),
    ...(from === undefined ? {} : { from }),
    ...(to === undefined ? {} : { to }),
    ...(chat === undefined ? {} : { chat_id: chat }),
    ...(document === undefined ? {} : { document_id: document }),
    ...(search.regex === true || search.regex === 'true' ? {regex:'true'} : {}),
    ...(search.with_file === true || search.with_file === 'true' ? { with_file: 'true' } : {}),
    ...(search.mentions_me === true || search.mentions_me === 'true' ? { mentions_me: 'true' } : {}),
    ...(search.participating === true || search.participating === 'true' ? { participating: 'true' } : {}),
  };
}

/** The filters of a search for the request. */
export function filtersOf(params: SearchParams): SearchFilters {
  let kinds = kindsOf(params.kinds);
  const noConversations = params.tab === 'conversations' && params.kinds !== undefined
    && !kinds.some((kind) => ITEM_KINDS.includes(kind as ItemKind));
  if (params.tab === 'conversations') {
    const items = kinds.filter((kind): kind is ItemKind => ITEM_KINDS.includes(kind as ItemKind));
    kinds = params.kinds === undefined ? [...ITEM_KINDS] : items;
  } else if (params.tab === 'chats') kinds = ['chat_message'];
  else if (params.tab === 'documents') kinds = ['document'];
  return {
    q: params.q ?? '',
    ...(params.regex === 'true' ? {regex:true} : {}),
    ...(params.document_id === undefined ? {} : {document_id:params.document_id}),
    ...(params.sort === undefined ? {} : { sort: params.sort }),
    scope: params.scope,
    kinds: noConversations || params.kinds === '' ? [] : kinds.length === 0 ? undefined : kinds,
    author: params.author,
    ...(params.created_by === undefined ? {} : { created_by: params.created_by }),
    ...(params.edited_by === undefined ? {} : { edited_by: params.edited_by }),
    ...(params.owner === undefined ? {} : { owner: params.owner }),
    ...(params.date_field === undefined ? {} : { date_field: params.date_field }),
    ...(params.title_only === 'true' ? { title_only: true } : {}),
    ...(params.shared_with_me === 'true' ? { shared_with_me: true } : {}),
    ...(params.file_type === undefined ? {} : { file_type: params.file_type }),
    from: params.from,
    to: params.to,
    ...(params.chat_id === undefined ? {} : { chat_id: params.chat_id }),
    ...(params.with_file === 'true' || params.tab === 'files' ? { with_file: true } : {}),
    ...(params.mentions_me === 'true' ? { mentions_me: true } : {}),
    ...(params.participating === 'true' ? { participating: true } : {}),
  };
}

/** An older link with a single kind still opens the matching category. */
export function tabOf(params: SearchParams): SearchTab {
  if (params.tab !== undefined) return params.tab;
  const kinds = kindsOf(params.kinds);
  if (kinds.length === 1 && kinds[0] === 'chat_message') return 'chats';
  if (kinds.length === 1 && kinds[0] === 'document') return 'documents';
  if (kinds.length > 0 && kinds.every((kind) => ITEM_KINDS.includes(kind as ItemKind))) return 'conversations';
  return 'all';
}

/** Preserve a deliberate item-kind selection while moving between categories. */
export function withTab(params: SearchParams, tab: SearchTab): SearchParams {
  const existing = kindsOf(params.kinds);
  const items = existing.filter((kind): kind is ItemKind => ITEM_KINDS.includes(kind as ItemKind));
  const deliberate = items.length < ITEM_KINDS.length && (items.length > 0
    || existing.includes('chat_message') && existing.includes('document')
    || params.kinds === '' && tabOf(params) === 'conversations');
  const kinds = deliberate ? [...items, 'chat_message', 'document'].join(',') : undefined;
  return readParams({ ...params, tab, kinds });
}

/** Whether a kind of item is among the kinds searched; no kinds at all means every kind. */
export function includes(params: SearchParams, kind: SearchKind): boolean {
  if (params.kinds === '') return false;
  const kinds = kindsOf(params.kinds);
  return kinds.length === 0 || kinds.includes(kind);
}

/** The kinds after one is switched on or off. All kinds need no list; none use an explicit empty list. */
export function withKind(params: SearchParams, kind: SearchKind, on: boolean): string | undefined {
  const now = new Set<SearchKind>(params.kinds === undefined ? SEARCH_KINDS : kindsOf(params.kinds));
  if (on) now.add(kind);
  else now.delete(kind);
  if (now.size === SEARCH_KINDS.length) return undefined;
  return SEARCH_KINDS.filter((candidate) => now.has(candidate)).join(',');
}

/** How many of the filters narrow the search. */
export function narrowing(params: SearchParams): number {
  const place = params.scope !== undefined || params.chat_id !== undefined || params.document_id !== undefined ? 1 : 0;
  const dates = params.from !== undefined || params.to !== undefined ? 1 : 0;
  return place + dates + [params.kinds, params.author, params.created_by, params.edited_by, params.owner, params.regex, params.with_file, params.mentions_me, params.participating, params.title_only, params.shared_with_me, params.file_type].filter((value) => value !== undefined).length;
}

/** Keeps the person's query and filters while choosing this one current chat and message kind. */
export function inChat(params: SearchParams, chatId: string, nodeId?: string): SearchParams {
  return { ...params, tab: 'chats', chat_id: chatId, document_id: undefined, kinds: 'chat_message', ...(nodeId === undefined ? {} : { scope: nodeId }) };
}

/** A delayed query write changes its own fields while retaining newer filter actions in the address. */
export function mergeParams(before: Record<string, unknown>, shown: SearchParams, next: SearchParams): SearchParams {
  const held = readParams(before);
  const fields = ['q', 'tab', 'scope', 'kinds', 'author', 'created_by', 'edited_by', 'owner', 'date_field', 'title_only', 'shared_with_me', 'file_type', 'from', 'to', 'chat_id', 'document_id', 'regex', 'with_file', 'mentions_me', 'participating', 'sort'] as const;
  return readParams(Object.fromEntries(fields.map((field) => [field, shown[field] === next[field] ? held[field] : next[field]] as const)));
}

/** Stores the chosen view without the words a person typed. */
export function savedView(params: SearchParams): string {
  const view = readParams({ ...params, q: undefined });
  return JSON.stringify(view);
}

/** A plain address resumes the validated device view; explicit filters and sort keep their address. */
export function restoredView(params: SearchParams, saved: string): SearchParams | undefined {
  if (savedView(params) !== '{}') return undefined;
  try {
    const parsed: unknown = JSON.parse(saved);
    if (parsed === null || Array.isArray(parsed) || typeof parsed !== 'object') return undefined;
    const view = readParams({ ...parsed, q: undefined });
    if (Object.keys(view).length === 0) return undefined;
    return { ...view, ...(params.q === undefined ? {} : { q: params.q }) };
  } catch { return undefined; }
}

export type { ItemKind };

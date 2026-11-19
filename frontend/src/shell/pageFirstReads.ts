/** Addressed reads own a retained model until the mounted page takes it over. */
import { InfiniteQueryObserver, QueryObserver, type QueryKey, type QueryObserverOptions } from '@tanstack/react-query';
import { client } from '@/api/client';
import { queryClient } from '@/api/query';
import { READ_NOW } from '@/api/freshness';
import { cancelRead, hasReadConsumer, readDemand, retainRead } from '@/api/reads';
import { scheduledReads } from '@/api/readScheduler';
import { nodeQuery, projectQuery, topicQuery, orgBySlugQuery } from '@/api/nodes';
import { pageQuery } from '@/api/access';
import { nodeChatOpeningQuery } from '@/api/chats';
import { sharedTreeQuery, threadSharedQuery } from '@/api/share';
import { outlineQuery } from '@/api/threads';
import { resourceSummaryKey, resourceSummaryQuery } from '@/api/summaries';
import { contentThreadRead, type ThreadReadRef } from '@/realtime/threadScope';
import { threadViewOf, useThreadStore } from '@/realtime/stores/thread';
import { orgOverviewQuery } from '@/api/orgOverview';
import type { UsageParams } from '@/api/usage';
import type { Node } from '@/generated/api-types';
import { isOrgIdAddress } from '@/features/navigation/orgAddress';
import { topicSchedulesQuery } from '@/api/inputSchedules';
import { environmentQuery } from '@/api/environments';
import { agentQuery, hostQuery, machineQuery, providerQuery, templateQuery } from '@/api/resources';
import { inventoryQuery } from '@/features/resources/inventoryRead';
import { INVENTORY_KINDS } from '@/features/resources/inventory';
import { AGENT_STATES, ENVIRONMENT_KINDS, ENVIRONMENT_STATUSES, HOST_STATES, PROVIDER_STATUSES, PROVIDER_VIEWS, TEMPLATE_KINDS, oneOf, readText } from '@/features/resources/search';
import type { ResourceKind } from '@/features/resources/trail';
import { retainDocument, warmDocument } from '@/realtime/live';
import { BOARD_PERIODS, DEFAULT_PERIOD, OTHER_WAYS, PEOPLE_ASKED, boardSpan } from '@/features/usage/boardParameters';
import { isListRoute, listSearch, storedListState } from '@/lib/listState';
import { readViewChoice } from '@/lib/viewChoices';
import { pageParam, type PageTarget } from '@/features/workbench/pageTargets';
import type { PageReadLease } from '@/features/workbench/pageReads';
import { allTabs, findTab } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

function addressChoices(target: PageTarget): Record<string, unknown> {
  const explicit = Object.keys(listSearch(target.search, target.route)).length > 0 || ['q', 'query', 'text'].some((key) => Object.hasOwn(target.search, key));
  if (!isListRoute(target.route) || explicit || target.search._list === true) return target.search;
  const kept = storedListState(readViewChoice(client.session.getState().userId, `page.${target.pathname}`, 'state', '{}'));
  return { ...target.search, ...kept };
}

function boardChoice(target: PageTarget, search: Record<string, unknown>, key: string, fallback: string): unknown {
  const list = record(search.list) ? search.list : {};
  const name = `usage-board.${key}`;
  if (Object.hasOwn(list, name)) return list[name];
  if (search._list === true) return fallback;
  return storedListState(`{"value":${readViewChoice(client.session.getState().userId, `page.${target.pathname}.usage-board`, key, JSON.stringify(fallback))}}`).value;
}

export function startAddressedPageReads(target: PageTarget, tab: string, initiallyShown = true): PageReadLease {
  const owner = ['page-read', tab] as const;
  const readers: { release: () => void; shown: (shown: boolean) => void }[] = [];
  const keys: QueryKey[] = [];
  let released = false, pageShown = initiallyShown, handed = false;
  const reader = client.session.getState();
  function start<Data, Key extends QueryKey>(options: QueryObserverOptions<Data, Error, Data, Data, Key>, after?: (data: Data) => void) {
    const native = queryClient.getQueryCache().build(queryClient, queryClient.defaultQueryOptions(options));
    const wanted = readDemand(native, pageShown);
    const observer = new QueryObserver(queryClient, { ...options, enabled: false, refetchInterval: false });
    retainRead(observer);
    const stop = observer.subscribe(() => undefined);
    let shown = pageShown;
    let attempted = false;
    const load = () => {
      attempted = true;
      const defaults = queryClient.defaultQueryOptions({ ...options, ...READ_NOW });
      void native.fetch(defaults).then((data) => {
        const current = client.session.getState();
        if (!released && shown && current.readerVersion === reader.readerVersion && current.userId === reader.userId) after?.(data);
      }).catch(() => undefined);
      if (!keys.includes(options.queryKey)) keys.push(options.queryKey);
      if (!handed) scheduledReads.supply(owner, keys);
    };
    const cancelUnused = () => {
      const held = queryClient.getQueryCache().get(native.queryHash);
      if (held !== undefined && !hasReadConsumer(held)) void cancelRead(held);
    };
    if (shown) load();
    readers.push({ shown: (next) => {
      if (shown === next) return;
      shown = next; wanted.shown(next);
      if (!next) cancelUnused(); else if (!attempted || native.state.data === undefined && native.state.fetchStatus === 'idle') load();
    }, release: () => { wanted.release(); cancelUnused(); stop(); } });
  }
  function startInventory(options: ReturnType<typeof inventoryQuery>) {
    const observer = new InfiniteQueryObserver(queryClient, { ...options, enabled: false, refetchInterval: false });
    retainRead(observer);
    const stop = observer.subscribe(() => undefined);
    const native = queryClient.getQueryCache().find({ queryKey: options.queryKey, exact: true });
    if (native === undefined) { stop(); return; }
    const wanted = readDemand(native, pageShown);
    const load = () => {
      void observer.refetch({ cancelRefetch: false }).catch(() => undefined);
      if (!keys.includes(options.queryKey)) keys.push(options.queryKey);
      if (!handed) scheduledReads.supply(owner, keys);
    };
    const cancelUnused = () => { if (!hasReadConsumer(native)) void cancelRead(native); };
    if (pageShown) load();
    readers.push({ shown: (shown) => { wanted.shown(shown); if (!shown) cancelUnused(); else if (native.state.data === undefined && native.state.fetchStatus === 'idle') load(); },
      release: () => { wanted.release(); cancelUnused(); stop(); } });
  }
  const search = addressChoices(target);
  const now = Date.now();
  const displayed = (name: string, id: string) => {
    const state = useWorkbench.getState(), layout = state.layout;
    if (findTab(layout, tab) === null) return name === 'chat';
    return allTabs(layout).some((candidate) => {
      if (candidate.view !== name) return false;
      const matches = candidate.of?.node !== undefined ? candidate.of.node === id
        : candidate.scope?.mode === 'fixed' ? candidate.scope.target.id === id : state.activeContent === tab;
      const place = matches ? findTab(layout, candidate.id) : null;
      if (place === null) return false;
      if (place.area === 'main') return place.group.active === candidate.id && (layout.maximized === null || layout.maximized === place.group.id);
      if (place.area === 'bottom') return layout.bottom.open && place.group.active === candidate.id;
      return layout[place.area].open && place.container?.name === layout[place.area].active && place.section?.open === true && !place.section.hidden;
    });
  };
  const chat = (id: string) => {
    if (!displayed('chat', id)) return;
    const saved = readViewChoice<string>(reader.userId, 'chat', 'scope', 'thread');
    const scope = ['thread', 'topic', 'project', 'org'].includes(saved) ? saved : 'thread';
    const chosen = readText(search.chat);
    start(nodeChatOpeningQuery(id, scope, chosen === undefined || chosen === '' ? null : chosen));
  };
  function startOutline(ref: ThreadReadRef) {
    const options = outlineQuery(ref);
    const observer = new InfiniteQueryObserver(queryClient, { ...options, enabled: false, refetchInterval: false });
    retainRead(observer);
    const stop = observer.subscribe(() => undefined);
    const native = queryClient.getQueryCache().find({ queryKey: options.queryKey, exact: true });
    if (native === undefined) { stop(); return; }
    const wanted = readDemand(native, pageShown);
    const load = () => {
      void observer.refetch({ cancelRefetch: false }).catch(() => undefined);
      keys.push(options.queryKey); if (!handed) scheduledReads.supply(owner, keys);
    };
    const cancelUnused = () => { if (!hasReadConsumer(native)) void cancelRead(native); };
    if (pageShown) load();
    readers.push({ shown: (shown) => { wanted.shown(shown); if (!shown) cancelUnused(); else if (native.state.data === undefined && native.state.fetchStatus === 'idle') load(); },
      release: () => { wanted.release(); cancelUnused(); stop(); } });
  }
  function afterThreadOpening(id: string) {
    const ref = contentThreadRead(id, tab);
    let done = false;
    const admit = () => {
      const current = client.session.getState(), view = threadViewOf(ref);
      if (done || released || !pageShown || current.readerVersion !== reader.readerVersion || current.userId !== reader.userId
        || view.closed !== null || view.read?.ready !== true || view.read.reader !== reader.readerVersion) return;
      done = true; stop();
      // History needs the opening's admitted selector; it never borrows an active execution selector.
      if (displayed('outline', id)) startOutline(ref);
    };
    const stop = useThreadStore.subscribe(admit);
    readers.push({ shown: (shown) => { if (shown) admit(); }, release: stop });
    admit();
  }
  const overviewUsage = (id: string): UsageParams => {
    const period = BOARD_PERIODS.find((period) => period === boardChoice(target, search, 'period', DEFAULT_PERIOD)) ?? DEFAULT_PERIOD;
    const other = OTHER_WAYS.find((other) => other === boardChoice(target, search, 'other', 'model')) ?? 'model';
    return { scope: `node:${id}`, ...boardSpan(period, now), breakdowns: ['person', other], limit: PEOPLE_ASKED, filters: {} };
  };
  const resource = /^\/resources\/([^/$]+)\/$/.exec(target.route)?.[1];
  if (resource !== undefined && Object.hasOwn(INVENTORY_KINDS, resource)) {
    const kind = resource as ResourceKind;
    const choice = (key: string, fallback: unknown): unknown => Object.hasOwn(search, key) ? search[key]
      : storedListState(`{"value":${readViewChoice(client.session.getState().userId, `page.${target.pathname}.resources.${kind}`, key, JSON.stringify(fallback))}}`).value;
    const view = oneOf(PROVIDER_VIEWS, choice('view', 'usable')) ?? 'usable';
    // The all view needs authoritative org membership; its mounted owner starts it once that input exists.
    if (view !== 'all') {
      const sorts = ['name', 'owner', 'created', 'last_activity', ...(kind === 'mounts' || kind === 'machines' ? [] : ['use'] as const), ...(kind === 'hosts' ? ['load', 'running'] as const : [])] as const;
      const sort = oneOf(sorts, choice('sort', 'name')) ?? 'name';
      const state = kind === 'providers' ? oneOf(PROVIDER_STATUSES, search.status)
        : kind === 'environments' ? oneOf(ENVIRONMENT_STATUSES, search.status)
          : kind === 'agents' ? oneOf(AGENT_STATES, search.state) : kind === 'hosts' ? oneOf(HOST_STATES, search.state)
            : kind === 'machines' ? oneOf(['online', 'offline', 'revoked'] as const, search.status) : undefined;
      startInventory(inventoryQuery(client.session.getState().userId ?? '', kind, { scope: 'mine', q: kind === 'mcp' || kind === 'mounts' || kind === 'skills' ? '' : (readText(search.q) ?? '').trim(), sort, direction: choice('direction', 'asc') === 'desc' ? 'desc' : 'asc', participating: choice('participating', false) === true },
        { view, state, kind: kind === 'environments' ? oneOf(ENVIRONMENT_KINDS, search.kind) : kind === 'templates' ? oneOf(TEMPLATE_KINDS, search.kind) : undefined, harness: kind === 'agents' ? readText(search.harness) : undefined }));
    }
  } else if (target.route === '/doc/$documentId/') {
    const id = pageParam(target, 'documentId');
    const model = retainDocument(id);
    let reading: (() => void) | null = initiallyShown ? warmDocument(id) : null;
    readers.push({ shown: (shown) => {
      if (shown && reading === null) reading = warmDocument(id);
      else if (!shown) { reading?.(); reading = null; }
    }, release: () => { reading?.(); reading = null; model(); } });
  } else if (target.route === '/thread/$threadId/') {
    const id = pageParam(target, 'threadId');
    start(pageQuery({ kind: 'thread', id }));
    chat(id);
    if (displayed('knowledge', id)) { start(threadSharedQuery(id)); start(sharedTreeQuery(id)); }
    const state = queryClient.getQueryData<{ timeline?: { summary_hidden?: boolean } }>(['me-state']);
    const summary = { kind: 'thread' as const, id }, continuation = readText(search.continuation);
    if ((continuation === undefined || continuation === '') && state?.timeline?.summary_hidden !== true
      && queryClient.getQueryData(resourceSummaryKey(summary)) === undefined) start(resourceSummaryQuery(summary));
    afterThreadOpening(id);
  } else if (target.route === '/topic/$topicId/' && search.tab !== 'settings') {
    const id = pageParam(target, 'topicId');
    if (search.tab === undefined || search.tab === 'overview') {
      start(topicQuery(id, overviewUsage(id)));
      start(topicSchedulesQuery(id));
    } else start(topicQuery(id));
    chat(id);
  } else if (target.route === '/project/$projectId/') {
    const id = pageParam(target, 'projectId');
    start(projectQuery(id, search.view === 'activity' ? undefined : { scope: `node:${id}`, ...boardSpan('week', now), breakdowns: [], filters: {} }));
  } else if (target.route === '/org/$slug/' || target.route === '/org/$slug/settings/$section/') {
    const slug = pageParam(target, 'slug');
    const tabs = ['overview', 'members', 'activity'] as const;
    const section = oneOf(tabs, search.tab) ?? oneOf(tabs, readViewChoice<string>(reader.userId, 'org', 'tab', 'overview')) ?? 'overview';
    const shownOrg = (org: Node) => {
      if (target.route !== '/org/$slug/' || org.kind !== 'org') return;
      if (!['members', 'activity'].includes(section)) start(orgOverviewQuery(org.id, { participating: false, usage: overviewUsage(org.id) }));
      // Members/Defaults retain their existing joint metadata admission with the visible Chat.
      if (section !== 'members' && org.org_kind !== 'personal') chat(org.id);
    };
    if (isOrgIdAddress(slug)) start(nodeQuery(slug), shownOrg); else start(orgBySlugQuery(slug), shownOrg);
  }
  else if (target.route === '/resources/providers/$id/') start(providerQuery(pageParam(target, 'id')));
  else if (target.route === '/resources/agents/$id/') start(agentQuery(pageParam(target, 'id')));
  else if (target.route === '/resources/environments/$id/') start(environmentQuery(pageParam(target, 'id')));
  else if (target.route === '/resources/machines/$id/') start(machineQuery(pageParam(target, 'id')));
  else if (target.route === '/resources/hosts/$id/') start(hostQuery(pageParam(target, 'id')));
  else if (target.route === '/resources/templates/$id/') start(templateQuery(pageParam(target, 'id')));
  return Object.assign(() => {
    if (released) return;
    released = true;
    for (const reader of readers) reader.release();
    scheduledReads.release(owner);
  }, { took: () => { handed = true; scheduledReads.release(owner); return !released; }, shown: (shown: boolean) => { pageShown = shown; if (!released) for (const reader of readers) reader.shown(shown); } });
}

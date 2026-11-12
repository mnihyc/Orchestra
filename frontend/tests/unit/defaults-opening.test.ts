/** A visible Defaults opening supplies only its independently admitted, current-reader projections. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError, client, RENEW_BEFORE_END_MS } from '@/api/client';
import { nodeChatOpeningQuery } from '@/api/chats';
import { nodeKey, nodeQuery } from '@/api/nodes';
import { capturePeopleHint, repairPeople } from '@/api/peopleChanges';
import { QueryObserver } from '@tanstack/react-query';
import { LiveQueryClient, queryClient } from '@/api/query';
import { providerKey } from '@/api/resources';
import { registryQuery, secretsKey, settingsKey } from '@/api/settings';
import type { NodeChatOpening, NodeDefaultsOpening, NodeChatOpeningGetQuery, Problem, Provider, ResolvedSetting, SettingDef } from '@/generated/api-types';
import { useChatStore, chatWriter } from '@/realtime/stores/chat';
import { ACME } from '../doubles/fixtures';
import { nodeChatOpening } from '../doubles/screen-openings';
import { World } from '../doubles/world';
import { deferred } from './continuation-fixtures';

const demand = { defaults: true, secret_names: true, default_provider_name: true, embeddings_provider_name: false } satisfies NodeChatOpeningGetQuery;
const saved = client.session.getState();
const world = new World(), ada = world.people.get('ada')!, grace = world.people.get('grace')!;
const node = world.places.node(ada, ACME.fixLogin);
const target = { kind: 'node', id: node.id } as const;
const providerId = '00000000-0000-4000-8022-000000000001';
let cache: LiveQueryClient, calls: URL[], native: NodeChatOpening, metadata: NodeDefaultsOpening;
const row: ResolvedSetting = { key: 'agent.default_provider_id', value: providerId, own: null,
  source: { scope: 'system', node: null }, locked_by: null, writable: true, parts: [] };

function adopt(person = ada) { client.adopt({ access_token: 'synthetic-defaults-reader', access_expires_at: new Date(Date.now() + RENEW_BEFORE_END_MS + 60_000).toISOString(), user: person.me }); }
function opening(project: NodeDefaultsOpening | null = metadata): NodeChatOpening { return { ...native, defaults: project }; }
function transport(reply: () => Response | Promise<Response>, fallback?: (url: URL) => Response | Promise<Response>) {
  vi.stubGlobal('fetch', (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init), url = new URL(request.url);
    calls.push(url);
    if (url.pathname === `/api/v1/nodes/${node.id}/chat-opening`) return reply();
    if (fallback !== undefined) return fallback(url);
    throw new Error(`Unexpected standalone read: ${request.method} ${url.pathname}`);
  });
}
const scopedProvider = () => [...providerKey(providerId), 'default-choice', `${client.session.getState().userId ?? ''}:${client.session.getState().readerVersion ?? 0}`];
const read = (wanted: NodeChatOpeningGetQuery = demand) => cache.query(nodeChatOpeningQuery(node.id, 'topic', null, wanted));
const problem: Problem = { type: 'orch:error:authz.denied', code: 'authz.denied', status: 403, title: 'Not allowed', detail: 'This native member is not readable.', request_id: 'defaults-member' };

beforeEach(async () => {
  queryClient.clear(); chatWriter.clear(); calls = []; adopt();
  cache = new LiveQueryClient({ defaultOptions: { ...queryClient.getDefaultOptions(), queries: { ...queryClient.getDefaultOptions().queries, retry: false } } });
  native = await nodeChatOpening(world, ada, node.id, new URLSearchParams({ scope: 'topic' }));
  const registry = (await world.requestFeeds.read(ada, '/settings/registry')).body as SettingDef[];
  const provider = (await world.requestFeeds.read(ada, `/providers/${providerId}`)).body as Provider;
  metadata = { node, registry, registry_problem: null, registry_retry_after: null, settings: [row], settings_problem: null, settings_retry_after: null,
    secrets: [], secrets_problem: null, secrets_retry_after: null,
    providers: [{ setting: row.key, id: providerId, provider, problem: null, retry_after: null }] };
});
afterEach(() => { cache.clear(); queryClient.clear(); chatWriter.clear(); vi.unstubAllGlobals(); client.session.setState(saved); });

it('opts into only the explicitly demanded existing native members on the one chat opening', async () => {
  transport(() => Response.json(opening())); await read();
  expect(calls).toHaveLength(1);
  expect(Object.fromEntries(calls[0]!.searchParams)).toEqual({ scope: 'topic', limit: '100', defaults: 'true', secret_names: 'true', default_provider_name: 'true', embeddings_provider_name: 'false' });
});

it('leaves ordinary chat reads unchanged and ignores unsolicited Defaults metadata', async () => {
  transport(() => Response.json(opening())); await cache.query(nodeChatOpeningQuery(node.id, 'topic', null));
  expect(Object.fromEntries(calls[0]!.searchParams)).toEqual({ scope: 'topic', limit: '100' });
  expect(cache.getQueryData(nodeKey(node.id))).toBeUndefined();
  expect(cache.getQueryData(registryQuery.queryKey)).toBeUndefined();
});

it('seeds the exact native Node, registry, resolved settings and secret names', async () => {
  transport(() => Response.json(opening())); await read();
  expect(cache.getQueryData(nodeKey(node.id))).toEqual(node);
  expect(cache.getQueryData(registryQuery.queryKey)).toEqual(metadata.registry);
  expect(cache.getQueryData(settingsKey(node.id))).toEqual(metadata.settings);
  expect(cache.getQueryData(secretsKey(target))).toEqual([]);
  expect(calls).toHaveLength(1);
});

it('publishes metadata before the selected chat or composer can be published', async () => {
  const observed: unknown[] = [];
  const leave = useChatStore.subscribe((state) => { if (native.opening !== null && state.byChat.has(native.opening.sync.chat.id)) observed.push(cache.getQueryData(settingsKey(node.id))); });
  try { transport(() => Response.json(opening())); await read(); expect(observed.length).toBeGreaterThan(0); expect(observed.every((value) => JSON.stringify(value) === JSON.stringify([row]))).toBe(true); }
  finally { leave(); }
});

it('seeds only the actual editable selected provider reader, without an option list or unused summary', async () => {
  transport(() => Response.json(opening())); await read();
  expect(cache.getQueryData(providerKey(providerId))).toBeUndefined();
  expect(cache.getQueryData(scopedProvider())).toEqual(metadata.providers[0]!.provider);
  expect(cache.getQueryData(['everything', 'provider-default-choices'])).toBeUndefined();
  expect(calls).toHaveLength(1);
});

it('seeds the canonical effective provider reader for a read-only row without priming an unmounted choice', async () => {
  transport(() => Response.json(opening({ ...metadata, settings: [{ ...row, writable: false }] }))); await read();
  expect(cache.getQueryData(providerKey(providerId))).toEqual(metadata.providers[0]!.provider);
  expect(cache.getQueryData(scopedProvider())).toBeUndefined();
});

it('does not seed secret names or provider names that this editor did not demand', async () => {
  transport(() => Response.json(opening())); await read({ defaults: true, secret_names: false, default_provider_name: false, embeddings_provider_name: false });
  expect(cache.getQueryData(secretsKey(target))).toBeUndefined();
  expect(cache.getQueryData(providerKey(providerId))).toBeUndefined();
  expect(cache.getQueryData(scopedProvider())).toBeUndefined();
});

it('rejects every metadata member when its native Node belongs to a different target', async () => {
  transport(() => Response.json(opening({ ...metadata, node: { ...node, id: ACME.web } }))); await read();
  expect(cache.getQueryData(nodeKey(ACME.web))).toBeUndefined(); expect(cache.getQueryData(registryQuery.queryKey)).toBeUndefined();
  expect(cache.getQueryData(settingsKey(node.id))).toBeUndefined(); expect(cache.getQueryData(providerKey(providerId))).toBeUndefined();
});

it('does not seed an unrelated provider or a provider whose returned native identity disagrees', async () => {
  const held = metadata.providers[0]!;
  transport(() => Response.json(opening({ ...metadata, providers: [{ ...held, id: ACME.web }, { ...held, provider: { ...held.provider!, id: ACME.fixLogin } }] }))); await read();
  expect(cache.getQueryData(providerKey(providerId))).toBeUndefined(); expect(cache.getQueryData(providerKey(ACME.web))).toBeUndefined();
});

it('retires stale private member data and preserves its native refusal and Retry-After independently from Chat', async () => {
  cache.setQueryData(settingsKey(node.id), [row]); cache.setQueryData(['settings', 'unrelated'], [row]);
  transport(() => Response.json(opening({ ...metadata, settings: null, settings_problem: problem, settings_retry_after: 3, providers: [] }))); await read();
  const state = cache.getQueryState(settingsKey(node.id));
  expect(state?.data).toBeUndefined(); expect(state?.error).toBeInstanceOf(ApiError);
  const error = state?.error; if (!(error instanceof ApiError)) throw new Error('The existing native error must be retained');
  expect(error.problem).toEqual(problem); expect(error.retryAt?.getTime()).toBeGreaterThanOrEqual(Date.now() + 2_000);
  expect(cache.getQueryData(['settings', 'unrelated'])).toEqual([row]);
  expect(useChatStore.getState().byChat.has(native.opening!.sync.chat.id)).toBe(true);
});

it('retires the exact provider name on an independent native denial, without granting Use', async () => {
  cache.setQueryData(providerKey(providerId), metadata.providers[0]!.provider); cache.setQueryData(scopedProvider(), metadata.providers[0]!.provider);
  transport(() => Response.json(opening({ ...metadata, providers: [{ setting: row.key, id: providerId, provider: null, problem, retry_after: null }] }))); await read();
  expect(cache.getQueryData(providerKey(providerId))).toBeUndefined();
  expect(cache.getQueryData(scopedProvider())).toBeUndefined(); expect(cache.getQueryState(scopedProvider())?.error).toBeInstanceOf(ApiError);
});

it.each(['omitted', 'null'] as const)('keeps canonical readers available for legacy %s metadata', async (shape) => {
  transport(() => Response.json(shape === 'omitted' ? native : opening(null))); await read();
  expect(cache.getQueryData(nodeKey(node.id))).toBeUndefined(); expect(cache.getQueryData(settingsKey(node.id))).toBeUndefined();
});

it('does not replace a newer native Node revision with an older opening row', async () => {
  const newer = { ...node, revision: node.revision + 1, name: 'Newer native name' }; cache.setQueryData(nodeKey(node.id), newer);
  const current = { ...row, value: 'a-current-provider' }; cache.setQueryData(settingsKey(node.id), [current]);
  transport(() => Response.json(opening())); await read(); expect(cache.getQueryData(nodeKey(node.id))).toEqual(newer);
  expect(cache.getQueryData(settingsKey(node.id))).toEqual([current]); expect(cache.getQueryData(registryQuery.queryKey)).toBeUndefined();
  expect(cache.getQueryData(scopedProvider())).toBeUndefined();
});

it('falls back without publishing any metadata when the envelope has no usable Node revision', async () => {
  transport(() => Response.json({ ...native, defaults: { ...metadata, node: { ...node, revision: null } } })); await read();
  expect(cache.getQueryData(nodeKey(node.id))).toBeUndefined(); expect(cache.getQueryData(registryQuery.queryKey)).toBeUndefined();
  expect(cache.getQueryData(settingsKey(node.id))).toBeUndefined();
});

it('does not invent a selected provider when the actual effective setting is explicitly null', async () => {
  transport(() => Response.json(opening({ ...metadata, settings: [{ ...row, value: null }] }))); await read();
  expect(cache.getQueryData(providerKey(providerId))).toBeUndefined(); expect(cache.getQueryData(scopedProvider())).toBeUndefined();
});

it('does not install a late previous-reader body over a current reader’s metadata', async () => {
  const started = deferred<undefined>(), released = deferred<Response>();
  transport(() => { started.resolve(undefined); return released.promise; });
  const pending = read(); const ended = pending.catch(() => undefined); await started.promise;
  adopt(grace); const current = { ...row, value: null }; cache.setQueryData(settingsKey(node.id), [current]);
  released.resolve(Response.json(opening())); await ended;
  expect(cache.getQueryData(settingsKey(node.id))).toEqual([current]); expect(cache.getQueryData(registryQuery.queryKey)).toBeUndefined();
});

it('does not publish a cancelled opening even if its transport returns a body afterwards', async () => {
  const started = deferred<undefined>(), released = deferred<Response>();
  transport(() => { started.resolve(undefined); return released.promise; });
  const pending = read(); const ended = pending.catch(() => undefined); await started.promise;
  await cache.cancelQueries({ queryKey: ['node-chat-opening', node.id], exact: true }); released.resolve(Response.json(opening())); await ended; await Promise.resolve();
  expect(cache.getQueryData(nodeKey(node.id))).toBeUndefined(); expect(cache.getQueryData(settingsKey(node.id))).toBeUndefined();
});

it('keeps an opening invalidation ordinary after its first successful projection', async () => {
  transport(() => Response.json(opening())); await read();
  await cache.invalidateQueries({ queryKey: ['node-chat-opening', node.id], exact: true }); await read();
  expect(calls).toHaveLength(2); expect(calls[0]!.searchParams.get('defaults')).toBe('true');
  for (const flag of ['defaults', 'secret_names', 'default_provider_name', 'embeddings_provider_name']) expect(calls[1]!.searchParams.get(flag)).toBeNull();
});

it('keeps a first opening refusal retry ordinary so canonical readers own recovery', async () => {
  let attempt = 0; transport(() => ++attempt === 1 ? Response.json(problem, { status: 403 }) : Response.json(opening()));
  await expect(read()).rejects.toBeInstanceOf(ApiError); await read();
  expect(calls).toHaveLength(2); expect(calls[1]!.searchParams.get('defaults')).toBeNull();
  expect(cache.getQueryData(settingsKey(node.id))).toBeUndefined(); expect(cache.getQueryData(providerKey(providerId))).toBeUndefined();
});

it('joins the already-running current Defaults registry instead of starting a duplicate registry GET', async () => {
  const started = deferred<undefined>(), released = deferred<Response>();
  transport(() => { started.resolve(undefined); return released.promise; }, () => Response.json(metadata.registry));
  const pending = read(); await started.promise;
  const registry = cache.query(registryQuery); const settled = registry.then((data) => ({ data }), (error: unknown) => ({ error }));
  released.resolve(Response.json(opening())); await pending;
  expect(await settled).toEqual({ data: metadata.registry });
  expect(calls.filter((url) => url.pathname === '/api/v1/settings/registry')).toEqual([]);
});

it('keeps the joined native registry refusal and Retry-After without an automatic duplicate GET', async () => {
  const started = deferred<undefined>(), released = deferred<Response>();
  transport(() => { started.resolve(undefined); return released.promise; }, () => Response.json(problem, { status: 403, headers: { 'Retry-After': '3' } }));
  const pending = read(); await started.promise;
  const registry = cache.query(registryQuery); const settled = registry.catch((error: unknown) => error);
  released.resolve(Response.json(opening({ ...metadata, registry: null, registry_problem: problem, registry_retry_after: 3 }))); await pending;
  const error = await settled; expect(error).toBeInstanceOf(ApiError);
  if (!(error instanceof ApiError)) throw new Error('Keep the independently admitted native refusal');
  expect(error.problem).toEqual(problem); expect(error.retryAt?.getTime()).toBeGreaterThan(Date.now() + 2_000);
  expect(calls.filter((url) => url.pathname === '/api/v1/settings/registry')).toEqual([]);
});

it('keeps the canonical registry fallback after a legacy joined opening has no projection', async () => {
  const started = deferred<undefined>(), released = deferred<Response>();
  transport(() => { started.resolve(undefined); return released.promise; }, () => Response.json(metadata.registry));
  const pending = read(); await started.promise; const registry = cache.query(registryQuery);
  released.resolve(Response.json(native)); await pending;
  expect(await registry).toEqual(metadata.registry); expect(calls.filter((url) => url.pathname === '/api/v1/settings/registry')).toHaveLength(1);
});

it('cancels only the registry consumer while its shared Defaults opening completes normally', async () => {
  const started = deferred<undefined>(), released = deferred<Response>(), registryReply = deferred<Response>();
  transport(() => { started.resolve(undefined); return released.promise; }, () => registryReply.promise);
  const pending = read(); await started.promise; const registry = cache.query(registryQuery); const settled = registry.catch(() => undefined);
  await cache.cancelQueries({ queryKey: registryQuery.queryKey, exact: true }); await settled;
  expect(cache.getQueryState(['node-chat-opening', node.id])?.fetchStatus).toBe('fetching');
  released.resolve(Response.json(opening())); registryReply.resolve(Response.json(metadata.registry)); await pending;
  expect(cache.getQueryData(settingsKey(node.id))).toEqual(metadata.settings);
  expect(calls.filter((url) => url.pathname === '/api/v1/settings/registry')).toEqual([]);
});

it('joins a fresh authority-generation Node from the existing aggregate without reusing old permissions', async () => {
  const fresh = { ...node, access: { ...node.access, permissions: ['topic.view', 'chat.view'] } };
  const started = deferred<undefined>(), released = deferred<Response>(); let attempt = 0;
  transport(() => ++attempt === 1 ? Response.json(opening()) : (started.resolve(undefined), released.promise), () => Response.json(fresh));
  // A bootstrap or another native consumer can have created Node before Chat's aggregate.
  cache.setQueryData(nodeKey(node.id), node);
  await read();
  const keys = cache.getQueryCache().getAll().map((query) => query.queryKey[0]);
  expect(keys.indexOf('node')).toBeLessThan(keys.indexOf('node-chat-opening'));
  const openingObserver = new QueryObserver(cache, nodeChatOpeningQuery(node.id, 'topic', null, demand, () => true));
  const nodeObserver = new QueryObserver(cache, nodeQuery(node.id));
  const leaveOpening = openingObserver.subscribe(() => undefined), leaveNode = nodeObserver.subscribe(() => undefined);
  const repair = repairPeople(cache, [], capturePeopleHint(cache)); const ended = repair.catch((error: unknown) => error);
  try {
    await started.promise;
    const request = calls.filter((url) => url.pathname.endsWith('/chat-opening'))[1]!;
    expect(request.searchParams.get('defaults')).toBe('true');
    for (const flag of ['secret_names', 'default_provider_name', 'embeddings_provider_name']) expect(request.searchParams.get(flag)).toBe('false');
    released.resolve(Response.json(opening({ ...metadata, node: fresh, providers: [], secrets: null, settings: [{ ...row, writable: false }] })));
    await ended;
    expect(calls.filter((url) => url.pathname === `/api/v1/nodes/${node.id}`)).toEqual([]);
    expect(cache.getQueryData(nodeKey(node.id))).toEqual(fresh);
    expect(cache.getQueryData(settingsKey(node.id))).toEqual([{ ...row, writable: false }]);
  } finally { released.resolve(Response.json(opening())); await ended; leaveOpening(); leaveNode(); }
});

it('keeps failed fresh-repair retries ordinary at the same hint cut and admits only a genuinely later cut', async () => {
  let attempt = 0; transport(() => ++attempt === 2 ? Response.json(problem, { status: 403 }) : Response.json(opening()));
  const scoped = () => cache.query({ ...nodeChatOpeningQuery(node.id, 'topic', null, demand, () => true), staleTime: 0 });
  await scoped(); capturePeopleHint(cache);
  await expect(scoped()).rejects.toBeInstanceOf(ApiError);
  expect(calls[1]!.searchParams.get('defaults')).toBe('true');
  for (const flag of ['secret_names', 'default_provider_name', 'embeddings_provider_name']) expect(calls[1]!.searchParams.get(flag)).toBe('false');
  await scoped();
  for (const flag of ['defaults', 'secret_names', 'default_provider_name', 'embeddings_provider_name']) expect(calls[2]!.searchParams.get(flag)).toBeNull();
  capturePeopleHint(cache); await scoped();
  expect(calls[3]!.searchParams.get('defaults')).toBe('true');
  for (const flag of ['secret_names', 'default_provider_name', 'embeddings_provider_name']) expect(calls[3]!.searchParams.get(flag)).toBe('false');
});

it('starts the canonical Node read immediately when no matching aggregate is already pending', async () => {
  transport(() => { throw new Error('A Node reader must not start a future Chat opening'); }, () => Response.json(node));
  expect(await cache.query(nodeQuery(node.id))).toEqual(node);
  expect(calls.map((url) => url.pathname)).toEqual([`/api/v1/nodes/${node.id}`]);
});

/** The actual opening settles cold topic observers without another summary transport. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { client } from '@/api/client';
import { nodeChatOpeningQuery } from '@/api/chats';
import { resourceSummaryKey, resourceSummaryQuery } from '@/api/summaries';
import { adoptSummaryOpening } from '@/api/summarySnapshots';
import type { NodeChatOpening, SummarySnapshot } from '@/generated/api-types';
import { chatWriter } from '@/realtime/stores/chat';
import { ACME } from '../doubles/fixtures';
import { nodeChatOpening } from '../doubles/screen-openings';
import { World } from '../doubles/world';

const wire = vi.hoisted(() => vi.fn<(request: Request) => Promise<Response>>());
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  const client = actual.createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch: wire });
  return { ...actual, client, api: client.api };
});
const saved = client.session.getState();
const target = { kind: 'topic', id: ACME.fixLogin } as const;
const caches: QueryClient[] = [];
function cache() {
  const value = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  caches.push(value); return value;
}
async function opening(withChat = true): Promise<NodeChatOpening> {
  const world = new World(), person = world.people.get('ada')!;
  world.summaries.control('/set', { ...target, text: 'The carried topic paragraph.', state: 'fresh' });
  if (!withChat) world.places.denied.set('ada', new Set(['chat.view']));
  return nodeChatOpening(world, person, target.id, new URLSearchParams());
}
afterEach(() => {
  for (const value of caches.splice(0)) value.clear();
  chatWriter.clear(); wire.mockReset(); client.session.setState(saved);
});

it.each([true, false])('settles an in-flight topic summary from the native opening (chat readable: %s)', async (withChat) => {
  const value = await opening(withChat), query = cache();
  wire.mockImplementation((request) => {
    expect(new URL(request.url).pathname).toBe(`/api/v1/nodes/${target.id}/chat-opening`);
    return Promise.resolve(Response.json(value));
  });
  const pending = query.query(resourceSummaryQuery(target));
  await query.query(nodeChatOpeningQuery(target.id, 'thread', null));
  expect((await pending).text).toBe('The carried topic paragraph.');
  expect(wire).toHaveBeenCalledOnce();
  const chat = value.opening?.sync.chat;
  if (chat !== undefined) {
    expect(query.getQueryData(resourceSummaryKey({ kind: 'chat', id: chat.id }, value.summary_range ?? undefined)))
      .toEqual(value.summaries?.find((view) => view.target.kind === 'chat'));
    expect(query.getQueryData(resourceSummaryKey({ kind: 'chat_title', id: chat.id })))
      .toEqual(value.summaries?.find((view) => view.target.kind === 'chat_title'));
  }
  expect(query.getQueryData(resourceSummaryKey(target, { after: 0, through: 1 }))).toBeUndefined();
});

it('keeps an omitted optional summary on its ordinary free read and later reads fresh state again', async () => {
  const value = await opening(), query = cache();
  const full = value.summaries?.find((view) => view.target.kind === 'topic');
  if (full === undefined) throw new Error('The fixture needs its own full topic summary');
  wire.mockImplementation((request) => {
    if (new URL(request.url).pathname.endsWith('/chat-opening')) return Promise.resolve(Response.json({ ...value, summaries: value.summaries?.filter((view) => view.target.kind !== 'topic') }));
    expect(new URL(request.url).pathname).toBe('/api/v1/summaries/batch');
    return Promise.resolve(Response.json({ items: [{ ...full, full }], unavailable: [], next_offset: null }));
  });
  const pending = query.query(resourceSummaryQuery(target));
  await query.query(nodeChatOpeningQuery(target.id, 'topic', null));
  expect(await pending).toEqual(full);
  expect(wire).toHaveBeenCalledTimes(2);
  await query.query(resourceSummaryQuery(target));
  expect(wire).toHaveBeenCalledTimes(3);
});

it('keeps the topic summary independent when a live frame has already advanced the selected chat', async () => {
  const value = await opening(), query = cache(), sync = value.opening?.sync;
  if (sync === undefined) throw new Error('The fixture needs its selected chat');
  chatWriter.reset(sync.chat.id, sync.chat, sync.messages, sync.has_older, sync.seq + 1);
  wire.mockImplementation((request) => {
    expect(new URL(request.url).pathname).toBe(`/api/v1/nodes/${target.id}/chat-opening`);
    return Promise.resolve(Response.json(value));
  });
  const pending = query.query(resourceSummaryQuery(target));
  await query.query(nodeChatOpeningQuery(target.id, 'topic', null));
  expect((await pending).text).toBe('The carried topic paragraph.');
  expect(wire).toHaveBeenCalledOnce();
  expect(query.getQueryData(resourceSummaryKey({ kind: 'chat', id: sync.chat.id }, value.summary_range ?? undefined))).toBeUndefined();
});

it('uses carried data only for the opening and keeps a later ordinary read current', async () => {
  const value = await opening(), query = cache();
  const full = value.summaries?.find((view) => view.target.kind === 'topic');
  if (full === undefined) throw new Error('The fixture needs its own full topic summary');
  const fresh = { ...full, revision: full.revision + 1, text: 'The later current paragraph.' };
  wire.mockImplementation((request) => Promise.resolve(Response.json(new URL(request.url).pathname.endsWith('/chat-opening')
    ? value : { items: [{ ...fresh, full: fresh }], unavailable: [], next_offset: null })));
  const pending = query.query(resourceSummaryQuery(target));
  await query.query(nodeChatOpeningQuery(target.id, 'topic', null));
  expect((await pending).text).toBe(full.text);
  expect(wire).toHaveBeenCalledOnce();
  expect((await query.query(resourceSummaryQuery(target))).text).toBe(fresh.text);
  expect(wire).toHaveBeenCalledTimes(2);
});

it('uses the opening for the first mounted viewer and rereads on a later sole mount', async () => {
  const value = await opening(false), query = cache();
  const full = value.summaries?.[0];
  if (full === undefined) throw new Error('The fixture needs its own full topic summary');
  wire.mockImplementation((request) => Promise.resolve(Response.json(new URL(request.url).pathname.endsWith('/chat-opening')
    ? value : { items: [{ ...full, full }], unavailable: [], next_offset: null })));
  await query.query(nodeChatOpeningQuery(target.id, 'topic', null));
  const mounted = new QueryObserver(query, resourceSummaryQuery(target));
  const leave = mounted.subscribe(() => undefined);
  expect(mounted.getCurrentResult().data?.text).toBe(full.text);
  expect(wire).toHaveBeenCalledOnce();
  leave();
  const returned = new QueryObserver(query, resourceSummaryQuery(target));
  const cleanup = returned.subscribe(() => undefined);
  try { await vi.waitFor(() => { expect(wire).toHaveBeenCalledTimes(2); }); }
  finally { cleanup(); }
});

it('does not adopt a completed opening after the reader changes', async () => {
  const value = await opening(false), query = cache();
  let deliver!: (value: Response) => void;
  wire.mockImplementation(() => new Promise<Response>((resolve) => { deliver = resolve; }));
  const pending = query.query(nodeChatOpeningQuery(target.id, 'topic', null));
  const refused = expect(pending).rejects.toMatchObject({ name: 'AbortError' });
  await vi.waitFor(() => { expect(wire).toHaveBeenCalledOnce(); });
  client.session.setState({ readerVersion: (saved.readerVersion ?? 0) + 1 });
  deliver(Response.json(value)); await refused;
  expect(query.getQueryCache().findAll({ queryKey: ['resource-summary'] })).toEqual([]);
});

it('rejects another topic or an empty root before seeding an opening', async () => {
  const value = await opening(false), query = cache();
  const full = value.summaries?.[0];
  if (full === undefined) throw new Error('The fixture needs its own full topic summary');
  const invalid: SummarySnapshot[] = [{ ...full, target: { kind: 'topic', id: ACME.mainThread } }, { ...full, root_org_id: '' }];
  for (const snapshot of invalid) expect(() => adoptSummaryOpening(query, { ...value, summaries: [snapshot] }, target.id)).toThrow();
  expect(query.getQueryCache().findAll({ queryKey: ['resource-summary'] })).toEqual([]);
});

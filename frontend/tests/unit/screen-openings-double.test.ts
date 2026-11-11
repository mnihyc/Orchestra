// @vitest-environment node
/** Aggregate transports retain native chat scopes, private drafts and independent document refusals. */
import { expect, it } from 'vitest';
import type { ChatDraft, Node } from '@/generated/api-types';
import { ACME } from '../doubles/fixtures';
import { chatOpening, nodeChatOpening, documentPages, orgOverview } from '../doubles/screen-openings';
import { Refusal, World } from '../doubles/world';

it('opens the org overview through its bounded native child and recent readers', async () => {
  const world = new World(), person = world.people.get('ada')!;
  const result = await orgOverview(world, person, ACME.org, new URLSearchParams());
  expect(result.children).toEqual((await world.requestFeeds.read(person, `/nodes/${ACME.org}/children?archived=exclude&limit=50`)).body);
  expect(result.recent).toEqual((await world.requestFeeds.read(person, `/search/names?kinds=thread&limit=50&org_id=${ACME.org}`)).body);
  expect(result.first_steps.status).toBe('ready');
  expect(result.usage.status).toBe('ready');
  expect(result.activity.status).toBe('ready');
  expect(result.outside_openings.status).toBe('ready');
  world.places.denied.set('ada', new Set(['org.view']));
  await expect(orgOverview(world, person, ACME.org, new URLSearchParams())).rejects.toBeInstanceOf(Refusal);
});

it('admits the selected thread conversation and only the readable scopes around it', async () => {
  const world = new World(), person = world.people.get('ada')!;
  const result = await nodeChatOpening(world, person, ACME.mainThread, new URLSearchParams({ scope: 'thread', limit: '100' }));
  expect(result.scopes.map((scope) => scope.node.kind)).toEqual(['thread', 'topic', 'project']);
  expect(result.opening?.sync.chat.node?.id).toBe(ACME.mainThread);
  for (const scope of result.scopes) {
    const native = (await world.requestFeeds.read(person, `/chats/${scope.chat.id}`)).body;
    expect(native).toMatchObject({ id: scope.chat.id, node: scope.node });
  }
  world.places.denied.set('ada', new Set(['chat.view']));
  const denied = await nodeChatOpening(world, person, ACME.mainThread, new URLSearchParams());
  expect(denied).toEqual({ scopes: [], opening: null });
});

it('carries the selected private draft without borrowing another reader’s words', async () => {
  const world = new World(), person = world.people.get('ada')!, other = world.people.get('dora')!;
  const node = (await world.requestFeeds.read(person, `/nodes/${ACME.fixLogin}`)).body as Node;
  const draft: ChatDraft = { parts: [{ type: 'text', text: 'Unsent words' }], reply_to: null, updated_at: new Date().toISOString() };
  world.chats.handle(person, 'PUT', `/chats/${node.chat_id}/draft`, new URLSearchParams(), draft);
  expect((await chatOpening(world, person, node.chat_id, new URLSearchParams())).draft?.parts).toEqual(draft.parts);
  expect((await chatOpening(world, other, node.chat_id, new URLSearchParams())).draft).toBeNull();
  world.places.denied.set('ada', new Set(['chat.view']));
  await expect(chatOpening(world, person, node.chat_id, new URLSearchParams())).rejects.toBeInstanceOf(Refusal);
});

it('carries only the topic in view alongside the selected chat, even when chat reading is unavailable', async () => {
  const world = new World(), person = world.people.get('ada')!, other = world.people.get('dora')!;
  const target = { kind: 'topic', id: ACME.fixLogin } as const;
  world.summaries.control('/set', { ...target, text: 'Only this reader’s topic paragraph.' });
  const opened = await nodeChatOpening(world, person, target.id, new URLSearchParams({ scope: 'project' }));
  expect(opened.opening?.sync.chat.node?.kind).toBe('project');
  expect(opened.summaries?.find((view) => view.target.kind === 'topic')).toMatchObject({ target, text: 'Only this reader’s topic paragraph.' });
  expect(opened.summaries).toHaveLength(3);
  const others = await nodeChatOpening(world, other, target.id, new URLSearchParams());
  expect(others.summaries?.find((view) => view.target.kind === 'topic')?.text).toBeNull();
  world.places.denied.set('ada', new Set(['chat.view']));
  const withoutChat = await nodeChatOpening(world, person, target.id, new URLSearchParams());
  expect(withoutChat.scopes).toEqual([]); expect(withoutChat.opening).toBeNull();
  expect(withoutChat.summaries).toHaveLength(1);
  expect(withoutChat.summaries?.[0]).toMatchObject({ target, text: 'Only this reader’s topic paragraph.' });
  expect(withoutChat.summary_range).toBeUndefined();
  world.places.denied.set('ada', new Set(['topic.view']));
  await expect(nodeChatOpening(world, person, target.id, new URLSearchParams())).rejects.toBeInstanceOf(Refusal);
});

it('batches first document pages without widening the native permission of any place', async () => {
  const world = new World(), person = world.people.get('ada')!;
  const deniedId = '00000000-0000-4000-8999-000000000001';
  const pages = await documentPages(world, person, ACME.web, new URLSearchParams({ node_ids: `${ACME.web},${deniedId}`, limit: '30' }));
  expect(pages).toHaveLength(2);
  expect(pages[0]?.page).toEqual((await world.requestFeeds.read(person, `/nodes/${ACME.web}/documents?limit=30`)).body);
  expect(pages[0]?.error).toBeNull();
  expect(pages[1]).toMatchObject({ node_id: deniedId, page: null, error: { status: 404 } });
});

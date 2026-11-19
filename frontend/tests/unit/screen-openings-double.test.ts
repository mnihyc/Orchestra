// @vitest-environment node
/** Aggregate transports retain native chat scopes, private drafts and independent document refusals. */
import { expect, it } from 'vitest';
import type { ChatDraft, Node } from '@/generated/api-types';
import { ACME } from '../doubles/fixtures';
import { chatOpening, nodeChatOpening, documentPages } from '../doubles/screen-openings';
import { Refusal, World } from '../doubles/world';

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

it('batches first document pages without widening the native permission of any place', async () => {
  const world = new World(), person = world.people.get('ada')!;
  const deniedId = '00000000-0000-4000-8999-000000000001';
  const pages = await documentPages(world, person, ACME.web, new URLSearchParams({ node_ids: `${ACME.web},${deniedId}`, limit: '30' }));
  expect(pages).toHaveLength(2);
  expect(pages[0]?.page).toEqual((await world.requestFeeds.read(person, `/nodes/${ACME.web}/documents?limit=30`)).body);
  expect(pages[0]?.error).toBeNull();
  expect(pages[1]).toMatchObject({ node_id: deniedId, page: null, error: { status: 404 } });
});

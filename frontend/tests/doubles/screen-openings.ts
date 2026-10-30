/** Screen transports reuse native fixture projections and preserve each caller's admission. */
import type { ChatOpening, Node, NodeChatOpening, NodeDocumentPage, NodeKind, NodeRef, Problem } from '../../src/generated/api-types.ts';
import { Refusal, type Person, type World } from './world.ts';

export async function chatOpening(world: World, person: Person, id: string, query: URLSearchParams): Promise<ChatOpening> {
  if (query.has('since')) throw new Refusal(400, 'request.invalid', 'an opening starts without a position');
  const sync = (await world.requestFeeds.read(person, `/chats/${id}/sync?${query}`)).body as ChatOpening['sync'];
  let draft: ChatOpening['draft'];
  try { draft = (await world.requestFeeds.read(person, `/chats/${id}/draft`)).body as ChatOpening['draft']; }
  catch (error) { if (error instanceof Refusal && error.answer.status === 403) draft = null; else throw error; }
  return { sync, draft };
}

export async function nodeChatOpening(world: World, person: Person, id: string, query: URLSearchParams): Promise<NodeChatOpening> {
  const own = (await world.requestFeeds.read(person, `/nodes/${id}`)).body as Node;
  const wanted: NodeKind[] = own.kind === 'org' || own.kind === 'project' ? [own.kind] : own.kind === 'thread' ? ['thread', 'topic', 'project'] : ['topic', 'project'];
  const scopes: NodeChatOpening['scopes'] = [];
  for (const kind of wanted) {
    const target = [...own.path].reverse().find((node) => node.kind === kind && !node.redacted);
    if (target === undefined) continue;
    try {
      const node = (await world.requestFeeds.read(person, `/nodes/${target.id}`)).body as Node;
      const chat = (await world.requestFeeds.read(person, `/chats/${node.chat_id}`)).body as ChatOpening['sync']['chat'];
      const reference: NodeRef = { id: node.id, kind: node.kind, name: node.name, redacted: false };
      scopes.push({ node: reference, chat: { id: chat.id, seq: chat.seq } });
    } catch (error) { if (!(error instanceof Refusal) || ![403, 404].includes(error.answer.status)) throw error; }
  }
  const chosen = scopes.find((scope) => scope.chat.id === query.get('chat_id'))
    ?? scopes.find((scope) => scope.node.kind === query.get('scope')) ?? scopes[0];
  const paging = new URLSearchParams(); if (query.has('limit')) paging.set('limit', query.get('limit') ?? '');
  return { scopes, opening: chosen === undefined ? null : await chatOpening(world, person, chosen.chat.id, paging) };
}

export async function documentPages(world: World, person: Person, id: string, query: URLSearchParams): Promise<NodeDocumentPage[]> {
  const ids = (query.get('node_ids') ?? '').split(',');
  // The same native maximum page size bounds the batch.
  if (ids[0] !== id || ids.length > 200) throw new Refusal(400, 'request.invalid', 'the node batch must fit one page');
  const pages: NodeDocumentPage[] = [];
  for (const nodeId of ids) {
    try {
      await world.requestFeeds.read(person, `/nodes/${nodeId}`);
      const paging = new URLSearchParams(); if (query.has('limit')) paging.set('limit', query.get('limit') ?? '');
      const page = (await world.requestFeeds.read(person, `/nodes/${nodeId}/documents?${paging}`)).body as NonNullable<NodeDocumentPage['page']>;
      pages.push({ node_id: nodeId, page, error: null });
    } catch (error) {
      if (!(error instanceof Refusal)) throw error;
      pages.push({ node_id: nodeId, page: null, error: error.answer.body as Problem });
    }
  }
  return pages;
}

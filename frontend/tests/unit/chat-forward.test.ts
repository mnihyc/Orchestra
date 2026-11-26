import { treeNode } from '../doubles/tree-node';
/** Where a forward can go and what the choice comes to, and what quoting items of a thread into its chat does: the chat is found by the tree the page holds, quotes wait for its composer once, and the panel is brought to the front. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { chatOfNode, discussInChat, takeQuotes, useDiscuss } from '@/features/chat/discuss';
import {
  destinationsOf,
  groupDestinations,
  groupLabel,
  nodeDestinations,
  personDestinations,
  refusedDestination,
  summary,
  threadDestinations,
} from '@/features/forward/destinations';
import { useWorkbench } from '@/features/workbench/store';
import { findView } from '@/features/workbench/layout';
import '@/features/chat/register';
import type { NameHit, TreeNode } from '@/generated/api-types';
import { type OrgTree, useTreeStore } from '@/realtime/stores/tree';

import { ADA, chat, DORA, GRACE } from '../doubles/chat-rows';
import { display } from '../doubles/thread-display';

const node = (id: string, kind: TreeNode['kind'], parent: string | null, more: Partial<TreeNode> = {}): TreeNode => treeNode({ id, kind, parent_id: parent, tree_seq: 1, ...more });

describe('where a forward can go', () => {
  const nodes = [
    node('acme', 'org', null, { chat: { id: 'chat-acme', seq: 3 } }),
    node('web', 'project', 'acme', { chat: { id: 'chat-web', seq: 9 } }),
    node('fix login', 'topic', 'web', { chat: { id: 'chat-fix', seq: 9 } }),
    node('old idea', 'topic', 'web', { archived: true }),
    node('removed', 'topic', 'web', { deleted: true }),
  ];

  const placeChats = [nodes[2], nodes[1], nodes[0], nodes[3], nodes[4]].flatMap((node) => node === undefined ? [] : [chat({
    id: node.chat.id, kind: 'node', name: null, node: { id: node.id, kind: node.kind, name: node.name, redacted: false },
    home: null, can: { ...chat().can, send: !node.archived && !node.deleted },
  })]);

  it('offers writable place chats in the native order and retains native refusals', () => {
    expect(nodeDestinations(placeChats).map((one) => one.label)).toEqual(['fix login', 'web', 'acme']);
    expect(nodeDestinations(placeChats).map((one) => one.label)).not.toContain('old idea');
    expect(nodeDestinations(placeChats).map((one) => one.label)).not.toContain('removed');
  });

  it('keeps the native name match and only its disclosed context instead of borrowing a loaded tree', () => {
    const source = placeChats[0]; if (source === undefined) throw new Error('Missing native chat');
    const found = nodeDestinations([{ ...source, home: { id: 'private-home', kind: 'project', name: null, redacted: true } }]);
    expect(found).toEqual([{ key: 'chat:chat-fix', kind: 'node', chatId: 'chat-fix', label: 'fix login', detail: null, nodeKind: 'topic' }]);
  });

  it('offers the groups the person can write in, by name or by the names of their people', () => {
    const crew = chat({ id: 'g1', kind: 'group', node: null, name: 'Release crew', members: [ADA, DORA] });
    const unnamed = chat({ id: 'g2', kind: 'group', node: null, name: null, members: [ADA, DORA, GRACE] });
    const reading = chat({ id: 'g3', kind: 'group', node: null, name: 'Read only', members: [DORA], can: { admin_only: ['chat.view'], participating: false, send: false, moderate: false, export: false, reason: 'inspecting' } });
    const direct = chat({ id: 'd1', kind: 'direct', node: null, members: [ADA, DORA] });
    const all = [crew, unnamed, reading, direct];

    expect(groupDestinations(all, '', ADA.id).map((one) => one.label)).toEqual(['Release crew', 'Dora Developer, Grace Hopper']);
    expect(groupDestinations(all, 'grace', ADA.id).map((one) => one.label)).toEqual(['Dora Developer, Grace Hopper']);
    expect(groupLabel(unnamed, null)).toBe('Ada Lovelace, Dora Developer, Grace Hopper');
  });

  it('offers people, never the person themselves nor a service', () => {
    const service = { ...GRACE, id: 'svc', kind: 'service' as const, username: 'bot' };
    const found = personDestinations([ADA, DORA, service], ADA.id);
    expect(found).toEqual([{ key: `user:${DORA.id}`, kind: 'person', userId: DORA.id, label: 'Dora Developer', detail: '@dora', person: DORA }]);
  });

  it('makes the request out of the choice: a chat by its id, a person by theirs', () => {
    const chosen = [...nodeDestinations(placeChats.filter((row) => row.id === 'chat-fix')), ...personDestinations([DORA], ADA.id)];
    expect(destinationsOf(chosen)).toEqual([{ chat_id: 'chat-fix' }, { user_id: DORA.id }]);
  });

  it('says what the choice comes to: how many chats, which people, how many places', () => {
    const chosen = [...nodeDestinations(placeChats.filter((row) => row.id === 'chat-fix')), ...personDestinations([DORA, GRACE], ADA.id)];
    expect(summary(chosen)).toEqual({ chats: 1, people: ['Dora Developer', 'Grace Hopper'], threads: [], places: 3 });
  });

  it('offers native agent-thread matches with state and readable context while excluding the source', () => {
    const hit = (id: string, archived = false): NameHit => ({ id, kind: 'thread', name: id, archived, home_path: [], address: null,
      avatar: null, color: null, custom_color: null, owner: null, recent: false, username: null, tag: null, thread_attention: null,
      path: [{ id: 'fix', kind: 'topic', name: 'fix login', redacted: false }, { id, kind: 'thread', name: id, redacted: false }], thread_display: display('running') });
    const threads = [hit('docs'), hit('repro'), hit('old', true)];
    const found = threadDestinations(threads, 'docs');
    expect(found).toEqual([{ key: 'thread:repro', kind: 'thread', threadId: 'repro', label: 'repro', detail: 'fix login', display: display('running') }]);
    expect(threadDestinations([hit('docs')]).map((one) => one.label)).toEqual(['docs']);
    expect(destinationsOf(found)).toEqual([{ thread_id: 'repro' }]);
    expect(summary(found)).toEqual({ chats: 0, people: [], threads: ['repro'], places: 1 });
    expect(refusedDestination(found, { thread_id: 'repro' })?.label).toBe('repro');
  });

  it('finds the destination a refusal names, by chat or by person, and none when it names nobody', () => {
    const chosen = [...nodeDestinations(placeChats.filter((row) => row.id === 'chat-fix')), ...personDestinations([DORA], ADA.id)];
    expect(refusedDestination(chosen, { chat_id: 'chat-fix' })?.label).toBe('fix login');
    expect(refusedDestination(chosen, { user_id: DORA.id })?.label).toBe('Dora Developer');
    expect(refusedDestination(chosen, { chat_id: 'elsewhere' })).toBeNull();
    expect(refusedDestination(chosen, null)).toBeNull();
    expect(refusedDestination(chosen, 'text')).toBeNull();
  });
});

describe('quoting items of a thread into its chat', () => {
  const tree: OrgTree = {
    nodes: new Map([node('acme', 'org', null), node('main thread', 'thread', 'acme', { chat: { id: 'chat-thread', seq: 0 } })].map((one) => [one.id, one])),
    lastSeq: 1,
    loaded: true,
    error: null,
  };

  beforeEach(() => {
    useTreeStore.setState({ byOrg: new Map([['acme', tree]]) });
    useDiscuss.setState({ quotes: new Map() });
    useWorkbench.getState().leave();
    vi.stubGlobal('requestAnimationFrame', vi.fn(() => 0));
  });
  afterEach(() => { useWorkbench.getState().leave(); vi.unstubAllGlobals(); });

  it('finds the chat of a thread in the tree the page holds, and of nothing else', () => {
    expect(chatOfNode('main thread')).toBe('chat-thread');
    expect(chatOfNode('somewhere else')).toBeNull();
  });

  it('leaves the quotes for the composer of that chat, and brings the panel of chats to the front', () => {
    expect(discussInChat('main thread', ['item-1', 'item-2'])).toBe(true);
    expect(takeQuotes('chat-thread')).toEqual([
      { threadId: 'main thread', itemId: 'item-1' },
      { threadId: 'main thread', itemId: 'item-2' },
    ]);
    expect(useWorkbench.getState().overlay).toBe('right');
    const opened = findView(useWorkbench.getState().layout, 'chat', { node: 'main thread' });
    expect(opened?.area).toBe('right');
    expect(opened?.tab.of).toEqual({ node: 'main thread' });
  });

  it('hands the quotes over once: what was taken is not there to be taken again', () => {
    discussInChat('main thread', ['item-1']);
    expect(takeQuotes('chat-thread')).toHaveLength(1);
    expect(takeQuotes('chat-thread')).toEqual([]);
  });

  it('adds to what already waits, and keeps the chats apart', () => {
    discussInChat('main thread', ['item-1']);
    discussInChat('main thread', ['item-2']);
    expect(takeQuotes('chat-thread').map((quote) => quote.itemId)).toEqual(['item-1', 'item-2']);
    expect(takeQuotes('chat-acme')).toEqual([]);
  });

  it('says no, and changes nothing, when the thread is not in the tree or nothing was picked', () => {
    expect(discussInChat('somewhere else', ['item-1'])).toBe(false);
    expect(discussInChat('main thread', [])).toBe(false);
    expect(useDiscuss.getState().quotes.size).toBe(0);
    expect(useWorkbench.getState().overlay).toBeNull();
  });
});

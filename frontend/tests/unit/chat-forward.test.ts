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
import type { TreeNode } from '@/generated/api-types';
import { type OrgTree, useTreeStore } from '@/realtime/stores/tree';

import { ADA, chat, DORA, GRACE } from '../doubles/chat-rows';
import { display } from '../doubles/thread-display';

const node = (id: string, kind: TreeNode['kind'], parent: string | null, more: Partial<TreeNode> = {}): TreeNode => ({
  participating: true,
  id,
  address: null,
  kind,
  name: id,
  parent_id: parent,
  color: null,
  docs: 0,
  archived: false,
  deleted: false,
  restricted: false,
  chat: { id: `chat-${id}`, seq: 0 },
  thread: null,
  topic: null,
  tree_seq: 1,
  ...more,
});

describe('where a forward can go', () => {
  const nodes = [
    node('acme', 'org', null, { chat: { id: 'chat-acme', seq: 3 } }),
    node('web', 'project', 'acme', { chat: { id: 'chat-web', seq: 9 } }),
    node('fix login', 'topic', 'web', { chat: { id: 'chat-fix', seq: 9 } }),
    node('old idea', 'topic', 'web', { archived: true }),
    node('removed', 'topic', 'web', { deleted: true }),
  ];

  it('offers the chats of places that take a message, the most recently active first and then by name', () => {
    const found = nodeDestinations(nodes, '', () => null);
    expect(found.map((one) => one.label)).toEqual(['fix login', 'web', 'acme']);
    // Archived and deleted places take no message.
    expect(found.map((one) => one.label)).not.toContain('old idea');
    expect(found.map((one) => one.label)).not.toContain('removed');
  });

  it('narrows the places by what was typed, in any case, and says where each lies', () => {
    const found = nodeDestinations(nodes, ' LOGIN ', (one) => `under ${one.parent_id ?? 'nothing'}`);
    expect(found).toEqual([expect.objectContaining({ key: 'chat:chat-fix', kind: 'node', detail: 'under web', nodeKind: 'topic' })]);
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
    expect(found).toEqual([{ key: `user:${DORA.id}`, kind: 'person', userId: DORA.id, label: 'Dora Developer', detail: '@dora' }]);
  });

  it('makes the request out of the choice: a chat by its id, a person by theirs', () => {
    const chosen = [...nodeDestinations(nodes, 'fix', () => null), ...personDestinations([DORA], ADA.id)];
    expect(destinationsOf(chosen)).toEqual([{ chat_id: 'chat-fix' }, { user_id: DORA.id }]);
  });

  it('says what the choice comes to: how many chats, which people, how many places', () => {
    const chosen = [...nodeDestinations(nodes, 'fix', () => null), ...personDestinations([DORA, GRACE], ADA.id)];
    expect(summary(chosen)).toEqual({ chats: 1, people: ['Dora Developer', 'Grace Hopper'], threads: [], places: 3 });
  });

  it('offers the agents of the threads, the most recently active first, never the thread the items come from', () => {
    const threads = [
      ...nodes,
      node('repro', 'thread', 'fix login', { thread: { state: 'running', display: display('running'), blocked_reason: null, pending_approvals: 0, activity_seq: 4, last_activity_at: null, attention: { uncommitted: null, unpushed: null, failed: false, stale: true, as_of: null, reason: null } } }),
      node('docs', 'thread', 'fix login', { thread: { state: 'idle', display: display('idle'), blocked_reason: null, pending_approvals: 0, activity_seq: 9, last_activity_at: null, attention: { uncommitted: null, unpushed: null, failed: false, stale: true, as_of: null, reason: null } } }),
      node('gone', 'thread', 'fix login', { deleted: true }),
    ];
    const found = threadDestinations(threads, '', () => 'fix login', 'docs');
    expect(found).toEqual([{ key: 'thread:repro', kind: 'thread', threadId: 'repro', label: 'repro', detail: 'fix login', state: 'running', display: display('running') }]);
    expect(threadDestinations(threads, 'DOC', () => null).map((one) => one.label)).toEqual(['docs']);
    // A forward to a thread is an input of the sender's there, asked for by the thread; a refusal that names it is found.
    expect(destinationsOf(found)).toEqual([{ thread_id: 'repro' }]);
    expect(summary(found)).toEqual({ chats: 0, people: [], threads: ['repro'], places: 1 });
    expect(refusedDestination(found, { thread_id: 'repro' })?.label).toBe('repro');
  });

  it('finds the destination a refusal names, by chat or by person, and none when it names nobody', () => {
    const chosen = [...nodeDestinations(nodes, 'fix', () => null), ...personDestinations([DORA], ADA.id)];
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

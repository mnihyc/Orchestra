/** Which rows of a place are ticked, what a tick on a place does to what is in it, and the request the ticks come to: everything as one word, whole places named, and the documents and chats of a place that is only partly in. */
import { describe, expect, it } from 'vitest';

import type { DocumentRef, TreeNode } from '@/generated/api-types';
import {
  CHOSEN_MAX,
  readTicks,
  type Scope,
  scopeOf,
  shownTick,
  tallies,
  tickRow,
  type Ticks,
  wantedOf,
} from '@/features/export/selection';

const row = (id: string, kind: TreeNode['kind'], name: string, parent: string | null, more: Partial<TreeNode> = {}): TreeNode => ({
  participating: true,
  id,
  address: null,
  kind,
  name,
  parent_id: parent,
  archived: false,
  deleted: false,
  restricted: false,
  color: null,
  docs: 0,
  chat: { id: `chat-${id}`, seq: 0 },
  thread: null,
  topic: kind === 'topic' ? { status: 'open', tag_ids: [] } : null,
  tree_seq: 1,
  ...more,
});

// Web has three folders (one with a topic and a thread), a topic of its own, and documents beside Web, a folder and the thread.
const NODES = [
  row('web', 'project', 'Web', 'org', { chat: { id: 'chat-web', seq: 4 } }),
  row('f1', 'project', 'Checkout', 'web'),
  row('f2', 'project', 'Payments', 'web'),
  row('f3', 'project', 'Rollout', 'web'),
  row('t1', 'topic', 'Fix login', 'web', { chat: { id: 'chat-t1', seq: 2 } }),
  row('th1', 'thread', 'Main thread', 't1'),
  row('th2', 'thread', 'Repro', 't1'),
  row('t2', 'topic', 'Cart rewrite', 'f1'),
];

const doc = (id: string, node: string): DocumentRef => ({ id, node: { id: node, kind: 'project', name: null, redacted: false }, title: id, page: 'follows' }) as unknown as DocumentRef;

const DOCUMENTS: Record<string, DocumentRef[]> = {
  web: [doc('d-plan', 'web'), doc('d-notes', 'web')],
  f1: [doc('d-design', 'f1')],
  th1: [doc('d-thread', 'th1')],
};

function scopeFor(options: { greyed?: string[]; complete?: boolean } = {}): Scope {
  const rows = new Map(NODES.map((node) => [node.id, node]));
  return scopeOf(rows, 'web', 'en', (place) => ({ items: place === 'th1' ? [] : (DOCUMENTS[place] ?? []), complete: options.complete ?? true }), new Set(options.greyed ?? []));
}

const none: Ticks = new Map();
const both = { chats: true, documents: true };

describe('the ticks of a place', () => {
  it('has everything ticked as it opens, and asks for everything as one word', () => {
    const scope = scopeFor();
    expect(shownTick(readTicks(scope, none).get('web'))).toBe(true);
    expect(wantedOf(scope, none, both).wanted).toEqual({ type: 'all' });
  });

  it('takes what is in a place when the place is cleared, and when it is ticked again', () => {
    const scope = scopeFor();
    const cleared = tickRow(scope, none, 'f1', false);
    const reading = readTicks(scope, cleared);
    expect(shownTick(reading.get('f1'))).toBe(false);
    expect(shownTick(reading.get('t2'))).toBe(false);
    expect(shownTick(reading.get('d-design'))).toBe(false);
    expect(shownTick(reading.get('web'))).toBe('mixed');
    const again = tickRow(scope, cleared, 'f1', true);
    expect(again.size, 'what says what a row would have anyway is not kept').toBe(0);
    expect(wantedOf(scope, again, both).wanted).toEqual({ type: 'all' });
  });

  it('shows a place half ticked while something in it is not, and empty when nothing is', () => {
    const scope = scopeFor();
    const some = tickRow(scope, none, 'th2', false);
    expect(shownTick(readTicks(scope, some).get('t1'))).toBe('mixed');
    expect(shownTick(readTicks(scope, some).get('th2'))).toBe(false);
    const nothing = tickRow(scope, none, 'web', false);
    expect(shownTick(readTicks(scope, nothing).get('web'))).toBe(false);
    expect(wantedOf(scope, nothing, both).wanted).toEqual({ type: 'nothing' });
  });
});

describe('the request the ticks come to', () => {
  it('names the whole places that are ticked when two of three folders are, and the place’s own chat and documents', () => {
    const scope = scopeFor();
    const ticks = tickRow(scope, none, 'f3', false);
    const { wanted, needDocuments } = wantedOf(scope, ticks, both);
    expect(needDocuments).toEqual([]);
    expect(wanted).toEqual({
      type: 'chosen',
      chosen: {
        // The two folders and the topic are whole; Web itself is only partly in, so what is its own is named: its chat and its documents.
        nodes: ['f1', 'f2', 't1'],
        documents: ['d-plan', 'd-notes'],
        chats: ['chat-web'],
      },
    });
  });

  it('names a document alone when only it is ticked, and not the chat of the place it lies beside', () => {
    const scope = scopeFor();
    const ticks = tickRow(scope, tickRow(scope, none, 'web', false), 'd-notes', true);
    expect(wantedOf(scope, ticks, both).wanted).toEqual({ type: 'chosen', chosen: { nodes: [], documents: ['d-notes'], chats: [] } });
  });

  it('leaves out one document of a ticked place by naming the others', () => {
    const scope = scopeFor();
    const ticks = tickRow(scope, none, 'd-plan', false);
    expect(wantedOf(scope, ticks, both).wanted).toEqual({
      type: 'chosen',
      chosen: { nodes: ['f1', 'f2', 'f3', 't1'], documents: ['d-notes'], chats: ['chat-web'] },
    });
  });

  it('asks nothing of documents, or of chats, when that kind is not wanted', () => {
    const scope = scopeFor();
    const ticks = tickRow(scope, none, 'f3', false);
    expect(wantedOf(scope, ticks, { chats: false, documents: false }).wanted).toEqual({ type: 'chosen', chosen: { nodes: ['f1', 'f2', 't1'], documents: [], chats: [] } });
  });

  it('says which places must be read whole first when their documents are only partly known', () => {
    const scope = scopeFor({ complete: false });
    const ticks = tickRow(scope, none, 'f3', false);
    expect(wantedOf(scope, ticks, both).needDocuments).toEqual(['web']);
    // A kind that is not wanted needs no list.
    expect(wantedOf(scope, ticks, { chats: true, documents: false }).needDocuments).toEqual([]);
  });

  it('does not name the chat of a place that is cleared', () => {
    const scope = scopeFor();
    const ticks = tickRow(scope, none, 't1', false);
    const wanted = wantedOf(scope, ticks, both).wanted;
    expect(wanted.type === 'chosen' && wanted.chosen.chats).toEqual(['chat-web']);
  });

  it('reads rows that may be read and not taken away as no part of the choice', () => {
    const scope = scopeFor({ greyed: ['th2', 'd-notes'] });
    expect(wantedOf(scope, none, both).wanted, 'a place with a row that cannot be ticked is still all of what can be').toEqual({ type: 'all' });
    const reading = readTicks(scope, none);
    expect(shownTick(reading.get('th2'))).toBe(false);
    expect(shownTick(reading.get('t1'))).toBe(true);
  });

  it('refuses to name more than a list may hold', () => {
    const many = Array.from({ length: CHOSEN_MAX + 2 }, (_, at) => row(`n${at}`, 'topic', `Topic ${at}`, 'web'));
    const rows = new Map([row('web', 'project', 'Web', 'org'), ...many].map((node) => [node.id, node]));
    const scope = scopeOf(rows, 'web', 'en', () => ({ items: [], complete: true }), new Set());
    // Everything but the first topic: a list of ten thousand and one.
    const ticks = tickRow(scope, none, 'n0', false);
    expect(wantedOf(scope, ticks, both).tooMany).toBe(true);
  });
});

describe('what lies in a place', () => {
  it('is counted below every place: the places, the topics and the threads', () => {
    const counts = tallies(scopeFor());
    expect(counts.get('web')).toEqual({ places: 3, topics: 2, threads: 2 });
    expect(counts.get('t1')).toEqual({ places: 0, topics: 0, threads: 2 });
    expect(counts.get('th1')).toEqual({ places: 0, topics: 0, threads: 0 });
  });

  it('stands in the order the left bar keeps: places, topics, then threads, by name', () => {
    const scope = scopeFor();
    expect(scope.under.get('web')).toEqual(['f1', 'f2', 'f3', 't1']);
    expect(scope.under.get('t1')).toEqual(['th1', 'th2']);
  });

  it('leaves out what was deleted', () => {
    const rows = new Map(NODES.map((node) => [node.id, node.id === 'f2' ? { ...node, deleted: true } : node]));
    const scope = scopeOf(rows, 'web', 'en', () => ({ items: [], complete: true }), new Set());
    expect(scope.under.get('web')).toEqual(['f1', 'f3', 't1']);
  });
});

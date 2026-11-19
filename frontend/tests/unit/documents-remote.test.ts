/** Other people's rows laid over a page: only what differs changes, what the person is writing is theirs, and the caret stays where it was. */
import { EditorState, TextSelection } from '@tiptap/pm/state';
import type { Node as PmNode } from '@tiptap/pm/model';
import { describe, expect, it } from 'vitest';

import { buildTree } from '@/features/documents/blocks';
import { applyRows, findContainer, REMOTE, replaceAll, type RemoteContext } from '@/features/documents/remote';
import { orchSchema, treeToJson } from '@/features/documents/schema';
import type { DocumentBlock } from '@/generated/api-types';

const schema = orchSchema();
let stamp = 1;

function row(id: string, parent: string | null, position: string, type: DocumentBlock['type'], words: string, props: object = {}): DocumentBlock {
  stamp += 1;
  return {
    id,
    document_id: 'doc',
    parent_id: parent,
    position,
    type,
    props,
    content: words === '' ? [] : [{ type: 'text', text: words }],
    removed: false,
    seq_created: 1,
    seq_updated: stamp,
    updated_by: null,
    updated_at: '2026-10-01T09:00:00.000Z',
  };
}

function stateOf(rows: DocumentBlock[]): EditorState {
  return EditorState.create({ doc: schema.nodeFromJSON(treeToJson(buildTree(rows))) });
}

/** The context for rows that arrived: the tree as it now is, and what was held before. */
function context(before: DocumentBlock[], after: DocumentBlock[], dirty: string[] = []): RemoteContext {
  const held = new Map(before.map((one) => [one.id, one]));
  const now = new Map(before.map((one) => [one.id, one]));
  for (const one of after) now.set(one.id, one);
  return { tree: buildTree(now.values()), before: held, isDirty: (id) => dirty.includes(id) };
}

function outline(node: PmNode, depth = 0): string[] {
  const lines: string[] = [];
  node.forEach((child) => {
    if (child.type.name === 'block' || child.type.name === 'leafBlock') {
      lines.push(`${'  '.repeat(depth)}${child.child(0).type.name} ${child.child(0).textContent}`.trimEnd());
      if (child.childCount > 1) lines.push(...outline(child.child(1), depth + 1));
    }
  });
  return lines;
}

const apply = (state: EditorState, before: DocumentBlock[], after: DocumentBlock[], dirty: string[] = []) => {
  const tr = applyRows(state, after, context(before, after, dirty));
  return { tr, state: tr === null ? state : state.apply(tr) };
};

describe('rows that arrive', () => {
  it('change the words of a block where they differ, and leave the caret where it was', () => {
    const before = [row('a', null, '1', 'paragraph', 'Hello world, again')];
    let state = stateOf(before);
    // The caret stands after "Hello", in the first word.
    state = state.apply(state.tr.setSelection(TextSelection.create(state.doc, 2 + 5)));
    const { state: next, tr } = apply(state, before, [{ ...before[0]!, content: [{ type: 'text', text: 'Hello there, again' }], seq_updated: 99 }]);
    expect(outline(next.doc)).toEqual(['paragraph Hello there, again']);
    expect(next.selection.from).toBe(2 + 5);
    // It is other people's work: not in the person's undo, and marked so that it is not sent back.
    expect(tr?.getMeta('addToHistory')).toBe(false);
    expect(tr?.getMeta(REMOTE)).toBe(true);
  });

  it('keep a block the person has an unsent change in', () => {
    const before = [row('a', null, '1', 'paragraph', 'Mine so far')];
    const { state } = apply(stateOf(before), before, [{ ...before[0]!, content: [{ type: 'text', text: 'Theirs' }], seq_updated: 99 }], ['a']);
    expect(outline(state.doc)).toEqual(['paragraph Mine so far']);
  });

  it('add a block after the one before it in the server\'s order, and under its parent', () => {
    const before = [row('a', null, '1', 'paragraph', 'First'), row('c', null, '3', 'paragraph', 'Third'), row('k', 'a', '1', 'bullet', 'Child')];
    const added = [row('b', null, '2', 'heading', 'Second', { level: 2 }), row('k2', 'a', '2', 'bullet', 'Child two')];
    const { state } = apply(stateOf(before), before, added);
    expect(outline(state.doc)).toEqual(['paragraph First', '  bullet Child', '  bullet Child two', 'heading Second', 'paragraph Third']);
  });

  it('take a removed block away, and keep what the person is writing in it, as a block of their own', () => {
    const before = [row('a', null, '1', 'paragraph', 'Stay'), row('b', null, '2', 'paragraph', 'Go'), row('c', 'b', '1', 'bullet', 'Under it')];
    const gone = [{ ...before[1]!, removed: true, seq_updated: 99 }, { ...before[2]!, removed: true, seq_updated: 99 }];
    expect(outline(apply(stateOf(before), before, gone).state.doc)).toEqual(['paragraph Stay']);
  });

  it('move a block with what is under it', () => {
    const before = [row('a', null, '1', 'paragraph', 'A'), row('b', null, '2', 'paragraph', 'B'), row('k', 'a', '1', 'bullet', 'Under A')];
    const moved = [{ ...before[0]!, position: '3', seq_updated: 99 }];
    expect(outline(apply(stateOf(before), before, moved).state.doc)).toEqual(['paragraph B', 'paragraph A', '  bullet Under A']);
  });

  it('change the type of a block, keeping its identifier', () => {
    const before = [row('a', null, '1', 'paragraph', 'Words')];
    const { state } = apply(stateOf(before), before, [{ ...before[0]!, type: 'heading', props: { level: 3 }, seq_updated: 99 }]);
    expect(outline(state.doc)).toEqual(['heading Words']);
    expect(state.doc.child(0).attrs.id).toBe('a');
    expect(state.doc.child(0).child(0).attrs.level).toBe(3);
  });

  it('change nothing when a row says what the page already says', () => {
    const before = [row('a', null, '1', 'paragraph', 'Same')];
    expect(apply(stateOf(before), before, [{ ...before[0]!, seq_updated: 99 }]).tr).toBeNull();
  });
});

describe('finding and replacing', () => {
  it('finds a container by its identifier, deep in the page', () => {
    const rows = [row('a', null, '1', 'paragraph', 'A'), row('k', 'a', '1', 'bullet', 'K'), row('j', 'k', '1', 'bullet', 'J')];
    const state = stateOf(rows);
    expect(findContainer(state.doc, 'j')?.node.child(0).textContent).toBe('J');
    expect(findContainer(state.doc, 'nobody')).toBeNull();
  });

  it('replaces the whole page with the rows as they are', () => {
    const before = [row('a', null, '1', 'paragraph', 'Old')];
    const state = stateOf(before);
    const tree = buildTree([row('x', null, '1', 'heading', 'New', { level: 1 }), row('y', null, '2', 'paragraph', 'Page')]);
    const next = state.apply(replaceAll(state, tree));
    expect(outline(next.doc)).toEqual(['heading New', 'paragraph Page']);
  });
});

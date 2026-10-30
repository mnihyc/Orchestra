/** What a page of blocks promises: positions that order as numbers of any length, a canonical form in which equal blocks are equal, and the batch that turns the server's blocks into a person's, which moves only what is out of place. */
import { describe, expect, it } from 'vitest';

import {
  buildTree,
  canonContent,
  canonProps,
  comparePositions,
  draftsOf,
  inBatches,
  longestIncreasing,
  planOps,
  sameBlock,
  type Draft,
} from '@/features/documents/blocks';
import type { DocumentBlock } from '@/generated/api-types';

let counter = 0;
/** A block as the server holds it, at the end of the siblings that are given. */
function row(id: string, parent: string | null, position: string, over: Partial<DocumentBlock> = {}): DocumentBlock {
  counter += 1;
  return {
    id,
    document_id: 'doc',
    parent_id: parent,
    position,
    type: 'paragraph',
    props: {},
    content: [{ type: 'text', text: id }],
    removed: false,
    seq_created: 1,
    seq_updated: counter,
    updated_by: null,
    updated_at: '2026-10-01T09:00:00.000Z',
    ...over,
  };
}

const flat = (...ids: string[]): DocumentBlock[] => ids.map((id, index) => row(id, null, String(index + 1)));
const draft = (id: string, parent: string | null = null, text: string = id): Draft => ({
  id,
  parentId: parent,
  type: 'paragraph',
  props: {},
  content: [{ type: 'text', text }],
});

describe('positions', () => {
  it('order as numbers, however many digits they have', () => {
    expect(comparePositions('1', '2')).toBeLessThan(0);
    expect(comparePositions('10', '9')).toBeGreaterThan(0);
    expect(comparePositions('1.5', '1.25')).toBeGreaterThan(0);
    expect(comparePositions('1.50', '1.5')).toBe(0);
    expect(comparePositions('-1', '0')).toBeLessThan(0);
    expect(comparePositions('-2', '-1')).toBeLessThan(0);
    expect(comparePositions('0.000000000000000000000000000001', '0.000000000000000000000000000002')).toBeLessThan(0);
    expect(comparePositions('123456789012345678901234567890', '123456789012345678901234567891')).toBeLessThan(0);
  });

  it('put equal positions in the order of their identifiers when blocks are listed', () => {
    const tree = buildTree([row('b', null, '1'), row('a', null, '1'), row('c', null, '0.5')]);
    expect(tree.children.get(null)).toEqual(['c', 'a', 'b']);
  });

  it('leave out a removed block and what hangs from nothing', () => {
    const tree = buildTree([row('a', null, '1'), row('b', null, '2', { removed: true }), row('c', 'b', '1'), row('d', 'a', '1')]);
    expect(draftsOf(tree).map((one) => one.id)).toEqual(['a', 'd']);
  });
});

describe('a block in its canonical form', () => {
  it('is the same block whether text was joined or not, and whether a default was written or not', () => {
    const joined = { type: 'paragraph' as const, props: { align: 'left' }, content: [{ type: 'text' as const, text: 'ab', marks: ['bold' as const] }] };
    const split = {
      type: 'paragraph' as const,
      props: {},
      content: [
        { type: 'text' as const, text: 'a', marks: ['bold' as const] },
        { type: 'text' as const, text: '', marks: ['italic' as const] },
        { type: 'text' as const, text: 'b', marks: ['bold' as const] },
      ],
    };
    expect(sameBlock(joined, split)).toBe(true);
    expect(canonProps({ level: 2, toggle: false, color: null })).toEqual({ level: 2 });
  });

  it('differs when one mark, one link or one tint differs', () => {
    const plain = { type: 'paragraph' as const, props: {}, content: [{ type: 'text' as const, text: 'x' }] };
    expect(sameBlock(plain, { ...plain, content: [{ type: 'text', text: 'x', marks: ['bold'] }] })).toBe(false);
    expect(sameBlock(plain, { ...plain, content: [{ type: 'text', text: 'x', href: 'https://example.org' }] })).toBe(false);
    expect(sameBlock(plain, { ...plain, content: [{ type: 'text', text: 'x', color: 3 }] })).toBe(false);
  });

  it('keeps the rows of a table as they are, a span of one being no span', () => {
    const rows = canonContent([{ cells: [{ content: [{ type: 'text', text: 'a' }], colspan: 1, rowspan: 2 }] }]);
    expect(rows).toEqual([{ cells: [{ content: [{ type: 'text', text: 'a' }], rowspan: 2 }] }]);
  });
});

describe('the longest run that is in order', () => {
  it('is found, so that a block moved down is the only one to move', () => {
    expect(longestIncreasing([1, 2, 3, 0])).toEqual([0, 1, 2]);
    expect(longestIncreasing([3, 0, 1, 2])).toEqual([1, 2, 3]);
    expect(longestIncreasing([])).toEqual([]);
  });
});

describe('the batch that makes the server\'s blocks a person\'s', () => {
  it('is nothing when nothing differs', () => {
    const base = buildTree(flat('a', 'b', 'c'));
    expect(planOps(base, draftsOf(base)).ops).toEqual([]);
  });

  it('is one update, naming the stamp it was written on, for a line that was typed in', () => {
    const rows = flat('a', 'b');
    const base = buildTree(rows);
    const plan = planOps(base, [draft('a'), draft('b', null, 'b, longer')]);
    expect(plan.ops).toEqual([
      { op: 'update', id: 'b', base_seq: rows[1]?.seq_updated, content: [{ type: 'text', text: 'b, longer' }] },
    ]);
  });

  it('is one insert after the block before it for a new block in the middle', () => {
    const base = buildTree(flat('a', 'b'));
    const plan = planOps(base, [draft('a'), draft('n'), draft('b')]);
    expect(plan.ops).toHaveLength(1);
    expect(plan.ops[0]).toMatchObject({ op: 'insert', parent_id: null, after_id: 'a', block: { id: 'n', type: 'paragraph' } });
  });

  it('is one move for a block taken down past many, and none for the many', () => {
    const base = buildTree(flat('a', 'b', 'c', 'd', 'e'));
    const plan = planOps(base, [draft('b'), draft('c'), draft('d'), draft('e'), draft('a')]);
    expect(plan.ops).toEqual([{ op: 'move', id: 'a', parent_id: null, after_id: 'e' }]);
  });

  it('is one move under the block above for an indent', () => {
    const base = buildTree(flat('a', 'b'));
    const plan = planOps(base, [draft('a'), draft('b', 'a')]);
    expect(plan.ops).toEqual([{ op: 'move', id: 'b', parent_id: 'a', after_id: null }]);
  });

  it('removes the top of what goes and not what is under it, and moves out what stays before it goes', () => {
    const base = buildTree([row('a', null, '1'), row('b', 'a', '1'), row('c', 'b', '1'), row('d', 'a', '2')]);
    // `a` is deleted and its child `d` is kept at the top level; `b` goes with `c`.
    const plan = planOps(base, [draft('d')]);
    const kinds = plan.ops.map((op) => `${op.op}:${'id' in op ? op.id : ''}`);
    expect(kinds).toEqual(['move:d', 'remove:a']);
  });

  it('sends the props of the new type when the type changed, and only what changed when it did not', () => {
    const rows = [row('h', null, '1', { type: 'heading', props: { level: 1 } })];
    const base = buildTree(rows);
    const retyped = planOps(base, [{ id: 'h', parentId: null, type: 'bullet', props: {}, content: [{ type: 'text', text: 'h' }] }]);
    expect(retyped.ops).toEqual([{ op: 'update', id: 'h', base_seq: rows[0]?.seq_updated, type: 'bullet', props: {} }]);
    const level = planOps(base, [{ id: 'h', parentId: null, type: 'heading', props: { level: 2 }, content: [{ type: 'text', text: 'h' }] }]);
    expect(level.ops).toEqual([{ op: 'update', id: 'h', base_seq: rows[0]?.seq_updated, props: { level: 2 } }]);
  });

  it('puts the updates first, so that a batch cut in pieces never updates a block it moved already', () => {
    const base = buildTree(flat('a', 'b', 'c'));
    const plan = planOps(base, [draft('c'), draft('a', null, 'a!'), draft('b')]);
    expect(plan.ops.map((op) => op.op)).toEqual(['update', 'move']);
  });

  it('leaves an empty paragraph at the end that the server does not have until something is written in it', () => {
    const base = buildTree(flat('a'));
    const empty: Draft = { id: 'n', parentId: null, type: 'paragraph', props: {}, content: [] };
    expect(planOps(base, [draft('a'), empty]).ops).toEqual([]);
    expect(planOps(base, [draft('a'), { ...empty, content: [{ type: 'text', text: 'x' }] }]).ops).toHaveLength(1);
  });

  it('cuts a long plan into batches of the most the server takes, in order', () => {
    const ops = Array.from({ length: 1_201 }, (_, index) => ({ op: 'remove' as const, id: String(index) }));
    const batches = inBatches(ops);
    expect(batches.map((batch) => batch.length)).toEqual([500, 500, 201]);
    expect(batches.flat()).toEqual(ops);
  });
});

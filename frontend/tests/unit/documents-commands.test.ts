/** The keys of a block editor on a state of blocks: Enter, Backspace, Delete, Tab and Shift+Tab, and what they leave of the tree. */
import { EditorState, TextSelection } from '@tiptap/pm/state';
import type { Node as PmNode } from '@tiptap/pm/model';
import { describe, expect, it } from 'vitest';

import { backspaceAtStart, deleteAtEnd, liftBlock, sinkBlock, splitBlock, type Command } from '@/features/documents/commands';
import { orchSchema } from '@/features/documents/schema';

const schema = orchSchema();

/** A block of text: its type, its words, and what is under it. */
function block(id: string, type: string, words: string, children: object[] = [], attrs: object = {}): object {
  return {
    type: 'block',
    attrs: { id },
    content: [{ type, attrs, content: words === '' ? [] : [{ type: 'text', text: words }] }, ...(children.length > 0 ? [{ type: 'blockGroup', content: children }] : [])],
  };
}

function stateOf(...blocks: object[]): EditorState {
  return EditorState.create({ doc: schema.nodeFromJSON({ type: 'doc', content: blocks }) });
}

/** Puts the caret in the block with this id, at an offset into its words (the end when none is given). */
function caret(state: EditorState, id: string, offset?: number): EditorState {
  let found = -1;
  state.doc.descendants((node, pos) => {
    if (node.attrs.id === id && found < 0) found = pos;
    return found < 0;
  });
  expect(found, `the block ${id}`).toBeGreaterThanOrEqual(0);
  const content = state.doc.nodeAt(found)!.child(0);
  const at = found + 2 + (offset ?? content.content.size);
  return state.apply(state.tr.setSelection(TextSelection.create(state.doc, at)));
}

function run(state: EditorState, command: Command): EditorState {
  let next = state;
  const done = command(state, (tr) => {
    next = state.apply(tr);
  });
  expect(done, 'the command acted').toBe(true);
  return next;
}

/** The tree as indented lines: `type words`, and what is under each. */
function outline(node: PmNode, depth = 0): string[] {
  const lines: string[] = [];
  node.forEach((child) => {
    if (child.type.name === 'block' || child.type.name === 'leafBlock') {
      const content = child.child(0);
      lines.push(`${'  '.repeat(depth)}${content.type.name} ${content.textContent}`.trimEnd());
      if (child.childCount > 1) lines.push(...outline(child.child(1), depth + 1));
    }
  });
  return lines;
}

describe('Enter', () => {
  it('ends a bullet and starts the next bullet beside it', () => {
    let state = stateOf(block('a', 'bullet', 'One'));
    state = run(caret(state, 'a'), splitBlock);
    expect(outline(state.doc)).toEqual(['bullet One', 'bullet']);
    expect(state.selection.$from.parent.type.name).toBe('bullet');
    expect(state.selection.$from.parent.content.size).toBe(0);
  });

  it('gives the new block an identifier of its own and keeps the old one', () => {
    let state = stateOf(block('a', 'bullet', 'One'));
    state = run(caret(state, 'a'), splitBlock);
    expect(state.doc.child(0).attrs.id).toBe('a');
    expect(state.doc.child(1).attrs.id).not.toBe('a');
    expect(state.doc.child(1).attrs.id).toBeTruthy();
  });

  it('moves the words after the caret to the new block, which takes the children', () => {
    let state = stateOf(block('a', 'paragraph', 'Hello world', [block('k', 'bullet', 'child')]));
    state = run(caret(state, 'a', 5), splitBlock);
    expect(outline(state.doc)).toEqual(['paragraph Hello', 'paragraph  world', '  bullet child']);
  });

  it('turns an empty bullet back into a paragraph', () => {
    let state = stateOf(block('a', 'bullet', ''));
    state = run(caret(state, 'a'), splitBlock);
    expect(outline(state.doc)).toEqual(['paragraph']);
  });

  it('starts the next of a to-do unticked and of a heading as a paragraph', () => {
    let state = stateOf(block('t', 'todo', 'Do it', [], { checked: true }), block('h', 'heading', 'Title', [], { level: 2 }));
    state = run(caret(state, 't'), splitBlock);
    expect(state.doc.child(1).child(0).type.name).toBe('todo');
    expect(state.doc.child(1).child(0).attrs.checked).toBe(false);
    state = run(caret(state, 'h'), splitBlock);
    expect(outline(state.doc)).toEqual(['todo Do it', 'todo', 'heading Title', 'paragraph']);
  });
});

describe('Tab and Shift+Tab', () => {
  it('Tab puts a block under the one above it, and Shift+Tab takes it out again', () => {
    let state = stateOf(block('a', 'bullet', 'One'), block('b', 'bullet', 'Two'));
    state = run(caret(state, 'b'), sinkBlock);
    expect(outline(state.doc)).toEqual(['bullet One', '  bullet Two']);
    state = run(caret(state, 'b'), liftBlock);
    expect(outline(state.doc)).toEqual(['bullet One', 'bullet Two']);
  });

  it('Tab on the first block does nothing', () => {
    const state = caret(stateOf(block('a', 'bullet', 'One')), 'a');
    expect(sinkBlock(state)).toBe(false);
  });

  it('Shift+Tab on a block that is not under another does nothing', () => {
    const state = caret(stateOf(block('a', 'bullet', 'One')), 'a');
    expect(liftBlock(state)).toBe(false);
  });

  it('Shift+Tab leaves the blocks that came after as children of the one lifted', () => {
    let state = stateOf(block('p', 'bullet', 'Parent', [block('x', 'bullet', 'X'), block('y', 'bullet', 'Y'), block('z', 'bullet', 'Z')]));
    state = run(caret(state, 'y'), liftBlock);
    expect(outline(state.doc)).toEqual(['bullet Parent', '  bullet X', 'bullet Y', '  bullet Z']);
  });

  it('keeps the identifiers through Tab and Shift+Tab', () => {
    let state = stateOf(block('a', 'bullet', 'One'), block('b', 'bullet', 'Two'));
    state = run(caret(state, 'b'), sinkBlock);
    state = run(caret(state, 'b'), liftBlock);
    expect([state.doc.child(0).attrs.id, state.doc.child(1).attrs.id]).toEqual(['a', 'b']);
  });
});

describe('Backspace', () => {
  it('at the start of a bullet turns it into a paragraph', () => {
    let state = stateOf(block('a', 'bullet', 'One'));
    state = run(caret(state, 'a', 0), backspaceAtStart);
    expect(outline(state.doc)).toEqual(['paragraph One']);
  });

  it('at the start of a paragraph joins it to the block above, which keeps its identifier', () => {
    let state = stateOf(block('a', 'paragraph', 'One'), block('b', 'paragraph', 'Two'));
    state = run(caret(state, 'b', 0), backspaceAtStart);
    expect(outline(state.doc)).toEqual(['paragraph OneTwo']);
    expect(state.doc.child(0).attrs.id).toBe('a');
    expect(state.doc.childCount).toBe(1);
  });

  it('at the start of the first child takes it out from under its parent', () => {
    let state = stateOf(block('a', 'paragraph', 'Parent', [block('k', 'paragraph', 'Kid')]));
    state = run(caret(state, 'k', 0), backspaceAtStart);
    expect(outline(state.doc)).toEqual(['paragraph Parent', 'paragraph Kid']);
  });

  it('the whole sequence of the page: nest two bullets, take one out, make it text', () => {
    let state = stateOf(block('a', 'bullet', 'A bulleted line'));
    state = run(caret(state, 'a'), splitBlock);
    const second = state.doc.child(1).attrs.id as string;
    state = state.apply(state.tr.insertText('Second bullet', state.selection.from));
    state = run(state, sinkBlock);
    expect(outline(state.doc)).toEqual(['bullet A bulleted line', '  bullet Second bullet']);
    state = run(state, splitBlock);
    state = state.apply(state.tr.insertText('Third, nested', state.selection.from));
    expect(outline(state.doc)).toEqual(['bullet A bulleted line', '  bullet Second bullet', '  bullet Third, nested']);
    state = run(state, liftBlock);
    expect(outline(state.doc)).toEqual(['bullet A bulleted line', '  bullet Second bullet', 'bullet Third, nested']);
    const third = state.selection.$from.node(state.selection.$from.depth - 1).attrs.id as string;
    state = run(caret(state, third, 0), backspaceAtStart);
    expect(outline(state.doc)).toEqual(['bullet A bulleted line', '  bullet Second bullet', 'paragraph Third, nested']);
    expect(second).toBeTruthy();
  });
});

describe('Delete', () => {
  it('at the end of a block joins the next to it', () => {
    let state = stateOf(block('a', 'paragraph', 'One'), block('b', 'paragraph', 'Two'));
    state = run(caret(state, 'a'), deleteAtEnd);
    expect(outline(state.doc)).toEqual(['paragraph OneTwo']);
  });
});

/** What the server's reader made of a pasted text, laid on a page of blocks: where it goes, what it replaces, and what it leaves alone. */
import { EditorState, TextSelection, type Transaction } from '@tiptap/pm/state';
import type { Node as PmNode } from '@tiptap/pm/model';
import { describe, expect, it } from 'vitest';

import { looksLikeMarkdown, pastePieces } from '@/features/documents/paste';
import type { Piece } from '@/features/documents/pieces';
import { orchSchema } from '@/features/documents/schema';

const schema = orchSchema();

function block(id: string, type: string, words: string, attrs: object = {}): object {
  return { type: 'block', attrs: { id }, content: [{ type, attrs, content: words === '' ? [] : [{ type: 'text', text: words }] }] };
}

function stateWith(blocks: object[], caretIn: string, offset?: number): EditorState {
  const doc = schema.nodeFromJSON({ type: 'doc', content: blocks });
  let state = EditorState.create({ doc });
  let found = -1;
  doc.descendants((node, pos) => {
    if (node.attrs.id === caretIn && found < 0) found = pos;
    return found < 0;
  });
  const content = doc.nodeAt(found)!.child(0);
  state = state.apply(state.tr.setSelection(TextSelection.create(doc, found + 2 + (offset ?? content.content.size))));
  return state;
}

/** A block as the reader gives it. */
const piece = (type: Piece['type'], words: string, props: Piece['props'] = {}, children: Piece[] = []): Piece => ({ type, props, content: words === '' ? [] : [{ type: 'text', text: words }], children });
const heading = (words: string, level = 1): Piece => piece('heading', words, { level });

function paste(state: EditorState, pieces: Piece[], source = ''): { state: EditorState; done: boolean } {
  let next = state;
  const done = pastePieces(state, pieces, source, (tr: Transaction) => {
    next = state.apply(tr);
  });
  return { state: next, done };
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

describe('pasting Markdown', () => {
  it('puts blocks after the block the caret is in', () => {
    const start = stateWith([block('a', 'paragraph', 'First'), block('z', 'paragraph', 'Last')], 'a');
    const { state, done } = paste(start, [heading('Title'), piece('bullet', 'one', {}, [piece('bullet', 'nested')]), piece('bullet', 'two')]);
    expect(done).toBe(true);
    expect(outline(state.doc)).toEqual(['paragraph First', 'heading Title', 'bullet one', '  bullet nested', 'bullet two', 'paragraph Last']);
  });

  it('takes the place of an empty line, also an empty bullet', () => {
    expect(outline(paste(stateWith([block('a', 'paragraph', 'First'), block('e', 'paragraph', '')], 'e'), [heading('Two', 2), piece('paragraph', 'text')]).state.doc)).toEqual(['paragraph First', 'heading Two', 'paragraph text']);
    expect(outline(paste(stateWith([block('e', 'bullet', '')], 'e'), [piece('numbered', 'a'), piece('numbered', 'b')]).state.doc)).toEqual(['numbered a', 'numbered b']);
  });

  it('gives every block it makes an identifier of its own', () => {
    const { state } = paste(stateWith([block('a', 'paragraph', 'First')], 'a'), [heading('One'), piece('bullet', 'x'), piece('bullet', 'y')]);
    const ids = new Set<string>();
    state.doc.descendants((node) => {
      if (typeof node.attrs.id === 'string') ids.add(node.attrs.id);
    });
    expect(ids.size).toBe(4);
  });

  it('puts the caret at the end of what was pasted, and a line after a block that holds no text', () => {
    const { state } = paste(stateWith([block('a', 'paragraph', 'First')], 'a'), [piece('bullet', 'one'), piece('bullet', 'two')]);
    expect(state.selection.$from.parent.textContent).toBe('two');
    const code = paste(stateWith([block('a', 'paragraph', 'First')], 'a'), [piece('code', 'let x = 1;', { language: 'ts' })]).state;
    expect(outline(code.doc)).toEqual(['paragraph First', 'codeBlock let x = 1;', 'paragraph']);
  });

  it('puts a line after a block that holds no text also where it ends the children of another block', () => {
    const parent = { type: 'block', attrs: { id: 'p' }, content: [{ type: 'bullet', content: [{ type: 'text', text: 'Parent' }] }, { type: 'blockGroup', content: [block('k', 'bullet', 'Child')] }] };
    const { state } = paste(stateWith([parent], 'k'), [piece('code', 'code', { language: '' })]);
    expect(outline(state.doc)).toEqual(['bullet Parent', '  bullet Child', '  codeBlock code', '  paragraph']);
  });

  it('puts a line of words, with its marks, into the line the caret is in', () => {
    const { state } = paste(stateWith([block('a', 'paragraph', 'Say  now')], 'a', 4), [{ type: 'paragraph', props: {}, content: [{ type: 'text', text: 'loudly', marks: ['bold'] }], children: [] }], '**loudly**');
    expect(outline(state.doc)).toEqual(['paragraph Say loudly now']);
    const bold = state.doc.child(0).child(0).child(1);
    expect(bold.marks.map((mark) => mark.type.name)).toEqual(['bold']);
    expect(bold.text).toBe('loudly');
  });

  it('leaves code and table cells alone', () => {
    const doc = schema.nodeFromJSON({ type: 'doc', content: [{ type: 'leafBlock', attrs: { id: 'c' }, content: [{ type: 'codeBlock', attrs: { language: '' }, content: [{ type: 'text', text: 'x' }] }] }] });
    const state = EditorState.create({ doc, selection: TextSelection.create(doc, 3) });
    expect(paste(state, [heading('not a heading'), piece('bullet', 'item')]).done).toBe(false);
  });

  it('lays nothing when the reader found nothing', () => {
    expect(paste(stateWith([block('a', 'paragraph', 'First')], 'a'), []).done).toBe(false);
  });

  it('asks the reader only for text that says something in Markdown', () => {
    expect(looksLikeMarkdown('plain words')).toBe(false);
    expect(looksLikeMarkdown('# A heading')).toBe(true);
    expect(looksLikeMarkdown('a **bold** word')).toBe(true);
    expect(looksLikeMarkdown('<details>\n<summary>More</summary>')).toBe(true);
  });

  it('sends exported fragments and relative files to the same reader', () => {
    for (const text of [
      '<!-- orch:block {"kind":"paragraph","props":{"align":"center"}} -->\n\ncentred\n\n<!-- /orch:block -->',
      '![Picture](images/picture.png)',
      '[Report](../report.md)',
      '<u>underlined</u>',
      '<span color="red">red words</span>',
      '<video src="movie.mp4">A film</video>',
      '$x + y$',
      '$$\nx + y\n$$',
    ]) expect(looksLikeMarkdown(text), text).toBe(true);
    expect(looksLikeMarkdown('a * b and c_d')).toBe(false);
    expect(looksLikeMarkdown('an <unknown> tag')).toBe(false);
  });

  it('leaves long unfinished link markers as ordinary words', () => {
    expect(looksLikeMarkdown('['.repeat(100_000))).toBe(false);
    expect(looksLikeMarkdown('[unfinished\n'.repeat(100_000))).toBe(false);
  });
});

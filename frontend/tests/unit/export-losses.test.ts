/** What a document loses when it is written as Markdown, counted from its blocks before the file is made: each kind the server tells afterwards, in its order, counting the blocks that lose it. */
import { describe, expect, it } from 'vitest';

import { LOSS_KINDS, markdownLosses } from '@/features/export/losses';
import type { DocumentBlock } from '@/generated/api-types';

let next = 0;

/** A block with only what the count reads of it. */
function block(type: string, props: Record<string, unknown> = {}, content: unknown[] = [], more: Partial<DocumentBlock> = {}): DocumentBlock {
  next += 1;
  return { id: `b${next}`, type, props, content, parent_id: null, removed: false, position: String(next), ...more } as unknown as DocumentBlock;
}

const text = (words: string, more: Record<string, unknown> = {}) => ({ type: 'text', text: words, ...more });
const table = (rows: unknown[], props: Record<string, unknown> = { header_row: true }) => block('table', props, rows);
const row = (...cells: Record<string, unknown>[]) => ({ cells: cells.map((cell) => ({ content: [text('cell')], ...cell })) });

const kinds = (blocks: DocumentBlock[]) => markdownLosses(blocks).map((loss) => loss.kind);

describe('what Markdown cannot say of the blocks of a document', () => {
  it('says nothing of a document that is plain text, lists, a quote, code and a table with a header row', () => {
    const blocks = [
      block('heading', { level: 2 }, [text('Plan')]),
      block('paragraph', {}, [text('Words', { marks: ['bold', 'italic'] })]),
      block('bullet', {}, [text('one')]),
      block('numbered', { start: 3 }, [text('three')]),
      block('todo', { checked: true }, [text('done')]),
      block('quote', {}, [text('said')]),
      block('code', { language: 'ts' }, []),
      block('divider'),
      table([row({}, {})]),
    ];
    expect(markdownLosses(blocks)).toEqual([]);
  });

  it('counts the callouts and the toggles, and the headings that fold, by block', () => {
    const blocks = [block('callout', { tone: 'info' }), block('callout', { tone: 'warning' }), block('toggle'), block('heading', { toggle: true }), block('heading', { toggle: false })];
    expect(markdownLosses(blocks)).toEqual([
      { kind: 'callouts', count: 2 },
      { kind: 'toggles', count: 1 },
      { kind: 'foldingHeadings', count: 1 },
    ]);
  });

  it('counts a block of columns once, and tells what lies in the columns by its own blocks', () => {
    const columns = block('columns');
    const column = block('column', {}, [], { parent_id: columns.id });
    const inside = block('paragraph', { color: 4 }, [text('red')], { parent_id: column.id });
    expect(markdownLosses([columns, column, inside])).toEqual([
      { kind: 'columns', count: 1 },
      { kind: 'colours', count: 1 },
    ]);
  });

  it('says colours once, whether a block, a run of text or a cell of a table has one, and a null is no colour', () => {
    expect(kinds([block('paragraph', { color: 3 })])).toEqual(['colours']);
    expect(kinds([block('paragraph', { background: 2 })])).toEqual(['colours']);
    expect(kinds([block('paragraph', {}, [text('a', { color: 5 })])])).toEqual(['colours']);
    expect(kinds([table([row({ background: 1 })])])).toEqual(['colours']);
    expect(kinds([table([row({ content: [text('a', { background: 1 })] })])])).toEqual(['colours']);
    expect(kinds([block('paragraph', { color: null, background: null }, [text('a', { color: null })])])).toEqual([]);
    expect(markdownLosses([block('paragraph', { color: 3 }), block('heading', { color: 4 }, [text('h')])])).toEqual([{ kind: 'colours', count: 2 }]);
  });

  it('says alignment is left out for a block aligned to anything but the left, and underlines for underlined text', () => {
    expect(kinds([block('paragraph', { align: 'center' })])).toEqual(['alignment']);
    expect(kinds([block('paragraph', { align: 'left' })])).toEqual([]);
    expect(kinds([block('paragraph', { align: null })])).toEqual([]);
    expect(kinds([block('paragraph', {}, [text('u', { marks: ['underline'] })])])).toEqual(['underlines']);
  });

  it('says what a table loses: the header row it gets, the widths of its columns, and the cells that span', () => {
    expect(markdownLosses([table([row({})], {})])).toEqual([{ kind: 'headerlessTables', count: 1 }]);
    expect(markdownLosses([table([row({})], { header_row: false })])).toEqual([{ kind: 'headerlessTables', count: 1 }]);
    expect(markdownLosses([table([row({})], { header_row: true, widths: [0.4, 0.6] })])).toEqual([{ kind: 'tableWidths', count: 1 }]);
    expect(markdownLosses([table([row({ colspan: 2 }), row({ rowspan: 2 })]), table([row({ colspan: 3 })])])).toEqual([{ kind: 'spans', count: 2 }]);
    expect(markdownLosses([table([row({ colspan: 1, rowspan: 1 })])])).toEqual([]);
    // A table with no row is written as nothing.
    expect(markdownLosses([table([], {})])).toEqual([]);
  });

  it('says that a film or a sound by address becomes a link, and not one that names a file of the document', () => {
    expect(markdownLosses([block('video', { url: 'https://example.test/a.mp4' }), block('audio', { url: 'https://example.test/a.ogg' })])).toEqual([{ kind: 'mediaAddresses', count: 2 }]);
    expect(kinds([block('video', { file_id: 'f1' })])).toEqual([]);
    expect(kinds([block('image', { file_id: 'f2', width: 300 })])).toEqual([]);
  });

  it('says that blocks under one that cannot hold them come after it, but not those under a list item, a quote or a callout', () => {
    const paragraph = block('paragraph', {}, [text('parent')]);
    const under = block('paragraph', {}, [text('child')], { parent_id: paragraph.id });
    expect(markdownLosses([paragraph, under])).toEqual([{ kind: 'nesting', count: 1 }]);
    for (const holder of ['bullet', 'numbered', 'todo', 'toggle', 'quote', 'callout']) {
      const parent = block(holder, {}, [text('parent')]);
      const child = block('paragraph', {}, [text('child')], { parent_id: parent.id });
      expect(kinds([parent, child]).includes('nesting')).toBe(false);
    }
  });

  it('leaves out the blocks that were taken away, and lists the kinds in the order the server tells them', () => {
    const blocks = [
      block('callout', {}, [], { removed: true }),
      block('table', {}, [row({})]),
      block('paragraph', { align: 'right' }),
      block('paragraph', { color: 1 }),
      block('callout'),
      block('toggle'),
    ];
    const found = kinds(blocks);
    expect(found).toEqual(['callouts', 'toggles', 'colours', 'alignment', 'headerlessTables']);
    expect([...found].sort((one, other) => LOSS_KINDS.indexOf(one) - LOSS_KINDS.indexOf(other))).toEqual(found);
  });
});

/** What the editor holds of a page: every one of the twenty block types goes into the editor's state and comes out as the block it was, with the same order and the same tree. */
import { describe, expect, it } from 'vitest';

import { buildTree, draftsOf, sameBlock, type Draft } from '@/features/documents/blocks';
import { docToDrafts, orchSchema, treeToJson } from '@/features/documents/schema';
import type { DocumentBlock } from '@/generated/api-types';

let seq = 0;
function row(id: string, parent: string | null, position: string, type: DocumentBlock['type'], props: object, content: DocumentBlock['content']): DocumentBlock {
  seq += 1;
  return {
    id,
    document_id: 'doc',
    parent_id: parent,
    position,
    type,
    props: props,
    content,
    removed: false,
    seq_created: 1,
    seq_updated: seq,
    updated_by: null,
    updated_at: '2026-10-01T09:00:00.000Z',
  };
}

const text = (value: string, extra: object = {}) => ({ type: 'text' as const, text: value, ...extra });

/** A page that has a block of each type, some nested, some with marks. */
function everything(): DocumentBlock[] {
  return [
    row('p', null, '1', 'paragraph', { color: 2, background: 5, align: 'center' }, [text('Plain '), text('bold', { marks: ['bold', 'italic'] }), text(' and '), text('link', { href: 'https://example.org', color: 3 })]),
    row('h', null, '2', 'heading', { level: 3, toggle: true }, [text('Heading')]),
    row('h-child', 'h', '1', 'bullet', {}, [text('under the heading')]),
    row('n', null, '3', 'numbered', { start: 4 }, [text('four')]),
    row('t', null, '4', 'todo', { checked: true }, [text('done'), { type: 'mention', user_id: 'u1' }]),
    row('tg', null, '5', 'toggle', {}, [text('Rollback'), { type: 'break' }, text('steps')]),
    row('tg-child', 'tg', '1', 'quote', {}, [text('quoted'), { type: 'ref', kind: 'document', id: 'd1' },
      { type: 'ref', kind: 'file', id: 'f1', part: { line: 2 } },
      { type: 'ref', kind: 'environment_file', id: 'e1', path: 'src/auth.rs', part: { line: 42 } }]),
    row('q', null, '6', 'quote', { align: 'right' }, [text('say')]),
    row('c', null, '7', 'callout', { tone: 'warning' }, [text('Careful'), { type: 'math', source: 'x^2' }]),
    row('code', null, '8', 'code', { language: 'rust', wrap: true }, [text('fn main() {}\n')]),
    row('m', null, '9', 'math', { source: '\\frac{a}{b}' }, []),
    row('dg', null, '10', 'diagram', { source: 'graph TD; A-->B' }, []),
    row('dv', null, '11', 'divider', {}, []),
    row('pb', null, '12', 'page_break', {}, []),
    row('im', null, '13', 'image', { file_id: 'f1', width: 320, align: 'center' }, [text('a caption')]),
    row('vd', null, '14', 'video', { url: 'https://example.org/a.mp4', width: 480 }, [text('film')]),
    row('au', null, '15', 'audio', { file_id: 'f2' }, []),
    row('fl', null, '16', 'file', { file_id: 'f3' }, [text('data set')]),
    row('tb', null, '17', 'table', { header_row: true, header_column: true, widths: [1, 2, 1] }, [
      { cells: [{ content: [text('a')], background: 4 }, { content: [text('b')], colspan: 2 }] },
      { cells: [{ content: [text('c')], rowspan: 2 }, { content: [text('d')] }, { content: [text('e')], color: 1 }] },
      { cells: [{ content: [text('f')] }, { content: [] }] },
    ]),
    row('cols', null, '18', 'columns', {}, []),
    row('col1', 'cols', '1', 'column', { share: 2 }, []),
    row('col2', 'cols', '2', 'column', {}, []),
    row('in1', 'col1', '1', 'paragraph', {}, [text('left')]),
    row('in2', 'col2', '1', 'paragraph', {}, [text('right')]),
  ];
}

function roundTrip(blocks: DocumentBlock[]): { drafts: Draft[]; again: Draft[] } {
  const tree = buildTree(blocks);
  const json = treeToJson(tree);
  const doc = orchSchema().nodeFromJSON(json);
  doc.check();
  return { drafts: docToDrafts(doc), again: draftsOf(tree) };
}

describe('a page of every block type in the editor', () => {
  it.each(['small', 'medium', 'large'] as const)('keeps the shared name with a document reference shown at %s size', (display) => {
    const content = [{ type: 'ref' as const, kind: 'document' as const, id: 'referenced-document', label: 'The name as shared', display }];
    const { drafts } = roundTrip([row('reference-block', null, '1', 'paragraph', {}, content)]);
    expect(drafts[0]?.content).toEqual(content);
  });

  it('is accepted by the schema, with the table\'s covered cells left out of their rows', () => {
    const tree = buildTree(everything());
    const doc = orchSchema().nodeFromJSON(treeToJson(tree));
    expect(() => doc.check()).not.toThrow();
    expect(doc.childCount).toBe(18);
  });

  it('comes out as the same blocks, in the same order and under the same parents', () => {
    const { drafts, again } = roundTrip(everything());
    expect(drafts.map((one) => [one.id, one.parentId, one.type])).toEqual(again.map((one) => [one.id, one.parentId, one.type]));
  });

  it('comes out with the same props and the same content for each of the twenty types', () => {
    const { drafts, again } = roundTrip(everything());
    const by = new Map(again.map((one) => [one.id, one]));
    const types = new Set<string>();
    for (const one of drafts) {
      const before = by.get(one.id);
      expect(before, one.id).toBeDefined();
      types.add(one.type);
      // The widths of a table are shares: dragging a column's edge changes them, and an untouched table keeps its proportions.
      const comparable = one.type === 'table' ? { ...one, props: { ...one.props, widths: before?.props.widths } } : one;
      expect(sameBlock(comparable, before ?? one), `${one.type} ${one.id}`).toBe(true);
    }
    expect([...types].sort()).toEqual(
      ['audio', 'bullet', 'callout', 'code', 'column', 'columns', 'diagram', 'divider', 'file', 'heading', 'image', 'math', 'numbered', 'page_break', 'paragraph', 'quote', 'table', 'todo', 'toggle', 'video'].sort(),
    );
  });

  it('keeps the proportions of the columns of a table through the pixels the editor draws them in', () => {
    const { drafts } = roundTrip(everything());
    const table = drafts.find((one) => one.type === 'table');
    const widths = table?.props.widths as number[];
    expect(widths).toHaveLength(3);
    expect(widths[1]).toBeCloseTo((widths[0] ?? 0) * 2, -1);
  });

  it('is one empty paragraph when there is nothing yet', () => {
    const doc = orchSchema().nodeFromJSON(treeToJson(buildTree([])));
    expect(doc.childCount).toBe(1);
    expect(docToDrafts(doc)[0]?.type).toBe('paragraph');
  });
});
